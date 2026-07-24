//! Render a quiz to a Canvas *New Quizzes* QTI package.
//!
//! Canvas imports quizzes as a zipped QTI/IMSCC bundle. The byte payload
//! returned here is intended to be written to a `.imscc`/`.zip` file that can be
//! uploaded directly to Canvas.

use crate::Result;
use crate::model::Quiz;

/// Render `quiz` as the bytes of a Canvas New Quizzes QTI package.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] until the QTI writer is implemented.
pub fn to_qti(quiz: &Quiz) -> Result<Vec<u8>> {
    let _ = quiz;
    Err(crate::Error::Export(
        "Canvas QTI export is not implemented yet".to_owned(),
    ))
}
