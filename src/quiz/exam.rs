//! One assembled exam sheet, ready to print.
//!
//! An [`Exam`] is the output of [`assemble`](super::assemble) and the input to
//! every print exporter. It is deliberately *finished*: the questions are chosen,
//! their order is fixed, any choice shuffling has already happened, and each
//! item carries the answer space it wants as a plain number.
//!
//! That is the invariant the answer key depends on. Every random decision is
//! made once, during assembly, and frozen here — so a sheet and its key are two
//! renderings of the same data and cannot disagree about which option is `B`.
//! A writer that reached for a random number would break that silently.
//!
//! That holds for every kind. Matching and ordering questions used to have
//! their presented order derived by the writer — a sort over the option text —
//! so `shuffle_choices` could not vary them and a key naming labels rather
//! than text would have had to re-derive that sort. The order now lives on
//! [`ExamItem::option_order`], decided once during assembly like every other
//! random choice.

use crate::label::sequence;
use crate::model::{Question, QuestionKind};
use crate::quiz::spec::Layout;

/// One question as it will appear on a sheet.
#[derive(Debug, Clone)]
pub struct ExamItem {
    /// The question, with any per-variant shuffling already applied.
    pub question: Question,
    /// Blank lines to leave after it for the student's answer.
    ///
    /// Already resolved from the question, its group and the layout, so a writer
    /// reads a number and never re-derives the precedence.
    pub answer_space: usize,
    /// Indices into this item's option list, in the order they print.
    ///
    /// Which list depends on the kind: the items of an ordering question, or
    /// the shared right-hand options of a matching one. Empty for the kinds
    /// whose payload already holds its options in print order — multiple
    /// choice and multiple select, which assembly permutes in place — and for
    /// the kinds with no options at all.
    ///
    /// Stored rather than derived so a sheet and its key cannot disagree about
    /// which option is `B`, and so a shuffling group can vary it per variant.
    pub option_order: Vec<usize>,
}

impl ExamItem {
    /// The options this item prints, in the order they print.
    ///
    /// The single source of that order: the print writers letter these and the
    /// run manifest records them, and a manifest reporting an order the sheet
    /// did not use is a record of a paper nobody sat. Empty for the kinds with
    /// no option list — true/false, whose two options are the writer's own
    /// words, and fill-in-the-blank, whose blanks sit in the prompt.
    ///
    /// For a matching question these are the right-hand options; the left
    /// prompts are not options to choose from.
    #[must_use]
    pub fn presented_options(&self) -> Vec<String> {
        match &self.question.kind {
            QuestionKind::MultipleChoice(set) => texts(&set.choices),
            QuestionKind::MultipleSelect(set) => texts(&set.choices),
            QuestionKind::Matching(matching) => {
                let options = matching.options();
                self.ordered(matching.display_order())
                    .into_iter()
                    .map(|index| options.get(index).copied().unwrap_or_default().to_owned())
                    .collect()
            }
            QuestionKind::Ordering(ordering) => self
                .ordered(ordering.display_order())
                .into_iter()
                .map(|index| ordering.items.get(index).cloned().unwrap_or_default())
                .collect(),
            QuestionKind::TrueFalse(_) | QuestionKind::FillInBlank(_) => Vec::new(),
        }
    }

    /// [`Self::option_order`] if assembly froze one, or `default` if it did not.
    ///
    /// An item from `assemble` always carries an order for the kinds that need
    /// one, so the fallback is for one built by hand — a test, or a caller that
    /// bypassed assembly. It is the order a *non-shuffling* group would have
    /// stored: assembly computes `hides_the_answer(display_order())`,
    /// `display_order` already ends in `hides_the_answer`, and a second
    /// application is a no-op because a rotated identity is never the identity
    /// again. A shuffling group's order cannot be recovered and is not guessed
    /// at — being unrecoverable is why it is stored in the first place.
    fn ordered(&self, default: Vec<usize>) -> Vec<usize> {
        if self.option_order.is_empty() {
            return default;
        }
        self.option_order.clone()
    }
}

