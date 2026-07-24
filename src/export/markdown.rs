//! Render a quiz to a single print-ready Markdown sheet.
//!
//! This target is for paper distribution, so it deliberately omits the answer
//! key and any per-question scoring hints.

use crate::model::Quiz;

/// Render `quiz` as a print-ready Markdown document with no solutions.
///
/// The body of each question is not rendered yet; this first pass emits only the
/// document header so the CLI wiring can be exercised end to end.
#[must_use]
pub fn to_print_markdown(quiz: &Quiz) -> String {
    // Only the title is emitted for now; question rendering lands with each
    // question type.
    format!("# {}\n", quiz.title)
}
