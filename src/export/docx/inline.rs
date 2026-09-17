//! Markdown inline structure → OOXML runs.
//!
//! A `w:p` is a list of runs, and a run carries one set of character marks. So
//! rendering `**bold** and *italic*` means splitting the text at every mark
//! boundary and giving each piece its own `w:r`. This module owns that split.
//!
//! # What is rendered, and what is not
//!
//! Part of the authoring format is character-level (emphasis, code spans,
//! math) and part is block-level (lists, tables, code blocks, images). This
//! module handles the first and **refuses** the second rather than dropping
//! it: a block it cannot render is emitted as its own Markdown source, so a
//! list prints as `- item` instead of silently losing its bullets. That
//! fallback is the same stopgap the print sheet already ships for tables —
//! text that looks like text, which an instructor can see and work around.
//!
//! Math is the exception, and deliberately: it is never printed as source. A
//! student who meets `$\frac{a}{b}$` on a page sees something that is not a
//! formula, and neither they nor the instructor finds out until the copies
//! are printed. So math that cannot be rendered — because it is unsupported,
//! *or* because the block around it is — fails the export instead.
//!
//! # Schema order
//!
//! `w:rPr` children are a sequence (`CT_RPr`), not a set: `w:rStyle`, then
//! `w:b`, `w:i`, `w:strike`. Emitting them in a different order is a schema
//! violation Word rejects the document for and `xmllint --noout` cannot see.

use std::fmt::Write as _;
use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

use super::omml::{self, Display};
use crate::export::escape_xml;
use crate::{Error, Result};

/// The character style inline code spans are set in.
///
/// A style reference rather than a `w:rFonts` on every run: the preformatted
/// *block* writer needs the same face, and one definition keeps the two from
/// drifting apart.
pub(super) const CODE_STYLE: &str = "Code";

/// What a rendered paragraph is, beyond the runs it holds.
#[derive(Debug, Clone, Copy)]
pub(super) enum Kind {
    /// Ordinary prose.
    Prose,
    /// A Markdown heading, at this level.
    Heading(u8),
    /// A display equation standing alone in its paragraph.
    ///
    /// Reported rather than centred here. `m:oMathPara` centres the whole
    /// *paragraph*, so only a caller that knows nothing else shares the line
    /// — a question number, in particular — may apply it. See
    /// [`Paragraph::centred`].
    Equation,
}

/// One rendered paragraph.
#[derive(Debug)]
pub(super) struct Paragraph {
    /// What the paragraph is, which decides how the caller sets it.
    pub(super) kind: Kind,
    /// The runs the paragraph is made of.
    pub(super) runs: String,
}

impl Paragraph {
    /// The runs, with a standalone display equation set apart on its own line.
    ///
    /// Callers that put something else in the same paragraph must use
    /// [`Self::runs`] instead: `m:oMathPara` centres everything on the line,
    /// so a question number in front of an equation would be centred with it.
    pub(super) fn centred(&self) -> String {
        match self.kind {
            Kind::Equation => format!("<m:oMathPara>{}</m:oMathPara>", self.runs),
            Kind::Prose | Kind::Heading(_) => self.runs.clone(),
        }
    }
}

/// Render `markdown` as the paragraphs it contains, in document order.
///
/// Only the runs are decided here. The caller picks the `w:pPr` to wrap them
/// in, because the same prose is set differently as a header and as a prompt.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if a `$…$` span cannot be converted to
/// OOXML math, or sits in a block that would be printed as source.
pub(super) fn paragraphs(markdown: &str) -> Result<Vec<Paragraph>> {
    // Normalised first: authored text arriving CRLF would otherwise carry a
    // stray `\r` into `<w:t>`, which XML line-ending normalisation rewrites on
    // read — the document would not round-trip through its own reader.
    let source = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let mut rendered = Vec::new();
    let mut covered = 0;
    for (range, block) in blocks(&source) {
        let from = line_start(&source, range.start);
        push_gap(&mut rendered, source.get(covered..from).unwrap_or_default());
        covered = range.end.max(covered);
        rendered.push(block.render(source.get(from..range.end).unwrap_or_default())?);
    }
    push_gap(&mut rendered, source.get(covered..).unwrap_or_default());
    Ok(rendered)
}

