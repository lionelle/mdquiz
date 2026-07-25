//! Parse Markdown + YAML question sources into the typed model.
//!
//! Each source file is one question. YAML front-matter (a `---`-delimited block
//! at the top of the file) carries the machine-readable answer data; everything
//! below it is the prompt as ordinary Markdown. Parsing splits the two: the
//! front-matter is deserialized into a [`QuestionSpec`] (a flat, `kind`-tagged
//! shape), and the body becomes the prompt. A directory of such files is
//! assembled into one [`ItemBank`].

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use pulldown_cmark::{Event, MetadataBlockKind, Options, Parser, Tag, TagEnd};
use serde::Deserialize;

use crate::Result;
use crate::model::{
    Blank, Choice, ChoiceSet, Feedback, FillInBlank, ItemBank, MatchMode, MatchPair, Matching,
    MultipleSelect, Ordering, Question, QuestionKind, ScoringMode, TrueFalse, blank_markers,
    is_local_image,
};

/// Reads a partial's raw Markdown given its bank-relative `path`, returning a
/// human-readable message on failure.
///
/// Supplied by the caller that owns the files (the CLI), so the parse layer
/// stays free of direct filesystem calls. The default reader
/// ([`reject_includes`]) rejects every `file:`, so `parse_question` and
/// `item_bank_from_sources` support inline content only; callers that can read
/// files use the `_with` variants.
///
/// The failure type is a plain `String` message (not the crate [`Error`]) so
/// callers need not depend on the crate's error type; [`read_partial`] wraps it
/// into [`crate::Error::InvalidQuestion`] with the question's context.
pub type PartialReader<'a> = dyn Fn(&str) -> std::result::Result<String, String> + 'a;

/// A [`PartialReader`] that rejects every include, for the disk-free entry
/// points where no directory context is available.
///
/// # Errors
///
/// Always returns a message explaining that `file:` needs a directory context.
fn reject_includes(path: &str) -> std::result::Result<String, String> {
    Err(format!(
        "`file:` include {path:?} needs a directory context; export from a directory"
    ))
}

/// The metadata every question shares, flattened into each [`QuestionSpec`].
#[derive(Debug, Deserialize)]
struct CommonSpec {
    /// Stable identifier, unique within the bank.
    id: String,
    /// Optional instructor-facing title (question name).
    #[serde(default)]
    title: Option<String>,
    /// Points for a correct answer; defaults to one.
    #[serde(default = "default_points")]
    points: f64,
    /// Optional organizational tags.
    #[serde(default)]
    tags: Vec<String>,
    /// Optional post-answer feedback.
    #[serde(default)]
    feedback: FeedbackSpec,
}

impl CommonSpec {
    /// Attach the shared metadata to a `prompt` and `kind` to form a question,
    /// resolving any `file:` feedback partials via `read`.
    ///
    /// # Errors
    ///
    /// Propagates any feedback-partial resolution failure.
    fn into_question(
        self,
        prompt: String,
        kind: QuestionKind,
        read: &PartialReader<'_>,
    ) -> Result<Question> {
        let feedback = self.feedback.resolve(read, &self.id)?;
        Ok(Question {
            id: self.id,
            title: self.title,
            prompt,
            points: self.points,
            tags: self.tags,
            feedback,
            kind,
        })
    }
}

/// One authored choice for a choice-based question.
///
/// The option content is exactly one of inline `text` or a `file` partial;
/// resolution enforces that. `correct` marks the answer.
#[derive(Debug, Deserialize)]
struct ChoiceSpec {
    /// Inline option text, as authored Markdown. Mutually exclusive with `file`.
    #[serde(default)]
    text: Option<String>,
    /// A partial file whose rendered Markdown is the option, resolved relative
    /// to the bank directory. Mutually exclusive with `text`.
    #[serde(default)]
    file: Option<String>,
    /// Whether this option is correct; defaults to false.
    #[serde(default)]
    correct: bool,
}

/// The authored value for one fill-in-the-blank blank.
///
/// Accepts either a bare list of acceptable answers (case-insensitive), or a
/// map with `answers` plus a `match` mode (`case_insensitive`/`exact`/`regex`).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum BlankSpec {
    /// Shorthand: a list of acceptable answers, matched case-insensitively.
    Answers(Vec<String>),
    /// Expanded form: acceptable answers plus a `match` mode.
    Detailed {
        /// Acceptable answers (or regex patterns when `match` is `regex`).
        answers: Vec<String>,
        /// How responses are matched; defaults to case-insensitive.
        #[serde(rename = "match", default)]
        match_mode: MatchMode,
    },
}

impl BlankSpec {
    /// Build the model [`Blank`] for marker `id` from this authored value.
    fn into_blank(self, id: String) -> Blank {
        match self {
            Self::Answers(answers) => Blank {
                id,
                answers,
                match_mode: MatchMode::CaseInsensitive,
            },
            Self::Detailed {
                answers,
                match_mode,
            } => Blank {
                id,
                answers,
                match_mode,
            },
        }
    }
}

/// An authored message or answer: inline Markdown text, or a `file:` partial.
///
/// Backs ordering items and feedback messages. (Choices carry their own
/// `text`/`file` fields instead, because they also need a `correct` flag.)
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MessageSpec {
    /// Inline Markdown text.
    Text(String),
    /// A partial file whose rendered Markdown is used in place of the message.
    File {
        /// The partial's path, relative to the bank directory.
        file: String,
    },
}

impl MessageSpec {
    /// Resolve to final Markdown, reading and rebasing a `file:` partial.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::InvalidQuestion`] naming `id` when the partial
    /// cannot be read.
    fn resolve(self, read: &PartialReader<'_>, id: &str) -> Result<String> {
        match self {
            Self::Text(text) => Ok(text),
            Self::File { file } => read_partial(&file, read, id),
        }
    }
}

/// The authored feedback block: each message is inline text or a `file:` partial.
#[derive(Debug, Default, Deserialize)]
struct FeedbackSpec {
    /// Shown regardless of correctness.
    #[serde(default)]
    general: Option<MessageSpec>,
    /// Shown when the answer is correct.
    #[serde(default)]
    correct: Option<MessageSpec>,
    /// Shown when the answer is incorrect.
    #[serde(default)]
    incorrect: Option<MessageSpec>,
}

impl FeedbackSpec {
    /// Resolve every present message into a model [`Feedback`].
    ///
    /// # Errors
    ///
    /// Propagates any partial-resolution failure (see [`MessageSpec::resolve`]).
    fn resolve(self, read: &PartialReader<'_>, id: &str) -> Result<Feedback> {
        Ok(Feedback {
            general: resolve_opt(self.general, read, id)?,
            correct: resolve_opt(self.correct, read, id)?,
            incorrect: resolve_opt(self.incorrect, read, id)?,
        })
    }
}

