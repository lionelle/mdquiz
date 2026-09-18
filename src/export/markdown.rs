//! Render an item bank to print-ready Markdown.
//!
//! [`to_print_markdown`] produces the student sheet — no answer key or scoring
//! hints. [`to_answer_key`] produces the matching instructor key: the same
//! numbering, but each question shows only its correct answer(s).

use std::fmt::Write as _;

use crate::Result;
use crate::label::sequence as choice_label;
use crate::model::{Blank, Choice, Matching, Ordering, Question, QuestionKind};

/// Render `bank` as a print-ready Markdown document with no solutions.
///
/// Questions are numbered from one under the bank name.
///
/// # Errors
///
/// Currently infallible; the `Result` is retained because `render_question`
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
/// (all of them, in [`Matching::display_order`] so they do not line up with
/// the prompts), no key.
///
/// The order comes from the model rather than a sort here: `options` lists the
/// pair answers before the distractors, so a plain sort over the text lands
/// back on the order that makes option *n* the answer to prompt *n* whenever
/// the answers happen to sort that way. `display_order` rotates off it.
fn render_matching(number: usize, prompt: &str, matching: &Matching) -> String {
    let mut out = format!("{number}. {prompt}\n");
    for (index, pair) in matching.pairs.iter().enumerate() {
        // Writing to a `String` is infallible, so the result is discarded.
        let _ = write!(out, "\n   {}. {}", index + 1, pair.left);
    }
    out.push('\n');
    let options = matching.options();
    for (position, index) in matching.display_order().into_iter().enumerate() {
        if let Some(right) = options.get(index) {
            let _ = write!(out, "\n   {}. {right}", choice_label(position));
        }
    }
    out.push('\n');
    out
}