/// One literal text run.
pub(super) fn text_run(text: &str) -> String {
    run(text, "")
}

/// One top-level Markdown block.
enum Block<'a> {
    /// A paragraph or heading, carried as the inline events inside it.
    Prose(Kind, Vec<Event<'a>>),
    /// Any other block: printed as its own source, never rendered. Its events
    /// are kept all the same, because math among them is refused rather than
    /// printed.
    Literal(Vec<Event<'a>>),
}

impl Block<'_> {
    /// This block as a paragraph, falling back to `source` if its inline
    /// structure cannot be rendered.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMath`] if the block holds math that cannot
    /// be rendered or would fall back to source.
    fn render(self, source: &str) -> Result<Paragraph> {
        let events = match self {
            Self::Prose(kind, events) => match runs(&events, kind)? {
                Some(runs) => return Ok(Paragraph { kind, runs }),
                // The markers are what prints now, so it is no longer a
                // heading: `## Part *2*` falls back as `## Part *2*`.
                None => events,
            },
            Self::Literal(events) => events,
        };
        refuse_math(&events)?;
        Ok(Paragraph {
            kind: Kind::Prose,
            runs: literal(source),
        })
    }
}

/// Split `source` into its top-level blocks, each with the span it came from.
///
/// Every `Start` is paired with the span of the *whole* element, not just its
/// opener, so a block that cannot be rendered has its own source to fall back
/// to.
fn blocks(source: &str) -> Vec<(Range<usize>, Block<'_>)> {
    let mut blocks = Vec::new();
    let mut events = Parser::new_ext(source, crate::export::MARKDOWN).into_offset_iter();
    while let Some((event, range)) = events.next() {
        match event {
            Event::Start(Tag::Paragraph) => {
                let inner = inside(&mut events);
                blocks.push((range, Block::Prose(prose_kind(&inner), inner)));
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let kind = Kind::Heading(depth(level));
                blocks.push((range, Block::Prose(kind, inside(&mut events))));
            }
            Event::Start(_) => blocks.push((range, Block::Literal(inside(&mut events)))),
            // Nothing else needs an arm. Every inline event is wrapped in a
            // block, raw HTML included; `inside` consumes the `End`s; and a
            // construct with no events of its own — a `***` rule — is
            // printed by the gap covering in `paragraphs`, which is where
            // anything the parser stays silent about is caught.
            _ => {}
        }
    }
    blocks
}

/// Whether a paragraph's events make it a display equation rather than prose.
fn prose_kind(events: &[Event<'_>]) -> Kind {
    if matches!(events, [Event::DisplayMath(_)]) {
        Kind::Equation
    } else {
        Kind::Prose
    }
}

/// A Markdown heading level as its number, 1–6.
///
/// Matched rather than cast so a level added upstream fails to compile here.
const fn depth(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Drain `events` up to the `End` closing the tag just opened, returning what
/// was inside it.
fn inside<'a>(events: &mut impl Iterator<Item = (Event<'a>, Range<usize>)>) -> Vec<Event<'a>> {
    let mut depth = 1_usize;
    let mut inner = Vec::new();
    for (event, _) in events {
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        inner.push(event);
    }
    inner
}

/// The offset of the start of the line `at` falls on.
///
/// The parser reports an indented code block from its *content*, not from its
/// indent, so slicing the reported span alone de-indents the first line —
/// and alignment is the one thing the literal fallback exists to keep.
fn line_start(source: &str, at: usize) -> usize {
    source
        .get(..at)
        .map_or(at, |before| before.rfind('\n').map_or(0, |line| line + 1))
}

// Landing on the newline itself rather than just past it would make no
// difference to what is printed — [`literal`] trims leading newlines — but it
// would to the gap boundary, so the `+ 1` is kept deliberate rather than
// tightened away.

/// Push `source` as a literal paragraph, unless it is only whitespace.
///
/// This covers what the parser reports *no event at all* for. A link
/// reference definition (`[id]: https://…`) is the live case: without this,
/// the line holding the URL would vanish from a document whose links already
/// fall back to source, leaving the address nowhere on the sheet.
fn push_gap(rendered: &mut Vec<Paragraph>, source: &str) {
    if !source.trim().is_empty() {
        rendered.push(Paragraph {
            kind: Kind::Prose,
            runs: literal(source),
        });
    }
}

