//! Turn a [`Spec`] and a tree of question files into printable [`Exam`]s.
//!
//! This is where every random decision is made. Which questions each variant
//! draws, and the order of their answer choices, are settled here and frozen
//! into the [`Exam`]; the writers that follow are pure functions of that value.
//! Keeping it that way is what stops a shuffled sheet and its answer key from
//! disagreeing — they are two renderings of one already-decided thing.
//!
//! That includes matching and ordering, whose presented order used to be
//! derived at render time (a sort over the option text) and so was identical
//! on every variant. It is settled here now and frozen into
//! [`ExamItem::option_order`], which is also what lets `shuffle_choices` reach
//! it.
//!
//! Diagram rendering is *not* done here yet. When it lands it belongs on each
//! group pool's questions, before the variant loop: the questions are shared,
//! so rendering there runs `mmdc`/`dot` once rather than once per variant.
//! Putting it in a writer would undo that.
//!
//! Nothing here touches the filesystem. The caller injects a [`SourceLister`]
//! that turns a group's folder into question sources and a
//! [`PartialReader`] that reads a file by path, so
//! the whole assembly is testable without a directory on disk.

use crate::model::Question;
use crate::parse::{self, PartialReader};
use crate::path::parent_dir;
use crate::quiz::exam::{Exam, ExamItem};
use crate::quiz::sample::{self, SampleRng, Source};
use crate::quiz::spec::{Group, Spec, Take, group_at};
use crate::{Error, Result};

/// Lists the question sources inside a group's folder.
///
/// Given a group's `dir` (relative to the spec), yields `(path, contents)` pairs
/// whose paths are also relative to the spec — so a question's own folder, and
/// therefore its partials and images, can be recovered from its path. Searching
/// recursively is the lister's job.
pub type SourceLister<'a> = dyn Fn(&str) -> std::result::Result<Vec<Source>, String> + 'a;

/// Everything assembly produces: the sheets, and anything worth telling the
/// author that did not justify refusing to build.
#[derive(Debug)]
pub struct Assembly {
    /// One exam per requested variant, in variant order.
    pub exams: Vec<Exam>,
    /// Warnings from validating the spec against the real question pools.
    pub warnings: Vec<String>,
}

/// One group's questions, parsed once and shared by every variant.
struct Pool {
    /// How many questions to take from it.
    take: Take,
    /// Blank lines to leave after each of its questions.
    answer_space: usize,
    /// Whether its answer choices are permuted per variant.
    shuffle_choices: bool,
    /// Its questions, keyed by spec-relative path so a draw stays reproducible.
    questions: Vec<(String, Question)>,
}

impl Pool {
    /// Just the questions, without their paths.
    fn questions(&self) -> impl Iterator<Item = &Question> {
        self.questions.iter().map(|(_, question)| question)
    }
}

/// Assemble `spec` into one [`Exam`] per variant.
///
/// Questions are parsed once and shared across variants; only the drawing and
/// shuffling is repeated, seeded per variant from `seed` so each sheet is
/// independently reproducible.
///
/// `spec.seed` is deliberately *not* consulted. The caller resolves it against
/// any `--seed` flag and entropy and passes the result, which keeps this a pure
/// function of its arguments — the same reason [`SampleRng::seeded`] takes a
/// seed rather than finding one.
///
/// # Errors
///
/// Returns [`Error::Spec`] if a group's folder cannot be listed, if the spec's
/// `header:`/`footer:` file cannot be read, or if the groups fail validation
/// against their real question counts; [`Error::InvalidQuestion`] if two
/// questions share an `id`; and [`Error::QuestionFile`] naming any question that
/// will not parse.
pub fn assemble(
    spec: &Spec,
    list: &SourceLister<'_>,
    read: &PartialReader<'_>,
    seed: u64,
) -> Result<Assembly> {
    let pools = load_pools(spec, list, read)?;
    // Across groups, not just within one: `path::nests` stops two groups naming
    // the same folder, but nothing stops two folders holding a copy-pasted id,
    // and an exam carrying it twice cannot be keyed.
    parse::check_unique_ids(pools.iter().flat_map(Pool::questions))?;
    let sizes: Vec<usize> = pools.iter().map(|pool| pool.questions.len()).collect();
    let warnings = spec.check_pools(&sizes)?;
    let blocks = Blocks::read(spec, read)?;
    let exams = (0..spec.variants)
        .map(|index| blocks.exam(spec, &pools, seed, index))
        .collect();
    Ok(Assembly { exams, warnings })
}

/// The prose blocks every variant shares, read once.
struct Blocks {
    /// Markdown rendered above the questions.
    header: Option<String>,
    /// Markdown rendered below them.
    footer: Option<String>,
}

impl Blocks {
    /// Load the spec's header and footer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Spec`] naming the key whose file cannot be read.
    fn read(spec: &Spec, read: &PartialReader<'_>) -> Result<Self> {
        Ok(Self {
            header: read_optional(spec.header.as_deref(), read, "header")?,
            footer: read_optional(spec.footer.as_deref(), read, "footer")?,
        })
    }

