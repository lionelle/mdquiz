//! Parse Markdown + YAML question sources into the typed model.
//!
//! Each source file is one question: an ordinary-Markdown prompt with a fenced
//! YAML block carrying the machine-readable answer data. A directory of such
//! files is assembled into one [`ItemBank`].
//!
//! Per-question parsing is not implemented in this first pass; the entry points
//! and their contracts are defined here so the rest of the pipeline can be built
//! against stable signatures. The bank-assembly logic (ordering and duplicate
//! detection) is real today and unit-tested below.

use std::collections::HashSet;

use crate::Result;
use crate::model::{ItemBank, Question};

/// Parse the full text of one question source into a [`Question`].
///
/// # Errors
///
/// Returns [`crate::Error::Parse`] when the source cannot be interpreted as a
/// question, or [`crate::Error::Yaml`] when the embedded YAML block is
/// malformed.
pub fn parse_question(source: &str) -> Result<Question> {
    // Placeholder until the Markdown/YAML reader lands. Returning a typed error
    // (rather than panicking) keeps the contract honest today.
    let _ = source;
    Err(crate::Error::Parse(
        "question parsing is not implemented yet".to_owned(),
    ))
}

/// Assemble an [`ItemBank`] from `(filename, content)` source pairs.
///
/// Sources are ordered by filename for a deterministic bank layout, then each
/// is parsed via [`parse_question`]. This is the disk-free seam the CLI wires
/// its directory walk onto, so the assembly stays testable without a process.
///
/// # Errors
///
/// Propagates any [`parse_question`] failure, and returns
/// [`crate::Error::InvalidQuestion`] if two questions share an `id`.
pub fn item_bank_from_sources<N, I>(name: N, sources: I) -> Result<ItemBank>
where
    N: Into<String>,
    I: IntoIterator<Item = (String, String)>,
{
    let mut sources: Vec<(String, String)> = sources.into_iter().collect();
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    let mut items = Vec::with_capacity(sources.len());
    for (_filename, content) in &sources {
        items.push(parse_question(content)?);
    }
    item_bank_from_questions(name, items)
}

/// Build an [`ItemBank`] from already-parsed questions, preserving their order.
///
/// # Errors
///
/// Returns [`crate::Error::InvalidQuestion`] if two questions share an `id`,
/// since ids must be unique within a bank for cross-references to resolve.
pub fn item_bank_from_questions<N, I>(name: N, items: I) -> Result<ItemBank>
where
    N: Into<String>,
    I: IntoIterator<Item = Question>,
{
    let items: Vec<Question> = items.into_iter().collect();
    let mut seen = HashSet::with_capacity(items.len());
    for question in &items {
        if !seen.insert(question.id.as_str()) {
            return Err(crate::Error::InvalidQuestion(format!(
                "duplicate question id {:?}",
                question.id
            )));
        }
    }
    Ok(ItemBank {
        name: name.into(),
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::QuestionKind;

    /// Build a minimal question with the given `id` for assembly tests.
    fn question(id: &str) -> Question {
        Question {
            id: id.to_owned(),
            prompt: String::new(),
            points: 1.0,
            kind: QuestionKind::TrueFalse,
        }
    }

    #[test]
    /// An empty question set assembles into an empty, named bank.
    fn empty_set_is_empty_bank() {
        let bank =
            item_bank_from_questions("module01", std::iter::empty()).expect("empty set is valid");
        assert_eq!(bank.name, "module01");
        assert!(bank.items.is_empty());
    }

    #[test]
    /// Questions are kept in the order they are supplied.
    fn preserves_question_order() {
        let bank = item_bank_from_questions("m", [question("a"), question("b"), question("c")])
            .expect("distinct ids are valid");
        let ids: Vec<&str> = bank.items.iter().map(|q| q.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    /// Two questions sharing an id are rejected as an invalid bank.
    fn duplicate_ids_are_rejected() {
        let err = item_bank_from_questions("m", [question("dup"), question("dup")])
            .expect_err("duplicate ids must fail");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// Duplicate ids are caught even when the two are not adjacent.
    fn non_adjacent_duplicate_ids_are_rejected() {
        let err = item_bank_from_questions("m", [question("a"), question("b"), question("a")])
            .expect_err("duplicate id must fail");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// An empty directory (no sources) assembles without touching the parser.
    fn empty_sources_assemble() {
        let bank = item_bank_from_sources("m", std::iter::empty()).expect("no sources is valid");
        assert!(bank.items.is_empty());
    }

    #[test]
    /// Non-empty sources surface the (not-yet-implemented) parse error.
    fn sources_propagate_parse_error() {
        let sources = [("q1.md".to_owned(), "anything".to_owned())];
        let err = item_bank_from_sources("m", sources).expect_err("parsing is a stub");
        assert!(matches!(err, crate::Error::Parse(_)));
    }
}
