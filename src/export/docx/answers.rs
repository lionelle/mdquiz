//! What is printed under a question's prompt for the student to answer in.
//!
//! One line per option, as runs. The container decides the `w:pPr` each line
//! is set with — the same division [`super::inline`] uses, and for the same
//! reason: the indent and the spacing that hold an option list under its
//! prompt are page furniture, while the label and the option text are content.
//!
//! The idioms follow the bank print sheet in `export::markdown`, so the two
//! artifacts read the same way: a lettered list for a single answer, a
//! checkbox where more than one may be picked, and a `____` blank wherever the
//! student writes something in.
//!
//! # Nothing is derived here
//!
//! A matching or ordering question's presented order is read from
//! [`ExamItem::option_order`], never recomputed. That order is the one thing a
//! sheet and its answer key must agree on, and it is decided once during
//! assembly; a writer that sorted the options itself would be the bug
//! [`crate::quiz::exam`] exists to prevent.

use std::fmt::Write as _;

use super::inline;
use super::numbering::Numbering;
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
    let mut filled = question.prompt.clone();
    for blank in &fitb.blanks {
        filled = filled.replace(&crate::model::blank_marker(&blank.id), "________");
    }
    filled
}

/// The lines of `item`'s answer structure, in print order.
///
/// Empty where a kind prints nothing under its prompt: a fill-in-the-blank
/// question carries its blanks *in* the prompt, so an option list below it
/// would have nothing to hold.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if an option holds math that
/// cannot be rendered.
pub(super) fn lines(item: &ExamItem, lists: &mut Numbering) -> Result<Vec<String>> {
    match &item.question.kind {
        QuestionKind::TrueFalse(_) => true_false(lists),
        QuestionKind::MultipleChoice(set) => choices(&set.choices, false, lists),
        QuestionKind::MultipleSelect(set) => choices(&set.choices, true, lists),
        QuestionKind::FillInBlank(_) => Ok(Vec::new()),
        QuestionKind::Matching(matching) => matching_lines(matching, &item.option_order, lists),
        QuestionKind::Ordering(ordering) => ordering_lines(ordering, &item.option_order, lists),
    }
}

/// The two options of a true/false question, each with a box to tick.
///
/// # Errors
///
/// Propagates from [`option_line`], which cannot fail for these two literals.
fn true_false(lists: &mut Numbering) -> Result<Vec<String>> {
    ["True", "False"]
        .into_iter()
        .map(|answer| option_line(CHECKBOX, answer, lists))
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
fn choices(choices: &[Choice], multiple: bool, lists: &mut Numbering) -> Result<Vec<String>> {
    choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let box_ = if multiple { CHECKBOX } else { "" };
            let label = format!("{box_}{}. ", choice_label(index));
            option_line(&label, &choice.text, lists)
        })
        .collect()
}

/// The left prompts of a matching question, then the options they draw from.
///
/// The two blocks are separated by a blank line, and every option is listed
/// including the distractors — a student who can count the options against the
/// prompts would otherwise get the last pair free.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if a prompt or option holds math
/// that cannot be rendered.
fn matching_lines(
    matching: &Matching,
    order: &[usize],
    lists: &mut Numbering,
) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for (index, pair) in matching.pairs.iter().enumerate() {
        let label = format!("{BLANK}{}. ", index + 1);
        lines.push(option_line(&label, &pair.left, lists)?);
    }
    lines.push(String::new());
    let options = matching.options();
    for (position, index) in presented(order, || matching.display_order())
        .into_iter()
        .enumerate()
    {
        let label = format!("{}. ", choice_label(position));
        let option = options.get(index).copied().unwrap_or_default();
        lines.push(option_line(&label, option, lists)?);
    }
    Ok(lines)
}

/// The items of an ordering question, each with a blank for its position.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if an item holds math that cannot
/// be rendered.
fn ordering_lines(
    ordering: &Ordering,
    order: &[usize],
    lists: &mut Numbering,
) -> Result<Vec<String>> {
    presented(order, || ordering.display_order())
        .into_iter()
        .map(|index| {
            let item = ordering.items.get(index).map_or("", String::as_str);
            option_line(BLANK, item, lists)
        })
        .collect()
}

/// `order` as frozen at assembly, or `default` if it is empty.
///
/// An [`ExamItem`] from `assemble` always carries an order. The fallback is
/// for one built by hand — a test, or a caller that bypasses assembly — and is
/// the same default assembly would have stored, so a sheet and a key built the
/// same way still agree on which option is `B`.
fn presented(order: &[usize], default: impl FnOnce() -> Vec<usize>) -> Vec<usize> {
    if order.is_empty() {
        return default();
    }
    order.to_vec()
}

/// One answer line: its label as literal text, then `text` rendered.
///
/// The label is literal because it is the writer's own — a letter, a number, a
/// blank — and must not be read as Markdown. The option text *is* authored, so
/// it goes through [`inline`] and gets its marks and math.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if `text` holds math that cannot
/// be rendered.
fn option_line(label: &str, text: &str, lists: &mut Numbering) -> Result<String> {
    let mut runs = inline::text_run(label);
    for block in inline::paragraphs(text, lists)? {
        let _ = write!(runs, "{}", block.runs);
    }
    Ok(runs)
}
