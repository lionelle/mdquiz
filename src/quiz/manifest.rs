//! The record of one quiz run: what each variant drew, and the seed that drew
//! it.
//!
//! Written beside the sheets so a paper handed out months ago can still be
//! accounted for — which questions variant B held, and in what order its
//! options were printed. Two things make that answerable: the seed, which
//! rebuilds the run byte for byte, and the presented option order, which says
//! what the paper actually said without needing to re-run anything.
//!
//! # What it does not record
//!
//! Not a permutation of the authored choices. Assembly shuffles a multiple
//! choice or multiple select payload in place and discards the order the author
//! wrote, so there is nothing left to diff against; what is recorded is the
//! option text in the order it printed, which is self-contained and needs no
//! second file to interpret.
//!
//! This costs nothing in correctness. For those two kinds the `correct` flag
//! rides on the choice itself, so the shuffle carries the answer along with the
//! text and a key cannot drift from its sheet. The two kinds where the answer
//! *cannot* ride along — matching and ordering, whose authored order is the
//! answer — are exactly the two that store
//! [`ExamItem::option_order`](crate::quiz::exam::ExamItem::option_order).

use serde::Serialize;

use crate::quiz::assemble::Assembly;
use crate::quiz::exam::Exam;

/// The record of one quiz run.
#[derive(Debug, Clone, Serialize)]
pub struct Manifest {
    /// The exam's name, as printed at the top of every sheet.
    pub name: String,
    /// The seed the run was drawn with.
    ///
    /// The whole point of the file: re-running the same spec with this seed
    /// reproduces every sheet exactly, so a paper is never unrecoverable.
    pub seed: u64,
    /// One record per variant, in the order they were assembled.
    pub variants: Vec<VariantRecord>,
}

/// What one variant drew.
#[derive(Debug, Clone, Serialize)]
pub struct VariantRecord {
    /// The variant's label, or `None` for a single-sheet quiz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// The questions it held, in the order they were printed.
    pub items: Vec<ItemRecord>,
}

/// One question as one variant printed it.
#[derive(Debug, Clone, Serialize)]
pub struct ItemRecord {
    /// The question's id, which is stable across variants and runs.
    pub id: String,
    /// The options as printed, in order.
    ///
    /// Omitted for the kinds with no option list rather than written as an
    /// empty sequence, so a true/false entry is one line and the file stays
    /// readable at forty questions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

impl Manifest {
    /// The record of assembling `assembly` with `seed`.
    #[must_use]
    pub fn of(assembly: &Assembly, seed: u64) -> Self {
        Self {
            name: assembly
                .exams
                .first()
                .map_or_else(String::new, |exam| exam.name.clone()),
            seed,
            variants: assembly.exams.iter().map(VariantRecord::of).collect(),
        }
    }

