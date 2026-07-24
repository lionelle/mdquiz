//! Render an item bank to a single print-ready Markdown sheet.
//!
//! This target is for paper distribution, so it deliberately omits the answer
//! key and any per-question scoring hints.

use crate::Result;
use crate::model::{Question, QuestionKind};

/// Render `bank` as a print-ready Markdown document with no solutions.
///
/// Questions are numbered from one under the bank name. Only rolled-out
/// question types render; others report a typed error rather than emitting a
/// silently wrong sheet.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] if the bank contains a question type this
/// exporter does not support yet.
pub fn to_print_markdown(bank: &crate::model::ItemBank) -> Result<String> {
    let mut out = format!("# {}\n", bank.name);
    for (index, question) in bank.items.iter().enumerate() {
        out.push('\n');
        out.push_str(&render_question(index + 1, question)?);
    }
    Ok(out)
}

/// Render a single numbered question, dispatching on its kind.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] for a question type not yet supported here.
fn render_question(number: usize, question: &Question) -> Result<String> {
    match &question.kind {
        QuestionKind::TrueFalse(_) => Ok(render_true_false(number, &question.prompt)),
        other => Err(other.unsupported_by("markdown", &question.id)),
    }
}

/// Render a true/false item: the prompt plus blank True/False checkboxes.
fn render_true_false(number: usize, prompt: &str) -> String {
    format!("{number}. {prompt}\n\n   - [ ] True\n   - [ ] False\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Feedback, ItemBank, Question, QuestionKind, TrueFalse};

    /// Build a one-question true/false bank for rendering tests.
    fn true_false_bank() -> ItemBank {
        ItemBank {
            name: "Module 1".to_owned(),
            items: vec![Question {
                id: "q1".to_owned(),
                title: None,
                prompt: "Binary search needs a sorted array.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
            }],
        }
    }

    #[test]
    /// An empty bank renders just the H1 header.
    fn header_uses_bank_name() {
        let bank = ItemBank {
            name: "Quiz 1".to_owned(),
            items: Vec::new(),
        };
        assert_eq!(to_print_markdown(&bank).expect("empty bank"), "# Quiz 1\n");
    }

    #[test]
    /// A true/false question renders numbered, with blank checkboxes and no key.
    fn renders_true_false_without_answer() {
        let rendered = to_print_markdown(&true_false_bank()).expect("bank renders");
        assert!(rendered.contains("1. Binary search needs a sorted array."));
        assert!(rendered.contains("- [ ] True"));
        assert!(rendered.contains("- [ ] False"));
        // The print sheet must never leak the correct answer.
        assert!(!rendered.contains("[x]"));
        assert!(!rendered.to_lowercase().contains("answer"));
    }

    #[test]
    /// Multiple questions are numbered sequentially from one, in order.
    fn numbers_questions_sequentially() {
        let tf = |id: &str, prompt: &str| Question {
            id: id.to_owned(),
            title: None,
            prompt: prompt.to_owned(),
            points: 1.0,
            tags: Vec::new(),
            feedback: Feedback::default(),
            kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
        };
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![tf("a", "First?"), tf("b", "Second?")],
        };
        let out = to_print_markdown(&bank).expect("renders");
        assert!(out.contains("1. First?"));
        assert!(out.contains("2. Second?"));
        assert!(out.find("1. First?") < out.find("2. Second?"));
    }

    #[test]
    /// title, tags, and feedback never appear on the print sheet.
    fn metadata_is_never_printed() {
        let mut bank = true_false_bank();
        if let Some(item) = bank.items.first_mut() {
            item.title = Some("SECRET-TITLE".to_owned());
            item.tags = vec!["SECRET-TAG".to_owned()];
            item.feedback = Feedback {
                general: Some("GENERAL-FB".to_owned()),
                correct: Some("CORRECT-FB".to_owned()),
                incorrect: Some("INCORRECT-FB".to_owned()),
            };
        }
        let out = to_print_markdown(&bank).expect("renders");
        assert!(!out.contains("SECRET-TITLE"));
        assert!(!out.contains("SECRET-TAG"));
        assert!(!out.contains("-FB"));
    }

    #[test]
    /// An unsupported question type reports a typed export error.
    fn unsupported_kind_errors() {
        let bank = ItemBank {
            name: "m".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: "p".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::Matching,
            }],
        };
        let err = to_print_markdown(&bank).expect_err("matching is unsupported");
        assert!(matches!(err, crate::Error::Export(_)));
    }
}
