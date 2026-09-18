//! The typed representation of quiz content.
//!
//! The reusable unit is a [`Question`]: shared metadata (identifier, prompt,
//! point value) plus a [`QuestionKind`] that names its type. Questions are
//! collected into an [`ItemBank`] — an ordered, named pool of items that maps
//! onto a Canvas *New Quizzes* item bank.
//!
//! Each [`QuestionKind`] carries its own type-specific payload (choices,
//! blanks, …). See CLAUDE.md for the roll-out order.
//!
//! # Growth path
//!
//! Only the item bank is modelled today. A future `Quiz` type will live
//! alongside [`ItemBank`]: in the Canvas *New Quizzes* model a quiz is built
//! from items that may be authored inline *or* drawn from an item bank, so a
//! `Quiz` will hold its own items plus references to banks. Keeping [`Question`]
//! independent of [`ItemBank`] is what leaves room for that.

use serde::{Deserialize, Serialize};

/// A named, ordered pool of questions: one Canvas *New Quizzes* item bank.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemBank {
    /// Human-readable bank name, shown at the top of every export.
    pub name: String,
    /// The questions, in the order they should be presented.
    pub items: Vec<Question>,
}

/// A single quiz question and its shared metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// Stable identifier, unique within the bank (used for cross-references).
    pub id: String,
    /// Instructor-facing title (question name). Shown in the Canvas bank
    /// editor, not to students; absent means fall back to [`Question::id`].
    pub title: Option<String>,
    /// The question prompt, as authored Markdown.
    pub prompt: String,
    /// Points awarded for a fully correct answer.
    pub points: f64,
    /// Free-form tags for mdquiz-side organization (e.g. future filtering by
    /// topic). Not exported to Canvas, which organizes via item banks instead.
    pub tags: Vec<String>,
    /// Optional feedback shown to students after answering.
    pub feedback: Feedback,
    /// The question type together with its type-specific payload.
    pub kind: QuestionKind,
}

impl Question {
    /// The authored Markdown fields rendered as rich (`text/html`) content: the
    /// prompt, choice texts, ordering items, and any feedback message.
    ///
    /// Content transforms (image bundling, diagram rendering) walk exactly these
    /// fields. Matching cells and blank answers export as plain text and are
    /// excluded. Kinds are matched explicitly (not `_`) so a future kind with a
    /// rich-text answer must decide whether it belongs here.
    pub(crate) fn rich_text_fields(&self) -> Vec<&str> {
        let mut fields = vec![self.prompt.as_str()];
        match &self.kind {
            QuestionKind::MultipleChoice(set) => {
                fields.extend(set.choices.iter().map(|choice| choice.text.as_str()));
            }
            QuestionKind::MultipleSelect(select) => {
                fields.extend(select.choices.iter().map(|choice| choice.text.as_str()));
            }
            QuestionKind::Ordering(ordering) => {
                fields.extend(ordering.items.iter().map(String::as_str));
            }
            QuestionKind::TrueFalse(_)
            | QuestionKind::FillInBlank(_)
            | QuestionKind::Matching(_) => {}
        }
        let feedback = &self.feedback;
        let messages = [
            feedback.general.as_deref(),
            feedback.correct.as_deref(),
            feedback.incorrect.as_deref(),
        ];
        fields.extend(messages.into_iter().flatten());
        fields
    }

    /// The [rich-text fields](Self::rich_text_fields), mutably, for transforms
    /// that rewrite content in place (e.g. replacing a diagram block).
    pub(crate) fn rich_text_fields_mut(&mut self) -> Vec<&mut String> {
        let mut fields = vec![&mut self.prompt];
        match &mut self.kind {
            QuestionKind::MultipleChoice(set) => {
                fields.extend(set.choices.iter_mut().map(|choice| &mut choice.text));
            }
            QuestionKind::MultipleSelect(select) => {
                fields.extend(select.choices.iter_mut().map(|choice| &mut choice.text));
            }
            QuestionKind::Ordering(ordering) => fields.extend(ordering.items.iter_mut()),
            QuestionKind::TrueFalse(_)
            | QuestionKind::FillInBlank(_)
            | QuestionKind::Matching(_) => {}
        }
        let feedback = &mut self.feedback;
        let messages = [
            feedback.general.as_mut(),
            feedback.correct.as_mut(),
            feedback.incorrect.as_mut(),
        ];
        fields.extend(messages.into_iter().flatten());
        fields
    }
}

