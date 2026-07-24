//! Crate-wide error type.
//!
//! The library never panics on bad input; every fallible operation returns
//! [`Result`]. The binary is free to turn these into `anyhow` reports at the
//! top level.

use thiserror::Error;

/// Convenience alias for results produced across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// All the ways an `mdquiz` operation can fail.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The Markdown/YAML source could not be parsed into a question.
    #[error("failed to parse quiz source: {0}")]
    Parse(String),

    /// A question was structurally valid but semantically incomplete
    /// (for example, a multiple-choice item with no correct answer).
    #[error("invalid question: {0}")]
    InvalidQuestion(String),

    /// An export target could not represent the quiz as given.
    #[error("failed to export quiz: {0}")]
    Export(String),

    /// An underlying I/O operation failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// The embedded YAML block could not be deserialized.
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_norway::Error),
}
