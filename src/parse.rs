//! Parse a Markdown + YAML source file into a [`Quiz`].
//!
//! The intended format is a Markdown document whose prompt is written as
//! ordinary Markdown, with a fenced YAML block carrying the machine-readable
//! answer data. Parsing is not implemented in this first pass; the entry point
//! and its contract are defined here so the rest of the pipeline can be built
//! against a stable signature.

use crate::Result;
use crate::model::Quiz;

/// Parse the full text of one quiz source into a [`Quiz`].
///
/// # Errors
///
/// Returns [`crate::Error::Parse`] when the source cannot be interpreted as a
/// quiz, or [`crate::Error::Yaml`] when the embedded YAML block is malformed.
pub fn parse_quiz(source: &str) -> Result<Quiz> {
    // Placeholder until the Markdown/YAML front-matter reader lands. Returning a
    // typed error (rather than panicking) keeps the contract honest today.
    let _ = source;
    Err(crate::Error::Parse(
        "quiz parsing is not implemented yet".to_owned(),
    ))
}
