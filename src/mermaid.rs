//! Render fenced ` ```mermaid ` blocks into bundled images.
//!
//! Canvas has no native mermaid support, so before a bank is exported each
//! mermaid block is rasterized to an image and the block replaced with a local
//! image reference (`generated/…`). Rendering itself is delegated to an injected
//! [`MermaidRenderer`] so this pass stays pure and testable; the CLI supplies a
//! real one by shelling out to the mermaid CLI (`mmdc`). A diagram that fails to
//! render (for example when the CLI is not installed) is **left as a code
//! block** and reported as a warning — it never fails the export.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

use crate::model::ItemBank;

/// The reserved bundle directory for mdquiz-generated images.
///
/// Diagram images land here (`generated/mermaid-<hash>.<ext>`); the CLI supplies
/// their bytes directly rather than reading them from the question directory.
pub const GENERATED_DIR: &str = "generated";

/// Whether `path` is one of this module's generated-diagram outputs.
///
/// The CLI uses this to skip generated paths when reading images from disk.
#[must_use]
pub fn is_generated_path(path: &str) -> bool {
    path.split('/').next() == Some(GENERATED_DIR)
}

/// Renders mermaid `source` to image bytes, or returns a message on failure.
///
/// Injected by the caller (the CLI shells out to `mmdc`) so this module needs no
/// external tool and stays unit-testable with a stub.
pub type MermaidRenderer<'a> = dyn Fn(&str) -> std::result::Result<Vec<u8>, String> + 'a;

/// The image format diagrams render to.
#[derive(Debug, Clone, Copy)]
pub enum DiagramFormat {
    /// Raster PNG — the most reliably rendered format inside Canvas.
    Png,
    /// Scalable SVG.
    Svg,
}

impl DiagramFormat {
    /// The file extension for this format.
    #[must_use]
    pub fn extension(self) -> &'static str {
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

/// Replace every mermaid block in `bank` with a generated image reference.
///
/// Each distinct diagram is rendered once (deduplicated by content); failures
/// are collected as warnings and leave the block untouched. Returns the images
/// to bundle and any warnings.
#[must_use]
pub fn render_diagrams(
    bank: &mut ItemBank,
    render: &MermaidRenderer<'_>,
    format: DiagramFormat,
) -> DiagramOutcome {
    let mut outcome = DiagramOutcome::default();
    let mut seen = HashSet::new();
    for question in &mut bank.items {
        let id = question.id.clone();
        for field in question.rich_text_fields_mut() {
            render_field(field, render, format, &id, &mut seen, &mut outcome);
        }
    }
    outcome
}

/// Render every mermaid block in one `field`, splicing image references in place.
fn render_field(
    field: &mut String,
    render: &MermaidRenderer<'_>,
    format: DiagramFormat,
    id: &str,
    seen: &mut HashSet<String>,
    outcome: &mut DiagramOutcome,
) {
    // Collect spans first (immutable borrow ends), then splice back-to-front so
    // earlier byte offsets stay valid as text is replaced.
    for (span, source) in mermaid_blocks(field).into_iter().rev() {
        let path = generated_path(&source, format);
        if seen.contains(&path) {
            field.replace_range(span, &image_reference(&path));
        } else if let Some(bytes) = render_one(render, &source, id, outcome) {
            seen.insert(path.clone());
            outcome.images.push((path.clone(), bytes));
            field.replace_range(span, &image_reference(&path));
        }
    }
}

/// Render a single diagram, recording a warning (and returning `None`) on failure.
fn render_one(
    render: &MermaidRenderer<'_>,
    source: &str,
    id: &str,
    outcome: &mut DiagramOutcome,
) -> Option<Vec<u8>> {
    match render(source.trim()) {
        Ok(bytes) => Some(bytes),
        Err(message) => {
            outcome.warnings.push(format!(
                "question {id:?}: mermaid diagram left as code ({message})"
            ));
            None
        }
    }
}

/// The `(byte span, source)` of every fenced ` ```mermaid ` block in `markdown`.
fn mermaid_blocks(markdown: &str) -> Vec<(Range<usize>, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (event, range) in Parser::new(markdown).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) if is_mermaid(&info) => {
                current = Some((range.start, String::new()));
            }
            Event::Text(text) => {
                if let Some((_, source)) = current.as_mut() {
                    source.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((start, source)) = current.take() {
                    blocks.push((start..range.end, source));
                }
            }
            _ => {}
        }
    }
    blocks
}

