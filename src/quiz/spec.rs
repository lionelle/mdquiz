//! The quiz blueprint: a YAML file describing how to build a printable exam.
//!
//! A spec names the topic folders to draw from, how many questions to take from
//! each, and how the resulting sheet is laid out. It is the reproducible,
//! reviewable counterpart to a pile of command-line flags — the same exam can be
//! rebuilt next term by re-running the file that made it.
//!
//! Parsing is separate from assembly. This module turns YAML into a validated
//! [`Spec`] and nothing more: it never touches the filesystem, so it is a pure
//! `&str -> Result<Spec>` transform. Checks that need to know how many questions
//! actually exist (is `take: 5` satisfiable? will five variants collide?) are
//! [`Spec::check_pools`], which takes the counts rather than going and reading
//! them.

use serde::Deserialize;

use crate::path::{escapes_dir, nests};
use crate::{Error, Result};

/// How many questions to take from a group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "TakeSpec")]
pub enum Take {
    /// Every question in the group, however many there are. The default.
    #[default]
    All,
    /// Exactly this many, drawn at random.
    Count(usize),
}

/// The `all` keyword, as its own enum so a typo is rejected.
///
/// A bare `#[serde(untagged)]` over `String` and `usize` would read `take: none`
/// and `take: al` as "all" — silently turning a typo into "use every question".
/// Naming the one legal keyword makes serde reject anything else.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TakeKeyword {
    /// Spelled `all` in the YAML.
    All,
}

/// The authored form of [`Take`]: either a count or the `all` keyword.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
enum TakeSpec {
    /// `take: 3`
    Count(usize),
    /// `take: all`
    Keyword(TakeKeyword),
}

impl From<TakeSpec> for Take {
    /// Collapse the authored form into the typed one.
    fn from(spec: TakeSpec) -> Self {
        match spec {
            TakeSpec::Count(count) => Self::Count(count),
            TakeSpec::Keyword(TakeKeyword::All) => Self::All,
        }
    }
}

/// One source folder and how much of it to use.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    /// The folder to draw from, relative to the spec's own directory. Searched
    /// recursively.
    pub dir: String,
    /// How many questions to take; without one, every question in the folder.
    #[serde(default)]
    pub take: Take,
    /// Blank lines to leave after each question from this group; falls back to
    /// [`Layout::answer_space`].
    #[serde(default)]
    pub answer_space: Option<usize>,
    /// Whether to permute this group's answer choices per variant; falls back to
    /// [`Layout::shuffle_choices`]. Affects multiple-choice and multiple-select
    /// questions only.
    #[serde(default)]
    pub shuffle_choices: Option<bool>,
}

/// How the printed sheet is laid out.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    /// Blank lines left after each question for the student's answer.
    #[serde(default = "default_answer_space")]
    pub answer_space: usize,
    /// Whether each question starts on a fresh page.
    #[serde(default)]
    pub page_break_between: bool,
    /// Whether answer choices are permuted per variant.
    ///
    /// Applies to multiple-choice and multiple-select questions. The other kinds
    /// have no choice list, or derive their order when rendered.
    #[serde(default)]
    pub shuffle_choices: bool,
    /// The repeating page footer, with `${...}` placeholders.
    #[serde(default)]
    pub page_footer: Option<String>,
}

/// The default blank lines left after a question.
const fn default_answer_space() -> usize {
    3
}

impl Default for Layout {
    /// A spec with no `layout:` block still has a layout.
    fn default() -> Self {
        Self {
            answer_space: default_answer_space(),
            page_break_between: false,
            shuffle_choices: false,
            page_footer: None,
        }
    }
}

/// A validated quiz blueprint.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// The exam's title, shown at the top of every variant.
    pub name: String,
    /// How many variants of this exam to produce.
    #[serde(default = "default_variants")]
    pub variants: usize,
    /// Seed for the draw; absent means a fresh draw on every run.
    #[serde(default)]
    pub seed: Option<u64>,
    /// Path to a Markdown file rendered once above the questions, relative to
    /// the spec's own folder.
    #[serde(default)]
    pub header: Option<String>,
    /// Path to a Markdown file rendered once below the questions, relative to
    /// the spec's own folder.
    #[serde(default)]
    pub footer: Option<String>,
    /// Sheet layout.
    #[serde(default)]
    pub layout: Layout,
    /// The folders to draw from, in the order they appear on the sheet.
    pub groups: Vec<Group>,
}

/// A spec with no `variants:` produces one sheet.
const fn default_variants() -> usize {
    1
}

/// The placeholders a `${...}` template may use.
///
/// `name` and `variant` are substituted as literal text. `page` and `pages`
/// become OOXML page-number *fields*, so they resolve per printed page and are
/// only meaningful in [`Layout::page_footer`].
const TEMPLATE_KEYS: [&str; 4] = ["name", "variant", "page", "pages"];