/// Feedback messages shown after a question is answered.
///
/// Each field is authored Markdown and optional; an all-empty [`Feedback`]
/// produces no feedback in any export. Exported to Canvas as QTI
/// `<itemfeedback>`; omitted from the print sheet, which carries no answer key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Feedback {
    /// Shown regardless of correctness.
    #[serde(default)]
    pub general: Option<String>,
    /// Shown when the answer is correct.
    #[serde(default)]
    pub correct: Option<String>,
    /// Shown when the answer is incorrect.
    #[serde(default)]
    pub incorrect: Option<String>,
}

/// The supported question types, following the Canvas *New Quizzes* model.
///
/// Each variant carries its type-specific payload; the roll-out order is the
/// order listed here. This serde shape is *not* the authored format — questions
/// are parsed via `parse::QuestionSpec`, which reads the flat `kind:` YAML tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QuestionKind {
    /// True/false question.
    TrueFalse(TrueFalse),
    /// Single-answer multiple choice.
    MultipleChoice(ChoiceSet),
    /// Multiple-answer ("select all that apply") multiple choice.
    MultipleSelect(MultipleSelect),
    /// Fill in the blank: one or more inline blanks, each matched against a
    /// list of acceptable answers (literal text or, per the blank's match
    /// mode, regex patterns).
    FillInBlank(FillInBlank),
    /// Match items in one column to items in another.
    Matching(Matching),
    /// Put items into the correct order.
    Ordering(Ordering),
}

impl QuestionKind {
    /// A short human-readable label for this type, used in diagnostics.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::TrueFalse(_) => "true/false",
            Self::MultipleChoice(_) => "multiple choice",
            Self::MultipleSelect(_) => "multiple select",
            Self::FillInBlank(_) => "fill in the blank",
            Self::Matching(_) => "matching",
            Self::Ordering(_) => "ordering",
        }
    }
}

/// The payload for a [`QuestionKind::TrueFalse`] question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrueFalse {
    /// Whether the correct answer is "true" (`true`) or "false" (`false`).
    pub answer: bool,
}

/// One selectable option in a choice-based question.
///
/// The same shape backs both single-answer [`QuestionKind::MultipleChoice`] and
/// multiple-answer [`QuestionKind::MultipleSelect`]; only how many choices may
/// be `correct` differs between them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    /// The option's text, as authored Markdown.
    pub text: String,
    /// Whether selecting this option counts as a correct answer.
    pub correct: bool,
}

/// The payload for a [`QuestionKind::MultipleChoice`] question.
///
/// The choices are presented in order; exactly one is `correct`, enforced when
/// parsing. (Multiple-select uses its own [`MultipleSelect`] payload.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceSet {
    /// The options, in presentation order.
    pub choices: Vec<Choice>,
}

/// The payload for a [`QuestionKind::MultipleSelect`] question.
///
/// A "choose all that apply" question: one or more choices are correct, and the
/// selection is graded per [`MultipleSelect::scoring`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultipleSelect {
    /// The options, in presentation order.
    pub choices: Vec<Choice>,
    /// How the selection is scored.
    pub scoring: ScoringMode,
}

/// How a multiple-select ("choose all that apply") question is graded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoringMode {
    /// Full marks only when every correct option and no incorrect option is
    /// selected. The default.
    #[default]
    AllOrNothing,
    /// Each correct selection adds an even share of the marks; each incorrect
    /// selection subtracts one (the total is clamped to zero).
    Partial,
}

/// The payload for a [`QuestionKind::FillInBlank`] question.
///
/// The prompt carries one or more inline blank markers (`{{name}}`); each named
/// blank has an entry here listing its acceptable answers. Presentation order
/// is defined by where the markers appear in the prompt, not by this list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillInBlank {
    /// The blanks referenced by the prompt, one per distinct marker name.
    pub blanks: Vec<Blank>,
}

/// One inline blank: a name plus the text answers accepted for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blank {
    /// The blank's name, matching a `{{name}}` marker in the prompt.
    pub id: String,
    /// Acceptable answers; a response matching any one (per [`Blank::match_mode`])
    /// is correct. For [`MatchMode::Regex`] these are regular-expression patterns.
    pub answers: Vec<String>,
    /// How a student response is compared against the answers.
    pub match_mode: MatchMode,
}