/// Whether a fenced code block's info string marks it as mermaid.
fn is_mermaid(info: &str) -> bool {
    info.split_whitespace().next() == Some("mermaid")
}

/// The bundle path for the diagram rendered from `source`, deduped by content.
fn generated_path(source: &str, format: DiagramFormat) -> String {
    format!(
        "{GENERATED_DIR}/mermaid-{}.{}",
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

/// The Markdown image reference that replaces a rendered mermaid block.
fn image_reference(path: &str) -> String {
    format!("![diagram]({path})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Feedback, Question, QuestionKind, TrueFalse};

    /// A one-question bank whose prompt is `prompt`.
    fn bank(prompt: &str) -> ItemBank {
        ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: prompt.to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
            }],
        }
    }

    /// A renderer returning fixed bytes for any diagram.
    fn ok_renderer() -> impl Fn(&str) -> std::result::Result<Vec<u8>, String> {
        |_source| Ok(vec![1, 2, 3])
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
        let outcome = render_diagrams(&mut b, &render, DiagramFormat::Png);
        assert!(prompt(&b).contains("![diagram](generated/mermaid-"));
        assert!(prompt(&b).contains(".png)") && !prompt(&b).contains("```mermaid"));
        assert_eq!(outcome.images.len(), 1);
        assert!(outcome.warnings.is_empty());
        // The bundle path in the prompt matches the generated image path.
        let path = outcome.images.first().map_or("", |(p, _)| p.as_str());
        assert!(prompt(&b).contains(path));
    }

    #[test]
    /// A non-mermaid fenced block is left untouched.
    fn leaves_other_code_blocks() {
        let mut b = bank("```rust\nfn main() {}\n```");
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b, &render, DiagramFormat::Png);
        assert!(prompt(&b).contains("```rust"));
        assert!(outcome.images.is_empty());
    }

    #[test]
    /// The same diagram twice renders once and bundles a single image.
    fn deduplicates_identical_diagrams() {
        let mut b = bank("```mermaid\ngraph TD; A-->B;\n```\n\n```mermaid\ngraph TD; A-->B;\n```");
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b, &render, DiagramFormat::Png);
        assert_eq!(outcome.images.len(), 1);
        assert_eq!(prompt(&b).matches("![diagram]").count(), 2);
    }

    #[test]
    /// A render failure warns and leaves the block as a code block.
    fn failure_warns_and_leaves_block() {
        let mut b = bank("```mermaid\nbad\n```");
        let render = |_source: &str| Err("mmdc not found".to_owned());
        let outcome = render_diagrams(&mut b, &render, DiagramFormat::Png);
        assert!(prompt(&b).contains("```mermaid"));
        assert!(outcome.images.is_empty());
        assert_eq!(outcome.warnings.len(), 1);
        let warning = outcome.warnings.first().map_or("", String::as_str);
        assert!(warning.contains("mmdc not found"));
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
        let outcome = render_diagrams(&mut b, &render, DiagramFormat::Png);
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
        let outcome = render_diagrams(&mut b, &renderer, DiagramFormat::Png);
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
        let outcome = render_diagrams(&mut b, &render, DiagramFormat::Png);
        assert_eq!(outcome.images.len(), 1);
    }

    #[test]
    /// SVG format uses the `.svg` extension in the generated path.
    fn svg_format_uses_svg_extension() {
        let mut b = bank("```mermaid\ngraph TD; A-->B;\n```");
        let render = ok_renderer();
        let outcome = render_diagrams(&mut b, &render, DiagramFormat::Svg);
        let path = outcome.images.first().map_or("", |(p, _)| p.as_str());
        assert!(path.contains(".svg"));
        assert!(prompt(&b).contains(".svg)"));
    }
}