/// Resolve an optional [`MessageSpec`], threading `read` and the question `id`.
///
/// # Errors
///
/// Propagates any partial-resolution failure (see [`MessageSpec::resolve`]).
fn resolve_opt(
    message: Option<MessageSpec>,
    read: &PartialReader<'_>,
    id: &str,
) -> Result<Option<String>> {
    message.map(|m| m.resolve(read, id)).transpose()
}

/// The authored YAML shape, tagged by `kind`, before the prompt is attached.
///
/// This is deliberately separate from [`QuestionKind`]: the prompt lives in the
/// Markdown body, not the YAML, so the two are joined in [`parse_question`].
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum QuestionSpec {
    /// A true/false question.
    TrueFalse {
        /// The shared question metadata.
        #[serde(flatten)]
        common: CommonSpec,
        /// Whether the correct answer is "true".
        answer: bool,
    },
    /// A single-answer multiple-choice question.
    MultipleChoice {
        /// The shared question metadata.
        #[serde(flatten)]
        common: CommonSpec,
        /// The options, in presentation order.
        choices: Vec<ChoiceSpec>,
    },
    /// A multiple-answer ("select all that apply") question.
    MultipleSelect {
        /// The shared question metadata.
        #[serde(flatten)]
        common: CommonSpec,
        /// The options, in presentation order.
        choices: Vec<ChoiceSpec>,
        /// How the selection is scored; defaults to all-or-nothing.
        #[serde(default)]
        scoring: ScoringMode,
    },
    /// A fill-in-the-blank question with inline `{{name}}` blanks.
    FillInBlank {
        /// The shared question metadata.
        #[serde(flatten)]
        common: CommonSpec,
        /// Acceptable answers keyed by blank name.
        blanks: BTreeMap<String, BlankSpec>,
    },
    /// A matching question: left prompts paired with right answers.
    Matching {
        /// The shared question metadata.
        #[serde(flatten)]
        common: CommonSpec,
        /// The left-to-right pairs, in presentation order.
        pairs: Vec<MatchPair>,
        /// Extra right-hand options that match no left.
        #[serde(default)]
        distractors: Vec<String>,
    },
    /// An ordering question: items to arrange in a correct sequence.
    Ordering {
        /// The shared question metadata.
        #[serde(flatten)]
        common: CommonSpec,
        /// The items, authored in their correct order (inline text or `file:`).
        items: Vec<MessageSpec>,
    },
}

/// The default point value when a question omits `points`.
fn default_points() -> f64 {
    1.0
}

impl QuestionSpec {
    /// Combine the YAML spec with its Markdown `prompt` into a [`Question`],
    /// resolving any `file:` partials via `read`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::InvalidQuestion`] when a choice is malformed
    /// (both or neither of `text`/`file`), or when any `file:` partial (choice,
    /// ordering item, or feedback message) cannot be read.
    fn into_question(self, prompt: String, read: &PartialReader<'_>) -> Result<Question> {
        match self {
            Self::TrueFalse { common, answer } => {
                common.into_question(prompt, QuestionKind::TrueFalse(TrueFalse { answer }), read)
            }
            Self::MultipleChoice { common, choices } => {
                choice_question(common, prompt, choices, read)
            }
            Self::MultipleSelect {
                common,
                choices,
                scoring,
            } => select_question(common, prompt, choices, scoring, read),
            Self::FillInBlank { common, blanks } => common.into_question(
                prompt,
                QuestionKind::FillInBlank(fill_in_blank(blanks)),
                read,
            ),
            Self::Matching {
                common,
                pairs,
                distractors,
            } => common.into_question(
                prompt,
                QuestionKind::Matching(Matching { pairs, distractors }),
                read,
            ),
            Self::Ordering { common, items } => ordering_question(common, prompt, items, read),
        }
    }
}

/// Convert authored [`ChoiceSpec`]s into model [`Choice`]s, resolving partials.
///
/// # Errors
///
/// Propagates any choice-content resolution failure (see [`resolve_content`]).
fn choices_vec(
    choices: Vec<ChoiceSpec>,
    read: &PartialReader<'_>,
    id: &str,
) -> Result<Vec<Choice>> {
    choices
        .into_iter()
        .map(|choice| {
            Ok(Choice {
                text: resolve_content(choice.text, choice.file, read, id)?,
                correct: choice.correct,
            })
        })
        .collect()
}

/// Build a resolved single-answer multiple-choice [`Question`].
///
/// Takes `common` by value so the choice partials can borrow its `id` for error
/// context before it is consumed into the question.
///
/// # Errors
///
/// Propagates any choice-content or feedback-partial resolution failure.
fn choice_question(
    common: CommonSpec,
    prompt: String,
    choices: Vec<ChoiceSpec>,
    read: &PartialReader<'_>,
) -> Result<Question> {
    let set = ChoiceSet {
        choices: choices_vec(choices, read, &common.id)?,
    };
    common.into_question(prompt, QuestionKind::MultipleChoice(set), read)
}

/// Build a resolved multiple-select [`Question`] with the given scoring.
///
/// # Errors
///
/// Propagates any choice-content or feedback-partial resolution failure.
fn select_question(
    common: CommonSpec,
    prompt: String,
    choices: Vec<ChoiceSpec>,
    scoring: ScoringMode,
    read: &PartialReader<'_>,
) -> Result<Question> {
    let select = MultipleSelect {
        choices: choices_vec(choices, read, &common.id)?,
        scoring,
    };
    common.into_question(prompt, QuestionKind::MultipleSelect(select), read)
}

/// Build a resolved ordering [`Question`], resolving each item's partials.
///
/// # Errors
///
/// Propagates any item- or feedback-partial resolution failure.
fn ordering_question(
    common: CommonSpec,
    prompt: String,
    items: Vec<MessageSpec>,
    read: &PartialReader<'_>,
) -> Result<Question> {
    let items = items
        .into_iter()
        .map(|item| item.resolve(read, &common.id))
        .collect::<Result<Vec<_>>>()?;
    common.into_question(prompt, QuestionKind::Ordering(Ordering { items }), read)
}

/// Resolve one choice's content: inline `text`, or a `file:` partial's rendered
/// Markdown (with its local images rebased to the bank directory).
///
/// # Errors
///
/// Returns [`crate::Error::InvalidQuestion`] naming `id` when both or neither of
/// `text`/`file` are given, or when the partial cannot be read.
fn resolve_content(
    text: Option<String>,
    file: Option<String>,
    read: &PartialReader<'_>,
    id: &str,
) -> Result<String> {
    match (text, file) {
        (Some(text), None) => Ok(text),
        (None, Some(path)) => read_partial(&path, read, id),
        (Some(_), Some(_)) => Err(crate::Error::InvalidQuestion(format!(
            "question {id:?} has a choice with both `text` and `file`"
        ))),
        (None, None) => Err(crate::Error::InvalidQuestion(format!(
            "question {id:?} has a choice with neither `text` nor `file`"
        ))),
    }
}

