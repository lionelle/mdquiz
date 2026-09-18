//! A question's answers: what the student writes in, and what the key says.
//!
//! Both live here because they must agree. The sheet letters its options from
//! the presented order and the key names those letters back — so a key built
//! from a different order than the sheet it grades is the one failure neither
//! document can reveal on its own. One module, one order.
//!
//! One line per option, as runs. The container decides the `w:pPr` each line
//! is set with — the same division [`super::inline`] uses, and for the same
//! reason: the indent and the spacing that hold an option list under its
//! prompt are page furniture, while the label and the option text are content.
//!
//! The idioms follow the bank print sheet in `export::markdown`, so the two
//! artifacts read the same way: a lettered list for a single answer, a
//! checkbox where more than one may be picked, and a [`BLANK`] wherever the
//! student writes a whole answer in. A blank *within* a sentence is a
//! different width and belongs to the prompt — see [`crate::model::BLANK_FILL`].
//!
//! # The presented order is stored, not derived
//!
//! A matching or ordering question's presented order is read from
//! [`ExamItem::presented_options`], which reads the order assembly froze. That
//! order is the one thing a sheet and its answer key must agree on, and it is
//! decided once during assembly; a writer that sorted the options itself would
//! be the bug [`crate::quiz::exam`] exists to prevent. The run manifest reads
//! the same accessor, so what it records is what was printed.

use std::fmt::Write as _;

use super::Refs;
use super::inline;
use super::inline::Kind;
use crate::Result;
use crate::label::sequence as choice_label;
use crate::model::{Choice, Matching, Ordering, Question, QuestionKind};
use crate::quiz::exam::ExamItem;

/// The blank a student writes an answer into.
const BLANK: &str = "____ ";

/// The box a student ticks when more than one option may be picked.
///
/// Brackets rather than `☐`: the ballot-box code point is missing from some
/// of the faces a reader may substitute, and a missing glyph prints as a box
/// that looks deliberate — so the sheet would be wrong in a way nobody can
/// see. Brackets are in every face.
const CHECKBOX: &str = "[ ] ";

/// `question`'s prompt as it should be printed.
///
/// Fill-in-the-blank markers become writing room. A `{{city}}` is an
/// instruction to the exporter rather than text, so printing the prompt
/// verbatim would put `{{city}}` on the exam paper.
pub(super) fn prompt(question: &Question) -> String {
    let QuestionKind::FillInBlank(fitb) = &question.kind else {
        return question.prompt.clone();
    };
    crate::model::fill_blanks(&question.prompt, &fitb.blanks)
}

/// The lines of `item`'s answer structure, in print order.
///
/// Empty where a kind prints nothing under its prompt: a fill-in-the-blank
/// question carries its blanks *in* the prompt, so an option list below it
/// would have nothing to hold.
///
/// An individual line may also be empty, and is a gap the caller must set like
/// any other — a matching question separates its prompts from its options that
/// way, because a gap is `w:pPr` and this module writes only runs. Do not
/// filter run-less lines out the way `docx::prose` does; that closes it.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if an option holds math that
/// cannot be rendered.
pub(super) fn lines(item: &ExamItem, refs: &mut Refs<'_>) -> Result<Vec<String>> {
    match &item.question.kind {
        QuestionKind::TrueFalse(_) => true_false(refs),
        QuestionKind::MultipleChoice(set) => choices(&set.choices, false, refs),
        QuestionKind::MultipleSelect(set) => choices(&set.choices, true, refs),
        QuestionKind::FillInBlank(_) => Ok(Vec::new()),
        QuestionKind::Matching(matching) => {
            matching_lines(matching, &item.presented_options(), refs)
        }
        QuestionKind::Ordering(_) => ordering_lines(&item.presented_options(), refs),
    }
}

/// The two options of a true/false question, each with a box to tick.
///
/// # Errors
///
/// Propagates from [`option_line`], which cannot fail for these two literals.
fn true_false(refs: &mut Refs<'_>) -> Result<Vec<String>> {
    ["True", "False"]
        .into_iter()
        .map(|answer| option_line(CHECKBOX, answer, refs))
        .collect()
}

