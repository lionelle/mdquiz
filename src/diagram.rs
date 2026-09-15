//! Render fenced diagram blocks (` ```mermaid `, ` ```dot `) into bundled images.
//!
//! Canvas renders neither mermaid nor Graphviz, so before a bank is exported
//! each diagram block is rendered to an image and the block replaced with a
//! local image reference (`generated/…`). Rendering itself is delegated to an
//! injected [`DiagramRenderer`] so this pass stays pure and testable; the CLI
//! supplies a real one by shelling out to the mermaid CLI (`mmdc`) or Graphviz
//! (`dot`). A diagram that fails to render (for example when the tool is not
//! installed) is **left as a code block** and reported as a warning — it never
//! fails the export.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

use crate::model::Question;

/// The reserved bundle directory for mdquiz-generated images.
///
/// Diagram images land here (`generated/<language>-<hash>.<ext>`); the CLI
/// supplies their bytes directly rather than reading them from the question
/// directory.
pub const GENERATED_DIR: &str = "generated";

/// Whether `path` is one of this module's generated-diagram outputs.
///
/// The CLI uses this to skip generated paths when reading images from disk.
#[must_use]
pub fn is_generated_path(path: &str) -> bool {
    path.split('/').next() == Some(GENERATED_DIR)
}

/// A diagram language recognised in a fenced code block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramLanguage {
    /// Mermaid (` ```mermaid `), rendered by the mermaid CLI.
    Mermaid,
    /// Graphviz DOT (` ```dot ` or ` ```graphviz `), rendered by `dot`.
    Graphviz,
}

impl DiagramLanguage {
    /// The language a fenced block's info string names, if it names one.
    ///
    /// Only the first word is considered, so trailing info
    /// (` ```mermaid theme=default `) still selects the language.
    #[must_use]
    pub(crate) fn from_info(info: &str) -> Option<Self> {
        match info.split_whitespace().next() {
            Some("mermaid") => Some(Self::Mermaid),
            Some("dot" | "graphviz") => Some(Self::Graphviz),
            _ => None,
        }
    }

    /// The language's canonical short name, used in generated filenames and
    /// warnings. It is the language, not the fence the author typed: a
    /// ` ```dot ` block is named `graphviz`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mermaid => "mermaid",
            Self::Graphviz => "graphviz",
        }
    }

    /// The file extension for this language's source, for callers that must
    /// hand their renderer a file rather than a string.
    ///
    /// It is load-bearing for mermaid: `mmdc` picks its input mode from the
    /// extension and treats a `.md` file as Markdown to extract diagrams *from*,
    /// so a bare diagram must not be written to one. `dot` ignores it.
    #[must_use]
    pub const fn source_extension(self) -> &'static str {
        match self {
            Self::Mermaid => "mmd",
            Self::Graphviz => "dot",
        }
    }
}

/// Renders a diagram `source` in some language to image bytes, or returns a
/// message on failure.
///
/// Injected by the caller (the CLI shells out to `mmdc` or `dot`) so this module
/// needs no external tool and stays unit-testable with a stub.
///
/// The bytes must be in the [`DiagramFormat`] passed to [`render_diagrams`]:
/// that format names the generated file's extension and is *not* passed here,
/// so a renderer that ignores it lands SVG bytes behind a `.png` path.
pub type DiagramRenderer<'a> =
    dyn Fn(DiagramLanguage, &str) -> std::result::Result<Vec<u8>, String> + 'a;

/// The image format diagrams render to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramFormat {
    /// Raster PNG — the most reliably rendered format inside Canvas.
    Png,
    /// Scalable SVG.
    Svg,
}

impl DiagramFormat {
    /// The file extension for this format.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Svg => "svg",
        }
    }
}

/// The result of a diagram pass: images to bundle, plus non-fatal warnings.
#[derive(Debug, Default)]
pub struct DiagramOutcome {
    /// Generated diagram images, keyed by their `generated/…` bundle path.
    pub images: Vec<(String, Vec<u8>)>,
    /// One message per diagram that could not be rendered (left as a code block).
    pub warnings: Vec<String>,
}

