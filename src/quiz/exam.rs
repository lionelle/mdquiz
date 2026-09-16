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
//! With one known exception: matching and ordering questions have their
//! presented order derived by the writer rather than stored here, so
//! `shuffle_choices` does not vary them. Nothing is inconsistent today, because
//! the answer key prints those answers as text rather than by label — but a key
//! that named the labels would have to re-derive that order, which is what this
//! type exists to prevent. See `Roadmap.md`.

use crate::label::sequence;
use crate::model::Question;
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

    /// The exam's title with its variant, as printed at the top of the sheet.
    #[must_use]
    pub fn title(&self) -> String {
        self.variant.as_ref().map_or_else(
            || self.name.clone(),
            |variant| format!("{} ({variant})", self.name),
        )
    }

    /// The total points on this sheet.
    ///
    /// Varies between variants once sampling is involved, so it is computed per
    /// exam rather than taken from the spec.
    #[must_use]
    pub fn total_points(&self) -> f64 {
        self.items.iter().map(|item| item.question.points).sum()
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
    /// The printed title names the variant only when there is one.
    fn title_includes_the_variant_when_present() {
        assert_eq!(exam(Some("B"), &[1.0]).title(), "Exam 1 (B)");
        assert_eq!(exam(None, &[1.0]).title(), "Exam 1");
    }

    #[test]
    /// Points are summed from the items, so each variant reports its own total.
    fn total_points_sums_the_items() {
        assert!((exam(None, &[1.0, 2.5, 0.5]).total_points() - 4.0).abs() < f64::EPSILON);
        assert!((exam(None, &[]).total_points() - 0.0).abs() < f64::EPSILON);
    }
}