/// The text of each choice, in order.
fn texts(choices: &[crate::model::Choice]) -> Vec<String> {
    choices.iter().map(|choice| choice.text.clone()).collect()
}

/// One variant of a quiz, assembled and ready to render.
#[derive(Debug, Clone)]
pub struct Exam {
    /// The exam's title, shared by every variant.
    pub name: String,
    /// This variant's label (`A`, `B`, …), or `None` for a single-sheet quiz.
    pub variant: Option<String>,
    /// Markdown to render once above the questions, already loaded.
    pub header: Option<String>,
    /// Markdown to render once below the questions, already loaded.
    pub footer: Option<String>,
    /// How the sheet is laid out.
    pub layout: Layout,
    /// The questions, in the order they will be printed.
    pub items: Vec<ExamItem>,
}

impl Exam {
    /// The label for the variant at `index`, or `None` when there is only one.
    ///
    /// A lone sheet has no variant to distinguish it from, and labelling it "A"
    /// would imply a "B" that does not exist.
    #[must_use]
    pub fn variant_label(index: usize, variants: usize) -> Option<String> {
        (variants > 1).then(|| sequence(index))
    }

    /// What the whole sheet is worth.
    ///
    /// Summed rather than stored: the items are the source of truth, and a
    /// total kept beside them is a total that can disagree with them.
    #[must_use]
    pub fn total_points(&self) -> f64 {
        self.items.iter().map(|item| item.question.points).sum()
    }

    /// The exam's title with its variant, as printed at the top of the sheet.
    #[must_use]
    pub fn title(&self) -> String {
        self.variant.as_ref().map_or_else(
            || self.name.clone(),
            |variant| format!("{} ({variant})", self.name),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Feedback, QuestionKind, TrueFalse};

    /// A true/false question worth `points`.
    fn question(id: &str, points: f64) -> Question {
        Question {
            id: id.to_owned(),
            title: None,
            prompt: "Q?".to_owned(),
            points,
            tags: Vec::new(),
            feedback: Feedback::default(),
            kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
        }
    }

    /// An exam named `name` holding `points`, one item each.
    fn exam(variant: Option<&str>, points: &[f64]) -> Exam {
        Exam {
            name: "Exam 1".to_owned(),
            variant: variant.map(ToOwned::to_owned),
            header: None,
            footer: None,
            layout: Layout::default(),
            items: points
                .iter()
                .enumerate()
                .map(|(index, points)| ExamItem {
                    question: question(&format!("q{index}"), *points),
                    answer_space: 3,
                    option_order: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    /// A single sheet carries no variant label; several are lettered.
    fn only_multi_variant_quizzes_are_labelled() {
        assert_eq!(Exam::variant_label(0, 1), None);
        assert_eq!(Exam::variant_label(0, 5).as_deref(), Some("A"));
        assert_eq!(Exam::variant_label(4, 5).as_deref(), Some("E"));
        assert_eq!(Exam::variant_label(26, 30).as_deref(), Some("27"));
    }

    #[test]
    /// The total is the sum of the questions on *this* sheet. Variants draw
    /// different questions, so a total taken from the bank would be wrong on
    /// every variant that did not draw all of it.
    fn the_total_sums_the_questions_on_this_sheet() {
        let quiz = exam(Some("B"), &[5.0, 1.0, 2.5]);
        assert!((quiz.total_points() - 8.5).abs() < f64::EPSILON);
    }

    #[test]
    /// An exam with no questions is worth zero rather than failing to add up.
    fn an_exam_with_no_questions_totals_zero() {
        assert!(exam(None, &[]).total_points().abs() < f64::EPSILON);
    }

    #[test]
    /// The printed title names the variant only when there is one.
    fn title_includes_the_variant_when_present() {
        assert_eq!(exam(Some("B"), &[1.0]).title(), "Exam 1 (B)");
        assert_eq!(exam(None, &[1.0]).title(), "Exam 1");
    }
}