/// Replace every diagram block in `items` with a generated image reference.
///
/// Each distinct diagram is rendered once (deduplicated by language and
/// content); failures are collected as warnings and leave the block untouched.
/// Returns the images to bundle and any warnings.
///
/// Operates on a question slice, so any collection of questions renders: a
/// caller holding an [`ItemBank`](crate::model::ItemBank) passes
/// `&mut bank.items`.
#[must_use]
pub fn render_diagrams(
    items: &mut [Question],
    render: &DiagramRenderer<'_>,
    format: DiagramFormat,
) -> DiagramOutcome {
    let mut outcome = DiagramOutcome::default();
    let mut seen = HashSet::new();
    for question in items {
        let id = question.id.clone();
        for field in question.rich_text_fields_mut() {
            render_field(field, render, format, &id, &mut seen, &mut outcome);
        }
    }
    outcome
}

/// Render every diagram block in one `field`, splicing image references in place.
fn render_field(
    field: &mut String,
    render: &DiagramRenderer<'_>,
    format: DiagramFormat,
    id: &str,
    seen: &mut HashSet<String>,
    outcome: &mut DiagramOutcome,
) {
    // Collect spans first (immutable borrow ends), then splice back-to-front so
    // earlier byte offsets stay valid as text is replaced.
    for (span, language, source) in diagram_blocks(field).into_iter().rev() {
        let path = generated_path(language, &source, format);
        if seen.contains(&path) {
            field.replace_range(span, &image_reference(&path));
        } else if let Some(bytes) = render_one(render, language, &source, id, outcome) {
            seen.insert(path.clone());
            outcome.images.push((path.clone(), bytes));
            field.replace_range(span, &image_reference(&path));
        }
    }
}

/// Render a single diagram, recording a warning (and returning `None`) on failure.
fn render_one(
    render: &DiagramRenderer<'_>,
    language: DiagramLanguage,
    source: &str,
    id: &str,
    outcome: &mut DiagramOutcome,
) -> Option<Vec<u8>> {
    match render(language, source.trim()) {
        Ok(bytes) => Some(bytes),
        Err(message) => {
            outcome.warnings.push(format!(
                "question {id:?}: {} diagram left as code ({message})",
                language.name()
            ));
            None
        }
    }
}

/// The `(byte span, language, source)` of every fenced diagram block in `markdown`.
fn diagram_blocks(markdown: &str) -> Vec<(Range<usize>, DiagramLanguage, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(usize, DiagramLanguage, String)> = None;
    for (event, range) in Parser::new(markdown).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                current = DiagramLanguage::from_info(&info)
                    .map(|language| (range.start, language, String::new()));
            }
            Event::Text(text) => {
                if let Some((_, _, source)) = current.as_mut() {
                    source.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((start, language, source)) = current.take() {
                    blocks.push((start..range.end, language, source));
                }
            }
            _ => {}
        }
    }
    blocks
}

/// The bundle path for the diagram rendered from `source`, deduped by language
/// and content.
fn generated_path(language: DiagramLanguage, source: &str, format: DiagramFormat) -> String {
    format!(
        "{GENERATED_DIR}/{}-{}.{}",
        language.name(),
        short_hash(source.trim()),
        format.extension()
    )
}