/// The delimiters are `${...}`, deliberately not `{{...}}`.
///
/// `{{name}}` is already the fill-in-the-blank marker (`model::blank_marker`),
/// so a header or footer that later became a question partial would sprout
/// blanks where it meant to name the exam.
const TEMPLATE_OPEN: &str = "${";

/// The closing delimiter, named so the error messages cannot drift from it.
const TEMPLATE_CLOSE: char = '}';

impl Spec {
    /// Parse and structurally validate a spec from YAML.
    ///
    /// Structural means everything decidable without reading the question
    /// folders. Use [`Spec::check_pools`] for the rest.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Yaml`] if the document is not valid YAML or has an
    /// unknown key, and [`Error::Spec`] naming the offending location if the
    /// blueprint itself does not make sense.
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let mut spec: Self = serde_norway::from_str(yaml)?;
        spec.normalize();
        spec.validate()?;
        Ok(spec)
    }

    /// Trim the authored strings before they are validated.
    ///
    /// What is checked must be what is later used: validating `dir.trim()` and
    /// then storing the untrimmed string would let `dir: " topics "` pass every
    /// check and then resolve to a folder that does not exist.
    fn normalize(&mut self) {
        self.name = self.name.trim().to_owned();
        for group in &mut self.groups {
            group.dir = group.dir.trim().to_owned();
        }
        for path in [&mut self.header, &mut self.footer].into_iter().flatten() {
            *path = path.trim().to_owned();
        }
    }

    /// Check everything decidable without reading the question folders.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Spec`] naming the offending location.
    fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(spec_error("name", "the exam needs a name"));
        }
        if self.variants == 0 {
            return Err(spec_error("variants", "must be at least 1"));
        }
        if self.groups.is_empty() {
            return Err(spec_error("groups", "a quiz needs at least one group"));
        }
        if let Some(footer) = &self.layout.page_footer {
            check_template(footer, "layout.page_footer")?;
        }
        for (key, path) in [("header", &self.header), ("footer", &self.footer)] {
            if let Some(path) = path
                && escapes_dir(path)
            {
                return Err(spec_error(key, "must stay inside the spec's folder"));
            }
        }
        self.validate_groups()
    }

    /// Check each group's folder and count, and that no two groups overlap.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Spec`] naming the offending group.
    fn validate_groups(&self) -> Result<()> {
        for (index, group) in self.groups.iter().enumerate() {
            let at = group_at(index, group);
            let dir = group.dir.as_str();
            if dir.is_empty() {
                return Err(spec_error(&at, "dir must name a folder"));
            }
            if escapes_dir(dir) {
                return Err(spec_error(
                    &at,
                    "dir must stay inside the spec's folder: no `..`, absolute path, or drive prefix",
                ));
            }
            if group.take == Take::Count(0) {
                return Err(spec_error(&at, "take: 0 would contribute no questions"));
            }
        }
        self.validate_no_overlap()
    }

    /// Reject groups whose folders nest, which would draw the same question
    /// twice and produce a duplicate `id` in one exam.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Spec`] naming the later of the two overlapping groups.
    fn validate_no_overlap(&self) -> Result<()> {
        for (index, group) in self.groups.iter().enumerate() {
            let dir = group.dir.as_str();
            let earlier = self.groups.iter().take(index);
            for (other_index, other) in earlier.enumerate() {
                let other_dir = other.dir.as_str();
                if !nests(dir, other_dir) {
                    continue;
                }
                let message = format!(
                    "folder overlaps groups[{other_index}] {other_dir:?}; \
                     a question drawn by both would appear twice"
                );
                return Err(spec_error(&group_at(index, group), &message));
            }
        }
        Ok(())
    }
}

/// Reject `${...}` placeholders that are not in [`TEMPLATE_KEYS`].
///
/// # Errors
///
/// Returns [`Error::Spec`] naming the unknown key and listing the legal ones,
/// so a typo does not silently print itself onto every page.
fn check_template(template: &str, at: &str) -> Result<()> {
    let mut rest = template;
    while let Some((_, after_open)) = rest.split_once(TEMPLATE_OPEN) {
        let Some((key, after_close)) = after_open.split_once(TEMPLATE_CLOSE) else {
            let message = format!("unclosed `{TEMPLATE_OPEN}` placeholder");
            return Err(spec_error(at, &message));
        };
        if !TEMPLATE_KEYS.contains(&key.trim()) {
            let known = TEMPLATE_KEYS.join("`, `");
            let message = format!(
                "unknown placeholder `{TEMPLATE_OPEN}{key}{TEMPLATE_CLOSE}`; known: `{known}`"
            );
            return Err(spec_error(at, &message));
        }
        rest = after_close;
    }
    Ok(())
}

