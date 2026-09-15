//! Reproducible random selection of question sources.
//!
//! Selection is a pure function of `(sources, limit, seed)`: the same seed over
//! the same questions always yields the same draw, whatever order the caller
//! discovered the files in. That matters because the caller reads them with
//! [`std::fs::read_dir`], which makes no ordering promise — so [`per_directory`]
//! sorts before it chooses rather than trusting its input. [`shuffle`] permutes
//! exactly what it is handed, so give it a slice that is already in a stable
//! order.
//!
//! The generator is deliberately not cryptographic. It picks quiz questions; it
//! is seeded so a draw can be re-created, and it is in-crate so no dependency is
//! taken for a few lines of arithmetic.

/// One question source: its bank-relative path and its file contents.
///
/// Sampling never looks at the contents — it groups and orders by path — but
/// carries them so the caller keeps the pairing.
pub type Source = (String, String);

/// Keep at most `limit` sources from each directory, chosen at random.
///
/// Sources are grouped by their parent directory (top-level files form one
/// group, keyed by `""`), and each group is independently down-sampled. Group
/// order and the relative order of the survivors are stable; only *which*
/// sources survive is random.
///
/// Input is sorted before grouping, so the draw depends on the seed alone and
/// not on the order the files happened to be read in.
#[must_use]
pub fn per_directory(mut sources: Vec<Source>, limit: usize, rng: &mut SampleRng) -> Vec<Source> {
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    let mut groups: Vec<(String, Vec<Source>)> = Vec::new();
    for source in sources {
        let dir = crate::path::parent_dir(&source.0);
        match groups.iter_mut().find(|(name, _)| name == dir) {
            Some((_, group)) => group.push(source),
            None => groups.push((dir.to_owned(), vec![source])),
        }
    }
    let mut kept = Vec::new();
    for (_, group) in groups {
        kept.extend(within_group(group, limit, rng));
    }
    kept
}

/// Randomly keep `limit` of `group` (all of it when it is already that small),
/// returning the survivors in path order.
#[must_use]
pub(crate) fn within_group(
    mut group: Vec<Source>,
    limit: usize,
    rng: &mut SampleRng,
) -> Vec<Source> {
    if group.len() <= limit {
        return group;
    }
    // Partial Fisher-Yates: move `limit` random items to the front, then drop
    // the rest. The re-sort below undoes the swaps' scrambling so the printed
    // sheet still reads in path order.
    for slot in 0..limit {
        let pick = slot + rng.index(group.len() - slot);
        group.swap(slot, pick);
    }
    group.truncate(limit);
    group.sort_by(|a, b| a.0.cmp(&b.0));
    group
}

/// Randomly permute `items` in place (Fisher-Yates).
pub fn shuffle<T>(items: &mut [T], rng: &mut SampleRng) {
    for index in (1..items.len()).rev() {
        items.swap(index, rng.index(index + 1));
    }
}

/// A small non-cryptographic PRNG (`SplitMix64`) for choosing questions.
///
/// Constructed from an explicit seed only: the library stays a pure function of
/// its inputs, and the binary decides where a seed comes from (a `--seed` flag,
/// or entropy when the author did not ask for a reproducible draw).
#[derive(Debug, Clone)]
pub struct SampleRng(u64);

impl SampleRng {
    /// A generator that will always produce the same sequence for `seed`.
    #[must_use]
    pub const fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    /// The next pseudo-random `u64`.
    const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A pseudo-random index in `0..bound`, or `0` when `bound` is `0`.
    ///
    /// Private: `0` is not a valid index into an empty range, so that fallback
    /// is cheap in-crate defence rather than a contract worth publishing. Both
    /// call sites already guarantee a non-zero bound.
    fn index(&mut self, bound: usize) -> usize {
        let Ok(bound) = u64::try_from(bound) else {
            return 0;
        };
        if bound == 0 {
            return 0;
        }
        loop {
            if let Some(index) = bounded(self.next_u64(), bound) {
                return index;
            }
        }
    }
}

