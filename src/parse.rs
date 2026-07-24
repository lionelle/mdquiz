//! Parse Markdown + YAML question sources into the typed model.
//!
//! Each source file is one question. YAML front-matter (a `---`-delimited block
//! at the top of the file) carries the machine-readable answer data; everything
//! below it is the prompt as ordinary Markdown. Parsing splits the two: the
//! front-matter is deserialized into a [`QuestionSpec`] (a flat, `kind`-tagged
//! shape), and the body becomes the prompt. A directory of such files is
//! assembled into one [`ItemBank`].

use std::collections::HashSet;
use std::ops::Range;

use pulldown_cmark::{Event, MetadataBlockKind, Options, Parser, Tag, TagEnd};
use serde::Deserialize;

use crate::Result;
use crate::model::{Feedback, ItemBank, Question, QuestionKind, TrueFalse};

/// The authored YAML shape, tagged by `kind`, before the prompt is attached.
///
/// This is deliberately separate from [`QuestionKind`]: the prompt lives in the
/// Markdown body, not the YAML, so the two are joined in [`parse_question`].
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum QuestionSpec {
    /// A true/false question.
    TrueFalse {
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
        feedback: Feedback,
        /// Whether the correct answer is "true".
        answer: bool,
    },
}

/// The default point value when a question omits `points`.
fn default_points() -> f64 {
    1.0
}

impl QuestionSpec {
    /// Combine the YAML spec with its Markdown `prompt` into a [`Question`].
    fn into_question(self, prompt: String) -> Question {
        match self {
            Self::TrueFalse {
                id,
                title,
                points,
                tags,
                feedback,
                answer,
            } => Question {
                id,
                title,
                prompt,
                points,
                tags,
                feedback,
                kind: QuestionKind::TrueFalse(TrueFalse { answer }),
            },
        }
    }
}

/// Parse the full text of one question source into a [`Question`].
///
/// # Errors
///
/// Returns [`crate::Error::Parse`] when the YAML front-matter is missing,
/// [`crate::Error::Yaml`] when it is malformed, and
/// [`crate::Error::InvalidQuestion`] when the prompt is empty.
pub fn parse_question(source: &str) -> Result<Question> {
    let (yaml, prompt) = extract_yaml_and_prompt(source)?;
    let spec: QuestionSpec = serde_norway::from_str(&yaml)?;
    let mut question = spec.into_question(prompt);
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
    Ok(question)
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
    for (filename, content) in &sources {
        let question = parse_question(content).map_err(|error| crate::Error::QuestionFile {
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
