//! The typed representation of quiz content.
//!
//! The reusable unit is a [`Question`]: shared metadata (identifier, prompt,
//! point value) plus a [`QuestionKind`] that names its type. Questions are
//! collected into an [`ItemBank`] — an ordered, named pool of items that maps
//! onto a Canvas *New Quizzes* item bank.
//!
//! The per-kind payloads (choices, blanks, match pairs, …) are intentionally
//! *not* modelled yet: this first pass only fixes the shape that the parser and
//! exporters agree on. Each variant is filled in — with its own tests — as the
//! corresponding question type is rolled out. See CLAUDE.md for the roll-out
//! order.
//!
//! # Growth path
//!
//! Only the item bank is modelled today. A future `Quiz` type will live
//! alongside [`ItemBank`]: in the Canvas *New Quizzes* model a quiz is built
//! from items that may be authored inline *or* drawn from an item bank, so a
//! `Quiz` will hold its own items plus references to banks. Keeping [`Question`]
//! independent of [`ItemBank`] is what leaves room for that.

use serde::{Deserialize, Serialize};

/// A named, ordered pool of questions: one Canvas *New Quizzes* item bank.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemBank {
    /// Human-readable bank name, shown at the top of every export.
    pub name: String,
    /// The questions, in the order they should be presented.
    pub items: Vec<Question>,
}

/// A single quiz question and its shared metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// Stable identifier, unique within the bank (used for cross-references).
    pub id: String,
    /// Instructor-facing title (question name). Shown in the Canvas bank
    /// editor, not to students; absent means fall back to [`Question::id`].
    pub title: Option<String>,
    /// The question prompt, as authored Markdown.
    pub prompt: String,
    /// Points awarded for a fully correct answer.
    pub points: f64,
    /// Free-form tags for mdquiz-side organization (e.g. future filtering by
    /// topic). Not exported to Canvas, which organizes via item banks instead.
    pub tags: Vec<String>,
    /// Optional feedback shown to students after answering.
    pub feedback: Feedback,
    /// The question type together with its (future) type-specific payload.
    pub kind: QuestionKind,
}

/// Feedback messages shown after a question is answered.
///
/// Each field is authored Markdown and optional; an all-empty [`Feedback`]
/// produces no feedback in any export. Exported to Canvas as QTI
/// `<itemfeedback>`; omitted from the print sheet, which carries no answer key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Feedback {
    /// Shown regardless of correctness.
    #[serde(default)]
    pub general: Option<String>,
    /// Shown when the answer is correct.
    #[serde(default)]
    pub correct: Option<String>,
    /// Shown when the answer is incorrect.
    #[serde(default)]
    pub incorrect: Option<String>,
}

/// The supported question types, following the Canvas *New Quizzes* model.
///
/// Each variant carries its type-specific payload as it is implemented;
/// not-yet-rolled-out variants stay unit-typed. The roll-out order is the order
/// listed here. This serde shape is *not* the authored format — questions are
/// parsed via `parse::QuestionSpec`, which reads the flat `kind:` YAML tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QuestionKind {
    /// True/false question.
    TrueFalse(TrueFalse),
    /// Single-answer multiple choice.
    MultipleChoice(ChoiceSet),
    /// Multiple-answer ("select all that apply") multiple choice.
    MultipleSelect(ChoiceSet),
    /// Fill in the blank, matched by literal text or regular expression.
    FillInBlank,
    /// Match items in one column to items in another.
    Matching,
    /// Put items into the correct order.
    Ordering,
}

impl QuestionKind {
    /// A short human-readable label for this type, used in diagnostics.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::TrueFalse(_) => "true/false",
            Self::MultipleChoice(_) => "multiple choice",
            Self::MultipleSelect(_) => "multiple select",
            Self::FillInBlank => "fill in the blank",
            Self::Matching => "matching",
            Self::Ordering => "ordering",
        }
    }

    /// Build the "not supported yet" export error for this kind.
    ///
    /// Shared by every exporter's reject arm so the message stays consistent
    /// and names both the target format and the offending question.
    pub(crate) fn unsupported_by(&self, target: &str, id: &str) -> crate::Error {
        crate::Error::Export(format!(
            "{target} export does not support {} questions yet (question {id:?})",
            self.label()
        ))
    }
}

/// The payload for a [`QuestionKind::TrueFalse`] question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrueFalse {
    /// Whether the correct answer is "true" (`true`) or "false" (`false`).
    pub answer: bool,
}

/// One selectable option in a choice-based question.
///
/// The same shape backs both single-answer [`QuestionKind::MultipleChoice`] and
/// multiple-answer [`QuestionKind::MultipleSelect`]; only how many choices may
/// be `correct` differs between them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    /// The option's text, as authored Markdown.
    pub text: String,
    /// Whether selecting this option counts as a correct answer.
    pub correct: bool,
}

/// The payload shared by the choice-based question types.
///
/// Backs both [`QuestionKind::MultipleChoice`] (single answer) and
/// [`QuestionKind::MultipleSelect`] (choose all that apply); the choices are
/// presented in order. How many may be `correct` is enforced when parsing:
/// exactly one for multiple choice, one or more for multiple select.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceSet {
    /// The options, in presentation order.
    pub choices: Vec<Choice>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Every kind reports its documented human-readable label.
    fn label_names_each_kind() {
        assert_eq!(
            QuestionKind::TrueFalse(TrueFalse { answer: true }).label(),
            "true/false"
        );
        assert_eq!(
            QuestionKind::MultipleChoice(ChoiceSet {
                choices: Vec::new()
            })
            .label(),
            "multiple choice"
        );
        assert_eq!(
            QuestionKind::MultipleSelect(ChoiceSet {
                choices: Vec::new()
            })
            .label(),
            "multiple select"
        );
        assert_eq!(QuestionKind::FillInBlank.label(), "fill in the blank");
        assert_eq!(QuestionKind::Matching.label(), "matching");
        assert_eq!(QuestionKind::Ordering.label(), "ordering");
    }

    #[test]
    /// The unsupported-kind error names the target format, kind, and question.
    fn unsupported_by_message_names_target_kind_and_id() {
        let err = QuestionKind::Matching.unsupported_by("Canvas", "q7");
        let message = err.to_string();
        assert!(message.contains("Canvas"));
        assert!(message.contains("matching"));
        assert!(message.contains("q7"));
    }
}
