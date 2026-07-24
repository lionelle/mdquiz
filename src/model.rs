//! The typed representation of a quiz.
//!
//! A [`Quiz`] is an ordered list of [`Question`]s. Every question shares a small
//! set of common metadata (identifier, prompt, point value) and carries a
//! [`QuestionKind`] that names its type. The per-kind payloads (choices, blanks,
//! match pairs, …) are intentionally *not* modelled yet: this first pass only
//! fixes the shape that the parser and exporters agree on. Each variant is
//! filled in — with its own tests — as the corresponding question type is rolled
//! out. See CLAUDE.md for the roll-out order.

use serde::{Deserialize, Serialize};

/// A complete quiz: a title plus its questions, in presentation order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Quiz {
    /// Human-readable quiz title, shown at the top of every export.
    pub title: String,
    /// The questions, in the order they should be presented.
    pub questions: Vec<Question>,
}

/// A single quiz question and its shared metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// Stable identifier, unique within the quiz (used for cross-references).
    pub id: String,
    /// The question prompt, as authored Markdown.
    pub prompt: String,
    /// Points awarded for a fully correct answer.
    pub points: f64,
    /// The question type together with its (future) type-specific payload.
    pub kind: QuestionKind,
}

/// The supported question types, following the Canvas *New Quizzes* model.
///
/// Variants are unit-typed for now; each gains a payload struct as it is
/// implemented. The initial roll-out order is the order listed here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QuestionKind {
    /// True/false question.
    TrueFalse,
    /// Single-answer multiple choice.
    MultipleChoice,
    /// Multiple-answer ("select all that apply") multiple choice.
    MultipleSelect,
    /// Fill in the blank, matched by literal text or regular expression.
    FillInBlank,
    /// Match items in one column to items in another.
    Matching,
    /// Put items into the correct order.
    Ordering,
}