    /// Build the exam for variant `index`, drawing from its own seeded stream.
    fn exam(&self, spec: &Spec, pools: &[Pool], seed: u64, index: usize) -> Exam {
        let stream = u64::try_from(index).unwrap_or(u64::MAX);
        let mut rng = SampleRng::derived(seed, stream);
        Exam {
            name: spec.name.clone(),
            variant: Exam::variant_label(index, spec.variants),
            header: self.header.clone(),
            footer: self.footer.clone(),
            layout: spec.layout.clone(),
            items: draw_items(pools, &mut rng),
        }
    }
}

/// Load and parse every group's questions, once.
///
/// # Errors
///
/// Returns [`Error::Spec`] if a folder cannot be listed, or
/// [`Error::QuestionFile`] naming the question file that failed to parse.
fn load_pools(spec: &Spec, list: &SourceLister<'_>, read: &PartialReader<'_>) -> Result<Vec<Pool>> {
    let mut pools = Vec::with_capacity(spec.groups.len());
    for (index, group) in spec.groups.iter().enumerate() {
        let sources = list(&group.dir).map_err(|message| Error::Spec {
            at: group_at(index, group),
            message,
        })?;
        let mut parsed = Vec::with_capacity(sources.len());
        for (path, contents) in sources {
            parsed.push((path.clone(), parse_one(&path, &contents, read)?));
        }
        // Path order, so a seeded draw does not depend on how the caller listed.
        parsed.sort_by(|a, b| a.0.cmp(&b.0));
        pools.push(pool_for(spec, group, parsed));
    }
    Ok(pools)
}

/// Build a group's pool, resolving its layout overrides against the spec's.
fn pool_for(spec: &Spec, group: &Group, questions: Vec<(String, Question)>) -> Pool {
    Pool {
        take: group.take,
        answer_space: group.answer_space.unwrap_or(spec.layout.answer_space),
        shuffle_choices: group.shuffle_choices.unwrap_or(spec.layout.shuffle_choices),
        questions,
    }
}

/// Parse one question, resolving its partials and images against its own folder.
///
/// # Errors
///
/// Returns the parse failure with the offending file named.
fn parse_one(path: &str, contents: &str, read: &PartialReader<'_>) -> Result<Question> {
    let base = parent_dir(path).to_owned();
    let scoped = |relative: &str| read(&join(&base, relative));
    let mut question =
        parse::parse_question_with(contents, &scoped).map_err(|error| Error::QuestionFile {
            file: path.to_owned(),
            message: error.to_string(),
        })?;
    parse::rebase_local_paths(&mut question, &base);
    Ok(question)
}

/// Join a spec-relative `base` folder and a `relative` path.
fn join(base: &str, relative: &str) -> String {
    if base.is_empty() {
        return relative.to_owned();
    }
    format!("{base}/{relative}")
}

/// Read an optional spec-relative file, naming the key if it cannot be read.
///
/// # Errors
///
/// Returns [`Error::Spec`] naming `key` when the file cannot be read.
fn read_optional(
    path: Option<&str>,
    read: &PartialReader<'_>,
    key: &str,
) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    read(path).map(Some).map_err(|message| Error::Spec {
        at: key.to_owned(),
        message: format!("cannot read {path:?}: {message}"),
    })
}

/// Draw one variant's items from every pool, in group order.
fn draw_items(pools: &[Pool], rng: &mut SampleRng) -> Vec<ExamItem> {
    let mut items = Vec::new();
    for pool in pools {
        for (_, mut question) in take_from(pool, rng) {
            if pool.shuffle_choices {
                shuffle_choices(&mut question, rng);
            }
            let option_order = option_order(&question, pool.shuffle_choices, rng);
            items.push(ExamItem {
                question,
                answer_space: pool.answer_space,
                option_order,
            });
        }
    }
    items
}

/// The questions this variant draws from one pool, in path order.
///
/// `Take::All` still goes through the sampler, which returns a group no larger
/// than its limit untouched and consumes no randomness — so a shared section
/// does not shift any later group's draw.
fn take_from(pool: &Pool, rng: &mut SampleRng) -> Vec<(String, Question)> {
    let limit = match pool.take {
        Take::All => pool.questions.len(),
        Take::Count(count) => count,
    };
    sample::within_group(pool.questions.clone(), limit, rng)
}