/// How a group is named in an error message: its index and its folder.
fn group_at(index: usize, group: &Group) -> String {
    format!("groups[{index}] {:?}", group.dir)
}

/// Build an [`Error::Spec`] for `at`.
fn spec_error(at: &str, message: &str) -> Error {
    Error::Spec {
        at: at.to_owned(),
        message: message.to_owned(),
    }
}

/// Above this chance of two variants drawing the same question set, warn.
///
/// Five percent is a judgement call, not a statistical threshold: below it the
/// warning would fire on reasonable question pools and be tuned out.
const COLLISION_WARN: f64 = 0.05;

/// Question-set counts are capped here. Anything this large makes collisions
/// negligible, and the exact value stops mattering — so a capped count is
/// treated as "plenty" and never as grounds for refusing a spec.
const COMBINATION_CAP: u128 = 1_000_000;

impl Spec {
    /// Check the spec against how many questions each group's folder holds.
    ///
    /// `pool_sizes` is parallel to [`Spec::groups`]. Split from
    /// [`Spec::from_yaml`] so this module never reads the filesystem: the caller
    /// counts the questions and passes the counts in.
    ///
    /// Returns warnings worth showing the author but not worth refusing to
    /// build — currently, variants that are likely to come out identical.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Spec`] when `pool_sizes` does not have exactly one entry
    /// per group, when a group's folder is empty, when `take:` asks for more
    /// questions than exist, or when the pools cannot possibly produce as many
    /// distinct variants as were asked for.
    pub fn check_pools(&self, pool_sizes: &[usize]) -> Result<Vec<String>> {
        if pool_sizes.len() != self.groups.len() {
            let message = format!(
                "expected one pool size per group, got {} for {} groups",
                pool_sizes.len(),
                self.groups.len()
            );
            return Err(spec_error("groups", &message));
        }
        let mut distinct: u128 = 1;
        for (index, (group, &available)) in self.groups.iter().zip(pool_sizes).enumerate() {
            let drawn = check_group_pool(index, group, available)?;
            distinct = distinct
                .saturating_mul(combinations_capped(available, drawn))
                .min(COMBINATION_CAP);
        }
        self.check_variant_supply(distinct)
    }

    /// Check that `distinct` question sets can supply the requested variants.
    ///
    /// Drawing different questions is not the only way variants differ: with
    /// `shuffle_choices` they also differ in the order of their answer options.
    /// A spec that shuffles therefore *can* produce distinguishable sheets from
    /// one question set, so a short pool is a warning there rather than an
    /// error. Without shuffling, too few question sets is genuinely impossible.
    ///
    /// Shuffling is taken at its word: this layer sees only the blueprint, not
    /// the questions, so it cannot tell whether they have choices to shuffle. A
    /// folder of fill-in-the-blank items set to shuffle passes here and still
    /// prints identical sheets.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Spec`] when nothing can tell the variants apart: fewer
    /// question sets than variants, and no group shuffling its choices.
    fn check_variant_supply(&self, distinct: u128) -> Result<Vec<String>> {
        let variants = u128::try_from(self.variants).unwrap_or(u128::MAX);
        // A capped count is a floor, not the true number, so it must never be
        // the basis for refusing a spec.
        if self.variants <= 1 || distinct >= variants || distinct >= COMBINATION_CAP {
            return Ok(self.collision_warning(distinct));
        }
        if !self.shuffles_any_choices() {
            let message = format!(
                "{} variants asked for but the groups can only make {distinct} distinct \
                 question set(s); widen a `take:` range, add questions, or set \
                 `shuffle_choices: true`",
                self.variants
            );
            return Err(spec_error("variants", &message));
        }
        Ok(vec![format!(
            "only {distinct} distinct question set(s) for {} variants, so some sheets \
             carry the same questions and differ only in the order of their choices",
            self.variants
        )])
    }

    /// The warning, if any, that two variants are apt to draw the same
    /// question set.
    ///
    /// A capped `distinct` is a floor on a much larger true count, so the risk
    /// is negligible and nothing is said.
    fn collision_warning(&self, distinct: u128) -> Vec<String> {
        if distinct >= COMBINATION_CAP {
            return Vec::new();
        }
        let risk = collision_chance(distinct, self.variants);
        if risk <= COLLISION_WARN {
            return Vec::new();
        }
        vec![format!(
            "{:.0}% chance two of the {} variants draw the same questions \
             (only {distinct} distinct question sets are possible)",
            risk * 100.0,
            self.variants
        )]
    }

    /// Whether any group permutes its answer choices per variant.
    ///
    /// A group's own `shuffle_choices` wins; otherwise the layout's setting
    /// applies. One shuffling group is enough to tell two sheets apart — as far
    /// as the blueprint can see. Whether those questions actually have choices,
    /// and enough of them to make the requested number of variants, is only
    /// knowable once the questions are loaded.
    fn shuffles_any_choices(&self) -> bool {
        self.groups
            .iter()
            .any(|group| group.shuffle_choices.unwrap_or(self.layout.shuffle_choices))
    }
}