/// How a fill-in-the-blank response is matched against the acceptable answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Case-insensitive exact text match (the default, and Canvas's default).
    #[default]
    CaseInsensitive,
    /// Case-sensitive exact text match. Canvas's QTI import cannot express case
    /// sensitivity, so this currently exports identically to
    /// [`MatchMode::CaseInsensitive`].
    Exact,
    /// A regular-expression pattern. Canvas cannot set this via import, so the
    /// blank is exported as a literal-text blank and flagged for manual setup
    /// (switch it to "Regular Expression Match" in the New Quizzes editor).
    Regex,
}

impl FillInBlank {
    /// The blanks in prompt order, each located by its `{{name}}` marker.
    ///
    /// Blanks are stored sorted by name, so exporters use this to recover the
    /// order the author actually wrote them in.
    pub(crate) fn ordered(&self, prompt: &str) -> Vec<&Blank> {
        blank_markers(prompt)
            .iter()
            .filter_map(|name| self.blanks.iter().find(|blank| &blank.id == name))
            .collect()
    }
}

/// The inline marker text for a blank named `name` (that is, `{{name}}`).
///
/// Quiz-spec templates deliberately use `${…}` (`quiz::spec::TEMPLATE_OPEN`)
/// rather than this syntax, so a header or footer that later became a question
/// partial cannot sprout blanks where it meant to name the exam. Changing
/// either delimiter means checking the other.
pub(crate) fn blank_marker(name: &str) -> String {
    ["{{", name, "}}"].concat()
}

/// The writing room a `{{name}}` marker prints as.
///
/// Wider than the `____` a whole answer is written on: this blank sits mid
/// sentence and has to read as a gap in the text rather than a rule under it.
pub(crate) const BLANK_FILL: &str = "________";

/// `prompt` with every one of `blanks`'s markers replaced by writing room.
///
/// A marker is an instruction to the exporter rather than text, so a sheet
/// printing the prompt verbatim would put `{{city}}` on the paper. Shared by
/// both print paths, so a blank is the same width whichever sheet a student is
/// handed.
pub(crate) fn fill_blanks(prompt: &str, blanks: &[Blank]) -> String {
    blanks.iter().fold(prompt.to_owned(), |filled, blank| {
        filled.replace(&blank_marker(&blank.id), BLANK_FILL)
    })
}

/// Scan `text` for inline `{{name}}` blank markers, in first-appearance order.
///
/// Names are trimmed; markers are de-duplicated, so a name reused in the prompt
/// yields a single entry. Drives blank validation and export ordering.
pub(crate) fn blank_markers(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some((_, after_open)) = rest.split_once("{{") {
        let Some((name, after_close)) = after_open.split_once("}}") else {
            break;
        };
        let name = name.trim();
        if !name.is_empty() && !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
        rest = after_close;
    }
    names
}

/// The payload for a [`QuestionKind::Matching`] question.
///
/// Each [`MatchPair`] links a left prompt to its correct right answer;
/// `distractors` are extra right-hand options that match no left.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Matching {
    /// The left-to-right pairs, in presentation order.
    pub pairs: Vec<MatchPair>,
    /// Extra right-hand options that are not the answer to any pair.
    pub distractors: Vec<String>,
}

/// One left prompt and its correct right-hand answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchPair {
    /// The left-hand prompt.
    pub left: String,
    /// The correct right-hand answer for [`MatchPair::left`].
    pub right: String,
}

/// `order`, adjusted if it is the identity — the authored order itself.
///
/// The authored order is the answer. For an ordering question it *is* the
/// sequence being asked for; for a matching one it pairs option *n* with the
/// prompt it answers, because the options list the pair answers before the
/// distractors. A text sort usually breaks that, but items authored in
/// alphabetical order sort straight back to it — and a shuffle can land on it
/// by chance, one time in six for three items.
///
/// Rotating by one is enough: it differs from the identity at every length
/// above one, and it consumes no randomness, so a seeded sheet stays
/// reproducible and an unshuffled one does not depend on the seed at all.
pub(crate) fn hides_the_answer(mut order: Vec<usize>) -> Vec<usize> {
    if order.len() > 1 && order.iter().enumerate().all(|(at, index)| at == *index) {
        order.rotate_left(1);
    }
    order
}