/// A lettered option list.
///
/// `multiple` adds a box to each: without one, nothing on the page says the
/// student may pick more than a single letter.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if a choice holds math that
/// cannot be rendered.
fn choices(choices: &[Choice], multiple: bool, refs: &mut Refs<'_>) -> Result<Vec<String>> {
    choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let box_ = if multiple { CHECKBOX } else { "" };
            let label = format!("{box_}{}. ", choice_label(index));
            option_line(&label, &choice.text, refs)
        })
        .collect()
}

/// The left prompts of a matching question, then the options they draw from.
///
/// Two differently labelled blocks — numbered prompts with a blank each, then
/// lettered options — separated by a gap, which is the empty line in the
/// middle.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if a prompt or option holds math
/// that cannot be rendered.
fn matching_lines(
    matching: &Matching,
    options: &[String],
    refs: &mut Refs<'_>,
) -> Result<Vec<String>> {
    let mut lines = matching_prompts(matching, refs)?;
    lines.push(String::new());
    lines.extend(matching_options(options, refs)?);
    Ok(lines)
}

/// The numbered left prompts, each with a blank for the letter it matches.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if a prompt holds math that
/// cannot be rendered.
fn matching_prompts(matching: &Matching, refs: &mut Refs<'_>) -> Result<Vec<String>> {
    matching
        .pairs
        .iter()
        .enumerate()
        .map(|(index, pair)| option_line(&format!("{BLANK}{}. ", index + 1), &pair.left, refs))
        .collect()
}

/// The lettered options, in the presented order.
///
/// Every option is listed, distractors included: a student who could count the
/// options against the prompts would otherwise get the last pair free.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if an option holds math that
/// cannot be rendered.
fn matching_options(options: &[String], refs: &mut Refs<'_>) -> Result<Vec<String>> {
    options
        .iter()
        .enumerate()
        .map(|(position, option)| {
            option_line(&format!("{}. ", choice_label(position)), option, refs)
        })
        .collect()
}

/// The items of an ordering question, each with a blank for its position.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if an item holds math that cannot
/// be rendered.
fn ordering_lines(items: &[String], refs: &mut Refs<'_>) -> Result<Vec<String>> {
    items
        .iter()
        .map(|item| option_line(BLANK, item, refs))
        .collect()
}

/// One answer line: its label as literal text, then `text` rendered.
///
/// The label is literal because it is the writer's own — a letter, a number, a
/// blank — and must not be read as Markdown. The option text *is* authored, so
/// it goes through [`inline`] and gets its marks and math.
///
/// An option is one line, so anything `inline` splits into several paragraphs
/// is run together onto this one and each paragraph's kind is dropped: a list
/// in an option keeps its text and loses its markers, which live in the
/// `w:pPr` a single line has no room for.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if `text` holds math that cannot
/// be rendered.
fn option_line(label: &str, text: &str, refs: &mut Refs<'_>) -> Result<String> {
    let blocks = inline::paragraphs(text, refs)?;
    // An option is one line, so every block is folded into it — and a table
    // is not something that folds. Its `w:tbl` inside a `w:p` is well-formed
    // XML that Word refuses to open, so an option that somehow holds one
    // prints its source instead. Nonsense to author, but a visibly odd
    // option beats an unopenable paper.
    if blocks.iter().any(|block| matches!(block.kind, Kind::Table)) {
        return Ok(inline::text_run(label) + &inline::text_run(text));
    }
    let mut runs = inline::text_run(label);
    for block in blocks {
        let _ = write!(runs, "{}", block.runs);
    }
    Ok(runs)
}