/// Check one group against its pool, returning how many questions it will draw.
///
/// # Errors
///
/// Returns [`Error::Spec`] when the folder is empty or holds fewer questions
/// than `take:` asks for. A short folder is an error rather than a silent
/// truncation because the author wrote a specific number, and a sheet quietly
/// missing two questions is worse than a build that stops.
fn check_group_pool(index: usize, group: &Group, available: usize) -> Result<usize> {
    let at = group_at(index, group);
    if available == 0 {
        return Err(spec_error(&at, "folder holds no questions"));
    }
    match group.take {
        Take::All => Ok(available),
        Take::Count(count) if count > available => {
            let message =
                format!("take: {count} but the folder holds only {available} question(s)");
            Err(spec_error(&at, &message))
        }
        Take::Count(count) => Ok(count),
    }
}

/// `C(n, k)`, saturating at [`COMBINATION_CAP`].
///
/// Exact for the small pools a question folder holds; beyond the cap the true
/// value cannot change any decision made from it.
fn combinations_capped(n: usize, k: usize) -> u128 {
    if k > n {
        return 0;
    }
    let k = k.min(n - k);
    let mut result: u128 = 1;
    for step in 1..=k {
        let (Ok(factor), Ok(divisor)) = (u128::try_from(n - k + step), u128::try_from(step)) else {
            return COMBINATION_CAP;
        };
        // The running value is always a binomial, so the division is exact and
        // no rounding creeps in.
        result = result.saturating_mul(factor) / divisor;
        if result >= COMBINATION_CAP {
            return COMBINATION_CAP;
        }
    }
    result
}