impl Matching {
    /// The distinct right-hand options (pair answers then distractors), in
    /// order. These become the shared choice list every left selects from.
    pub(crate) fn options(&self) -> Vec<&str> {
        let mut options: Vec<&str> = Vec::new();
        let rights = self
            .pairs
            .iter()
            .map(|pair| pair.right.as_str())
            .chain(self.distractors.iter().map(String::as_str));
        for right in rights {
            if !options.contains(&right) {
                options.push(right);
            }
        }
        options
    }

    /// The option indices in display order: sorted by text, so the presented
    /// list does not line each option up with the prompt it answers.
    ///
    /// [`Self::options`] returns the pair answers before the distractors, so
    /// its own order makes option *n* the answer to prompt *n*. This is the
    /// default a sheet uses when its group does not shuffle.
    pub(crate) fn display_order(&self) -> Vec<usize> {
        let options = self.options();
        let mut indexed: Vec<(usize, &str)> = options.iter().copied().enumerate().collect();
        indexed.sort_by(|left, right| left.1.cmp(right.1));
        hides_the_answer(indexed.into_iter().map(|(index, _)| index).collect())
    }
}

/// The payload for a [`QuestionKind::Ordering`] question.
///
/// `items` are authored in the *correct* order; the exporters shuffle them for
/// display (see `Ordering::display_order`) so the shown sequence is not the
/// answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ordering {
    /// The items to arrange, listed in their correct order.
    pub items: Vec<String>,
}

impl Ordering {
    /// The item indices in display order: sorted by text so the presented
    /// sequence never hands the student the correct order.
    pub(crate) fn display_order(&self) -> Vec<usize> {
        let mut indexed: Vec<(usize, &str)> =
            self.items.iter().map(String::as_str).enumerate().collect();
        indexed.sort_by(|left, right| left.1.cmp(right.1));
        hides_the_answer(indexed.into_iter().map(|(index, _)| index).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Every kind reports its documented human-readable label.
    fn label_names_each_kind() {
        let tf = QuestionKind::TrueFalse(TrueFalse { answer: true });
        let mc = QuestionKind::MultipleChoice(ChoiceSet {
            choices: Vec::new(),
        });
        let ms = QuestionKind::MultipleSelect(MultipleSelect {
            choices: Vec::new(),
            scoring: ScoringMode::AllOrNothing,
        });
        let fitb = QuestionKind::FillInBlank(FillInBlank { blanks: Vec::new() });
        let matching = QuestionKind::Matching(Matching {
            pairs: Vec::new(),
            distractors: Vec::new(),
        });
        let cases = [
            (tf, "true/false"),
            (mc, "multiple choice"),
            (ms, "multiple select"),
            (fitb, "fill in the blank"),
            (matching, "matching"),
            (
                QuestionKind::Ordering(Ordering { items: Vec::new() }),
                "ordering",
            ),
        ];
        for (kind, label) in cases {
            assert_eq!(kind.label(), label);
        }
    }

    #[test]
    /// `options` lists pair answers first then distractors, deduplicated by
    /// value while preserving that order.
    fn matching_options_orders_and_dedups() {
        let pair = |right: &str| MatchPair {
            left: "l".to_owned(),
            right: right.to_owned(),
        };
        let matching = Matching {
            // "X" repeats across pairs; the "Y" distractor duplicates a pair.
            pairs: vec![pair("X"), pair("Y"), pair("X")],
            distractors: vec!["Y".to_owned(), "Z".to_owned()],
        };
        assert_eq!(matching.options(), ["X", "Y", "Z"]);
    }

    #[test]
    /// `display_order` presents items sorted by text, not in their (correct)
    /// authored order.
    fn ordering_display_order_sorts_by_text() {
        let ordering = Ordering {
            items: vec!["gamma".to_owned(), "alpha".to_owned(), "beta".to_owned()],
        };
        // Authored order is 0,1,2 (gamma, alpha, beta); sorted is alpha, beta,
        // gamma -> original indices 1,2,0.
        assert_eq!(ordering.display_order(), [1, 2, 0]);
    }
}