/// The key's line for `item`: its correct answer, as runs.
///
/// Named by *label* wherever the student answers with one, because that is
/// what the grader is comparing against — a letter circled on the sheet. The
/// labels come from the same presented order the sheet lettered, so the two
/// cannot disagree.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if an answer holds math that
/// cannot be rendered.
pub(super) fn key_line(item: &ExamItem, refs: &mut Refs<'_>) -> Result<String> {
    match &item.question.kind {
        QuestionKind::TrueFalse(tf) => {
            Ok(inline::text_run(if tf.answer { "True" } else { "False" }))
        }
        QuestionKind::MultipleChoice(set) => correct_choices(&set.choices, refs),
        QuestionKind::MultipleSelect(set) => correct_choices(&set.choices, refs),
        QuestionKind::FillInBlank(fitb) => blank_answers(&fitb.blanks, refs),
        QuestionKind::Matching(matching) => matching_key(matching, &item.presented_options(), refs),
        QuestionKind::Ordering(ordering) => ordering_key(ordering, refs),
    }
}

/// The correct choices as `label. text`, joined; covers single and multi-select.
///
/// The label is the choice's own position, which is the position it prints in:
/// assembly shuffles the payload itself for these kinds, so there is no
/// separate order to consult.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if a choice holds math that
/// cannot be rendered.
fn correct_choices(choices: &[Choice], refs: &mut Refs<'_>) -> Result<String> {
    let correct: Vec<(usize, &Choice)> = choices
        .iter()
        .enumerate()
        .filter(|(_, choice)| choice.correct)
        .collect();
    let mut lines = Vec::new();
    for (index, choice) in correct {
        let label = format!("{}. ", choice_label(index));
        lines.push(option_line(&label, &choice.text, refs)?);
    }
    Ok(joined(&lines))
}

/// Each blank's accepted answers, as `name: a, b`, joined.
///
/// Every accepted answer, not just the first: the grader needs to know a
/// response is right, and a blank matched by regex has no single spelling.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if an answer holds math that
/// cannot be rendered.
fn blank_answers(blanks: &[crate::model::Blank], refs: &mut Refs<'_>) -> Result<String> {
    let mut lines = Vec::new();
    for blank in blanks {
        let label = format!("{}: ", blank.id);
        lines.push(option_line(&label, &blank.answers.join(", "), refs)?);
    }
    Ok(joined(&lines))
}

/// Each pair as `n. left → LABEL`, joined.
///
/// The label is the one the sheet printed against that pair's right-hand
/// answer, found by looking the answer up in the presented order. A key naming
/// the answer's *text* would grade a shuffled variant just as well, but a
/// grader reading the letter a student wrote wants the letter.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if a prompt holds math that
/// cannot be rendered.
fn matching_key(matching: &Matching, options: &[String], refs: &mut Refs<'_>) -> Result<String> {
    let mut lines = Vec::new();
    for (index, pair) in matching.pairs.iter().enumerate() {
        let label = label_of(&pair.right, options);
        let prefix = format!("{}. ", index + 1);
        let mut runs = option_line(&prefix, &pair.left, refs)?;
        let _ = write!(runs, "{}", inline::text_run(&format!(" → {label}")));
        lines.push(runs);
    }
    Ok(joined(&lines))
}

/// The printed label of the option whose text is `right`.
///
/// Looked up in the options *as printed*, so the letter this returns is the
/// letter on the paper. Empty when the answer is not among them, which parsing
/// rejects: a pair's right-hand side is always one of the options, because
/// that is where [`Matching::options`] collects them from.
fn label_of(right: &str, options: &[String]) -> String {
    options
        .iter()
        .position(|option| option == right)
        .map_or_else(String::new, choice_label)
}

/// The items in their correct order, numbered.
///
/// The authored order is the answer, so this is the payload read straight
/// through — the presented order is the sheet's business, not the key's.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if an item holds math that cannot
/// be rendered.
fn ordering_key(ordering: &Ordering, refs: &mut Refs<'_>) -> Result<String> {
    let mut lines = Vec::new();
    for (index, item) in ordering.items.iter().enumerate() {
        let label = format!("{}. ", index + 1);
        lines.push(option_line(&label, item, refs)?);
    }
    Ok(joined(&lines))
}

/// Several answers on one line, separated so they read as a list.
fn joined(lines: &[String]) -> String {
    lines.join(&inline::text_run(";  "))
}
