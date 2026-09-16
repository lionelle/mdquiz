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

    /// A specific question file in a bank failed to parse; names the file so
    /// the author knows which of many files to fix.
    #[error("in question file {file}: {message}")]
    QuestionFile {
        /// The offending file's name.
        file: String,
        /// The underlying failure, rendered.
        message: String,
    },

    /// A quiz spec (the YAML blueprint) was invalid; names where in the spec
    /// the problem is so the author knows what to fix.
    #[error("invalid quiz spec at {at}: {message}")]
    Spec {
        /// Where the problem is: a group (`groups[1] "topics/graphs"`) or a
        /// top-level key (`variants`). Not every spec failure is group-scoped.
        at: String,
        /// The underlying failure, rendered.
        message: String,
    },

    /// A question was structurally valid but semantically incomplete
    /// (for example, a multiple-choice item with no correct answer).
    #[error("invalid question: {0}")]
    InvalidQuestion(String),

    /// Math that cannot be put on paper faithfully.
    ///
    /// Either the LaTeX was rejected outright, or it uses a construct with no
    /// Word equivalent. Both are refusals rather than best-effort renders: a
    /// silently wrong equation on a printed exam is worse than a failed build.
    #[error("cannot render math {latex:?}: {reason}")]
    UnsupportedMath {
        /// The offending LaTeX, as the author wrote it.
        latex: String,
        /// Why it cannot be rendered.
        reason: String,
    },

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