/// Read partial `path` via `read` and rebase its local images to the bank root.
///
/// # Errors
///
/// Returns [`crate::Error::InvalidQuestion`] naming `id` when `read` fails.
fn read_partial(path: &str, read: &PartialReader<'_>, id: &str) -> Result<String> {
    let raw = read(path).map_err(|message| {
        crate::Error::InvalidQuestion(format!(
            "question {id:?} cannot include {path:?}: {message}"
        ))
    })?;
    Ok(rebase_images(&raw, parent_dir(path)))
}

/// The directory portion of a partial `path` (`""` when it has none).
fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => path.get(..index).unwrap_or(""),
        None => "",
    }
}

/// Rewrite each local image URL in `markdown` to sit under `base` (the partial's
/// directory), so bundling resolves it relative to the bank root. External and
/// absolute URLs are left untouched; an empty `base` needs no rewrite.
fn rebase_images(markdown: &str, base: &str) -> String {
    if base.is_empty() {
        return markdown.to_owned();
    }
    let mut spans = image_url_spans(markdown);
    // Apply back-to-front so earlier byte offsets stay valid as text is spliced.
    spans.sort_by_key(|span| std::cmp::Reverse(span.0));
    let mut out = markdown.to_owned();
    for (start, end, url) in spans {
        out.replace_range(start..end, &format!("{base}/{url}"));
    }
    out
}

/// The `(start, end, url)` byte spans of every local image URL in `markdown`.
fn image_url_spans(markdown: &str) -> Vec<(usize, usize, String)> {
    let mut spans = Vec::new();
    for (event, range) in Parser::new(markdown).into_offset_iter() {
        let Event::Start(Tag::Image { dest_url, .. }) = event else {
            continue;
        };
        if !is_local_image(&dest_url) {
            continue;
        }
        if let Some(span) = markdown.get(range.clone())
            && let Some(offset) = span.rfind(dest_url.as_ref())
        {
            let start = range.start + offset;
            spans.push((start, start + dest_url.len(), dest_url.to_string()));
        }
    }
    spans
}

/// Convert the authored blank map into a [`FillInBlank`] payload.
///
/// Blanks are stored sorted by name (the [`BTreeMap`] order); presentation
/// order is recovered from the prompt markers by the exporters.
fn fill_in_blank(blanks: BTreeMap<String, BlankSpec>) -> FillInBlank {
    let blanks = blanks
        .into_iter()
        .map(|(id, spec)| spec.into_blank(id))
        .collect();
    FillInBlank { blanks }
}

/// Parse the full text of one question source into a [`Question`].
///
/// Supports inline content only; any `file:` partial is rejected. Use
/// [`parse_question_with`] to resolve partials against a reader.
///
/// # Errors
///
/// Returns [`crate::Error::Parse`] when the YAML front-matter is missing,
/// [`crate::Error::Yaml`] when it is malformed, and
/// [`crate::Error::InvalidQuestion`] when the prompt is empty or a `file:`
/// partial is used.
pub fn parse_question(source: &str) -> Result<Question> {
    parse_question_with(source, &reject_includes)
}

/// Parse one question source, resolving `file:` partials via `read`.
///
/// # Errors
///
/// As [`parse_question`], plus any partial-resolution failure surfaced through
/// `read` (see [`resolve_content`]).
pub fn parse_question_with(source: &str, read: &PartialReader<'_>) -> Result<Question> {
    let (yaml, prompt) = extract_yaml_and_prompt(source)?;
    let spec: QuestionSpec = serde_norway::from_str(&yaml)?;
    let mut question = spec.into_question(prompt, read)?;
    // A leading `# H1` names the question, but only as a shorthand: an explicit
    // `title:` field wins and leaves the body untouched.
    if question.title.is_none() {
        let (title, body) = split_leading_title(&question.prompt);
        question.title = title;
        question.prompt = body;
    }
    if question.prompt.is_empty() {
        return Err(crate::Error::InvalidQuestion(format!(
            "question {:?} has an empty prompt",
            question.id
        )));
    }
    validate_question(&question)?;
    Ok(question)
}

/// Check a parsed question against its kind's semantic rules.
///
/// # Errors
///
/// Returns [`crate::Error::InvalidQuestion`] when a question is structurally
/// valid but semantically incomplete (e.g. a multiple-choice item without
/// exactly one correct choice).
fn validate_question(question: &Question) -> Result<()> {
    match &question.kind {
        QuestionKind::MultipleChoice(set) => validate_choices(&question.id, &set.choices, false),
        QuestionKind::MultipleSelect(set) => validate_choices(&question.id, &set.choices, true),
        QuestionKind::FillInBlank(fitb) => {
            validate_blanks(&question.id, &question.prompt, &fitb.blanks)
        }
        QuestionKind::Matching(matching) => validate_matching(&question.id, matching),
        QuestionKind::Ordering(ordering) => validate_ordering(&question.id, ordering),
        _ => Ok(()),
    }
}

/// Validate a matching question: at least two pairs, each with non-empty text.
///
/// # Errors
///
/// Returns [`crate::Error::InvalidQuestion`] naming `id` when the rules fail.
fn validate_matching(id: &str, matching: &Matching) -> Result<()> {
    if matching.pairs.len() < 2 {
        return Err(crate::Error::InvalidQuestion(format!(
            "question {id:?} needs at least two matching pairs"
        )));
    }
    for pair in &matching.pairs {
        if pair.left.trim().is_empty() || pair.right.trim().is_empty() {
            return Err(crate::Error::InvalidQuestion(format!(
                "question {id:?} has a matching pair with an empty side"
            )));
        }
    }
    Ok(())
}

/// Validate an ordering question: at least two items, each with non-empty text.
///
/// # Errors
///
/// Returns [`crate::Error::InvalidQuestion`] naming `id` when the rules fail.
fn validate_ordering(id: &str, ordering: &Ordering) -> Result<()> {
    if ordering.items.len() < 2 {
        return Err(crate::Error::InvalidQuestion(format!(
            "question {id:?} needs at least two items to order"
        )));
    }
    if ordering.items.iter().any(|item| item.trim().is_empty()) {
        return Err(crate::Error::InvalidQuestion(format!(
            "question {id:?} has an empty ordering item"
        )));
    }
    Ok(())
}

