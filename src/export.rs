//! Export a [`crate::model::ItemBank`] to a distribution format.
//!
//! Two targets are supported:
//!
//! * [`markdown`] — a single print-ready Markdown sheet with no answer key, and
//! * [`canvas`] — a Canvas *New Quizzes* QTI package.

pub mod canvas;
pub mod markdown;

/// The export formats the CLI can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Print-ready Markdown, no solutions.
    Markdown,
    /// Canvas New Quizzes QTI package.
    Canvas,
}
