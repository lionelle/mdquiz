//! Render an item bank to a single print-ready Markdown sheet.
//!
//! This target is for paper distribution, so it deliberately omits the answer
//! key and any per-question scoring hints.

use std::fmt::Write as _;

use crate::Result;
use crate::model::{Blank, Choice, Matching, Ordering, Question, QuestionKind};

/// Render `bank` as a print-ready Markdown document with no solutions.
///
/// Questions are numbered from one under the bank name.
///
/// # Errors
///
/// Currently infallible; the `Result` is retained because [`render_question`]
/// keeps a fallible signature for future `#[non_exhaustive]` question kinds.
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
/// Currently infallible; the `Result` is retained because [`QuestionKind`] is
/// `#[non_exhaustive]`, so a future kind this sheet cannot represent can be
/// rejected as a new arm without a signature change rippling to every caller.
#[expect(
    clippy::unnecessary_wraps,
    reason = "fallible signature reserved for future non_exhaustive QuestionKind variants"
)]
fn render_question(number: usize, question: &Question) -> Result<String> {
    match &question.kind {
        QuestionKind::TrueFalse(_) => Ok(render_true_false(number, &question.prompt)),
        QuestionKind::MultipleChoice(set) => Ok(render_choices(
            number,
            &question.prompt,
            &set.choices,
            false,
        )),
        QuestionKind::MultipleSelect(set) => {
            Ok(render_choices(number, &question.prompt, &set.choices, true))
        }
        QuestionKind::FillInBlank(fitb) => {
            Ok(render_fill_in_blank(number, &question.prompt, &fitb.blanks))
        }
        QuestionKind::Matching(matching) => Ok(render_matching(number, &question.prompt, matching)),
        QuestionKind::Ordering(ordering) => Ok(render_ordering(number, &question.prompt, ordering)),
    }
}

/// Render an ordering item: the items shown sorted (never in the correct order),
/// each with a blank to write its position, no key.
fn render_ordering(number: usize, prompt: &str, ordering: &Ordering) -> String {
    let mut out = format!("{number}. {prompt}\n");
    for index in ordering.display_order() {
        if let Some(item) = ordering.items.get(index) {
            // Writing to a `String` is infallible, so the result is discarded.
            let _ = write!(out, "\n   ____ {item}");
        }
    }
    out.push('\n');
    out
}

/// Render a matching item: numbered left prompts, then lettered right options
/// (all options, sorted so they do not line up with the prompts), no key.
fn render_matching(number: usize, prompt: &str, matching: &Matching) -> String {
    let mut out = format!("{number}. {prompt}\n");
    for (index, pair) in matching.pairs.iter().enumerate() {
        // Writing to a `String` is infallible, so the result is discarded.
        let _ = write!(out, "\n   {}. {}", index + 1, pair.left);
    }
    out.push('\n');
    let mut options = matching.options();
    options.sort_unstable();
    for (index, right) in options.iter().enumerate() {
        let _ = write!(out, "\n   {}. {right}", choice_label(index));
    }
    out.push('\n');
    out
}

/// Render a fill-in-the-blank item: each `{{name}}` marker becomes a blank line.
fn render_fill_in_blank(number: usize, prompt: &str, blanks: &[Blank]) -> String {
    let mut filled = prompt.to_owned();
    for blank in blanks {
        filled = filled.replace(&crate::model::blank_marker(&blank.id), "________");
    }
    format!("{number}. {filled}\n")
}

/// Render a true/false item: the prompt plus blank True/False checkboxes.
fn render_true_false(number: usize, prompt: &str) -> String {
    format!("{number}. {prompt}\n\n   - [ ] True\n   - [ ] False\n")
}

/// Render a choice-based item: the prompt then lettered options, no key.
///
/// With `multiple`, options carry a blank checkbox to signal that more than one
/// may be selected; otherwise they are a plain lettered list.
fn render_choices(number: usize, prompt: &str, choices: &[Choice], multiple: bool) -> String {
    let mut out = format!("{number}. {prompt}\n");
    for (index, choice) in choices.iter().enumerate() {
        let label = choice_label(index);
        // Writing to a `String` is infallible, so the result is discarded.
        if multiple {
            let _ = write!(out, "\n   - [ ] {label}. {}", choice.text);
        } else {
            let _ = write!(out, "\n   {label}. {}", choice.text);
        }
    }
    out.push('\n');
    out
}

