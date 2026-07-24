//! Render an item bank to a Canvas *New Quizzes* QTI package.
//!
//! Canvas imports item banks as a zipped QTI/IMSCC bundle. The byte payload
//! returned here is intended to be written to a `.imscc`/`.zip` file that can be
//! uploaded directly to Canvas as a New Quizzes item bank.

use crate::Result;
use crate::model::ItemBank;

/// Render `bank` as the bytes of a Canvas New Quizzes QTI package.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] until the QTI writer is implemented.
pub fn to_qti(bank: &ItemBank) -> Result<Vec<u8>> {
    let _ = bank;
    Err(crate::Error::Export(
        "Canvas QTI export is not implemented yet".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// The unimplemented QTI writer reports a typed export error.
    fn to_qti_is_unimplemented() {
        let err = to_qti(&ItemBank::default()).expect_err("QTI export is a stub");
        assert!(matches!(err, crate::Error::Export(_)));
    }
}