    /// The manifest as the YAML to write.
    ///
    /// YAML rather than JSON because a spec is YAML: an author reading the
    /// record of a run should not have to change notation to do it.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error`] if the record cannot be serialised.
    pub fn to_yaml(&self) -> crate::Result<String> {
        Ok(serde_norway::to_string(self)?)
    }
}

impl VariantRecord {
    /// The record of one assembled variant.
    fn of(exam: &Exam) -> Self {
        Self {
            variant: exam.variant.clone(),
            items: exam
                .items
                .iter()
                .map(|item| ItemRecord {
                    id: item.question.id.clone(),
                    options: item.presented_options(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Choice, ChoiceSet, Feedback, Ordering, Question, QuestionKind, TrueFalse};
    use crate::quiz::exam::ExamItem;
    use crate::quiz::spec::Layout;

    /// An item holding `kind`, presented in `option_order`.
    fn item(id: &str, kind: QuestionKind, option_order: Vec<usize>) -> ExamItem {
        ExamItem {
            question: Question {
                id: id.to_owned(),
                title: None,
                prompt: "Prompt.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind,
            },
            answer_space: 3,
            option_order,
        }
    }

    /// An exam labelled `variant`, holding `items`.
    fn exam(variant: Option<&str>, items: Vec<ExamItem>) -> Exam {
        Exam {
            name: "Exam 1".to_owned(),
            variant: variant.map(ToOwned::to_owned),
            header: None,
            footer: None,
            layout: Layout::default(),
            items,
        }
    }

    /// A two-option multiple-choice payload, `first` correct.
    fn choices(first: &str, second: &str) -> QuestionKind {
        QuestionKind::MultipleChoice(ChoiceSet {
            choices: vec![
                Choice {
                    text: first.to_owned(),
                    correct: true,
                },
                Choice {
                    text: second.to_owned(),
                    correct: false,
                },
            ],
        })
    }

    #[test]
    /// Every variant is recorded, with its questions in printed order and its
    /// options as printed. Without the options the record cannot say what
    /// paper a student actually sat.
    fn it_records_every_variant_and_its_printed_options() {
        let assembly = Assembly {
            exams: vec![
                exam(
                    Some("A"),
                    vec![item("q1", choices("alpha", "beta"), vec![])],
                ),
                exam(
                    Some("B"),
                    vec![item("q1", choices("beta", "alpha"), vec![])],
                ),
            ],
            warnings: Vec::new(),
        };
        let manifest = Manifest::of(&assembly, 42);
        assert_eq!(manifest.seed, 42);
        assert_eq!(manifest.name, "Exam 1");
        assert_eq!(
            labelled_options(&manifest),
            ["A: alpha, beta", "B: beta, alpha"]
        );
    }

    /// Each variant's label with its first item's options, for comparison.
    fn labelled_options(manifest: &Manifest) -> Vec<String> {
        manifest
            .variants
            .iter()
            .map(|variant| {
                let label = variant.variant.clone().unwrap_or_default();
                let options = variant
                    .items
                    .first()
                    .map(|item| item.options.join(", "))
                    .unwrap_or_default();
                format!("{label}: {options}")
            })
            .collect()
    }

    #[test]
    /// The recorded order is the order the sheet printed, read from the same
    /// accessor the writers use. A manifest reporting an order no sheet used is
    /// a record of a paper nobody sat.
    fn the_recorded_order_is_the_printed_order() {
        let kind = QuestionKind::Ordering(Ordering {
            items: vec!["Compile".to_owned(), "Link".to_owned(), "Run".to_owned()],
        });
        let items = vec![item("phases", kind, vec![2, 0, 1])];
        let exam = exam(None, items);
        let printed = exam
            .items
            .first()
            .map(ExamItem::presented_options)
            .unwrap_or_default();
        let manifest = Manifest::of(
            &Assembly {
                exams: vec![exam],
                warnings: Vec::new(),
            },
            1,
        );
        let recorded = manifest
            .variants
            .first()
            .and_then(|variant| variant.items.first())
            .map(|item| item.options.clone())
            .unwrap_or_default();
        assert_eq!(recorded, ["Run", "Compile", "Link"]);
        assert_eq!(recorded, printed, "the record and the sheet disagree");
    }

    #[test]
    /// A kind with no option list records its id alone. Forty true/false
    /// questions should not each cost an empty sequence.
    fn a_kind_without_options_records_only_its_id() {
        let items = vec![item(
            "tf",
            QuestionKind::TrueFalse(TrueFalse { answer: true }),
            Vec::new(),
        )];
        let manifest = Manifest::of(
            &Assembly {
                exams: vec![exam(None, items)],
                warnings: Vec::new(),
            },
            5,
        );
        let yaml = manifest.to_yaml().expect("serialises");
        assert!(yaml.contains("id: tf"), "{yaml}");
        assert!(!yaml.contains("options"), "{yaml}");
    }

    #[test]
    /// The YAML carries the seed and the variant labels, and a lone sheet
    /// writes no variant key at all.
    fn the_yaml_records_the_seed_and_omits_an_absent_variant() {
        let items = vec![item("q1", choices("alpha", "beta"), Vec::new())];
        let manifest = Manifest::of(
            &Assembly {
                exams: vec![exam(None, items)],
                warnings: Vec::new(),
            },
            7,
        );
        let yaml = manifest.to_yaml().expect("serialises");
        assert!(yaml.contains("seed: 7"), "{yaml}");
        assert!(yaml.contains("name: Exam 1"), "{yaml}");
        assert!(!yaml.contains("variant:"), "{yaml}");
    }

    #[test]
    /// An assembly that drew nothing still serialises, rather than failing and
    /// taking the sheets down with it.
    fn an_empty_assembly_still_records() {
        let manifest = Manifest::of(
            &Assembly {
                exams: Vec::new(),
                warnings: Vec::new(),
            },
            3,
        );
        assert!(manifest.to_yaml().is_ok());
        assert!(manifest.variants.is_empty());
    }
}