/// The letter label for a choice at `index`: `A`..`Z`, then a 1-based number.
fn choice_label(index: usize) -> String {
    if let Ok(offset) = u8::try_from(index)
        && offset < 26
    {
        return char::from(b'A' + offset).to_string();
    }
    (index + 1).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Blank, Choice, ChoiceSet, Feedback, FillInBlank, ItemBank, MatchMode, MatchPair, Matching,
        MultipleSelect, Question, QuestionKind, ScoringMode, TrueFalse,
    };

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
    /// A multiple-choice item renders lettered options with no answer marked.
    fn renders_multiple_choice_without_answer() {
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "mc".to_owned(),
                title: None,
                prompt: "Which is O(1)?".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::MultipleChoice(ChoiceSet {
                    choices: vec![
                        Choice {
                            text: "Hash lookup".to_owned(),
                            correct: true,
                        },
                        Choice {
                            text: "Linear scan".to_owned(),
                            correct: false,
                        },
                    ],
                }),
            }],
        };
        let out = to_print_markdown(&bank).expect("renders");
        assert!(out.contains("1. Which is O(1)?"));
        assert!(out.contains("A. Hash lookup"));
        assert!(out.contains("B. Linear scan"));
        // No answer key: correctness must not leak into the sheet.
        assert!(!out.contains("correct"));
        assert!(!out.contains("[x]"));
    }

    #[test]
    /// A multiple-select item renders blank checkboxes, one per option, no key.
    fn renders_multiple_select_with_checkboxes() {
        let choice = |text: &str, correct: bool| Choice {
            text: text.to_owned(),
            correct,
        };
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "ms".to_owned(),
                title: None,
                prompt: "Select all sorted-input algorithms.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::MultipleSelect(MultipleSelect {
                    scoring: ScoringMode::AllOrNothing,
                    choices: vec![
                        choice("Binary search", true),
                        choice("Linear search", false),
                    ],
                }),
            }],
        };
        let out = to_print_markdown(&bank).expect("renders");
        assert!(out.contains("- [ ] A. Binary search"));
        assert!(out.contains("- [ ] B. Linear search"));
        assert!(!out.contains("[x]"));
    }

    #[test]
    /// A fill-in-the-blank item renders each marker as a blank line, no answers.
    fn renders_fill_in_blank_as_blank_lines() {
        let make_blank = |id: &str, answer: &str| Blank {
            id: id.to_owned(),
            answers: vec![answer.to_owned()],
            match_mode: MatchMode::CaseInsensitive,
        };
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "fitb".to_owned(),
                title: None,
                prompt: "HTTP {{method}} returns {{code}}.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::FillInBlank(FillInBlank {
                    blanks: vec![make_blank("method", "GET"), make_blank("code", "404")],
                }),
            }],
        };
        let out = to_print_markdown(&bank).expect("renders");
        assert!(out.contains("1. HTTP ________ returns ________."));
        assert!(!out.contains("{{"));
        assert!(!out.contains("GET"));
    }

    #[test]
    /// A matching item renders numbered prompts and lettered options, no key.
    fn renders_matching_without_answer_key() {
        let pair = |left: &str, right: &str| MatchPair {
            left: left.to_owned(),
            right: right.to_owned(),
        };
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "mt".to_owned(),
                title: None,
                prompt: "Match each type to its size.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::Matching(Matching {
                    pairs: vec![pair("char", "1 byte"), pair("int", "4 bytes")],
                    distractors: vec!["8 bytes".to_owned()],
                }),
            }],
        };
        let out = to_print_markdown(&bank).expect("renders");
        assert!(out.contains("1. Match each type to its size."));
        assert!(out.contains("   1. char"));
        assert!(out.contains("   2. int"));
        // Right options are lettered, sorted, and include the distractor.
        assert!(out.contains("A. 1 byte"));
        assert!(out.contains("C. 8 bytes"));
        // No answer key: no pairing (e.g. "left = right") is written out.
        assert!(!out.contains('='));
    }

    #[test]
    /// Choice labels are letters up to Z, then fall back to 1-based numbers.
    fn choice_labels_letter_then_number() {
        assert_eq!(choice_label(0), "A");
        assert_eq!(choice_label(25), "Z");
        assert_eq!(choice_label(26), "27");
    }

    #[test]
    /// An ordering item lists its options sorted (never in the correct order),
    /// each with a write-in blank, and no answer key.
    fn renders_ordering_without_answer_key() {
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "ord".to_owned(),
                title: None,
                prompt: "Order the phases.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                // Authored (correct) order; display sorts to Compile, Link, Run.
                kind: QuestionKind::Ordering(Ordering {
                    items: vec!["Run".to_owned(), "Compile".to_owned(), "Link".to_owned()],
                }),
            }],
        };
        let out = to_print_markdown(&bank).expect("renders");
        assert!(out.contains("1. Order the phases."));
        // Sorted display order, not the authored order, with write-in blanks.
        let items = out.find("____ Compile").expect("compile listed");
        let link = out.find("____ Link").expect("link listed");
        let run = out.find("____ Run").expect("run listed");
        assert!(items < link && link < run);
    }
}
