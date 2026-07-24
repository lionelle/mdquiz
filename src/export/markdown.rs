//! Render an item bank to a single print-ready Markdown sheet.
//!
//! This target is for paper distribution, so it deliberately omits the answer
//! key and any per-question scoring hints.

use crate::model::ItemBank;

/// Render `bank` as a print-ready Markdown document with no solutions.
///
/// The body of each question is not rendered yet; this first pass emits only the
/// document header so the CLI wiring can be exercised end to end.
#[must_use]
pub fn to_print_markdown(bank: &ItemBank) -> String {
    format!("# {}\n", bank.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// The print sheet leads with the bank name as an H1 header.
    fn header_uses_bank_name() {
        let bank = ItemBank {
            name: "Quiz 1".to_owned(),
            items: Vec::new(),
        };
        assert_eq!(to_print_markdown(&bank), "# Quiz 1\n");
    }
}