/// Validate a fill-in-the-blank question: markers and blank definitions must
/// agree, there must be at least one blank, and each must have an answer.
///
/// # Errors
///
/// Returns [`crate::Error::InvalidQuestion`] naming `id` when the rules fail.
fn validate_blanks(id: &str, prompt: &str, blanks: &[Blank]) -> Result<()> {
    if blanks.is_empty() {
        return Err(crate::Error::InvalidQuestion(format!(
            "question {id:?} has no blanks; mark one inline with {{{{name}}}}"
        )));
    }
    let defined: HashSet<&str> = blanks.iter().map(|blank| blank.id.as_str()).collect();
    let marked: HashSet<String> = blank_markers(prompt).into_iter().collect();
    for blank in blanks {
        if !marked.contains(&blank.id) {
            return Err(crate::Error::InvalidQuestion(format!(
                "question {id:?} defines blank {:?} that never appears in the prompt",
                blank.id
            )));
        }
        if blank.answers.is_empty() {
            return Err(crate::Error::InvalidQuestion(format!(
                "question {id:?} blank {:?} has no answers",
                blank.id
            )));
        }
    }
    for name in &marked {
        if !defined.contains(name.as_str()) {
            return Err(crate::Error::InvalidQuestion(format!(
                "question {id:?} uses blank {name:?} with no answers defined"
            )));
        }
    }
    Ok(())
}

/// Validate a choice list: at least two options and the right number correct.
///
/// With `allow_multiple`, one or more choices may be correct; otherwise exactly
/// one must be. Shared by multiple-choice and multiple-select.
///
/// # Errors
///
/// Returns [`crate::Error::InvalidQuestion`] naming `id` when the rules fail.
fn validate_choices(id: &str, choices: &[Choice], allow_multiple: bool) -> Result<()> {
    if choices.len() < 2 {
        return Err(crate::Error::InvalidQuestion(format!(
            "question {id:?} needs at least two choices"
        )));
    }
    let correct = choices.iter().filter(|choice| choice.correct).count();
    let ok = if allow_multiple {
        correct >= 1
    } else {
        correct == 1
    };
    if ok {
        return Ok(());
    }
    let need = if allow_multiple {
        "at least one correct choice"
    } else {
        "exactly one correct choice"
    };
    Err(crate::Error::InvalidQuestion(format!(
        "question {id:?} needs {need}, found {correct}"
    )))
}

/// Split a leading level-1 ATX heading (`# ...`) off the front of `prompt`.
///
/// Returns the heading text as the title and the remaining prompt. With no
/// leading `# ` heading, the title is `None` and the prompt is unchanged.
fn split_leading_title(prompt: &str) -> (Option<String>, String) {
    let (first_line, rest) = prompt.split_once('\n').unwrap_or((prompt, ""));
    let Some(heading) = first_line.strip_prefix("# ") else {
        return (None, prompt.to_owned());
    };
    let title = heading.trim().trim_end_matches('#').trim();
    if title.is_empty() {
        return (None, prompt.to_owned());
    }
    (Some(title.to_owned()), rest.trim_start().to_owned())
}

/// Accumulator that locates the YAML front-matter block while scanning events.
#[derive(Default)]
struct YamlScan {
    /// The front-matter's inner text, once its closing `---` is seen.
    yaml: Option<String>,
    /// The byte span of the whole front-matter block, for removal from prompt.
    span: Option<Range<usize>>,
    /// Text accumulated inside the currently open front-matter block.
    buf: String,
    /// Start offset of the open front-matter block, or `None` when outside it.
    open: Option<usize>,
}