/// A short, stable hex digest of `text` for a deduplicated filename.
fn short_hash(text: &str) -> String {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The Markdown image reference that replaces a rendered diagram block.
fn image_reference(path: &str) -> String {
    format!("![diagram]({path})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Feedback, ItemBank, Question, QuestionKind, TrueFalse};

    /// A true/false question `id` asking `prompt`.
    fn question(id: &str, prompt: &str) -> Question {
        Question {
            id: id.to_owned(),
            title: None,
            prompt: prompt.to_owned(),
            points: 1.0,
            tags: Vec::new(),
            feedback: Feedback::default(),
            kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
        }
    }

    /// A one-question bank whose prompt is `prompt`.
    fn bank(prompt: &str) -> ItemBank {
        ItemBank {
            name: "M".to_owned(),
            items: vec![question("q", prompt)],
        }
    }

    /// A renderer returning fixed bytes for any diagram.
    fn ok_renderer() -> impl Fn(DiagramLanguage, &str) -> std::result::Result<Vec<u8>, String> {
        |_language, _source| Ok(vec![1, 2, 3])
    }

    /// The first question's prompt.
    fn prompt(bank: &ItemBank) -> &str {
        bank.items.first().map_or("", |q| q.prompt.as_str())
    }

    #[test]
    /// A mermaid block becomes an image reference and yields one bundled image.
    fn renders_block_to_image_reference() {
        let mut b = bank("Before\n\n```mermaid\ngraph TD; A-->B;\n```\n\nAfter");
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert!(prompt(&b).contains("![diagram](generated/mermaid-"));
        assert!(prompt(&b).contains(".png)") && !prompt(&b).contains("```mermaid"));
        assert_eq!(outcome.images.len(), 1);
        assert!(outcome.warnings.is_empty());
        // The bundle path in the prompt matches the generated image path.
        let path = outcome.images.first().map_or("", |(p, _)| p.as_str());
        assert!(prompt(&b).contains(path));
    }

    #[test]
    /// A non-diagram fenced block is left untouched.
    fn leaves_other_code_blocks() {
        let mut b = bank("```rust\nfn main() {}\n```");
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert!(prompt(&b).contains("```rust"));
        assert!(outcome.images.is_empty());
    }

    #[test]
    /// The same diagram twice renders once and bundles a single image.
    fn deduplicates_identical_diagrams() {
        let mut b = bank("```mermaid\ngraph TD; A-->B;\n```\n\n```mermaid\ngraph TD; A-->B;\n```");
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert_eq!(outcome.images.len(), 1);
        assert_eq!(prompt(&b).matches("![diagram]").count(), 2);
    }

    #[test]
    /// A render failure warns (naming the language) and leaves the block as code.
    fn failure_warns_and_leaves_block() {
        let mut b = bank("```mermaid\nbad\n```");
        let render = |_language: DiagramLanguage, _source: &str| Err("mmdc not found".to_owned());
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert!(prompt(&b).contains("```mermaid"));
        assert!(outcome.images.is_empty());
        assert_eq!(outcome.warnings.len(), 1);
        let warning = outcome.warnings.first().map_or("", String::as_str);
        assert!(warning.contains("mmdc not found") && warning.contains("mermaid diagram"));
    }

    /// A one-question multiple-choice bank: first choice `choice`, `general`
    /// feedback, and a throwaway second choice.
    fn choice_bank(choice: &str, general: &str) -> ItemBank {
        use crate::model::{Choice, ChoiceSet};
        ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: "Pick one.".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback {
                    general: Some(general.to_owned()),
                    correct: None,
                    incorrect: None,
                },
                kind: QuestionKind::MultipleChoice(ChoiceSet {
                    choices: vec![
                        Choice {
                            text: choice.to_owned(),
                            correct: true,
                        },
                        Choice {
                            text: "No".to_owned(),
                            correct: false,
                        },
                    ],
                }),
            }],
        }
    }

    #[test]
    /// Diagrams render in a choice and in feedback (the non-prompt mutable walk);
    /// two distinct diagrams yield two images.
    fn renders_in_choice_and_feedback() {
        let mut b = choice_bank(
            "```mermaid\ngraph TD; A-->B;\n```",
            "Recall:\n\n```mermaid\ngraph TD; X-->Y;\n```",
        );
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        let question = b.items.first().expect("question");
        assert!(matches!(
            &question.kind,
            QuestionKind::MultipleChoice(set) if set.choices.first().is_some_and(|c|
                c.text.contains("![diagram](generated/mermaid-") && !c.text.contains("```mermaid"))
        ));
        assert!(
            question
                .feedback
                .general
                .as_deref()
                .is_some_and(|g| g.contains("![diagram](generated/mermaid-"))
        );
        assert_eq!(outcome.images.len(), 2);
    }

    #[test]
    /// A mermaid block delivered through a `file:` partial renders — partials are
    /// resolved (in parsing) before the diagram pass walks the bank.
    fn mermaid_from_partial_renders() {
        let reader = |path: &str| -> std::result::Result<String, String> {
            if path == "p/d.md" {
                Ok("```mermaid\ngraph TD; A-->B;\n```".to_owned())
            } else {
                Err(format!("no {path}"))
            }
        };
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: p/d.md\n    correct: true\n  - text: No\n---\n\nPick?\n";
        let mut b = crate::parse::item_bank_from_sources_with(
            "m",
            [("q.md".to_owned(), source.to_owned())],
            &reader,
        )
        .expect("bank");
        let renderer = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &renderer, DiagramFormat::Png);
        assert_eq!(outcome.images.len(), 1);
        assert!(matches!(
            b.items.first().map(|q| &q.kind),
            Some(QuestionKind::MultipleChoice(set)) if set.choices.first()
                .is_some_and(|c| c.text.contains("![diagram](generated/mermaid-"))
        ));
    }

    #[test]
    /// A mermaid fence with trailing info (` ```mermaid theme=… `) still renders.
    fn mermaid_with_trailing_info_renders() {
        let mut b = bank("```mermaid theme=default\ngraph TD; A-->B;\n```");
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert_eq!(outcome.images.len(), 1);
    }

    #[test]
    /// SVG format uses the `.svg` extension in the generated path.
    fn svg_format_uses_svg_extension() {
        let mut b = bank("```mermaid\ngraph TD; A-->B;\n```");
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Svg);
        let path = outcome.images.first().map_or("", |(p, _)| p.as_str());
        assert!(path.contains(".svg"));
        assert!(prompt(&b).contains(".svg)"));
    }

    #[test]
    /// Every recognised fence maps to its language; anything else is not a
    /// diagram, and the mapping is case-sensitive like other fence languages.
    fn from_info_maps_known_fences_only() {
        for (info, expected) in [
            ("mermaid", Some(DiagramLanguage::Mermaid)),
            ("mermaid theme=default", Some(DiagramLanguage::Mermaid)),
            ("dot", Some(DiagramLanguage::Graphviz)),
            ("graphviz", Some(DiagramLanguage::Graphviz)),
            ("  dot  ", Some(DiagramLanguage::Graphviz)),
            ("", None),
            ("   ", None),
            ("rust", None),
            ("dotnet", None),
            ("graphql", None),
            // Fence languages are matched exactly, in lower case.
            ("DOT", None),
            ("Mermaid", None),
        ] {
            assert_eq!(DiagramLanguage::from_info(info), expected, "info {info:?}");
        }
    }

    #[test]
    /// A language's own name is a fence that selects it again, so the name in a
    /// generated path or warning is always one an author can write.
    fn language_name_round_trips_through_from_info() {
        for language in [DiagramLanguage::Mermaid, DiagramLanguage::Graphviz] {
            assert_eq!(DiagramLanguage::from_info(language.name()), Some(language));
        }
    }

    #[test]
    /// Each language names itself and its source file distinctly — the names
    /// key generated paths, so a collision would alias two languages' diagrams.
    fn languages_have_distinct_names_and_source_extensions() {
        let mermaid = DiagramLanguage::Mermaid;
        let graphviz = DiagramLanguage::Graphviz;
        assert_ne!(mermaid.name(), graphviz.name());
        assert_ne!(mermaid.source_extension(), graphviz.source_extension());
        // `mmdc` reads a `.md` input as Markdown to extract diagrams from.
        assert_ne!(mermaid.source_extension(), "md");
    }

    #[test]
    /// Both ` ```dot ` and ` ```graphviz ` fences render as Graphviz diagrams.
    fn renders_graphviz_fences() {
        for fence in ["dot", "graphviz"] {
            let mut b = bank(&format!("```{fence}\ndigraph {{ a -> b; }}\n```"));
            let render = ok_renderer();
            let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
            assert_eq!(outcome.images.len(), 1);
            assert!(prompt(&b).contains("![diagram](generated/graphviz-"));
            assert!(!prompt(&b).contains("```"));
        }
    }

    #[test]
    /// Each block is rendered with its own language.
    fn passes_language_to_renderer() {
        use std::cell::RefCell;
        let seen = RefCell::new(Vec::new());
        let render = |language: DiagramLanguage, _source: &str| {
            seen.borrow_mut().push(language);
            Ok(vec![1])
        };
        let mut b = bank("```mermaid\ngraph TD; A-->B;\n```\n\n```dot\ndigraph { a -> b; }\n```");
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert_eq!(outcome.images.len(), 2);
        let mut languages = seen.into_inner();
        languages.sort_by_key(|l| l.name());
        assert_eq!(
            languages,
            [DiagramLanguage::Graphviz, DiagramLanguage::Mermaid]
        );
    }

    #[test]
    /// Identical source in two languages renders twice, to distinct paths — the
    /// dedup key is the language as well as the content.
    fn languages_do_not_share_generated_paths() {
        let mut b = bank("```mermaid\nsame\n```\n\n```dot\nsame\n```");
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert_eq!(outcome.images.len(), 2);
        assert!(prompt(&b).contains("generated/mermaid-"));
        assert!(prompt(&b).contains("generated/graphviz-"));
    }

    /// A two-question bank whose prompts are `first` and `second`.
    fn two_question_bank(first: &str, second: &str) -> ItemBank {
        ItemBank {
            name: "M".to_owned(),
            items: vec![question("q1", first), question("q2", second)],
        }
    }

    #[test]
    /// The same diagram in two different questions renders once — the dedup set
    /// spans the bank, not one field — and both prompts share the one image.
    fn deduplicates_identical_diagrams_across_questions() {
        let block = "```dot\ndigraph { a -> b; }\n```";
        let mut b = two_question_bank(block, block);
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert_eq!(outcome.images.len(), 1);
        let path = outcome.images.first().map_or("", |(p, _)| p.as_str());
        assert!(b.items.iter().all(|q| q.prompt.contains(path)));
    }

    #[test]
    /// Questions are walked in order: two distinct diagrams bundle in question
    /// order, and an empty slice renders nothing.
    fn questions_are_walked_in_order() {
        let mut b = two_question_bank(
            "```mermaid\ngraph TD; A-->B;\n```",
            "```dot\ndigraph { a -> b; }\n```",
        );
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        let paths: Vec<&str> = outcome.images.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths.len(), 2);
        assert!(paths.first().is_some_and(|p| p.contains("mermaid-")));
        assert!(paths.get(1).is_some_and(|p| p.contains("graphviz-")));
        let empty = render_diagrams(&mut [], &render, DiagramFormat::Png);
        assert!(empty.images.is_empty() && empty.warnings.is_empty());
    }

    #[test]
    /// A failing diagram between two succeeding ones keeps its own code block
    /// and disturbs neither spliced sibling nor the surrounding text — the
    /// splice runs back-to-front, so a skipped span must not shift the rest —
    /// and warns once naming the question and the language that failed.
    fn failed_block_leaves_sibling_renders_intact() {
        let mut b = bank(
            "```mermaid\ngraph TD; A-->B;\n```\n\n```dot\ndigraph { a -> b; }\n```\n\n\
             ```mermaid\ngraph TD; X-->Y;\n```\n\nEnd",
        );
        let render = |language: DiagramLanguage, _source: &str| match language {
            DiagramLanguage::Mermaid => Ok(vec![1, 2, 3]),
            DiagramLanguage::Graphviz => Err("dot not found".to_owned()),
        };
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert_eq!(outcome.images.len(), 2);
        assert_eq!(
            prompt(&b).matches("![diagram](generated/mermaid-").count(),
            2
        );
        assert!(prompt(&b).contains("```dot\ndigraph { a -> b; }\n```"));
        assert!(prompt(&b).ends_with("End"));
        assert_eq!(outcome.warnings.len(), 1);
        let warning = outcome.warnings.first().map_or("", String::as_str);
        assert!(warning.contains("question \"q\"") && warning.contains("graphviz diagram"));
    }

    #[test]
    /// Failures are not cached the way renders are: every occurrence of an
    /// unrenderable diagram warns, so each block needing attention is named.
    fn repeated_failures_warn_once_per_occurrence() {
        let mut b = bank("```dot\nsame\n```\n\n```dot\nsame\n```");
        let render = |_language: DiagramLanguage, _source: &str| Err("dot not found".to_owned());
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert!(outcome.images.is_empty());
        assert_eq!(outcome.warnings.len(), 2);
    }

    #[test]
    /// An unlabelled fence is not a diagram, and a mixed bank leaves it alone.
    fn unlabelled_fence_is_not_a_diagram() {
        let mut b = bank("```\ndigraph { a -> b; }\n```\n\n```dot\ndigraph { a -> b; }\n```");
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b.items, &render, DiagramFormat::Png);
        assert_eq!(outcome.images.len(), 1);
        assert!(prompt(&b).contains("```\ndigraph { a -> b; }\n```"));
    }
}
