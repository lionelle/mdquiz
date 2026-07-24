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