/// Render a fill-in-the-blank item: each `{{name}}` marker becomes a blank line.
fn render_fill_in_blank(number: usize, prompt: &str, blanks: &[Blank]) -> String {
    format!("{number}. {}\n", crate::model::fill_blanks(prompt, blanks))
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

/// Render `bank` as a Markdown **answer key**: each question number with only
/// its correct answer(s), for the instructor.
///
/// Pairs with [`to_print_markdown`] — the numbering matches, so the key reads
/// alongside the answer-free question sheet.
#[must_use]
pub fn to_answer_key(bank: &crate::model::ItemBank) -> String {
    let mut out = format!("# {} — Answer Key\n", bank.name);
    for (index, question) in bank.items.iter().enumerate() {
        out.push('\n');
        out.push_str(&render_answer(index + 1, question));
    }
    out
}

/// Render one numbered question's correct answer(s), dispatching on its kind.
fn render_answer(number: usize, question: &Question) -> String {
    let answer = match &question.kind {
        QuestionKind::TrueFalse(tf) => {
            (if tf.answer { "**True**" } else { "**False**" }).to_owned()
        }
        QuestionKind::MultipleChoice(set) => correct_choices(&set.choices),
        QuestionKind::MultipleSelect(set) => correct_choices(&set.choices),
        QuestionKind::FillInBlank(fitb) => blank_answers(&fitb.blanks),
        QuestionKind::Matching(matching) => matching_answer(matching),
        QuestionKind::Ordering(ordering) => ordering_answer(ordering),
    };
    format!("{number}. {answer}\n")
}

/// The correct choices as `label. text`, joined; covers single and multi-select.
fn correct_choices(choices: &[Choice]) -> String {
    choices
        .iter()
        .enumerate()
        .filter(|(_, choice)| choice.correct)
        .map(|(index, choice)| format!("{}. {}", choice_label(index), choice.text))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Each blank's accepted answers, as `name: a, b`, joined.
fn blank_answers(blanks: &[Blank]) -> String {
    blanks
        .iter()
        .map(|blank| format!("{}: {}", blank.id, blank.answers.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Each correct left → right pairing, joined.
fn matching_answer(matching: &Matching) -> String {
    matching
        .pairs
        .iter()
        .map(|pair| format!("{} → {}", pair.left, pair.right))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The items in their correct order, numbered.
fn ordering_answer(ordering: &Ordering) -> String {
    ordering
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| format!("{}. {item}", index + 1))
        .collect::<Vec<_>>()
        .join("; ")
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

    /// A bank of one matching question whose rights sort into the order that
    /// pairs each with its own prompt — the case a plain sort gets wrong.
    fn matching_bank() -> ItemBank {
        let pair = |left: &str, right: &str| MatchPair {
            left: left.to_owned(),
            right: right.to_owned(),
        };
        ItemBank {
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
        }
    }

    #[test]
    /// A matching item renders numbered prompts and lettered options, no key.
    fn renders_matching_without_answer_key() {
        let out = to_print_markdown(&matching_bank()).expect("renders");
        assert!(out.contains("1. Match each type to its size."));
        assert!(out.contains("   1. char"));
        assert!(out.contains("   2. int"));
        // Every option is lettered and the distractor is among them: leaving
        // it out would let a student count options against prompts and get
        // the last pair free.
        for right in ["1 byte", "4 bytes", "8 bytes"] {
            assert!(out.contains(right), "{right} is missing: {out}");
        }
        // And option A is *not* prompt 1's answer. These rights sort into the
        // order that pairs each with its own prompt, so a plain sort here
        // would print the whole answer; `display_order` rotates off it.
        assert!(
            !out.contains("A. 1 byte"),
            "the options line up with the prompts: {out}"
        );
        // No answer key: no pairing (e.g. "left = right") is written out.
        assert!(!out.contains('='));
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

    #[test]
    /// Items authored in alphabetical order still are not presented in it.
    /// The display order is a text sort, so an author who happens to write the
    /// steps alphabetically would otherwise get the answer printed on the
    /// sheet — the one thing this sort exists to prevent.
    fn an_alphabetically_authored_ordering_is_not_presented_in_order() {
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "ord".to_owned(),
                title: None,
                prompt: "Order the phases.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                // Authored (correct) order, and already alphabetical: the text
                // sort alone would hand this straight back.
                kind: QuestionKind::Ordering(Ordering {
                    items: vec!["Compile".to_owned(), "Link".to_owned(), "Run".to_owned()],
                }),
            }],
        };
        let out = to_print_markdown(&bank).expect("renders");
        let compile = out.find("____ Compile").expect("compile listed");
        let link = out.find("____ Link").expect("link listed");
        let run = out.find("____ Run").expect("run listed");
        assert!(
            !(compile < link && link < run),
            "the sheet printed the answer: {out}"
        );
    }

    #[test]
    /// The print sheet keeps math and diagram source verbatim (no rendering).
    fn print_keeps_math_and_diagrams_literal() {
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: "Cost $O(n)$ and\n\n```mermaid\ngraph TD; A-->B;\n```\n\n\
                    ```dot\ndigraph { a -> b; }\n```\n\n| Op | Cost |\n|----|------|\n\
                    | push | O(1) |"
                    .to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
            }],
        };
        let out = to_print_markdown(&bank).expect("renders");
        assert!(out.contains("$O(n)$") && out.contains("```mermaid") && out.contains("```dot"));
        // A pipe table stays literal pipe text: the print sheet renders no HTML.
        assert!(out.contains("| push | O(1) |") && !out.contains("<table"));
        assert!(!out.contains("equation_image") && !out.contains("![diagram]"));
    }

    /// A one-question bank for the given `kind`.
    fn key_question(kind: QuestionKind) -> Question {
        Question {
            id: "q".to_owned(),
            title: None,
            prompt: "p".to_owned(),
            points: 1.0,
            tags: Vec::new(),
            feedback: Feedback::default(),
            kind,
        }
    }

    #[test]
    /// The answer key shows only correct answers, numbered like the sheet.
    fn answer_key_shows_only_correct_answers() {
        let bank = ItemBank {
            name: "Q".to_owned(),
            items: vec![
                key_question(QuestionKind::TrueFalse(TrueFalse { answer: false })),
                key_question(QuestionKind::MultipleChoice(ChoiceSet {
                    choices: vec![
                        Choice {
                            text: "Right".to_owned(),
                            correct: true,
                        },
                        Choice {
                            text: "Wrong".to_owned(),
                            correct: false,
                        },
                    ],
                })),
            ],
        };
        let key = to_answer_key(&bank);
        assert!(key.contains("# Q — Answer Key"));
        assert!(key.contains("1. **False**"));
        assert!(key.contains("2. A. Right"));
        // The distractor is absent — the key lists correct answers only.
        assert!(!key.contains("Wrong"));
    }
}