impl YamlScan {
    /// Fold one Markdown event (with its byte range) into the scan.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Parse`] if a second front-matter block is seen.
    fn feed(&mut self, event: &Event, range: &Range<usize>) -> Result<()> {
        match event {
            Event::Start(Tag::MetadataBlock(MetadataBlockKind::YamlStyle)) => {
                if self.yaml.is_some() {
                    return Err(crate::Error::Parse(
                        "more than one YAML front-matter block in a question".to_owned(),
                    ));
                }
                self.buf.clear();
                self.open = Some(range.start);
            }
            Event::Text(text) if self.open.is_some() => self.buf.push_str(text),
            Event::End(TagEnd::MetadataBlock(_)) => {
                if let Some(start) = self.open.take() {
                    self.span = Some(start..range.end);
                    self.yaml = Some(std::mem::take(&mut self.buf));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Split `source` into its YAML front-matter and the Markdown prompt below it.
///
/// # Errors
///
/// Returns [`crate::Error::Parse`] if there is no `---` front-matter block, if
/// more than one is present, or if the block span is not on char boundaries.
fn extract_yaml_and_prompt(source: &str) -> Result<(String, String)> {
    let mut scan = YamlScan::default();
    let parser = Parser::new_ext(source, Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    for (event, range) in parser.into_offset_iter() {
        scan.feed(&event, &range)?;
    }
    let yaml = scan.yaml.ok_or_else(|| {
        crate::Error::Parse("no YAML front-matter (--- block) found in question".to_owned())
    })?;
    let prompt = remove_span(source, scan.span.unwrap_or(0..0))?;
    Ok((yaml, prompt.trim().to_owned()))
}

/// Return `source` with the byte range `span` removed.
///
/// # Errors
///
/// Returns [`crate::Error::Parse`] if `span` does not fall on char boundaries.
fn remove_span(source: &str, span: Range<usize>) -> Result<String> {
    let before = source
        .get(..span.start)
        .ok_or_else(|| crate::Error::Parse("YAML block span is not a char boundary".to_owned()))?;
    let after = source
        .get(span.end..)
        .ok_or_else(|| crate::Error::Parse("YAML block span is not a char boundary".to_owned()))?;
    Ok(format!("{before}{after}"))
}

/// Assemble an [`ItemBank`] from `(filename, content)` source pairs.
///
/// Supports inline content only; use [`item_bank_from_sources_with`] to resolve
/// `file:` partials against a reader.
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
    item_bank_from_sources_with(name, sources, &reject_includes)
}

/// Assemble an [`ItemBank`], resolving `file:` partials via `read`.
///
/// Sources are ordered by filename for a deterministic bank layout, then each is
/// parsed via [`parse_question_with`]. This is the seam the CLI wires its
/// directory walk onto (supplying a reader rooted at the bank directory), so the
/// assembly stays testable without a process.
///
/// # Errors
///
/// Propagates any [`parse_question_with`] failure, and returns
/// [`crate::Error::InvalidQuestion`] if two questions share an `id`.
pub fn item_bank_from_sources_with<N, I>(
    name: N,
    sources: I,
    read: &PartialReader<'_>,
) -> Result<ItemBank>
where
    N: Into<String>,
    I: IntoIterator<Item = (String, String)>,
{
    let mut sources: Vec<(String, String)> = sources.into_iter().collect();
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    let mut items = Vec::with_capacity(sources.len());
    for (filename, content) in &sources {
        let question =
            parse_question_with(content, read).map_err(|error| crate::Error::QuestionFile {
                file: filename.clone(),
                message: error.to_string(),
            })?;
        items.push(question);
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

    /// Build a minimal true/false question with the given `id` for tests.
    fn question(id: &str) -> Question {
        Question {
            id: id.to_owned(),
            title: None,
            prompt: "prompt".to_owned(),
            points: 1.0,
            tags: Vec::new(),
            feedback: Feedback::default(),
            kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
        }
    }

    /// A well-formed true/false source: YAML front-matter then the prompt.
    const TRUE_FALSE_SOURCE: &str = "---\nid: tf-binary-search\nkind: true_false\nanswer: true\n\
        ---\n\nBinary search needs a sorted array.\n";

    /// A well-formed single-answer multiple-choice source.
    const MULTIPLE_CHOICE_SOURCE: &str = "---\nid: mc-q\nkind: multiple_choice\nchoices:\n\
        \x20 - text: Right\n    correct: true\n  - text: Wrong\n  - text: Also wrong\n\
        ---\n\nWhich one?\n";

    #[test]
    /// A multiple-choice source captures the ordered choices and the answer.
    fn parses_multiple_choice() {
        let question = parse_question(MULTIPLE_CHOICE_SOURCE).expect("valid source");
        assert_eq!(question.id, "mc-q");
        assert_eq!(question.prompt, "Which one?");
        assert!(matches!(
            &question.kind,
            QuestionKind::MultipleChoice(mc)
                if mc.choices.len() == 3
                    && mc.choices.iter().filter(|choice| choice.correct).count() == 1
                    && mc.choices.first().is_some_and(|choice| choice.correct)
        ));
    }

    #[test]
    /// A multiple-choice question with no correct choice is rejected.
    fn multiple_choice_needs_a_correct_choice() {
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n  - text: A\n  - text: B\n\
            ---\n\nPick?\n";
        let err = parse_question(source).expect_err("no correct choice");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// A single-answer question with two correct choices is rejected.
    fn multiple_choice_rejects_two_correct() {
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - text: A\n    correct: true\n  - text: B\n    correct: true\n---\n\nPick?\n";
        let err = parse_question(source).expect_err("two correct choices");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// A multiple-choice question with fewer than two choices is rejected.
    fn multiple_choice_needs_two_choices() {
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n  - text: Only\n    correct: true\n\
            ---\n\nPick?\n";
        let err = parse_question(source).expect_err("one choice");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    /// A [`PartialReader`] backed by an in-memory `(path, contents)` table.
    fn map_reader(
        files: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> std::result::Result<String, String> {
        move |path| {
            files
                .iter()
                .find(|(name, _)| *name == path)
                .map(|(_, body)| (*body).to_owned())
                .ok_or_else(|| format!("no such partial {path}"))
        }
    }

    #[test]
    /// A `file:` choice takes its content from the named partial.
    fn choice_file_is_resolved_from_partial() {
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: partials/right.md\n    correct: true\n  - text: Wrong\n---\n\nPick?\n";
        let reader = map_reader(&[("partials/right.md", "The **right** answer")]);
        let question = parse_question_with(source, &reader).expect("valid");
        assert!(matches!(
            &question.kind,
            QuestionKind::MultipleChoice(mc)
                if mc.choices.first().is_some_and(|c| c.text == "The **right** answer" && c.correct)
        ));
    }

    #[test]
    /// A partial's local image is rebased to a bank-relative path; externals stay.
    fn choice_partial_rebases_local_images() {
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: partials/a.md\n    correct: true\n  - text: No\n---\n\nPick?\n";
        let reader = map_reader(&[(
            "partials/a.md",
            "![t](pic.png) and ![x](https://ex.com/y.png)",
        )]);
        let question = parse_question_with(source, &reader).expect("valid");
        assert!(matches!(
            &question.kind,
            QuestionKind::MultipleChoice(mc) if mc.choices.first().is_some_and(|c|
                c.text.contains("![t](partials/pic.png)") && c.text.contains("https://ex.com/y.png"))
        ));
    }

    #[test]
    /// A `file:` choice whose partial cannot be read is a typed error naming it.
    fn choice_file_missing_errors() {
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: nope.md\n    correct: true\n  - text: No\n---\n\nPick?\n";
        let reader = map_reader(&[]);
        let err = parse_question_with(source, &reader).expect_err("missing partial");
        assert!(
            matches!(err, crate::Error::InvalidQuestion(message) if message.contains("nope.md"))
        );
    }

    #[test]
    /// A choice with both `text` and `file`, or neither, is rejected.
    fn choice_text_file_are_exclusive() {
        let both = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - text: A\n    file: a.md\n    correct: true\n  - text: B\n---\n\nPick?\n";
        let neither = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - correct: true\n  - text: B\n---\n\nPick?\n";
        let reader = map_reader(&[("a.md", "A")]);
        assert!(matches!(
            parse_question_with(both, &reader).expect_err("both"),
            crate::Error::InvalidQuestion(_)
        ));
        assert!(matches!(
            parse_question_with(neither, &reader).expect_err("neither"),
            crate::Error::InvalidQuestion(_)
        ));
    }

    #[test]
    /// The disk-free `parse_question` rejects a `file:` include with a clear error.
    fn parse_question_rejects_file_include() {
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: a.md\n    correct: true\n  - text: B\n---\n\nPick?\n";
        let err = parse_question(source).expect_err("no reader");
        assert!(
            matches!(err, crate::Error::InvalidQuestion(message) if message.contains("directory context"))
        );
    }

    #[test]
    /// `rebase_images` prefixes only local URLs, and only when there is a base.
    fn rebase_images_prefixes_local_only() {
        let md = "![a](one.png) ![b](sub/two.png) ![c](https://x/y.png)";
        let rebased = rebase_images(md, "partials");
        assert!(rebased.contains("![a](partials/one.png)"));
        assert!(rebased.contains("![b](partials/sub/two.png)"));
        assert!(rebased.contains("https://x/y.png"));
        // An empty base is a no-op.
        assert_eq!(rebase_images(md, ""), md);
    }

    #[test]
    /// A `file:` choice resolves and rebases through directory assembly too.
    fn item_bank_with_reader_resolves_file_choice() {
        let src = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: partials/a.md\n    correct: true\n  - text: No\n---\n\nPick?\n";
        let reader = map_reader(&[("partials/a.md", "![t](pic.png)")]);
        let bank = item_bank_from_sources_with("m", [("q.md".to_owned(), src.to_owned())], &reader)
            .expect("bank");
        assert!(matches!(
            bank.items.first().map(|q| &q.kind),
            Some(QuestionKind::MultipleChoice(mc))
                if mc.choices.first().is_some_and(|c| c.text.contains("![t](partials/pic.png)"))
        ));
    }

    #[test]
    /// A bank-root partial (no directory) leaves its local image path unchanged.
    fn root_partial_leaves_image_unrebased() {
        let src = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: a.md\n    correct: true\n  - text: No\n---\n\nPick?\n";
        let reader = map_reader(&[("a.md", "![t](pic.png)")]);
        let question = parse_question_with(src, &reader).expect("valid");
        assert!(matches!(
            &question.kind,
            QuestionKind::MultipleChoice(mc) if mc.choices.first().is_some_and(|c|
                c.text.contains("![t](pic.png)") && !c.text.contains("/pic.png"))
        ));
    }

    #[test]
    /// A `file:` choice works for multiple-select too (both paths, per policy).
    fn multiple_select_supports_file_choice() {
        let src = "---\nid: q\nkind: multiple_select\nchoices:\n\
            \x20 - file: partials/a.md\n    correct: true\n  - text: No\n---\n\nSelect all.\n";
        let reader = map_reader(&[("partials/a.md", "Answer A")]);
        let question = parse_question_with(src, &reader).expect("valid");
        assert!(matches!(
            &question.kind,
            QuestionKind::MultipleSelect(ms)
                if ms.choices.first().is_some_and(|c| c.text == "Answer A")
        ));
    }

    #[test]
    /// A multiple-select source allows more than one correct choice.
    fn parses_multiple_select_with_two_correct() {
        let source = "---\nid: ms-q\nkind: multiple_select\nchoices:\n\
            \x20 - text: A\n    correct: true\n  - text: B\n    correct: true\n  - text: C\n\
            ---\n\nSelect all.\n";
        let question = parse_question(source).expect("valid source");
        assert!(matches!(
            &question.kind,
            QuestionKind::MultipleSelect(select)
                if select.choices.iter().filter(|choice| choice.correct).count() == 2
                    && select.scoring == ScoringMode::AllOrNothing
        ));
    }

    #[test]
    /// An explicit `scoring: partial` is captured; it otherwise defaults.
    fn parses_multiple_select_partial_scoring() {
        let source = "---\nid: q\nkind: multiple_select\nscoring: partial\nchoices:\n\
            \x20 - text: A\n    correct: true\n  - text: B\n---\n\nSelect all.\n";
        let question = parse_question(source).expect("valid source");
        assert!(matches!(
            &question.kind,
            QuestionKind::MultipleSelect(select) if select.scoring == ScoringMode::Partial
        ));
    }

    /// A matching source with two pairs and one distractor.
    const MATCHING_SOURCE: &str = "---\nid: mt\nkind: matching\npairs:\n\
        \x20 - left: char\n    right: 1 byte\n  - left: int\n    right: 4 bytes\n\
        distractors: [8 bytes]\n---\n\nMatch each type to its size.\n";

    #[test]
    /// A matching source captures ordered pairs and distractors.
    fn parses_matching() {
        let question = parse_question(MATCHING_SOURCE).expect("valid source");
        assert!(matches!(
            &question.kind,
            QuestionKind::Matching(matching)
                if matching.pairs.len() == 2
                    && matching.distractors == ["8 bytes"]
                    && matching.pairs.first().is_some_and(|p| p.left == "char" && p.right == "1 byte")
        ));
    }

    #[test]
    /// A matching question with fewer than two pairs is rejected.
    fn matching_needs_two_pairs() {
        let source = "---\nid: q\nkind: matching\npairs:\n  - left: a\n    right: b\n\
            ---\n\nMatch.\n";
        let err = parse_question(source).expect_err("one pair");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// A matching pair with an empty side is rejected.
    fn matching_rejects_empty_side() {
        let source = "---\nid: q\nkind: matching\npairs:\n  - left: a\n    right: b\n\
            \x20 - left: c\n    right: \"\"\n---\n\nMatch.\n";
        let err = parse_question(source).expect_err("empty right");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// An ordering source captures its items in authored (correct) order.
    fn parses_ordering() {
        let source = "---\nid: ord\nkind: ordering\nitems:\n  - Compile\n  - Link\n  - Run\n\
            ---\n\nOrder the build phases.\n";
        let question = parse_question(source).expect("valid source");
        assert!(matches!(
            &question.kind,
            QuestionKind::Ordering(ordering)
                if ordering.items == ["Compile", "Link", "Run"]
        ));
    }

    #[test]
    /// Exactly two items is the accepted minimum (guards the `< 2` boundary).
    fn ordering_accepts_exactly_two_items() {
        let source = "---\nid: q\nkind: ordering\nitems:\n  - a\n  - b\n---\n\nOrder.\n";
        assert!(parse_question(source).is_ok());
    }

    #[test]
    /// An ordering question with fewer than two items is rejected.
    fn ordering_needs_two_items() {
        let source = "---\nid: q\nkind: ordering\nitems:\n  - only\n---\n\nOrder.\n";
        let err = parse_question(source).expect_err("one item");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// An ordering question with an empty item is rejected.
    fn ordering_rejects_empty_item() {
        let source = "---\nid: q\nkind: ordering\nitems:\n  - a\n  - \"\"\n---\n\nOrder.\n";
        let err = parse_question(source).expect_err("empty item");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// A multiple-select question with no correct choice is rejected.
    fn multiple_select_needs_a_correct_choice() {
        let source = "---\nid: q\nkind: multiple_select\nchoices:\n  - text: A\n  - text: B\n\
            ---\n\nSelect all.\n";
        let err = parse_question(source).expect_err("no correct choice");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    /// A fill-in-the-blank source: a shorthand list blank and a detailed blank.
    const FILL_IN_BLANK_SOURCE: &str = "---\nid: fitb-http\nkind: fill_in_blank\nblanks:\n\
        \x20 method: [GET, get]\n  code:\n    answers: [\"404\", \"Not Found\"]\n    match: exact\n\
        ---\n\nAn HTTP {{method}} request that fails returns status {{code}}.\n";

    #[test]
    /// A fill-in-the-blank source captures each blank's answers and match mode.
    fn parses_fill_in_blank() {
        let question = parse_question(FILL_IN_BLANK_SOURCE).expect("valid source");
        assert!(matches!(&question.kind, QuestionKind::FillInBlank(_)));
        if let QuestionKind::FillInBlank(fitb) = &question.kind {
            assert_eq!(fitb.blanks.len(), 2);
            let code = fitb
                .blanks
                .iter()
                .find(|b| b.id == "code")
                .expect("code blank");
            assert_eq!(code.answers, ["404", "Not Found"]);
            assert_eq!(code.match_mode, MatchMode::Exact);
            let method = fitb
                .blanks
                .iter()
                .find(|b| b.id == "method")
                .expect("method blank");
            assert_eq!(method.answers, ["GET", "get"]);
            assert_eq!(method.match_mode, MatchMode::CaseInsensitive);
        }
    }

    #[test]
    /// A detailed blank with no `match:` key defaults to case-insensitive.
    fn detailed_blank_defaults_to_case_insensitive() {
        let source = "---\nid: q\nkind: fill_in_blank\nblanks:\n  a:\n    answers: [x]\n\
            ---\n\nHas {{a}}.\n";
        let question = parse_question(source).expect("valid source");
        assert!(matches!(
            &question.kind,
            QuestionKind::FillInBlank(fitb)
                if fitb.blanks.first().is_some_and(|b| b.match_mode == MatchMode::CaseInsensitive)
        ));
    }

    #[test]
    /// A `match: regex` blank parses to the regex match mode.
    fn parses_regex_blank() {
        let source = "---\nid: q\nkind: fill_in_blank\nblanks:\n  n:\n    answers: ['\\\\d{3}']\n\
            \x20   match: regex\n---\n\nEnter a 3-digit number: {{n}}.\n";
        let question = parse_question(source).expect("valid source");
        assert!(matches!(
            &question.kind,
            QuestionKind::FillInBlank(fitb)
                if fitb.blanks.first().is_some_and(|b| b.match_mode == MatchMode::Regex)
        ));
    }

    #[test]
    /// `blank_markers` returns marker names in order, de-duplicated.
    fn blank_markers_are_ordered_and_deduped() {
        assert_eq!(
            blank_markers("a {{one}} b {{two}} c {{one}}"),
            ["one", "two"]
        );
        assert!(blank_markers("no markers here").is_empty());
        // Unclosed marker and empty/whitespace markers yield nothing.
        assert!(blank_markers("a {{open").is_empty());
        assert!(blank_markers("x {{}} y").is_empty());
        assert!(blank_markers("x {{   }} y").is_empty());
    }

    #[test]
    /// A fill-in-the-blank question with no blanks at all is rejected.
    fn fill_in_blank_with_no_blanks_is_rejected() {
        let source = "---\nid: q\nkind: fill_in_blank\nblanks: {}\n---\n\nNothing to fill.\n";
        let err = parse_question(source).expect_err("no blanks");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// A prompt marker with no defined answers is rejected.
    fn fill_in_blank_marker_without_answers_is_rejected() {
        let source =
            "---\nid: q\nkind: fill_in_blank\nblanks:\n  a: [x]\n---\n\n{{a}} and {{b}}.\n";
        let err = parse_question(source).expect_err("marker b undefined");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// A defined blank that never appears in the prompt is rejected.
    fn fill_in_blank_orphan_blank_is_rejected() {
        let source =
            "---\nid: q\nkind: fill_in_blank\nblanks:\n  a: [x]\n  b: [y]\n---\n\n{{a}} only.\n";
        let err = parse_question(source).expect_err("blank b never used");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// A blank with an empty answer list is rejected.
    fn fill_in_blank_empty_answers_is_rejected() {
        let source = "---\nid: q\nkind: fill_in_blank\nblanks:\n  a: []\n---\n\nHas {{a}}.\n";
        let err = parse_question(source).expect_err("empty answers");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// A true/false source yields the prompt, id, default points, and answer.
    fn parses_true_false_question() {
        let question = parse_question(TRUE_FALSE_SOURCE).expect("valid source");
        assert_eq!(question.id, "tf-binary-search");
        assert_eq!(question.prompt, "Binary search needs a sorted array.");
        assert!((question.points - 1.0).abs() < f64::EPSILON);
        assert_eq!(question.title, None);
        assert!(question.tags.is_empty());
        assert!(matches!(
            question.kind,
            QuestionKind::TrueFalse(TrueFalse { answer: true })
        ));
    }

    #[test]
    /// Optional `title` and `tags` are captured when present.
    fn parses_title_and_tags() {
        let source = "---\nid: q\ntitle: Binary search\nkind: true_false\n\
            tags: [searching, algorithms]\nanswer: true\n---\n\nProm?\n";
        let question = parse_question(source).expect("valid source");
        assert_eq!(question.title.as_deref(), Some("Binary search"));
        assert_eq!(question.tags, ["searching", "algorithms"]);
    }

    #[test]
    /// A leading `# H1` in the body becomes the title and is stripped out.
    fn leading_h1_becomes_title() {
        let source = "---\nid: q\nkind: true_false\nanswer: true\n---\n\n\
            # Binary search prerequisites\n\nRequires a sorted array.\n";
        let question = parse_question(source).expect("valid source");
        assert_eq!(
            question.title.as_deref(),
            Some("Binary search prerequisites")
        );
        assert_eq!(question.prompt, "Requires a sorted array.");
    }

    #[test]
    /// An explicit `title:` field wins; a body `# H1` is left as prompt content.
    fn explicit_title_beats_leading_h1() {
        let source = "---\nid: q\ntitle: From field\nkind: true_false\nanswer: true\n---\n\n\
            # A heading\n\nBody.\n";
        let question = parse_question(source).expect("valid source");
        assert_eq!(question.title.as_deref(), Some("From field"));
        assert!(question.prompt.contains("# A heading"));
    }

    #[test]
    /// A body that is not a heading leaves the title unset and prompt intact.
    fn non_heading_body_has_no_title() {
        let question = parse_question(TRUE_FALSE_SOURCE).expect("valid source");
        assert_eq!(question.title, None);
        assert_eq!(question.prompt, "Binary search needs a sorted array.");
    }

    #[test]
    /// An empty `# ` heading is not treated as a title; the line stays in place.
    fn empty_heading_is_not_a_title() {
        let source = "---\nid: q\nkind: true_false\nanswer: true\n---\n\n#  \n\nBody.\n";
        let question = parse_question(source).expect("valid source");
        assert_eq!(question.title, None);
        assert!(question.prompt.starts_with('#'));
    }

    #[test]
    /// A trailing `#` ATX closing sequence is trimmed from the extracted title.
    fn heading_closing_hashes_are_trimmed() {
        let source = "---\nid: q\nkind: true_false\nanswer: true\n---\n\n# Title ##\n\nBody.\n";
        let question = parse_question(source).expect("valid source");
        assert_eq!(question.title.as_deref(), Some("Title"));
        assert_eq!(question.prompt, "Body.");
    }

    #[test]
    /// Optional feedback messages are captured when present.
    fn parses_feedback() {
        let source = "---\nid: q\nkind: true_false\nanswer: true\n\
            feedback:\n  correct: Nice work.\n  incorrect: Not quite.\n---\n\nProm?\n";
        let question = parse_question(source).expect("valid source");
        assert_eq!(question.feedback.correct.as_deref(), Some("Nice work."));
        assert_eq!(question.feedback.incorrect.as_deref(), Some("Not quite."));
        assert_eq!(question.feedback.general, None);
    }

    #[test]
    /// Only general feedback is captured when it is the sole message set.
    fn parses_general_only_feedback() {
        let source = "---\nid: q\nkind: true_false\nanswer: true\n\
            feedback:\n  general: See chapter 3.\n---\n\nQ?\n";
        let question = parse_question(source).expect("valid source");
        assert_eq!(question.feedback.general.as_deref(), Some("See chapter 3."));
        assert_eq!(question.feedback.correct, None);
        assert_eq!(question.feedback.incorrect, None);
    }

    #[test]
    /// An ordering item can come from a `file:` partial, with images rebased.
    fn ordering_item_from_file() {
        let source = "---\nid: q\nkind: ordering\nitems:\n\
            \x20 - Inline first\n  - file: steps/two.md\n---\n\nOrder.\n";
        let reader = map_reader(&[("steps/two.md", "![d](d.png)")]);
        let question = parse_question_with(source, &reader).expect("valid");
        assert!(matches!(
            &question.kind,
            QuestionKind::Ordering(o)
                if o.items.first().is_some_and(|i| i == "Inline first")
                    && o.items.get(1).is_some_and(|i| i.contains("![d](steps/d.png)"))
        ));
    }

    #[test]
    /// A feedback message can come from a `file:` partial.
    fn feedback_message_from_file() {
        let source = "---\nid: q\nkind: true_false\nanswer: true\n\
            feedback:\n  general:\n    file: fb/hint.md\n---\n\nA fact.\n";
        let reader = map_reader(&[("fb/hint.md", "The **hint**.")]);
        let question = parse_question_with(source, &reader).expect("valid");
        assert_eq!(question.feedback.general.as_deref(), Some("The **hint**."));
    }

    #[test]
    /// Each feedback field is wired to its own partial (guards field swaps).
    fn feedback_correct_and_incorrect_from_files() {
        let source = "---\nid: q\nkind: true_false\nanswer: true\nfeedback:\n\
            \x20 correct:\n    file: fb/c.md\n  incorrect:\n    file: fb/i.md\n---\n\nA fact.\n";
        let reader = map_reader(&[("fb/c.md", "Right!"), ("fb/i.md", "Wrong.")]);
        let question = parse_question_with(source, &reader).expect("valid");
        assert_eq!(question.feedback.correct.as_deref(), Some("Right!"));
        assert_eq!(question.feedback.incorrect.as_deref(), Some("Wrong."));
    }

    #[test]
    /// A missing partial in an ordering item is a typed error naming it.
    fn ordering_item_file_missing_errors() {
        let source = "---\nid: q\nkind: ordering\nitems:\n\
            \x20 - First\n  - file: steps/missing.md\n---\n\nOrder.\n";
        let reader = map_reader(&[]);
        let err = parse_question_with(source, &reader).expect_err("missing partial");
        assert!(
            matches!(err, crate::Error::InvalidQuestion(message) if message.contains("missing.md"))
        );
    }

    #[test]
    /// The disk-free entry point rejects a `file:` feedback message too.
    fn parse_question_rejects_feedback_file() {
        let source = "---\nid: q\nkind: true_false\nanswer: true\n\
            feedback:\n  general:\n    file: fb.md\n---\n\nA fact.\n";
        let err = parse_question(source).expect_err("no reader");
        assert!(
            matches!(err, crate::Error::InvalidQuestion(message) if message.contains("directory context"))
        );
    }

    #[test]
    /// Title and tags survive assembly through `item_bank_from_sources`.
    fn item_bank_preserves_title_and_tags() {
        let source =
            "---\nid: q\ntitle: T\nkind: true_false\ntags: [x, y]\nanswer: true\n---\n\nQ?\n";
        let bank = item_bank_from_sources("m", [("q.md".to_owned(), source.to_owned())])
            .expect("valid source");
        let question = bank.items.first().expect("one question");
        assert_eq!(question.title.as_deref(), Some("T"));
        assert_eq!(question.tags, ["x", "y"]);
    }

    #[test]
    /// An omitted feedback block leaves every message empty.
    fn feedback_defaults_to_empty() {
        let question = parse_question(TRUE_FALSE_SOURCE).expect("valid source");
        assert_eq!(question.feedback.general, None);
        assert_eq!(question.feedback.correct, None);
        assert_eq!(question.feedback.incorrect, None);
    }

    #[test]
    /// An explicit `points` value overrides the default.
    fn honours_explicit_points() {
        let source = "---\nid: q\nkind: true_false\npoints: 2.5\nanswer: false\n---\n\nQ?\n";
        let question = parse_question(source).expect("valid source");
        assert!((question.points - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    /// A source with no front-matter block is a parse error, not a panic.
    fn missing_front_matter_is_parse_error() {
        let err = parse_question("Just a prompt, no YAML.").expect_err("no front-matter");
        assert!(matches!(err, crate::Error::Parse(_)));
    }

    #[test]
    /// Malformed front-matter YAML surfaces as a YAML error.
    fn malformed_yaml_is_yaml_error() {
        let source = "---\nid: [unterminated\n---\n\nQ?\n";
        let err = parse_question(source).expect_err("malformed yaml");
        assert!(matches!(err, crate::Error::Yaml(_)));
    }

    #[test]
    /// A question with front-matter but no prompt body is rejected.
    fn empty_prompt_is_invalid() {
        let source = "---\nid: q\nkind: true_false\nanswer: true\n---\n";
        let err = parse_question(source).expect_err("empty prompt");
        assert!(matches!(err, crate::Error::InvalidQuestion(_)));
    }

    #[test]
    /// A code fence in the body stays in the prompt; only front-matter is cut.
    fn body_code_fence_is_preserved_in_prompt() {
        let source = "---\nid: q\nkind: true_false\nanswer: true\n---\n\n\
            Consider:\n\n```rust\nfn main() {}\n```\n";
        let question = parse_question(source).expect("valid source");
        assert!(question.prompt.contains("```rust"));
        assert!(question.prompt.contains("fn main() {}"));
        assert!(!question.prompt.contains("kind: true_false"));
    }

    #[test]
    /// A failing file in a bank surfaces its filename for the author.
    fn source_error_names_the_file() {
        let sources = [("module01/q3.md".to_owned(), "no yaml here".to_owned())];
        let err = item_bank_from_sources("m", sources).expect_err("q3 is malformed");
        let message = err.to_string();
        assert!(matches!(err, crate::Error::QuestionFile { .. }));
        assert!(message.contains("module01/q3.md"));
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
    /// Sources are assembled in filename order, regardless of input order.
    fn sources_are_ordered_by_filename() {
        let second = "---\nid: b\nkind: true_false\nanswer: false\n---\n\nSecond?\n";
        let first = "---\nid: a\nkind: true_false\nanswer: true\n---\n\nFirst?\n";
        let sources = [
            ("q2.md".to_owned(), second.to_owned()),
            ("q1.md".to_owned(), first.to_owned()),
        ];
        let bank = item_bank_from_sources("m", sources).expect("valid sources");
        let ids: Vec<&str> = bank.items.iter().map(|q| q.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    /// An empty directory (no sources) assembles without touching the parser.
    fn empty_sources_assemble() {
        let bank = item_bank_from_sources("m", std::iter::empty()).expect("no sources is valid");
        assert!(bank.items.is_empty());
    }
}