/// The index `word` maps to in `0..bound`, or `None` when it falls in the
/// rejection window.
///
/// Lemire's debiased bounded generation: the index is the high half of a
/// widened multiply, and words whose low half lands in the short residue window
/// are rejected. A plain `% bound` keeps them and so skews toward low indices —
/// here that would mean earlier questions being drawn more often than later
/// ones.
///
/// Split out from [`SampleRng::index`] because for small bounds the rejection
/// window is vanishingly unlikely to be hit by chance, so a test can only reach
/// this branch by calling it directly.
fn bounded(word: u64, bound: u64) -> Option<usize> {
    let threshold = u128::from(bound.wrapping_neg() % bound);
    let product = u128::from(word) * u128::from(bound);
    if product & u128::from(u64::MAX) < threshold {
        return None;
    }
    // `product >> 64 < bound`, and `bound` came from a `usize`, so this fits.
    usize::try_from(product >> 64).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build `(path, "")` sources for the given relative paths.
    fn sources(paths: &[&str]) -> Vec<Source> {
        paths
            .iter()
            .map(|p| ((*p).to_owned(), String::new()))
            .collect()
    }

    /// The directory of each kept source, in order.
    fn dirs_of(kept: &[Source]) -> Vec<&str> {
        kept.iter()
            .map(|(p, _)| crate::path::parent_dir(p))
            .collect()
    }

    /// The paths of each kept source, in order.
    fn paths_of(kept: &[Source]) -> Vec<&str> {
        kept.iter().map(|(p, _)| p.as_str()).collect()
    }

    #[test]
    /// Sampling caps each directory at `limit` and keeps smaller groups whole.
    fn per_directory_caps_each_group() {
        let src = sources(&["a/1.md", "a/2.md", "a/3.md", "b/1.md"]);
        let kept = per_directory(src, 2, &mut SampleRng::seeded(1));
        let dirs = dirs_of(&kept);
        assert_eq!(kept.len(), 3); // a: 3 -> 2, b: 1 -> 1
        assert_eq!(dirs.iter().filter(|d| **d == "a").count(), 2);
        assert_eq!(dirs.iter().filter(|d| **d == "b").count(), 1);
    }

    #[test]
    /// A fixed seed makes sampling reproducible, and survivors stay in path order.
    fn per_directory_is_deterministic_and_ordered() {
        let src = sources(&["d/0.md", "d/1.md", "d/2.md", "d/3.md", "d/4.md"]);
        let first = per_directory(src.clone(), 3, &mut SampleRng::seeded(42));
        let again = per_directory(src, 3, &mut SampleRng::seeded(42));
        assert_eq!(first, again);
        assert_eq!(first.len(), 3);
        let paths = paths_of(&first);
        let mut sorted = paths.clone();
        sorted.sort_unstable();
        assert_eq!(paths, sorted);
    }

    #[test]
    /// The same seed draws the same questions however the caller ordered the
    /// input. `read_dir` makes no ordering promise, so without this a `--seed`
    /// would not actually reproduce a draw.
    fn per_directory_ignores_input_order() {
        let ordered = sources(&["d/0.md", "d/1.md", "d/2.md", "d/3.md", "d/4.md"]);
        let expected = per_directory(ordered.clone(), 3, &mut SampleRng::seeded(7));
        // Every rotation of the same set must yield the same survivors.
        for rotation in 1..ordered.len() {
            let mut shuffled = ordered.clone();
            shuffled.rotate_left(rotation);
            let kept = per_directory(shuffled, 3, &mut SampleRng::seeded(7));
            assert_eq!(paths_of(&kept), paths_of(&expected), "rotation {rotation}");
        }
        let mut reversed = ordered;
        reversed.reverse();
        let kept = per_directory(reversed, 3, &mut SampleRng::seeded(7));
        assert_eq!(paths_of(&kept), paths_of(&expected), "reversed");
    }

    #[test]
    /// Different seeds generally draw different questions (so `--seed` is doing
    /// something), while any one seed stays stable.
    fn different_seeds_draw_differently() {
        let src = sources(&["d/0.md", "d/1.md", "d/2.md", "d/3.md", "d/4.md"]);
        let draws: Vec<Vec<String>> = (0..8)
            .map(|seed| {
                per_directory(src.clone(), 2, &mut SampleRng::seeded(seed))
                    .into_iter()
                    .map(|(path, _)| path)
                    .collect()
            })
            .collect();
        assert!(
            draws.windows(2).any(|pair| pair.first() != pair.get(1)),
            "eight seeds all produced the same draw"
        );
    }

    #[test]
    /// A group no larger than the limit is kept whole, untouched.
    fn within_group_keeps_small_groups_whole() {
        let src = sources(&["a/1.md", "a/2.md"]);
        let kept = within_group(src.clone(), 5, &mut SampleRng::seeded(3));
        assert_eq!(kept, src);
        assert_eq!(within_group(src.clone(), 2, &mut SampleRng::seeded(3)), src);
        assert!(within_group(Vec::new(), 3, &mut SampleRng::seeded(3)).is_empty());
    }

    #[test]
    /// A zero limit keeps nothing rather than everything.
    fn within_group_with_a_zero_limit_keeps_nothing() {
        let src = sources(&["a/1.md", "a/2.md"]);
        assert!(within_group(src, 0, &mut SampleRng::seeded(3)).is_empty());
    }

    #[test]
    /// A group already at the limit consumes no randomness. If it drew, adding
    /// one question to one directory would shift every *later* directory's draw
    /// for the same seed, so `--seed` would stop reproducing the parts of a
    /// sheet that did not change.
    fn within_group_at_the_limit_draws_nothing() {
        let src = sources(&["a/1.md", "a/2.md", "a/3.md"]);
        let mut rng = SampleRng::seeded(5);
        assert_eq!(within_group(src.clone(), 3, &mut rng), src);
        assert_eq!(rng.index(1_000_000), SampleRng::seeded(5).index(1_000_000));
    }

    #[test]
    /// Every question in a directory is drawn about equally often. Taking the
    /// swap partner from the whole group instead of the not-yet-picked tail
    /// still returns `limit` questions in path order, so only a fairness check
    /// notices that some questions are quietly favoured.
    fn within_group_draws_each_question_about_equally_often() {
        let pool = sources(&["a/0.md", "a/1.md", "a/2.md", "a/3.md", "a/4.md"]);
        let mut counts = [0_u32; 5];
        for seed in 0..2_000 {
            for (path, _) in within_group(pool.clone(), 2, &mut SampleRng::seeded(seed)) {
                if let Some(slot) = pool.iter().position(|(p, _)| *p == path)
                    && let Some(count) = counts.get_mut(slot)
                {
                    *count += 1;
                }
            }
        }
        // 2000 draws of 2 from 5 => 800 expected per question, sd ~ 17.
        for count in counts {
            assert!((720..=880).contains(&count), "unfair draw: {counts:?}");
        }
    }

    #[test]
    /// Top-level files form their own group (keyed by `""`) and are capped
    /// independently of the subdirectories beside them.
    fn per_directory_groups_top_level_files_separately() {
        let src = sources(&["x.md", "y.md", "z.md", "a/1.md", "a/2.md", "a/3.md"]);
        let kept = per_directory(src, 2, &mut SampleRng::seeded(11));
        let dirs = dirs_of(&kept);
        assert_eq!(kept.len(), 4);
        assert_eq!(dirs.iter().filter(|d| d.is_empty()).count(), 2);
        assert_eq!(dirs.iter().filter(|d| **d == "a").count(), 2);
    }

    #[test]
    /// Shuffling can produce *every* order. A truncated loop bound pins an item
    /// to one position and a Sattolo-style `rng.index(index)` can never leave an
    /// item where it was; both still return "some different order", so the
    /// determinism test alone cannot see that orders became unreachable.
    fn shuffle_reaches_every_order() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..2_000 {
            let mut items: Vec<u32> = (0..4).collect();
            shuffle(&mut items, &mut SampleRng::seeded(seed));
            seen.insert(items);
        }
        assert_eq!(seen.len(), 24, "only {} of 24 orders reachable", seen.len());
    }

    #[test]
    /// `index` draws *through* `bounded`. Unit-testing `bounded` alone does not
    /// prove that, so pin them together: with a power-of-two bound nothing is
    /// ever rejected, so `index` must return exactly what `bounded` maps the
    /// next word to. A plain `% bound` would not.
    fn index_draws_through_bounded() {
        let bound: u64 = 1 << 20; // power of two => zero threshold => no redraws
        let as_usize = usize::try_from(bound).expect("bound fits in usize");
        let mut rng = SampleRng::seeded(31);
        let mut words = SampleRng::seeded(31);
        for step in 0..50 {
            assert_eq!(
                Some(rng.index(as_usize)),
                bounded(words.next_u64(), bound),
                "step {step}"
            );
        }
    }

    #[test]
    /// The rejection window is real: a word inside it is refused, one outside
    /// maps to the expected index. Unreachable through `index`, which redraws.
    fn bounded_rejects_the_residue_window() {
        // 2^64 mod 3 == 1, so only word 0 has a low half below the threshold.
        assert_eq!(bounded(0, 3), None);
        assert_eq!(bounded(u64::MAX, 3), Some(2));
        // A power-of-two bound has a zero threshold, so nothing is ever rejected.
        assert_eq!(bounded(0, 4), Some(0));
        assert_eq!(bounded(u64::MAX, 4), Some(3));
    }

    #[test]
    /// The PRNG always yields an index within bounds, including the degenerate
    /// bounds a caller could reach.
    fn index_is_always_in_bounds() {
        let mut rng = SampleRng::seeded(7);
        for _ in 0..100 {
            assert!(rng.index(5) < 5);
        }
        assert_eq!(rng.index(1), 0);
        assert_eq!(rng.index(0), 0); // empty range: no valid index, no panic
    }

    #[test]
    /// A cheap smoke test that no bucket is starved or dominant.
    ///
    /// Deliberately *not* a bias test: at this bound the skew of a plain
    /// `% bound` is `2^64 % 3` out of `2^64`, far too small for any sample size
    /// to see. `index_draws_through_bounded` is what guards the debiasing.
    fn index_is_roughly_uniform() {
        let mut rng = SampleRng::seeded(2024);
        let mut counts = [0_u32; 3];
        for _ in 0..3_000 {
            let index = rng.index(3);
            if let Some(count) = counts.get_mut(index) {
                *count += 1;
            }
        }
        for count in counts {
            assert!(
                (800..1200).contains(&count),
                "skewed distribution: {counts:?}"
            );
        }
    }

    #[test]
    /// Shuffling permutes the items and is reproducible for a fixed seed.
    fn shuffle_permutes_deterministically() {
        let original: Vec<u32> = (0..8).collect();
        let mut first = original.clone();
        let mut again = original.clone();
        shuffle(&mut first, &mut SampleRng::seeded(99));
        shuffle(&mut again, &mut SampleRng::seeded(99));
        assert_eq!(first, again); // same seed -> same order
        assert_ne!(first, original); // it actually reordered
        let mut sorted = first.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, original); // and it is a permutation (nothing lost)
    }
}