/// Render one paragraph's inline events as runs.
///
/// `Ok(None)` means the paragraph holds something this writer does not render
/// — an image, a link, raw HTML — and the caller should fall back to literal
/// source.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if a math span cannot be converted, or
/// if the paragraph is falling back and holds math.
fn runs(events: &[Event<'_>], kind: Kind) -> Result<Option<String>> {
    let display = match kind {
        Kind::Equation => Display::Block,
        Kind::Prose | Kind::Heading(_) => Display::Inline,
    };
    let mut xml = String::new();
    let mut marks = Marks::default();
    for event in events {
        if marks.apply(event) {
            continue;
        }
        match event {
            Event::Text(text) => xml.push_str(&run(text, &marks.properties())),
            Event::Code(text) => xml.push_str(&run(text, &marks.as_code().properties())),
            // A soft break is a line wrap in the source, not in the output;
            // the paragraph reflows, so it is a space. A hard break is
            // authored on purpose and keeps its line.
            Event::SoftBreak => xml.push_str(&run(" ", &marks.properties())),
            Event::HardBreak => xml.push_str("<w:r><w:br/></w:r>"),
            Event::InlineMath(latex) => xml.push_str(&omml::to_omml(latex, Display::Inline)?),
            Event::DisplayMath(latex) => xml.push_str(&omml::to_omml(latex, display)?),
            // One arm, so the set of constructs this writer claims to render
            // cannot drift from the set it actually renders. The whole
            // paragraph falls back, and `refuse_math` is given *every* event
            // rather than only those reached so far — otherwise whether an
            // exam exported would depend on which came first, the math or the
            // link.
            _ => return refuse_math(events).map(|()| None),
        }
    }
    Ok(Some(xml))
}

/// Refuse `events` if any of them is math.
///
/// Called on the paths that print Markdown *source*. Everything else this
/// writer cannot lay out degrades to visible text, which an author can see
/// and work around; `$\frac{a}{b}$` on an exam paper is instead a formula the
/// student cannot read and nobody notices until the copies are run off.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] naming the first expression found.
fn refuse_math(events: &[Event<'_>]) -> Result<()> {
    for event in events {
        if let Event::InlineMath(latex) | Event::DisplayMath(latex) = event {
            return Err(Error::UnsupportedMath {
                latex: latex.to_string(),
                reason: "it sits inside Markdown the Word writer cannot lay out yet — a list, \
                         table, quote, link or image — where it would print as LaTeX source. \
                         Move it into a paragraph of its own."
                    .to_owned(),
            });
        }
    }
    Ok(())
}

/// How deeply each character mark is open on the current run.
///
/// Depths, not flags. `CommonMark` *nests* same-kind emphasis, so
/// `**outer __inner__ tail**` opens strong twice; a flag flipped on each
/// event would leave `inner` plain — the least emphasised word in the
/// sentence, which is the opposite of what it says.
#[derive(Debug, Default, Clone, Copy)]
struct Marks {
    /// Depth of open `` `code` `` spans.
    code: u32,
    /// Depth of open `**strong**` spans.
    bold: u32,
    /// Depth of open `*emphasis*` spans.
    italic: u32,
    /// Depth of open `~~strikethrough~~` spans.
    strike: u32,
}

impl Marks {
    /// Apply `event` if it opens or closes a mark, reporting whether it did.
    fn apply(&mut self, event: &Event<'_>) -> bool {
        match event {
            Event::Start(Tag::Strong) => self.bold += 1,
            Event::End(TagEnd::Strong) => self.bold = self.bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => self.italic += 1,
            Event::End(TagEnd::Emphasis) => self.italic = self.italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => self.strike += 1,
            Event::End(TagEnd::Strikethrough) => self.strike = self.strike.saturating_sub(1),
            _ => return false,
        }
        true
    }

    /// The same marks with a code span open.
    ///
    /// Additive: a code span keeps the emphasis around it, so ``**`x`**`` is
    /// bold code. A code span cannot nest, so this needs no matching close.
    const fn as_code(self) -> Self {
        Self {
            code: self.code + 1,
            ..self
        }
    }

    /// The `w:rPr` for these marks, in schema order, or nothing if unmarked.
    fn properties(self) -> String {
        let mut properties = String::new();
        if self.code > 0 {
            let _ = write!(properties, r#"<w:rStyle w:val="{CODE_STYLE}"/>"#);
        }
        if self.bold > 0 {
            properties.push_str("<w:b/>");
        }
        if self.italic > 0 {
            properties.push_str("<w:i/>");
        }
        if self.strike > 0 {
            properties.push_str("<w:strike/>");
        }
        if properties.is_empty() {
            properties
        } else {
            format!("<w:rPr>{properties}</w:rPr>")
        }
    }
}

/// One text run carrying `properties`, or nothing if there is no text.
fn run(text: &str, properties: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    format!(
        r#"<w:r>{properties}<w:t xml:space="preserve">{}</w:t></w:r>"#,
        escape_xml(text)
    )
}

/// A block this writer cannot render, as runs of its own Markdown source.
///
/// Line breaks are explicit: a `w:p` collapses newlines, so a list or a pipe
/// table would otherwise arrive as one run-on line and lose the alignment
/// that makes it readable at all.
fn literal(source: &str) -> String {
    // Blank lines are trimmed from both ends, but *only* newlines from the
    // front: a gap between two blocks starts with the newline that ended the
    // last one, and stripping spaces as well would de-indent a code block —
    // which is the one thing this fallback exists to preserve.
    source
        .trim_end()
        .trim_start_matches('\n')
        .lines()
        .map(text_run)
        .collect::<Vec<_>>()
        .join("<w:r><w:br/></w:r>")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runs of the single paragraph `markdown` renders to.
    fn only(markdown: &str) -> String {
        let mut rendered = paragraphs(markdown).expect("renders");
        assert_eq!(rendered.len(), 1, "expected one paragraph: {rendered:?}");
        rendered.pop().map(|block| block.runs).unwrap_or_default()
    }

    #[test]
    /// Each mark reaches its run, and only the text it wraps.
    fn emphasis_marks_the_text_it_wraps() {
        let runs = only("plain **bold** *italic* ~~gone~~ `code`");
        for (mark, text) in [
            ("<w:b/>", "bold"),
            ("<w:i/>", "italic"),
            ("<w:strike/>", "gone"),
            (r#"<w:rStyle w:val="Code"/>"#, "code"),
        ] {
            let expected = format!(
                r#"<w:r><w:rPr>{mark}</w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#
            );
            assert!(runs.contains(&expected), "no {mark} run for {text}: {runs}");
        }
        assert!(
            runs.contains(r#"<w:r><w:t xml:space="preserve">plain </w:t></w:r>"#),
            "unmarked text gained properties: {runs}"
        );
    }

    #[test]
    /// Nested marks of *different* kinds combine on the inner run.
    fn nested_marks_combine_on_one_run() {
        let runs = only("**bold *and italic* bold**");
        assert!(
            runs.contains(
                r#"<w:rPr><w:b/><w:i/></w:rPr><w:t xml:space="preserve">and italic</w:t>"#
            ),
            "{runs}"
        );
        // The bold must survive the italic closing, or the tail loses it.
        assert!(
            runs.contains(r#"<w:rPr><w:b/></w:rPr><w:t xml:space="preserve"> bold</w:t>"#),
            "bold did not survive the nested italic: {runs}"
        );
    }

    #[test]
    /// Nested marks of the *same* kind do not cancel. `CommonMark` nests them,
    /// so a flag flipped per event would leave `****very****` unemphasised
    /// and the inner word of `**a __b__ c**` the only plain one on the line.
    fn same_kind_nesting_does_not_cancel() {
        for (source, mark) in [
            ("****very****", "<w:b/>"),
            ("**outer __inner__ tail**", "<w:b/>"),
            ("*a _b_ c*", "<w:i/>"),
            ("~~a ~~b~~ c~~", "<w:strike/>"),
        ] {
            let runs = only(source);
            assert!(runs.contains(mark), "{source} lost {mark}: {runs}");
            assert!(
                !runs.contains(r#"<w:r><w:t xml:space="preserve">"#),
                "{source} left a run unmarked: {runs}"
            );
        }
    }

    #[test]
    /// `w:rPr` children are a sequence. Word rejects a document that writes
    /// them in any other order, and `xmllint --noout` cannot see it.
    fn run_properties_are_written_in_schema_order() {
        let runs = only("**~~*`all four`*~~**");
        assert!(
            runs.contains(r#"<w:rPr><w:rStyle w:val="Code"/><w:b/><w:i/><w:strike/></w:rPr>"#),
            "{runs}"
        );
    }

    #[test]
    /// A code span is literal: the marks inside it are text, not formatting.
    fn a_code_span_does_not_re_enter_markdown() {
        let runs = only("`**not bold**`");
        assert!(runs.contains(">**not bold**<"), "{runs}");
        assert!(!runs.contains("<w:b/>"), "{runs}");
    }

    #[test]
    /// Inline math becomes real OOXML math, not the LaTeX source.
    fn inline_math_becomes_omml() {
        let runs = only(r"the cost is $O(n^2)$ here");
        assert!(runs.contains("<m:oMath>"), "{runs}");
        assert!(!runs.contains("O(n^2)"), "LaTeX source leaked: {runs}");
        assert!(
            runs.contains(r#"<w:t xml:space="preserve"> here</w:t>"#),
            "text after the math was lost: {runs}"
        );
    }

    #[test]
    /// Display math alone in a paragraph reports itself as an equation, and
    /// `centred` is what sets it apart — never the runs, which a caller may
    /// have to put a question number in front of.
    fn display_math_alone_is_an_equation() {
        let mut rendered = paragraphs(r"$$\frac{a}{b}$$").expect("renders");
        let block = rendered.pop().expect("one paragraph");
        assert!(matches!(block.kind, Kind::Equation), "{block:?}");
        assert!(!block.runs.contains("oMathPara"), "{block:?}");
        assert!(block.centred().starts_with("<m:oMathPara>"), "{block:?}");
    }

    #[test]
    /// Display math written mid-sentence is not an equation paragraph:
    /// `m:oMathPara` centres the whole paragraph, so it would drag the
    /// surrounding words into the middle of the page.
    fn display_math_mid_sentence_stays_in_the_sentence() {
        let mut rendered = paragraphs(r"Evaluate $$\frac{a}{b}$$ now.").expect("renders");
        let block = rendered.pop().expect("one paragraph");
        assert!(matches!(block.kind, Kind::Prose), "{block:?}");
        assert!(!block.centred().contains("oMathPara"), "{block:?}");
    }

    #[test]
    /// Math that cannot be converted fails the export. It is the one thing
    /// never degraded to source: a formula that printed as `\frac{a}{b}`
    /// still looks like a question the student must answer.
    fn unconvertible_math_is_an_error() {
        let refused = paragraphs(r"$\begin{unknown}x\end{unknown}$");
        assert!(
            matches!(refused, Err(Error::UnsupportedMath { .. })),
            "{refused:?}"
        );
    }

    #[test]
    /// Math inside a block that falls back to source is refused too. The
    /// fallback prints Markdown, and printing `$x^2$` on an exam is exactly
    /// what the no-degrading rule exists to prevent.
    fn math_inside_a_literal_block_is_refused() {
        for source in [
            "- solve $x^2$",
            "| $x^2$ | 2 |\n|---|---|\n| a | b |",
            "> recall $e^{i\\pi}$",
            r"Given $x^2$, see [the handout](http://a.example).",
            r"Given ![fig](f.png), find $x^2$.",
        ] {
            let refused = paragraphs(source);
            assert!(
                matches!(refused, Err(Error::UnsupportedMath { .. })),
                "{source} was not refused: {refused:?}"
            );
        }
    }

    #[test]
    /// The refusal does not depend on word order. Converting math as the
    /// walk reached it, then discovering the link afterwards, made the same
    /// content export or fail depending on which came first.
    fn refusal_does_not_depend_on_word_order() {
        let before = paragraphs(r"$x^2$ and [a](b)");
        let after = paragraphs(r"see [a](b) and $x^2$");
        assert!(
            matches!(before, Err(Error::UnsupportedMath { .. })),
            "{before:?}"
        );
        assert!(
            matches!(after, Err(Error::UnsupportedMath { .. })),
            "{after:?}"
        );
    }

    #[test]
    /// A block with no math is not refused — only math is undegradable.
    fn a_literal_block_without_math_is_fine() {
        assert!(only("- first\n- second").contains(">- first<"));
    }

    #[test]
    /// A list is not rendered yet, so it prints as its own source — bullets
    /// and all. Dropping the markers would leave a run-on sentence that
    /// reads as prose.
    fn a_list_falls_back_to_its_own_source() {
        let runs = only("- first\n- second");
        assert!(runs.contains(">- first<"), "{runs}");
        assert!(runs.contains(">- second<"), "{runs}");
        assert!(
            runs.contains("<w:r><w:br/></w:r>"),
            "rows ran together: {runs}"
        );
    }

    #[test]
    /// A pipe table keeps its rows on separate lines, which is the whole of
    /// what makes it readable as a table.
    fn a_table_falls_back_line_by_line() {
        let runs = only("| a | b |\n|---|---|\n| 1 | 2 |");
        assert_eq!(runs.matches("<w:r><w:br/></w:r>").count(), 2, "{runs}");
        assert!(runs.contains(">| a | b |<"), "{runs}");
    }

    #[test]
    /// An indented code block keeps its indent. The parser reports the block
    /// from its *content*, so slicing that span alone de-indents the first
    /// line — and alignment is the one thing this fallback exists to keep.
    fn an_indented_code_block_keeps_its_indent() {
        let runs = only("    let x = 1;\n    let y = 2;");
        assert_eq!(runs.matches(">    let").count(), 2, "{runs}");
    }

    #[test]
    /// Widening a block to its line start must not widen it to the
    /// *document* start: a block after a paragraph would then reprint
    /// everything before it.
    fn widening_a_block_does_not_swallow_what_precedes_it() {
        let rendered = paragraphs("intro\n\n    let x = 1;").expect("renders");
        assert_eq!(rendered.len(), 2, "{rendered:?}");
        let code = rendered.last().expect("two paragraphs");
        assert!(code.runs.contains(">    let x = 1;<"), "{code:?}");
        assert!(
            !code.runs.contains("intro"),
            "the paragraph reprinted: {code:?}"
        );
    }

    #[test]
    /// …nor to the middle of the line before. A list may interrupt a
    /// paragraph with no blank line between them — forgetting that blank
    /// line is an ordinary authoring slip — and an off-by-one here drags the
    /// last letter of the paragraph onto the list.
    fn widening_a_block_starts_at_a_line_boundary() {
        let rendered = paragraphs("text\n- item").expect("renders");
        assert_eq!(rendered.len(), 2, "{rendered:?}");
        let list = rendered.last().expect("two paragraphs");
        assert_eq!(
            list.runs, r#"<w:r><w:t xml:space="preserve">- item</w:t></w:r>"#,
            "the paragraph bled into the list: {list:?}"
        );
    }

    #[test]
    /// A gap between two blocks starts with the newline that ended the last
    /// one. Printing it would open the paragraph with a blank line the
    /// author never wrote.
    fn a_gap_does_not_open_with_a_blank_line() {
        let rendered = paragraphs("a\n\n[ref]: https://a.example\n\nb").expect("renders");
        assert_eq!(rendered.len(), 3, "{rendered:?}");
        let gap = rendered.get(1).expect("three paragraphs");
        assert!(gap.runs.starts_with("<w:r><w:t"), "leading break: {gap:?}");
    }

    #[test]
    /// An unsupported *inline* construct falls the whole paragraph back, so
    /// the image reference is still visible rather than silently dropped.
    fn an_unsupported_inline_falls_the_paragraph_back() {
        let runs = only("see ![alt](plot.png) for the shape");
        assert!(runs.contains("![alt](plot.png)"), "{runs}");
    }

    #[test]
    /// A link reference definition is printed even though the parser reports
    /// no event for it at all. Links themselves fall back to source, so
    /// losing the definition would leave the address nowhere on the sheet.
    fn a_link_reference_definition_is_not_lost() {
        let rendered =
            paragraphs("See [the syllabus][syl].\n\n[syl]: https://a.example/s").expect("renders");
        let printed: String = rendered.iter().map(|block| block.runs.clone()).collect();
        assert!(printed.contains("[the syllabus][syl]"), "{rendered:?}");
        assert!(printed.contains("https://a.example/s"), "{rendered:?}");
    }

    #[test]
    /// A heading is a heading, not a paragraph beginning with a hash.
    fn headings_carry_their_level() {
        let kinds: Vec<String> = paragraphs("# One\n\n### Three\n\nprose")
            .expect("renders")
            .iter()
            .map(|block| format!("{:?}", block.kind))
            .collect();
        assert_eq!(kinds, ["Heading(1)", "Heading(3)", "Prose"]);
    }

    #[test]
    /// A heading renders its own inline marks, and not the `#` markers.
    fn a_heading_renders_its_inline_marks() {
        let runs = only("## Part *two*");
        assert!(runs.contains(r#"<w:rPr><w:i/></w:rPr><w:t xml:space="preserve">two</w:t>"#));
        assert!(!runs.contains('#'), "the marker reached the page: {runs}");
    }

    #[test]
    /// A heading that falls back prints its markers, so it is no longer set
    /// as a heading — the `##` on the page would be a heading twice over.
    fn a_heading_that_falls_back_is_not_a_heading() {
        let mut rendered = paragraphs("## See ![fig](f.png)").expect("renders");
        let block = rendered.pop().expect("one paragraph");
        assert!(matches!(block.kind, Kind::Prose), "{block:?}");
        assert!(block.runs.contains("## See"), "{block:?}");
    }

    #[test]
    /// A wrapped source line is one paragraph, so the wrap is a space; an
    /// authored hard break is not, so it keeps its line.
    fn soft_breaks_reflow_and_hard_breaks_do_not() {
        assert!(!only("line one\nline two").contains("<w:br/>"));
        // Carries a mark so the assertion cannot be satisfied by the literal
        // fallback, which emits a break run of its own for every line.
        let hard = only("line **one**  \nline two");
        assert!(hard.contains("<w:r><w:br/></w:r>"), "{hard}");
        assert!(hard.contains("<w:b/>"), "the paragraph fell back: {hard}");
    }

    #[test]
    /// A block with no inline structure of its own still prints. A rule that
    /// silently vanished would leave the instructor nothing to notice it by.
    fn a_rule_prints_as_its_source() {
        assert!(only("***").contains(">***<"));
    }

    #[test]
    /// Raw HTML is escaped text, not markup: a `<div>` in a prompt cannot
    /// reach the document as an element.
    fn raw_html_is_printed_escaped() {
        let runs = only("<div>see the handout</div>");
        assert!(runs.contains("see the handout"), "{runs}");
        assert!(
            runs.contains("&lt;div&gt;"),
            "raw HTML was interpreted: {runs}"
        );
    }

    #[test]
    /// Blank-line-separated blocks stay separate paragraphs.
    fn blocks_are_separate_paragraphs() {
        assert_eq!(paragraphs("one\n\ntwo").expect("renders").len(), 2);
    }

    #[test]
    /// A `\r` cannot survive in `<w:t>`: XML line-ending normalisation
    /// rewrites it on read, so the document would not round-trip itself.
    fn carriage_returns_do_not_reach_the_document() {
        let rendered = paragraphs("one\r\n\r\ntwo\rthree").expect("renders");
        assert_eq!(rendered.len(), 2, "{rendered:?}");
        assert!(
            rendered.iter().all(|block| !block.runs.contains('\r')),
            "{rendered:?}"
        );
    }

    #[test]
    /// Authored markup is escaped on both paths — through runs and through
    /// the literal fallback.
    fn text_is_escaped_on_both_paths() {
        assert!(only("a **&** b").contains("&amp;"));
        assert!(only("- a & b").contains("&amp;"));
    }

    #[test]
    /// Empty input renders nothing at all, rather than a stray blank line.
    fn empty_markdown_renders_no_paragraphs() {
        assert!(paragraphs("").expect("renders").is_empty());
        assert!(paragraphs("   \n\n  ").expect("renders").is_empty());
    }
}

#[cfg(test)]
mod probe5 {
    #[test]
    fn dump() {
        for source in ["text\n- item", "text\n# Heading", "a\n\n    code"] {
            println!("--- {source:?}\n  {:?}", super::paragraphs(source));
        }
    }
}