/// The chance that at least two of `variants` independent draws from `distinct`
/// equally likely question sets coincide — the birthday problem.
///
/// Callers must keep `variants` within `distinct` (or at most 1); with
/// `distinct` capped at [`COMBINATION_CAP`] that bounds the loop and keeps the
/// running count inside a `u32`.
fn collision_chance(distinct: u128, variants: usize) -> f64 {
    if distinct == 0 {
        return 1.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a probability needs no more precision than f64 gives, and the count is capped"
    )]
    let total = distinct as f64;
    let mut all_distinct = 1.0_f64;
    for taken in 0..variants {
        let taken = f64::from(u32::try_from(taken).unwrap_or(u32::MAX));
        all_distinct *= (total - taken).max(0.0) / total;
    }
    1.0 - all_distinct
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid spec, with `{EXTRA}` replaced by the test's own keys.
    fn yaml(extra: &str) -> String {
        format!("name: Exam 1\n{extra}groups:\n  - dir: topics/trees\n    take: 3\n")
    }

    /// Parse a spec that is expected to be valid.
    fn parse(extra: &str) -> Spec {
        Spec::from_yaml(&yaml(extra)).expect("spec should parse")
    }

    /// The rendered error from a spec that is expected to be rejected.
    fn reject(extra: &str) -> String {
        Spec::from_yaml(&yaml(extra))
            .expect_err("spec should be rejected")
            .to_string()
    }

    #[test]
    /// A spec with only the required keys parses, and every optional key takes
    /// its documented default.
    fn minimal_spec_uses_documented_defaults() {
        let spec = parse("");
        assert_eq!(spec.name, "Exam 1");
        assert_eq!(spec.variants, 1);
        assert_eq!(spec.seed, None);
        assert_eq!(spec.layout.answer_space, 3);
        assert!(!spec.layout.page_break_between);
        assert!(!spec.layout.shuffle_choices);
        assert_eq!(spec.groups.len(), 1);
        assert_eq!(
            spec.groups.first().map(|group| group.take),
            Some(Take::Count(3))
        );
    }

    #[test]
    /// A group with no `take:` uses every question in its folder.
    fn a_group_without_take_uses_every_question() {
        let spec = Spec::from_yaml("name: E\ngroups:\n  - dir: t\n").expect("parses");
        assert_eq!(spec.groups.first().map(|group| group.take), Some(Take::All));
    }

    #[test]
    /// `take: all` is the one legal keyword; a near-miss is rejected rather
    /// than silently read as "use every question".
    fn take_accepts_all_but_rejects_typos() {
        let spec = Spec::from_yaml("name: E\ngroups:\n  - dir: t\n    take: all\n");
        assert_eq!(
            spec.expect("all parses")
                .groups
                .first()
                .map(|group| group.take),
            Some(Take::All)
        );
        for typo in ["al", "none", "every", "ALL"] {
            let yaml = format!("name: E\ngroups:\n  - dir: t\n    take: {typo}\n");
            assert!(
                Spec::from_yaml(&yaml).is_err(),
                "take: {typo} should be rejected"
            );
        }
    }

    #[test]
    /// A misspelled key is an error, not a silently ignored one. A blueprint
    /// that quietly drops `variants: 5` would print one sheet and say nothing.
    fn unknown_keys_are_rejected() {
        for bad in ["varients: 5\n", "Layout:\n  answer_space: 2\n"] {
            let error = Spec::from_yaml(&yaml(bad))
                .expect_err("unknown key")
                .to_string();
            assert!(error.contains("unknown field"), "{bad}: {error}");
        }
        for nested in [
            "name: E\ngroups:\n  - dir: t\n    tak: 3\n",
            "name: E\nlayout:\n  answer_spce: 2\ngroups:\n  - dir: t\n",
        ] {
            let error = Spec::from_yaml(nested)
                .expect_err("unknown key")
                .to_string();
            assert!(error.contains("unknown field"), "{nested}: {error}");
        }
    }

    #[test]
    /// The blueprint must describe an actual exam.
    fn structurally_impossible_specs_are_rejected() {
        assert!(reject("variants: 0\n").contains("at least 1"));
        let empty_groups = Spec::from_yaml("name: E\ngroups: []\n")
            .expect_err("no groups")
            .to_string();
        assert!(
            empty_groups.contains("at least one group"),
            "{empty_groups}"
        );
        let blank_name = Spec::from_yaml("name: '  '\ngroups:\n  - dir: t\n")
            .expect_err("blank name")
            .to_string();
        assert!(blank_name.contains("needs a name"), "{blank_name}");
        let zero = "name: E\ngroups:\n  - dir: t\n    take: 0\n";
        assert!(
            Spec::from_yaml(zero)
                .expect_err("take: 0")
                .to_string()
                .contains("no questions")
        );
    }

    #[test]
    /// A group folder may not reach outside the spec's own directory, using the
    /// same rule as `file:` partials and images.
    fn group_dirs_may_not_escape() {
        for dir in ["../other", "/etc", "a/../../b"] {
            let yaml = format!("name: E\ngroups:\n  - dir: {dir}\n");
            let error = Spec::from_yaml(&yaml).expect_err("escape").to_string();
            assert!(error.contains("stay inside"), "{dir}: {error}");
        }
    }

    #[test]
    /// Overlapping groups are rejected: a question inside both would be drawn
    /// twice and collide on its `id` in one exam.
    fn overlapping_groups_are_rejected() {
        let overlapping = [
            ("topics", "topics/trees"),
            ("topics/trees", "topics"),
            ("topics/trees", "topics/trees"),
            ("topics/trees/", "topics/trees"),
        ];
        for (first, second) in overlapping {
            let yaml = format!("name: E\ngroups:\n  - dir: {first}\n  - dir: {second}\n");
            let error = Spec::from_yaml(&yaml).expect_err("overlap").to_string();
            assert!(error.contains("overlaps"), "{first} vs {second}: {error}");
        }
        // Sibling folders that merely share a prefix do not overlap.
        let siblings = "name: E\ngroups:\n  - dir: topics/tree\n  - dir: topics/trees\n";
        assert!(Spec::from_yaml(siblings).is_ok());
    }

    #[test]
    /// Authored strings are trimmed before they are validated, so what passed
    /// the checks is what later gets resolved against the filesystem.
    fn surrounding_whitespace_is_trimmed() {
        let yaml = "name: \"  Exam 1  \"\ngroups:\n  - dir: \"  topics/trees  \"\n";
        let spec = Spec::from_yaml(yaml).expect("parses");
        assert_eq!(spec.name, "Exam 1");
        assert_eq!(
            spec.groups.first().map(|g| g.dir.as_str()),
            Some("topics/trees")
        );
        // Whitespace does not smuggle a duplicate folder past the overlap check.
        let dupes = "name: E\ngroups:\n  - dir: topics\n  - dir: \"  topics  \"\n";
        assert!(Spec::from_yaml(dupes).is_err_and(|e| e.to_string().contains("overlaps")));
    }

    #[test]
    /// Header and footer are paths, held to the same escape rule as every other
    /// authored path in the crate.
    fn header_and_footer_may_not_escape() {
        for key in ["header", "footer"] {
            let yaml = format!("name: E\n{key}: ../outside.md\ngroups:\n  - dir: t\n");
            let error = Spec::from_yaml(&yaml).expect_err("escape").to_string();
            assert!(error.contains("stay inside"), "{key}: {error}");
            assert!(error.contains(key), "error should name the key: {error}");
        }
        let ok = "name: E\nheader: templates/header.md\ngroups:\n  - dir: t\n";
        assert_eq!(
            Spec::from_yaml(ok).expect("parses").header.as_deref(),
            Some("templates/header.md")
        );
    }

    /// A spec drawing `take` from one group, for `variants` variants.
    fn pool_spec(take: &str, variants: usize) -> Spec {
        let yaml =
            format!("name: E\nvariants: {variants}\ngroups:\n  - dir: t\n    take: {take}\n");
        Spec::from_yaml(&yaml).expect("spec parses")
    }

    #[test]
    /// `take: N` against a folder that cannot supply N is an error, naming both
    /// numbers. Silently shipping a short sheet is worse than failing.
    fn take_more_than_the_folder_holds_is_rejected() {
        let spec = pool_spec("5", 1);
        let error = spec.check_pools(&[2]).expect_err("short pool").to_string();
        assert!(error.contains("take: 5"), "{error}");
        assert!(error.contains("only 2"), "{error}");
        // `take: all` is always satisfiable.
        assert!(pool_spec("all", 1).check_pools(&[2]).is_ok());
    }

    #[test]
    /// An empty folder is an error whatever the `take:`.
    fn an_empty_folder_is_rejected() {
        for take in ["all", "1"] {
            let error = pool_spec(take, 1)
                .check_pools(&[0])
                .expect_err("empty folder")
                .to_string();
            assert!(error.contains("no questions"), "{take}: {error}");
        }
    }

    #[test]
    /// A caller passing the wrong number of pool sizes is told so rather than
    /// having groups silently treated as empty.
    fn mismatched_pool_counts_are_rejected() {
        for pools in [&[][..], &[3, 3][..]] {
            let error = pool_spec("1", 1)
                .check_pools(pools)
                .expect_err("mismatch")
                .to_string();
            assert!(error.contains("one pool size per group"), "{error}");
        }
    }

    #[test]
    /// Asking for more variants than the pools can distinguish is refused. Four
    /// questions choose three is four possible sheets, so five variants would
    /// repeat one with certainty.
    fn impossible_variant_counts_are_rejected() {
        let error = pool_spec("3", 5)
            .check_pools(&[4])
            .expect_err("pigeonhole")
            .to_string();
        assert!(error.contains("4 distinct question set"), "{error}");
        // Exactly enough is allowed.
        assert!(pool_spec("3", 4).check_pools(&[4]).is_ok());
    }

    #[test]
    /// Possible but likely-identical variants warn rather than fail: six
    /// questions choose three is twenty sheets, and five draws collide ~42% of
    /// the time.
    fn likely_identical_variants_warn() {
        let warnings = pool_spec("3", 5).check_pools(&[6]).expect("possible");
        assert_eq!(warnings.len(), 1);
        let warning = warnings.first().map(String::as_str).unwrap_or_default();
        assert!(warning.contains("same questions"), "{warning}");
        assert!(warning.contains("20 distinct"), "{warning}");
        // A deep enough pool draws quietly.
        assert!(pool_spec("5", 5).check_pools(&[30]).expect("ok").is_empty());
        // A single variant can never collide with itself.
        assert!(pool_spec("3", 1).check_pools(&[4]).expect("ok").is_empty());
    }

    #[test]
    /// A shared section that permutes its choices is legitimate, not impossible.
    ///
    /// `take: all` draws one question set by definition, so the pigeonhole rule
    /// alone would reject it — yet it is the documented way to write a section
    /// every student answers with the options in a different order.
    fn layout_shuffle_makes_one_question_set_acceptable() {
        let yaml = concat!(
            "name: E\nvariants: 5\n",
            "layout:\n  shuffle_choices: true\n",
            "groups:\n  - dir: t\n    take: all\n",
        );
        let spec = Spec::from_yaml(yaml).expect("parses");
        let warnings = spec
            .check_pools(&[12])
            .expect("shuffling is a real difference");
        let warning = warnings.first().map(String::as_str).unwrap_or_default();
        assert!(warning.contains("order of their choices"), "{warning}");
    }

    #[test]
    /// A per-group override counts just as much as the layout default.
    fn group_shuffle_override_counts_too() {
        let yaml = concat!(
            "name: E\nvariants: 5\n",
            "groups:\n  - dir: t\n    take: all\n    shuffle_choices: true\n",
        );
        let spec = Spec::from_yaml(yaml).expect("parses");
        assert!(spec.check_pools(&[12]).is_ok());
    }

    #[test]
    /// Without shuffling there is nothing to tell the sheets apart, and the
    /// error names the way out rather than just refusing.
    fn without_shuffling_one_question_set_is_impossible() {
        let yaml = "name: E\nvariants: 5\ngroups:\n  - dir: t\n    take: all\n";
        let spec = Spec::from_yaml(yaml).expect("parses");
        let error = spec
            .check_pools(&[12])
            .expect_err("nothing varies")
            .to_string();
        assert!(error.contains("shuffle_choices"), "{error}");
    }

    #[test]
    /// A capped question-set count is a floor, never grounds for refusing: a
    /// pool with astronomically many combinations must not be reported as
    /// having only `COMBINATION_CAP` of them.
    fn a_capped_count_never_refuses_a_spec() {
        let yaml = "name: E\nvariants: 2000000\ngroups:\n  - dir: t\n    take: 25\n";
        let spec = Spec::from_yaml(yaml).expect("parses");
        // 50 choose 25 is ~1.26e14, far more than the variants asked for, but
        // the count saturates at the cap — which must not read as "too few".
        assert!(spec.check_pools(&[50]).expect("plenty").is_empty());
    }

    #[test]
    /// A group must name a folder. `dir` is trimmed first, so empty and
    /// whitespace-only are the same blank — and neither is caught by the escape
    /// rule, which sees no components at all in an empty path.
    fn a_blank_group_dir_is_rejected() {
        for dir in ["\"\"", "\"   \""] {
            let yaml = format!("name: E\ngroups:\n  - dir: {dir}\n");
            let error = Spec::from_yaml(&yaml).expect_err("blank dir").to_string();
            assert!(error.contains("must name a folder"), "{dir}: {error}");
        }
    }

    #[test]
    /// Every rejection names the offending group by index, and an overlap names
    /// both sides, so an author with a dozen groups knows which line to fix.
    fn errors_name_the_offending_group_by_index() {
        let escaping = "name: E\ngroups:\n  - dir: topics/trees\n  - dir: ../elsewhere\n";
        let error = Spec::from_yaml(escaping).expect_err("escape").to_string();
        assert!(error.contains(r#"groups[1] "../elsewhere""#), "{error}");
        let overlap = "name: E\ngroups:\n  - dir: a\n  - dir: b\n  - dir: a/deep\n";
        let error = Spec::from_yaml(overlap).expect_err("overlap").to_string();
        assert!(error.contains(r#"groups[2] "a/deep""#), "{error}");
        assert!(error.contains(r#"groups[0] "a""#), "{error}");
    }

    #[test]
    /// `take:` equal to the pool size is satisfiable — the folder supplies
    /// exactly what was asked for. It draws the whole folder, so it yields one
    /// question set and a second variant has nothing left to differ by.
    fn take_equal_to_the_pool_is_allowed() {
        let warnings = pool_spec("3", 1).check_pools(&[3]).expect("exact fit");
        assert!(warnings.is_empty(), "{warnings:?}");
        let error = pool_spec("3", 2)
            .check_pools(&[3])
            .expect_err("no second set")
            .to_string();
        assert!(error.contains("1 distinct question set"), "{error}");
    }

    #[test]
    /// The warning has a real 5% threshold and quotes the odds: 196 question
    /// sets over five variants collide 5.01% of the time and warn; 197 sets is
    /// 4.99% and stays quiet.
    fn the_collision_warning_fires_at_five_percent() {
        let warnings = pool_spec("1", 5).check_pools(&[196]).expect("possible");
        let warning = warnings.first().map(String::as_str).unwrap_or_default();
        assert!(
            warning.starts_with("5% chance two of the 5 variants"),
            "{warning}"
        );
        let quiet = pool_spec("1", 5)
            .check_pools(&[197])
            .expect("under the bar");
        assert!(quiet.is_empty(), "{quiet:?}");
    }

    #[test]
    /// One shuffling group is enough to tell two sheets apart, even when every
    /// other group is identical on every variant.
    fn one_shuffling_group_among_several_is_enough() {
        let shuffled = concat!(
            "name: E\nvariants: 5\ngroups:\n",
            "  - dir: a\n    take: all\n",
            "  - dir: b\n    take: all\n    shuffle_choices: true\n",
        );
        let spec = Spec::from_yaml(shuffled).expect("parses");
        let warnings = spec
            .check_pools(&[4, 4])
            .expect("shuffling is a difference");
        let warning = warnings.first().map(String::as_str).unwrap_or_default();
        assert!(warning.contains("order of their choices"), "{warning}");
        let plain = shuffled.replace("    shuffle_choices: true\n", "");
        let error = Spec::from_yaml(&plain)
            .expect("parses")
            .check_pools(&[4, 4])
            .expect_err("nothing varies")
            .to_string();
        assert!(error.contains("shuffle_choices"), "{error}");
    }

    #[test]
    /// `header` and `footer` are trimmed before the escape rule sees them:
    /// `" ../x.md"` has no `..` *component* until the padding is gone, so the
    /// normalize-then-validate order is what closes the hole.
    fn padded_header_paths_are_trimmed_before_the_escape_check() {
        let escaping = "name: E\nheader: \"  ../outside.md  \"\ngroups:\n  - dir: t\n";
        let error = Spec::from_yaml(escaping).expect_err("escape").to_string();
        assert!(error.contains("stay inside"), "{error}");
        let ok = "name: E\nfooter: \"  templates/footer.md  \"\ngroups:\n  - dir: t\n";
        assert_eq!(
            Spec::from_yaml(ok).expect("parses").footer.as_deref(),
            Some("templates/footer.md")
        );
    }

    #[test]
    /// Placeholders may be spaced out inside the braces, and the legal set is
    /// closed: a plausible-but-unsupported key is still an error.
    fn placeholder_keys_are_trimmed_and_the_set_is_closed() {
        let padded = "layout:\n  page_footer: \"${ name } p${ page }\"\n";
        assert!(Spec::from_yaml(&yaml(padded)).is_ok());
        for bad in ["answers", "date", "Name"] {
            let footer = format!("layout:\n  page_footer: \"${{{bad}}}\"\n");
            let error = reject(&footer);
            assert!(error.contains("unknown placeholder"), "{bad}: {error}");
        }
    }

    #[test]
    /// A seed round-trips, and a negative one is rejected by the type.
    fn seed_round_trips_and_rejects_negatives() {
        assert_eq!(parse("seed: 20260915\n").seed, Some(20_260_915));
        assert!(Spec::from_yaml(&yaml("seed: -1\n")).is_err());
    }

    #[test]
    /// Binomials are exact for realistic folders and saturate beyond the cap.
    fn combinations_are_exact_then_capped() {
        assert_eq!(combinations_capped(4, 3), 4);
        assert_eq!(combinations_capped(6, 3), 20);
        assert_eq!(combinations_capped(10, 5), 252);
        assert_eq!(combinations_capped(5, 5), 1);
        assert_eq!(combinations_capped(5, 0), 1);
        assert_eq!(combinations_capped(3, 5), 0); // more than the pool holds
        assert_eq!(combinations_capped(200, 100), 1_000_000);
        assert_eq!(COMBINATION_CAP, 1_000_000);
    }

    #[test]
    /// The birthday calculation matches its known values.
    fn collision_chance_matches_known_values() {
        assert!((collision_chance(4, 1) - 0.0).abs() < 1e-9);
        assert!((collision_chance(4, 5) - 1.0).abs() < 1e-9); // pigeonhole
        // 6 choose 3 = 20 distinct sheets, 5 variants -> ~41.9%.
        assert!((collision_chance(20, 5) - 0.418_4).abs() < 1e-3);
        assert!((collision_chance(0, 3) - 1.0).abs() < 1e-9);
    }

    #[test]
    /// Distinct exams multiply across groups, so several shallow groups can
    /// still support many variants.
    fn distinct_exams_multiply_across_groups() {
        // `concat!` rather than a wrapped literal: rustfmt reflows a continued
        // string and would silently corrupt the YAML indentation.
        let yaml = concat!(
            "name: E\nvariants: 5\ngroups:\n",
            "  - dir: a\n    take: 1\n",
            "  - dir: b\n    take: 1\n",
            "  - dir: c\n    take: 1\n",
        );
        let spec = Spec::from_yaml(yaml).expect("parses");
        // 3 x 3 x 3 = 27 possible exams. Enough to build 5 variants, but five
        // draws from 27 still collide about a third of the time, so it warns.
        let warnings = spec.check_pools(&[3, 3, 3]).expect("possible");
        let warning = warnings.first().map(String::as_str).unwrap_or_default();
        assert!(warning.contains("27 distinct"), "{warning}");
        // 10 x 10 x 10 = 1000: deep enough to draw 5 variants quietly.
        assert!(spec.check_pools(&[10, 10, 10]).expect("ok").is_empty());
        // 2 x 1 x 1 = 2 possible exams: cannot make 5 distinct variants at all.
        // Asserted by reason, not just `is_err`: an unrelated fault in the
        // short-pool check would otherwise fail this test for the wrong cause.
        let error = spec
            .check_pools(&[2, 1, 1])
            .expect_err("too few")
            .to_string();
        assert!(error.contains("2 distinct question set"), "{error}");
    }

    #[test]
    /// Only the documented placeholders are accepted, and the error lists them.
    fn page_footer_placeholders_are_checked() {
        let ok = "layout:\n  page_footer: \"${name} (${variant}) p${page}/${pages}\"\n";
        assert!(Spec::from_yaml(&yaml(ok)).is_ok());
        let bad = "layout:\n  page_footer: \"${nmae}\"\n";
        let error = reject(bad);
        assert!(error.contains("unknown placeholder"), "{error}");
        assert!(error.contains("variant"), "error should list known keys");
        let unclosed = "layout:\n  page_footer: \"${name\"\n";
        assert!(reject(unclosed).contains("unclosed"));
    }
}