/// Permute a question's answer choices in place, if it has any.
///
/// Only the kinds whose payload *is* the printed list. Matching and ordering
/// must keep their authored order — it is the answer — so their presentation
/// is a separate permutation, held in [`ExamItem::option_order`] by
/// [`option_order`]. True/false and fill-in-the-blank have nothing to reorder.
fn shuffle_choices(question: &mut Question, rng: &mut SampleRng) {
    use crate::model::QuestionKind;
    match &mut question.kind {
        QuestionKind::MultipleChoice(set) => sample::shuffle(&mut set.choices, rng),
        QuestionKind::MultipleSelect(set) => sample::shuffle(&mut set.choices, rng),
        QuestionKind::TrueFalse(_)
        | QuestionKind::FillInBlank(_)
        | QuestionKind::Matching(_)
        | QuestionKind::Ordering(_) => {}
    }
}

/// The order `question`'s options are printed in, as indices into the list its
/// kind presents.
///
/// Empty for the kinds that have no separate presentation: their payload is
/// already the printed list. For matching and ordering the authored order is
/// the answer, so it is never used as-is — `display_order` text-sorts it when
/// the group does not shuffle, a permutation replaces it when the group does,
/// and either way [`crate::model::hides_the_answer`] keeps it off the
/// identity.
fn option_order(question: &Question, shuffle: bool, rng: &mut SampleRng) -> Vec<usize> {
    use crate::model::QuestionKind;
    let mut order = match &question.kind {
        QuestionKind::Ordering(ordering) => ordering.display_order(),
        QuestionKind::Matching(matching) => matching.display_order(),
        QuestionKind::TrueFalse(_)
        | QuestionKind::FillInBlank(_)
        | QuestionKind::MultipleChoice(_)
        | QuestionKind::MultipleSelect(_) => return Vec::new(),
    };
    if shuffle {
        sample::shuffle(&mut order, rng);
    }
    crate::model::hides_the_answer(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::QuestionKind;
    use std::collections::HashMap;

    /// A true/false question file with the given id.
    fn true_false(id: &str) -> String {
        format!("---\nid: {id}\nkind: true_false\nanswer: true\n---\n\n{id}?\n")
    }

    /// A multiple-choice question file with four lettered options.
    fn multiple_choice(id: &str) -> String {
        format!(
            "---\nid: {id}\nkind: multiple_choice\nchoices:\n  \
             - text: alpha\n    correct: true\n  - text: beta\n  \
             - text: gamma\n  - text: delta\n---\n\n{id}?\n"
        )
    }

    /// A lister over an in-memory tree of `dir -> [(path, contents)]`.
    fn lister(
        tree: HashMap<String, Vec<Source>>,
    ) -> impl Fn(&str) -> std::result::Result<Vec<Source>, String> {
        move |dir: &str| {
            tree.get(dir)
                .cloned()
                .ok_or_else(|| format!("no such folder: {dir}"))
        }
    }

    /// A tree with one folder of `count` true/false questions.
    fn simple_tree(dir: &str, count: usize) -> HashMap<String, Vec<Source>> {
        // Ids carry the folder: question ids must be unique across the whole
        // exam, not just within one group.
        let sources = (0..count)
            .map(|n| (format!("{dir}/q{n}.md"), true_false(&format!("{dir}-q{n}"))))
            .collect();
        HashMap::from([(dir.to_owned(), sources)])
    }

    /// A multiple-choice question's options as `(text, correct)`, in the order
    /// they will be printed. Empty for any other kind, which fails the
    /// assertions that use it rather than panicking.
    fn options(question: &Question) -> Vec<(String, bool)> {
        match &question.kind {
            QuestionKind::MultipleChoice(set) => set
                .choices
                .iter()
                .map(|choice| (choice.text.clone(), choice.correct))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A matching question whose `pairs` are its answer key.
    const MATCHING: &str = concat!(
        "---\nid: m\nkind: matching\npairs:\n  - left: char\n    right: 1 byte\n",
        "  - left: int\n    right: 4 bytes\ndistractors: [8 bytes]\n---\n\nMatch.\n",
    );

    /// An ordering question, authored in the correct order.
    const ORDERING: &str =
        "---\nid: o\nkind: ordering\nitems:\n  - Compile\n  - Link\n  - Run\n---\n\nOrder.\n";

    /// A fill-in-the-blank question with two named blanks.
    const FILL_IN_BLANK: &str = concat!(
        "---\nid: f\nkind: fill_in_blank\nblanks:\n  one: [alpha]\n  two: [beta]\n",
        "---\n\n{{one}} then {{two}}.\n",
    );

    /// The stored presentation order of the item with `id`, on every exam.
    fn option_orders(assembly: &Assembly, id: &str) -> Vec<Vec<usize>> {
        assembly
            .exams
            .iter()
            .filter_map(|exam| {
                exam.items
                    .iter()
                    .find(|item| item.question.id == id)
                    .map(|item| item.option_order.clone())
            })
            .collect()
    }

    #[test]
    /// A presented order is never the authored one, even where the text sort
    /// lands back on it. Both fixtures are that awkward case: `Compile, Link,
    /// Run` is already alphabetical, and a matching question's options sort
    /// into the order that pairs each with the prompt it answers. The sort
    /// alone would therefore print an ordering question's answer on the sheet
    /// and line every match up with its own prompt.
    fn a_presented_order_is_never_the_authored_one() {
        let yaml = "name: E\ngroups:\n  - dir: t\n    take: all\n";
        let tree = HashMap::from([(
            "t".to_owned(),
            vec![
                ("t/m.md".to_owned(), MATCHING.to_owned()),
                ("t/o.md".to_owned(), ORDERING.to_owned()),
            ],
        )]);
        let assembly = run(yaml, tree, 3);
        for id in ["m", "o"] {
            let order = option_orders(&assembly, id).pop().expect("one exam");
            assert!(order.len() > 1, "{id} stored no order at all");
            assert!(
                order.iter().enumerate().any(|(at, index)| at != *index),
                "{id} presents its options in the authored order: {order:?}"
            );
        }
    }

    #[test]
    /// A shuffling group varies the presented order between variants, which is
    /// the whole reason it is stored rather than derived: a sort over the
    /// option text is identical on every sheet.
    fn a_shuffling_group_varies_the_presented_order() {
        let yaml = concat!(
            "name: E\nvariants: 6\n",
            "groups:\n  - dir: t\n    take: all\n    shuffle_choices: true\n",
        );
        let tree = HashMap::from([(
            "t".to_owned(),
            vec![("t/o.md".to_owned(), ORDERING.to_owned())],
        )]);
        let distinct: std::collections::HashSet<Vec<usize>> =
            option_orders(&run(yaml, tree, 11), "o")
                .into_iter()
                .collect();
        assert!(
            distinct.len() > 1,
            "six variants all presented one order: {distinct:?}"
        );
    }

    #[test]
    /// Without shuffling the presented order is the text sort, rotated off the
    /// authored order, and it does not depend on the seed at all — no
    /// randomness is consumed deciding it.
    ///
    /// `Compile, Link, Run` sorts to `[0, 1, 2]`, which is the answer, so the
    /// stored order is that rotated by one.
    fn the_presented_order_is_the_text_sort_when_nothing_shuffles() {
        let yaml = "name: E\ngroups:\n  - dir: t\n    take: all\n";
        let tree = HashMap::from([(
            "t".to_owned(),
            vec![("t/o.md".to_owned(), ORDERING.to_owned())],
        )]);
        for seed in [7, 11] {
            assert_eq!(
                option_orders(&run(yaml, tree.clone(), seed), "o"),
                vec![vec![1, 2, 0]],
                "seed {seed} changed an order nothing shuffles"
            );
        }
    }

    #[test]
    /// The kinds whose payload is already the printed list store no separate
    /// order. A writer that found one would have two lists to reconcile, and
    /// nothing says which of them the labels belong to.
    fn kinds_without_a_separate_presentation_store_no_order() {
        let yaml = "name: E\ngroups:\n  - dir: t\n    take: all\n    shuffle_choices: true\n";
        let tree = HashMap::from([(
            "t".to_owned(),
            vec![
                ("t/c.md".to_owned(), multiple_choice("c")),
                ("t/f.md".to_owned(), FILL_IN_BLANK.to_owned()),
                ("t/t.md".to_owned(), true_false("t")),
            ],
        )]);
        let assembly = run(yaml, tree, 5);
        for id in ["c", "f", "t"] {
            assert_eq!(
                option_orders(&assembly, id).pop().expect("one exam"),
                Vec::<usize>::new(),
                "{id} stored a presentation order it has no list for"
            );
        }
    }

    /// An ordering question's items in stored order; empty for other kinds.
    fn ordering_items(question: &Question) -> Vec<String> {
        match &question.kind {
            QuestionKind::Ordering(ordering) => ordering.items.clone(),
            _ => Vec::new(),
        }
    }

    /// A matching question's pairs as `left -> right`; empty for other kinds.
    fn matching_pairs(question: &Question) -> Vec<String> {
        match &question.kind {
            QuestionKind::Matching(matching) => matching
                .pairs
                .iter()
                .map(|pair| format!("{} -> {}", pair.left, pair.right))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A fill-in-the-blank question's blank ids in stored order.
    fn blank_ids(question: &Question) -> Vec<String> {
        match &question.kind {
            QuestionKind::FillInBlank(fitb) => {
                fitb.blanks.iter().map(|blank| blank.id.clone()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// A reader over an in-memory `path -> contents` map.
    fn files(entries: &[(&str, &str)]) -> impl Fn(&str) -> std::result::Result<String, String> {
        let map: HashMap<String, String> = entries
            .iter()
            .map(|(path, body)| ((*path).to_owned(), (*body).to_owned()))
            .collect();
        move |path: &str| {
            map.get(path)
                .cloned()
                .ok_or_else(|| format!("no such file: {path}"))
        }
    }

    /// The assembled question with `id` on the first exam.
    fn find<'a>(assembly: &'a Assembly, id: &str) -> Option<&'a Question> {
        assembly
            .exams
            .first()?
            .items
            .iter()
            .map(|item| &item.question)
            .find(|question| question.id == id)
    }

    /// A tree with one folder holding a true/false question per id.
    fn tree_with(dir: &str, ids: &[&str]) -> HashMap<String, Vec<Source>> {
        let sources = ids
            .iter()
            .map(|id| (format!("{dir}/{id}.md"), true_false(id)))
            .collect();
        HashMap::from([(dir.to_owned(), sources)])
    }

    /// A reader that finds nothing, for specs with no partials.
    fn no_files(path: &str) -> std::result::Result<String, String> {
        Err(format!("no such file: {path}"))
    }

    /// Assemble `yaml` against `tree`, seeded.
    fn run(yaml: &str, tree: HashMap<String, Vec<Source>>, seed: u64) -> Assembly {
        let spec = Spec::from_yaml(yaml).expect("spec parses");
        assemble(&spec, &lister(tree), &no_files, seed).expect("assembles")
    }

    /// The question ids on each exam, in order.
    fn ids(assembly: &Assembly) -> Vec<Vec<String>> {
        assembly
            .exams
            .iter()
            .map(|exam| {
                exam.items
                    .iter()
                    .map(|item| item.question.id.clone())
                    .collect()
            })
            .collect()
    }

    #[test]
    /// One variant per request, each drawing the asked-for number of questions.
    fn produces_one_exam_per_variant() {
        let yaml = "name: E\nvariants: 3\ngroups:\n  - dir: t\n    take: 2\n";
        let assembly = run(yaml, simple_tree("t", 6), 1);
        assert_eq!(assembly.exams.len(), 3);
        for exam in &assembly.exams {
            assert_eq!(exam.items.len(), 2);
        }
        let labels: Vec<Option<&str>> = assembly
            .exams
            .iter()
            .map(|exam| exam.variant.as_deref())
            .collect();
        assert_eq!(labels, [Some("A"), Some("B"), Some("C")]);
    }

    #[test]
    /// The same seed rebuilds the same sheets exactly.
    fn a_seed_reproduces_every_variant() {
        let yaml = "name: E\nvariants: 4\ngroups:\n  - dir: t\n    take: 3\n";
        let first = run(yaml, simple_tree("t", 9), 20_260_915);
        let again = run(yaml, simple_tree("t", 9), 20_260_915);
        assert_eq!(ids(&first), ids(&again));
        let other = run(yaml, simple_tree("t", 9), 7);
        assert_ne!(ids(&first), ids(&other), "a different seed should differ");
    }

    #[test]
    /// Adding variants leaves the existing ones untouched, because each draws
    /// from its own derived stream rather than a shared sequence.
    fn adding_a_variant_does_not_disturb_the_others() {
        let three = "name: E\nvariants: 3\ngroups:\n  - dir: t\n    take: 2\n";
        let five = "name: E\nvariants: 5\ngroups:\n  - dir: t\n    take: 2\n";
        let before = ids(&run(three, simple_tree("t", 8), 42));
        let after = ids(&run(five, simple_tree("t", 8), 42));
        assert_eq!(after.len(), 5);
        assert_eq!(before, after.get(..3).unwrap_or_default().to_vec());
    }

    #[test]
    /// Each variant generally draws a different question set.
    fn variants_differ_from_one_another() {
        let yaml = "name: E\nvariants: 5\ngroups:\n  - dir: t\n    take: 3\n";
        let drawn = ids(&run(yaml, simple_tree("t", 12), 3));
        let distinct: std::collections::HashSet<Vec<String>> = drawn.iter().cloned().collect();
        assert!(distinct.len() > 1, "every variant drew the same questions");
    }

    #[test]
    /// Answer space is resolved during assembly: a group override beats the
    /// layout default, so a writer reads a number and never re-derives it.
    fn answer_space_precedence_is_resolved_here() {
        let yaml = concat!(
            "name: E\nlayout:\n  answer_space: 2\n",
            "groups:\n  - dir: a\n    take: all\n",
            "  - dir: b\n    take: all\n    answer_space: 9\n",
        );
        let mut tree = simple_tree("a", 1);
        tree.extend(simple_tree("b", 1));
        let assembly = run(yaml, tree, 1);
        let spaces: Vec<usize> = assembly
            .exams
            .first()
            .map(|exam| exam.items.iter().map(|item| item.answer_space).collect())
            .unwrap_or_default();
        assert_eq!(spaces, [2, 9]);
    }

    #[test]
    /// Choice shuffling happens during assembly and is frozen into the exam, so
    /// a sheet and its key cannot disagree about which option is which. The
    /// correct answer travels with its text rather than staying at one index.
    fn choices_are_shuffled_into_the_exam_not_left_to_the_writer() {
        let yaml = concat!(
            "name: E\nvariants: 6\n",
            "groups:\n  - dir: t\n    take: all\n    shuffle_choices: true\n",
        );
        let tree = HashMap::from([(
            "t".to_owned(),
            vec![("t/q.md".to_owned(), multiple_choice("q"))],
        )]);
        let assembly = run(yaml, tree, 5);
        let mut orders = std::collections::HashSet::new();
        for exam in &assembly.exams {
            let item = exam.items.first().expect("one item");
            let options = options(&item.question);
            assert_eq!(options.len(), 4);
            orders.insert(
                options
                    .iter()
                    .map(|(text, _)| text.clone())
                    .collect::<Vec<String>>(),
            );
            // Exactly one option is correct, and it is still "alpha" whatever
            // position it moved to — the answer follows the text, not the slot.
            let correct: Vec<&str> = options
                .iter()
                .filter(|(_, correct)| *correct)
                .map(|(text, _)| text.as_str())
                .collect();
            assert_eq!(correct, ["alpha"]);
        }
        assert!(orders.len() > 1, "six variants all used one choice order");
    }

    #[test]
    /// Without `shuffle_choices` the authored order is preserved exactly.
    fn choices_keep_their_authored_order_by_default() {
        let yaml = "name: E\ngroups:\n  - dir: t\n    take: all\n";
        let tree = HashMap::from([(
            "t".to_owned(),
            vec![("t/q.md".to_owned(), multiple_choice("q"))],
        )]);
        let assembly = run(yaml, tree, 5);
        let item = assembly
            .exams
            .first()
            .and_then(|exam| exam.items.first())
            .expect("one item");
        let texts: Vec<String> = options(&item.question)
            .into_iter()
            .map(|(text, _)| text)
            .collect();
        assert_eq!(texts, ["alpha", "beta", "gamma", "delta"]);
    }

    #[test]
    /// Shuffling touches only kinds that carry an answer-choice list. An
    /// ordering question stores its items in the *correct* order and the key
    /// prints them straight from that vector, so permuting them here would make
    /// the key assert a wrong sequence; a matching question's pairs *are* the
    /// key.
    fn shuffling_leaves_kinds_without_choices_alone() {
        let yaml = concat!(
            "name: E\nvariants: 4\n",
            "groups:\n  - dir: t\n    take: all\n    shuffle_choices: true\n",
        );
        let tree = HashMap::from([(
            "t".to_owned(),
            vec![
                ("t/m.md".to_owned(), MATCHING.to_owned()),
                ("t/o.md".to_owned(), ORDERING.to_owned()),
                ("t/f.md".to_owned(), FILL_IN_BLANK.to_owned()),
            ],
        )]);
        let assembly = run(yaml, tree, 9);
        assert_eq!(assembly.exams.len(), 4);
        for exam in &assembly.exams {
            for question in exam.items.iter().map(|item| &item.question) {
                match question.id.as_str() {
                    "o" => assert_eq!(ordering_items(question), ["Compile", "Link", "Run"]),
                    "m" => assert_eq!(
                        matching_pairs(question),
                        ["char -> 1 byte", "int -> 4 bytes"]
                    ),
                    _ => assert_eq!(blank_ids(question), ["one", "two"]),
                }
            }
        }
    }

    #[test]
    /// A question's partials and images resolve against its *own* folder, not
    /// the group's and not its full path, so the bundler is handed the file the
    /// author meant.
    fn partials_and_images_resolve_against_the_question_folder() {
        let source = concat!(
            "---\nid: q\nkind: multiple_choice\nchoices:\n",
            "  - file: shared/yes.md\n    correct: true\n  - text: no\n---\n\n",
            "Which? ![d](pic.png)\n",
        );
        let tree = HashMap::from([(
            "t".to_owned(),
            vec![("t/sub/q.md".to_owned(), source.to_owned())],
        )]);
        let read = files(&[("t/sub/shared/yes.md", "yes ![p](inner.png)")]);
        let spec = Spec::from_yaml("name: E\ngroups:\n  - dir: t\n").expect("parses");
        let assembly = assemble(&spec, &lister(tree), &read, 1).expect("assembles");
        let question = find(&assembly, "q").expect("the question");
        assert!(
            question.prompt.contains("![d](t/sub/pic.png)"),
            "prompt image not rebased onto the question folder: {}",
            question.prompt
        );
        let (text, correct) = options(question).first().cloned().unwrap_or_default();
        assert!(
            correct,
            "the partial choice should still be the correct one"
        );
        assert!(
            text.contains("![p](t/sub/shared/inner.png)"),
            "partial resolved or rebased against the wrong folder: {text:?}"
        );
    }

    #[test]
    /// A question at the top of its group has no folder to resolve against, so
    /// its partial is read at the path as authored, with no leading separator.
    fn a_top_level_question_resolves_partials_unprefixed() {
        let source = concat!(
            "---\nid: q\nkind: multiple_choice\nchoices:\n",
            "  - file: yes.md\n    correct: true\n  - text: no\n---\n\nWhich?\n",
        );
        let tree = HashMap::from([(".".to_owned(), vec![("q.md".to_owned(), source.to_owned())])]);
        let read = files(&[("yes.md", "yes")]);
        let spec = Spec::from_yaml("name: E\ngroups:\n  - dir: \".\"\n").expect("parses");
        let assembly = assemble(&spec, &lister(tree), &read, 1).expect("assembles");
        let question = find(&assembly, "q").expect("the question");
        let (text, _) = options(question).first().cloned().unwrap_or_default();
        assert!(text.contains("yes"), "partial not resolved: {text:?}");
    }

    #[test]
    /// Header and footer are read once during assembly and travel on every
    /// variant, each in its own field.
    fn header_and_footer_reach_every_variant() {
        let yaml = concat!(
            "name: E\nvariants: 2\nheader: tpl/head.md\nfooter: tpl/foot.md\n",
            "groups:\n  - dir: t\n    take: 1\n",
        );
        let read = files(&[("tpl/head.md", "# Read carefully"), ("tpl/foot.md", "End.")]);
        let spec = Spec::from_yaml(yaml).expect("parses");
        let assembly = assemble(&spec, &lister(simple_tree("t", 4)), &read, 1).expect("assembles");
        assert_eq!(assembly.exams.len(), 2);
        for exam in &assembly.exams {
            assert_eq!(exam.header.as_deref(), Some("# Read carefully"));
            assert_eq!(exam.footer.as_deref(), Some("End."));
        }
    }

    #[test]
    /// A header that cannot be read stops the build and names both the key and
    /// the path, rather than printing a sheet that silently lost it.
    fn an_unreadable_header_names_the_key_and_path() {
        let yaml = "name: E\nheader: tpl/gone.md\ngroups:\n  - dir: t\n";
        let spec = Spec::from_yaml(yaml).expect("parses");
        let error = assemble(&spec, &lister(simple_tree("t", 2)), &no_files, 1)
            .expect_err("unreadable header")
            .to_string();
        assert!(error.contains("header"), "{error}");
        assert!(error.contains("tpl/gone.md"), "{error}");
    }

    #[test]
    /// The exam carries the spec's title and layout, so a writer never has to
    /// consult the spec; a lone variant is left unlabelled.
    fn the_exam_carries_the_specs_name_and_layout() {
        let yaml = concat!(
            "name: Final\nlayout:\n  page_break_between: true\n",
            "  page_footer: \"${name}\"\ngroups:\n  - dir: t\n    take: all\n",
        );
        let assembly = run(yaml, simple_tree("t", 1), 1);
        let exam = assembly.exams.first().expect("one exam");
        assert_eq!(exam.name, "Final");
        assert_eq!(exam.variant, None);
        assert_eq!(exam.title(), "Final");
        assert!(exam.layout.page_break_between);
        assert_eq!(exam.layout.page_footer.as_deref(), Some("${name}"));
        assert_eq!(exam.items.len(), 1);
    }

    #[test]
    /// A group override beats the layout in *both* directions: it can turn
    /// shuffling off where the layout turns it on, and can ask for less answer
    /// space than the layout's default, not only more.
    fn a_group_override_can_lower_the_layouts_setting() {
        let yaml = concat!(
            "name: E\nvariants: 3\nlayout:\n  answer_space: 8\n  shuffle_choices: true\n",
            "groups:\n  - dir: t\n    take: 2\n    answer_space: 1\n",
            "    shuffle_choices: false\n",
        );
        let sources = (0..4)
            .map(|n| (format!("t/q{n}.md"), multiple_choice(&format!("q{n}"))))
            .collect();
        let tree = HashMap::from([("t".to_owned(), sources)]);
        let assembly = run(yaml, tree, 5);
        for item in assembly.exams.iter().flat_map(|exam| &exam.items) {
            assert_eq!(item.answer_space, 1, "the group asked for less, not more");
            let texts: Vec<String> = options(&item.question)
                .into_iter()
                .map(|(text, _)| text)
                .collect();
            assert_eq!(texts, ["alpha", "beta", "gamma", "delta"]);
        }
    }

    #[test]
    /// A `take: all` group consumes no randomness, so putting a shared section
    /// in front of a sampled one leaves that group's draw exactly where it was.
    fn a_take_all_group_does_not_shift_a_later_draw() {
        let alone = "name: E\nvariants: 3\ngroups:\n  - dir: t\n    take: 2\n";
        let behind = concat!(
            "name: E\nvariants: 3\ngroups:\n  - dir: shared\n    take: all\n",
            "  - dir: t\n    take: 2\n",
        );
        let mut tree = simple_tree("t", 6);
        tree.extend(tree_with("shared", &["s0", "s1"]));
        let expected = ids(&run(alone, simple_tree("t", 6), 4));
        let after: Vec<Vec<String>> = ids(&run(behind, tree, 4))
            .iter()
            .map(|exam| exam.iter().skip(2).cloned().collect())
            .collect();
        assert_eq!(after, expected, "the shared section moved the sampled draw");
    }

    #[test]
    /// A group whose folder holds no questions is refused and named, rather
    /// than printing a sheet that is quietly short.
    fn an_empty_folder_is_refused_and_named() {
        let tree = HashMap::from([("t".to_owned(), Vec::new())]);
        let spec = Spec::from_yaml("name: E\ngroups:\n  - dir: t\n").expect("parses");
        let error = assemble(&spec, &lister(tree), &no_files, 1)
            .expect_err("empty folder")
            .to_string();
        assert!(error.contains("holds no questions"), "{error}");
        assert!(error.contains("groups[0]"), "{error}");
    }

    #[test]
    /// The draw is pinned to the seed, not merely self-consistent: these are the
    /// questions seed 20260915 picks. Re-running a spec next term has to rebuild
    /// the same exam, so a change to how a variant's stream is derived shows up
    /// here rather than as a quietly different sheet.
    fn the_draw_is_pinned_to_its_seed() {
        let yaml = "name: E\nvariants: 3\ngroups:\n  - dir: t\n    take: 2\n";
        assert_eq!(
            ids(&run(yaml, simple_tree("t", 6), 20_260_915)),
            [["t-q3", "t-q5"], ["t-q4", "t-q5"], ["t-q3", "t-q4"]]
        );
    }

    #[test]
    /// The draw does not depend on the order the lister happened to return.
    fn listing_order_does_not_change_the_draw() {
        let yaml = "name: E\nvariants: 2\ngroups:\n  - dir: t\n    take: 3\n";
        let tree = simple_tree("t", 7);
        let mut reversed = tree.clone();
        reversed.get_mut("t").expect("the folder").reverse();
        assert_eq!(ids(&run(yaml, tree, 11)), ids(&run(yaml, reversed, 11)));
    }

    #[test]
    /// Pool validation runs against the real counts, and its warnings are
    /// carried out rather than printed from inside the library.
    fn pool_validation_reaches_the_caller() {
        let yaml = "name: E\nvariants: 5\ngroups:\n  - dir: t\n    take: 3\n";
        let assembly = run(yaml, simple_tree("t", 6), 1);
        let warning = assembly
            .warnings
            .first()
            .map(String::as_str)
            .unwrap_or_default();
        assert!(warning.contains("same questions"), "{warning}");

        // And a folder that cannot supply its `take:` is an error, not a warning.
        let spec = Spec::from_yaml("name: E\ngroups:\n  - dir: t\n    take: 9\n").expect("parses");
        let error = assemble(&spec, &lister(simple_tree("t", 2)), &no_files, 1)
            .expect_err("short pool")
            .to_string();
        assert!(error.contains("only 2"), "{error}");
    }

    #[test]
    /// Two groups holding the same question id are refused. Overlapping folders
    /// are caught by the spec, but nothing stops two distinct folders carrying a
    /// copy-pasted id, and an exam holding it twice cannot be keyed.
    fn a_duplicate_id_across_groups_is_refused() {
        let tree = HashMap::from([
            (
                "a".to_owned(),
                vec![("a/q.md".to_owned(), true_false("same"))],
            ),
            (
                "b".to_owned(),
                vec![("b/q.md".to_owned(), true_false("same"))],
            ),
        ]);
        let spec = Spec::from_yaml("name: E\ngroups:\n  - dir: a\n  - dir: b\n").expect("parses");
        let error = assemble(&spec, &lister(tree), &no_files, 1)
            .expect_err("duplicate id")
            .to_string();
        assert!(error.contains("duplicate question id"), "{error}");
        assert!(error.contains("same"), "error should name the id: {error}");
    }

    #[test]
    /// A folder the lister cannot find names the group rather than failing bare.
    fn a_missing_folder_names_its_group() {
        let spec = Spec::from_yaml("name: E\ngroups:\n  - dir: gone\n").expect("parses");
        let error = assemble(&spec, &lister(HashMap::new()), &no_files, 1)
            .expect_err("missing folder")
            .to_string();
        assert!(
            error.contains(r#"groups[0] "gone""#),
            "the error should name the group, not merely echo the lister: {error}"
        );
    }

    #[test]
    /// A question that will not parse names the file it came from.
    fn a_broken_question_names_its_file() {
        let tree = HashMap::from([(
            "t".to_owned(),
            vec![(
                "t/bad.md".to_owned(),
                "---\nid: x\nkind: nonsense\n---\n\nQ\n".to_owned(),
            )],
        )]);
        let spec = Spec::from_yaml("name: E\ngroups:\n  - dir: t\n").expect("parses");
        let error = assemble(&spec, &lister(tree), &no_files, 1)
            .expect_err("bad question")
            .to_string();
        assert!(error.contains("t/bad.md"), "{error}");
    }
}
