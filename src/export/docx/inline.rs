//! Markdown inline structure → OOXML runs.
//!
//! A `w:p` is a list of runs, and a run carries one set of character marks. So
//! rendering `**bold** and *italic*` means splitting the text at every mark
//! boundary and giving each piece its own `w:r`. This module owns that split.
//!
//! # What is rendered, and what is not
//!
//! Part of the authoring format is character-level (emphasis, code spans,
//! math) and part is block-level. Lists are laid out: [`list`] and
//! [`item_paragraphs`] hand each item a `w:numId` from [`super::numbering`],
//! so the markers are Word's own rather than characters typed into the text.
//! Images are placed: an `![alt](path.png)` whose bytes were supplied becomes
//! a `w:drawing` from [`super::media`], carrying its alt text as the
//! description. One that was not — a remote URL, a missing file, a format
//! that is not PNG — falls back with the rest.
//!
//! Tables are laid out: [`super::table`] turns a pipe table into a `w:tbl`,
//! which is the one authored construct that is neither a run nor a paragraph
//! — so it is reported as [`Kind::Table`] and the caller emits it whole.
//!
//! Everything else block-level — code blocks, quotes, links — is **refused**
//! rather than dropped: a block this module cannot render is emitted as its
//! own Markdown source, so it prints as the Markdown an instructor typed
//! instead of silently losing its shape. A code block, whose indentation
//! carries meaning, is set in [`CODE_STYLE`] so the alignment survives; so
//! is a table that fell back, for the same reason. Every other refused block
//! keeps the body face. One item a list cannot lay out sends the *whole*
//! list back to source, and one cell a table cannot lay out sends the whole
//! table, because half of either formatted and half printed as Markdown
//! reads worse than either.
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

use pulldown_cmark::{Alignment, Event, HeadingLevel, Parser, Tag, TagEnd};

use super::numbering::{Item, MAX_LEVEL, Marker};
use super::omml::{self, Display};
use super::{Refs, table};
use crate::export::escape_xml;
use crate::{Error, Result};

/// The character style inline code spans and preformatted blocks are set in.
///
/// A style reference rather than a `w:rFonts` on every run: the preformatted
/// *block* writer uses the same face (see [`Face::properties`]), and one
/// definition keeps the two from drifting apart.
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
    /// A paragraph inside a list item, in the list [`Item`] names.
    Item(Item),
    /// A whole `w:tbl`, already complete.
    ///
    /// The odd one out: a table is a block-level sibling of `w:p`, not
    /// something that fits inside one, so the caller emits it as-is rather
    /// than wrapping it. [`super::set`] is the one place that decides.
    Table,
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
            Kind::Prose | Kind::Heading(_) | Kind::Item(_) | Kind::Table => self.runs.clone(),
        }
    }
}

/// Render `markdown` as the paragraphs it contains, in document order.
///
/// Only the runs are decided here. The caller picks the `w:pPr` to wrap them
/// in, because the same prose is set differently as a header and as a prompt.
///
/// `refs` accumulates the lists and images the document uses; it is threaded
/// in rather than built here because a `w:numId` and an `r:embed` must be
/// unique across the whole document, and a prompt does not know what the
/// header already opened.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if a `$…$` span cannot be converted to
/// OOXML math, or sits in a block that would be printed as source.
pub(super) fn paragraphs(markdown: &str, refs: &mut Refs<'_>) -> Result<Vec<Paragraph>> {
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
        block.render(
            source.get(from..range.end).unwrap_or_default(),
            refs,
            &mut rendered,
        )?;
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
    /// A list, carried as the events inside it. Unlike the others it yields
    /// several paragraphs — one per item, and more where an item holds more.
    List(Marker, Vec<Event<'a>>),
    /// A code block, carried as the events inside it. Printed as its own
    /// source in a monospace face — not a fallback from a richer rendering,
    /// but how a fence is rendered.
    Preformatted(Vec<Event<'a>>),
    /// A pipe table, with its column alignments and the events inside it.
    Table(Vec<Alignment>, Vec<Event<'a>>),
    /// Any other block: printed as its own source, never rendered. Its events
    /// are kept all the same, because math among them is refused rather than
    /// printed.
    Literal(Vec<Event<'a>>),
}

impl<'e> Block<'e> {
    /// Append this block's paragraphs to `out`, falling back to `source` where
    /// its inline structure cannot be rendered — and printing `source` outright
    /// for the blocks that are always set that way.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMath`] if the block holds math that cannot
    /// be rendered or would fall back to source.
    fn render(self, source: &str, refs: &mut Refs<'_>, out: &mut Vec<Paragraph>) -> Result<()> {
        // A block is laid out speculatively: the runs are built, and a later
        // event can still send the whole block to source. Anything those runs
        // registered goes back with them — an image bundled into a package
        // that no longer draws it is a part nothing cites.
        let mark = refs.media.mark();
        let Some((events, face)) = self.laid_out(refs, out)? else {
            return Ok(());
        };
        refs.media.rewind(mark);
        refuse_math(&events)?;
        out.push(Paragraph {
            kind: Kind::Prose,
            runs: literal(source, &face.properties()),
        });
        Ok(())
    }

    /// Lay this block out into `out`, or report the events and face it must
    /// be printed as source with.
    ///
    /// `Ok(None)` means it was laid out and there is nothing left to do.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMath`] if the block holds math that cannot
    /// be rendered.
    fn laid_out(
        self,
        refs: &mut Refs<'_>,
        out: &mut Vec<Paragraph>,
    ) -> Result<Option<(Vec<Event<'e>>, Face)>> {
        match self {
            Self::Prose(kind, events) => {
                let Some(runs) = runs(&events, kind, refs)? else {
                    // The markers are what prints now, so it is no longer a
                    // heading: `## Part *2*` falls back as `## Part *2*`.
                    return Ok(Some((events, Face::Body)));
                };
                out.push(Paragraph { kind, runs });
            }
            Self::List(marker, events) => {
                let Some(paragraphs) = list(&events, marker, 0, refs)? else {
                    return Ok(Some((events, Face::Body)));
                };
                out.extend(paragraphs);
            }
            Self::Preformatted(events) => return Ok(Some((events, Face::Code))),
            Self::Table(alignments, events) => {
                let Some(xml) = table::table(&alignments, &events, refs)? else {
                    // A cell this writer cannot lay out sends the whole
                    // table to source, in the monospace face that keeps its
                    // columns lined up on the page.
                    return Ok(Some((events, Face::Code)));
                };
                out.push(Paragraph {
                    kind: Kind::Table,
                    runs: xml,
                });
            }
            Self::Literal(events) => return Ok(Some((events, Face::Body))),
        }
        Ok(None)
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
            Event::Start(Tag::List(first)) => {
                let marker = first.map_or(Marker::Bullet, Marker::Ordered);
                blocks.push((range, Block::List(marker, inside(&mut events))));
            }
            Event::Start(Tag::CodeBlock(_)) => {
                blocks.push((range, Block::Preformatted(inside(&mut events))));
            }
            Event::Start(Tag::Table(alignments)) => {
                blocks.push((range, Block::Table(alignments, inside(&mut events))));
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
            Event::Start(_) => depth = depth.saturating_add(1),
            Event::End(_) => {
                // Saturating, as in `nested`: the two count the same tags to
                // the same rule, and the `break` is the only reason a bare
                // decrement cannot go below zero here.
                depth = depth.saturating_sub(1);
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
            runs: literal(source, ""),
        });
    }
}

/// One piece of a list item.
enum Segment<'a, 'e> {
    /// Inline events forming one paragraph.
    Text(&'a [Event<'e>]),
    /// A list nested inside this item, and how it marks its own items.
    Nested(Marker, &'a [Event<'e>]),
}

/// The paragraphs of one list at nesting `level`.
///
/// `Ok(None)` means an item held something this writer does not render, and
/// the caller should print the whole list as source — one item silently
/// losing a link while its neighbours kept theirs would be worse than a list
/// that is visibly unformatted throughout.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if an item holds math that cannot be
/// rendered.
fn list(
    events: &[Event<'_>],
    marker: Marker,
    level: u8,
    refs: &mut Refs<'_>,
) -> Result<Option<Vec<Paragraph>>> {
    let id = refs.lists.open(marker, level);
    let mut rendered = Vec::new();
    for item_events in items(events) {
        let item = Item {
            list: id,
            level,
            marked: true,
        };
        let Some(paragraphs) = item_paragraphs(item_events, item, refs)? else {
            return Ok(None);
        };
        rendered.extend(paragraphs);
    }
    Ok(Some(rendered))
}

/// The paragraphs of one list item, including any list nested in it.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if the item holds math that cannot be
/// rendered.
fn item_paragraphs(
    events: &[Event<'_>],
    mut item: Item,
    refs: &mut Refs<'_>,
) -> Result<Option<Vec<Paragraph>>> {
    let mut rendered = Vec::new();
    let segments = segments(events);
    if !matches!(segments.first(), Some(Segment::Text(_))) {
        rendered.push(empty_marker(item));
        item.marked = false;
    }
    for segment in segments {
        match segment {
            Segment::Text(events) => {
                let kind = Kind::Item(item);
                let Some(runs) = runs(events, kind, refs)? else {
                    return Ok(None);
                };
                rendered.push(Paragraph { kind, runs });
                item.marked = false;
            }
            Segment::Nested(nested, events) => {
                let deeper = item.level.saturating_add(1).min(MAX_LEVEL);
                let Some(paragraphs) = list(events, nested, deeper, refs)? else {
                    return Ok(None);
                };
                rendered.extend(paragraphs);
            }
        }
    }
    Ok(Some(rendered))
}

/// The paragraph an item's marker hangs on when the item has no text of its
/// own — one written empty, or holding nothing but a nested list.
///
/// Without it the item vanishes: the list comes out a line shorter, and in a
/// numbered one every item after it moves up, so `1.` followed by
/// `2. second` prints "second" as item 1.
fn empty_marker(item: Item) -> Paragraph {
    Paragraph {
        kind: Kind::Item(item),
        runs: String::new(),
    }
}

/// The events inside each `Item` of a list, in order.
fn items<'a, 'e>(events: &'a [Event<'e>]) -> Vec<&'a [Event<'e>]> {
    let mut items = Vec::new();
    let mut index = 0;
    while let Some(event) = events.get(index) {
        if matches!(event, Event::Start(Tag::Item)) {
            let (inner, next) = nested(events, index);
            items.push(inner);
            index = next;
        } else {
            index += 1;
        }
    }
    items
}

/// Split a list item into the paragraphs and nested lists it holds.
///
/// A *tight* list gives an item's text as bare inline events; a *loose* one
/// wraps each in a paragraph. Treating the paragraph tags as separators
/// rather than as content handles both without asking which kind this is.
fn segments<'a, 'e>(events: &'a [Event<'e>]) -> Vec<Segment<'a, 'e>> {
    let mut segments = Vec::new();
    let (mut start, mut index) = (0, 0);
    while let Some(event) = events.get(index) {
        match event {
            Event::Start(Tag::List(first)) => {
                push_text(&mut segments, events.get(start..index));
                let (inner, next) = nested(events, index);
                let marker = first.map_or(Marker::Bullet, Marker::Ordered);
                segments.push(Segment::Nested(marker, inner));
                (start, index) = (next, next);
            }
            Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph) => {
                push_text(&mut segments, events.get(start..index));
                index += 1;
                start = index;
            }
            _ => index += 1,
        }
    }
    push_text(&mut segments, events.get(start..));
    segments
}

/// Push `events` as a text segment, unless there are none.
fn push_text<'a, 'e>(segments: &mut Vec<Segment<'a, 'e>>, events: Option<&'a [Event<'e>]>) {
    if let Some(events) = events.filter(|events| !events.is_empty()) {
        segments.push(Segment::Text(events));
    }
}

/// The events nested inside the tag opening at `start`, and the index just
/// past its `End`.
fn nested<'a, 'e>(events: &'a [Event<'e>], start: usize) -> (&'a [Event<'e>], usize) {
    let mut depth = 0_usize;
    for (index, event) in events.iter().enumerate().skip(start) {
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return (events.get(start + 1..index).unwrap_or_default(), index + 1);
                }
            }
            _ => {}
        }
    }
    // Unterminated, which pulldown does not produce: take the rest.
    (events.get(start + 1..).unwrap_or_default(), events.len())
}

/// Render one paragraph's inline events as runs.
///
/// `Ok(None)` means the paragraph holds something this writer does not render
/// — a link, raw HTML, an image whose bytes were not supplied — and the
/// caller should fall back to literal source.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if a math span cannot be converted, or
/// if the paragraph is falling back and holds math.
fn runs(events: &[Event<'_>], kind: Kind, refs: &mut Refs<'_>) -> Result<Option<String>> {
    let display = match kind {
        Kind::Equation => Display::Block,
        Kind::Prose | Kind::Heading(_) | Kind::Item(_) | Kind::Table => Display::Inline,
    };
    marked_runs(events, Marks::default(), display, refs)
}

/// Render one table cell's inline events as runs, opening with `marks`.
///
/// Cells are the one caller that starts with a mark already applied: a header
/// cell is bold before its own `**…**` adds anything.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if a math span cannot be converted.
pub(super) fn runs_for(
    events: &[Event<'_>],
    marks: Marks,
    refs: &mut Refs<'_>,
) -> Result<Option<String>> {
    marked_runs(events, marks, Display::Inline, refs)
}

/// Render `events` as runs, starting from `marks`.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if a math span cannot be converted, or
/// if the paragraph is falling back and holds math.
fn marked_runs(
    events: &[Event<'_>],
    mut marks: Marks,
    display: Display,
    refs: &mut Refs<'_>,
) -> Result<Option<String>> {
    let mut xml = String::new();
    let mut at = 0_usize;
    while let Some(event) = events.get(at) {
        let opened = at;
        at = at.saturating_add(1);
        if marks.apply(event) {
            continue;
        }
        // An image is the one construct here that spans several events: its
        // alt text sits between the `Start` and the `End`, so the walk steps
        // over the whole span rather than meeting the inner text again as
        // prose the picture already carries.
        if let Event::Start(Tag::Image { dest_url, .. }) = event {
            let (inner, past) = nested(events, opened);
            at = past;
            let Some(drawing) = refs.media.drawing(dest_url, &alt_text(inner)) else {
                return refuse_math(events).map(|()| None);
            };
            xml.push_str(&drawing);
            continue;
        }
        // One fallback, so the set of constructs this writer claims to render
        // cannot drift from the set it actually renders. The whole paragraph
        // falls back, and `refuse_math` is given *every* event rather than
        // only those reached so far — otherwise whether an exam exported
        // would depend on which came first, the math or the link.
        let Some(rendered) = character_run(event, marks, display)? else {
            return refuse_math(events).map(|()| None);
        };
        xml.push_str(&rendered);
    }
    Ok(Some(xml))
}

/// One run for an event that carries nothing but character-level structure.
///
/// `Ok(None)` for anything else — a link, raw HTML — which sends the whole
/// paragraph to literal source.
///
/// # Errors
///
/// Returns [`Error::UnsupportedMath`] if a math span cannot be converted.
fn character_run(event: &Event<'_>, marks: Marks, display: Display) -> Result<Option<String>> {
    Ok(Some(match event {
        Event::Text(text) => run(text, &marks.properties()),
        Event::Code(text) => run(text, &marks.as_code().properties()),
        // A soft break is a line wrap in the source, not in the output; the
        // paragraph reflows, so it is a space. A hard break is authored on
        // purpose and keeps its line.
        Event::SoftBreak => run(" ", &marks.properties()),
        Event::HardBreak => "<w:r><w:br/></w:r>".to_owned(),
        Event::InlineMath(latex) => omml::to_omml(latex, Display::Inline)?,
        Event::DisplayMath(latex) => omml::to_omml(latex, display)?,
        _ => return Ok(None),
    }))
}

/// The alt text of an image: the plain text authored inside its `![…]`.
///
/// It becomes the picture's description in the document, which is what a
/// screen reader announces — so an unlabelled figure on an exam paper is
/// unlabelled for the student who needs the label most.
fn alt_text(events: &[Event<'_>]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Text(text) | Event::Code(text) => Some(text.as_ref()),
            _ => None,
        })
        .collect()
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
                reason: "it sits inside Markdown the Word writer cannot lay out yet — a \
                         table, quote, link or image, or a list holding one of those — \
                         where it would print as LaTeX source. Move it into a paragraph \
                         of its own."
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
pub(super) struct Marks {
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
    /// Marks with strong already open, for a cell that is a column heading.
    pub(super) const fn bold() -> Self {
        Self {
            code: 0,
            bold: 1,
            italic: 0,
            strike: 0,
        }
    }

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

/// Which character face a block printed as its own source is set in.
#[derive(Debug, Clone, Copy)]
enum Face {
    /// The body face, for a block that merely falls back to source.
    Body,
    /// The monospace face, for a block set preformatted on purpose.
    Code,
}

impl Face {
    /// The `w:rPr` every line of the block carries.
    ///
    /// The code face is built through the same [`Marks`] path an inline
    /// `` `code` `` span takes, so the block face and the span face resolve to
    /// one `w:rStyle` and cannot drift apart.
    fn properties(self) -> String {
        match self {
            Self::Body => String::new(),
            Self::Code => Marks::default().as_code().properties(),
        }
    }
}

/// A block printed as its own Markdown source, one run per line.
///
/// Line breaks are explicit: a `w:p` collapses newlines, so a list or a pipe
/// table would otherwise arrive as one run-on line and lose the alignment
/// that makes it readable at all.
///
/// `properties` is the `w:rPr` every line carries — empty for a block falling
/// back to source, [`code_marks`] for one set preformatted on purpose.
fn literal(source: &str, properties: &str) -> String {
    // Blank lines are trimmed from both ends, but *only* newlines from the
    // front: a gap between two blocks starts with the newline that ended the
    // last one, and stripping spaces as well would de-indent a code block —
    // which is the one thing this fallback exists to preserve.
    source
        .trim_end()
        .trim_start_matches('\n')
        .lines()
        .map(|line| run(line, properties))
        .collect::<Vec<_>>()
        .join("<w:r><w:br/></w:r>")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render `markdown` into a document with nothing else in it and no
    /// images supplied.
    ///
    /// Each call gets a fresh [`Refs`], so a test's `w:numId`s start at 1 and
    /// do not depend on what another test rendered.
    fn rendered(markdown: &str) -> Result<Vec<Paragraph>> {
        paragraphs(markdown, &mut Refs::new(&[]))
    }

    /// Render `markdown` with `images` available to place.
    fn rendered_with(markdown: &str, images: &[(String, Vec<u8>)]) -> Result<Vec<Paragraph>> {
        paragraphs(markdown, &mut Refs::new(images))
    }

    /// One supplied image, a one-inch square named `figure.png`.
    fn figure() -> Vec<(String, Vec<u8>)> {
        vec![(
            "figure.png".to_owned(),
            super::super::media::tests::png(96, 96, None),
        )]
    }

    /// The runs of the single paragraph `markdown` renders to, with `figure`
    /// supplied.
    fn only_with_figure(markdown: &str) -> String {
        let mut rendered = rendered_with(markdown, &figure()).expect("renders");
        assert_eq!(rendered.len(), 1, "expected one paragraph: {rendered:?}");
        rendered.pop().map(|block| block.runs).unwrap_or_default()
    }

    /// The runs of the single paragraph `markdown` renders to.
    fn only(markdown: &str) -> String {
        let mut rendered = rendered(markdown).expect("renders");
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
        let mut rendered = rendered(r"$$\frac{a}{b}$$").expect("renders");
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
        let mut rendered = rendered(r"Evaluate $$\frac{a}{b}$$ now.").expect("renders");
        let block = rendered.pop().expect("one paragraph");
        assert!(matches!(block.kind, Kind::Prose), "{block:?}");
        assert!(!block.centred().contains("oMathPara"), "{block:?}");
    }

    #[test]
    /// Math that cannot be converted fails the export. It is the one thing
    /// never degraded to source: a formula that printed as `\frac{a}{b}`
    /// still looks like a question the student must answer.
    fn unconvertible_math_is_an_error() {
        let refused = rendered(r"$\begin{unknown}x\end{unknown}$");
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
            "> recall $e^{i\\pi}$",
            r"Given $x^2$, see [the handout](http://a.example).",
            r"Given ![fig](f.png), find $x^2$.",
            // A list *renders* now, so math in one reaches the page as math.
            // These two fall back — a link, then an image — and the fallback
            // is the path that would print the LaTeX.
            r"- see [the handout](http://a.example) and $x^2$",
            "- solve $x^2$\n- see ![fig](f.png)",
        ] {
            let refused = rendered(source);
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
        let before = rendered(r"$x^2$ and [a](b)");
        let after = rendered(r"see [a](b) and $x^2$");
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
        assert!(only("> quoted prose").contains(">&gt; quoted prose<"));
    }

    #[test]
    /// A list becomes one paragraph per item, each carrying the numbering it
    /// counts in rather than a marker typed into its text.
    fn a_list_becomes_one_paragraph_per_item() {
        let rendered = rendered("- first\n- second").expect("renders");
        assert_eq!(rendered.len(), 2, "{rendered:?}");
        for block in &rendered {
            assert!(
                matches!(block.kind, Kind::Item(Item { level: 0, .. })),
                "{block:?}"
            );
            assert!(!block.runs.contains('-'), "the marker is text: {block:?}");
        }
    }

    #[test]
    /// `items` steps past anything that is not an item rather than stalling
    /// on it. Nothing the parser emits inside a list is anything else, so
    /// this pins a guard the parser makes unnecessary — and which a wrong
    /// step would turn into a hang rather than a wrong page.
    fn items_steps_past_what_is_not_an_item() {
        let events = [
            Event::SoftBreak,
            Event::Start(Tag::Item),
            Event::Text("only".into()),
            Event::End(TagEnd::Item),
        ];
        assert_eq!(items(&events).len(), 1);
    }

    #[test]
    /// A nested list is the same list one level deeper, not a new one.
    /// Bullets carry no counter, so every bulleted list shares a `w:numId`.
    fn a_nested_list_goes_one_level_deeper() {
        let rendered = rendered("- outer\n  - inner\n- back").expect("renders");
        let levels: Vec<u8> = rendered
            .iter()
            .filter_map(|block| match block.kind {
                Kind::Item(item) => Some(item.level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, [0, 1, 0], "{rendered:?}");
    }

    #[test]
    /// Only the first paragraph of an item is marked. Marking the rest
    /// prints an item's own second paragraph as a second item.
    fn only_an_items_first_paragraph_carries_the_marker() {
        let rendered = rendered("- para one\n\n  para two").expect("renders");
        let marks: Vec<bool> = rendered
            .iter()
            .filter_map(|block| match block.kind {
                Kind::Item(item) => Some(item.marked),
                _ => None,
            })
            .collect();
        assert_eq!(marks, [true, false], "{rendered:?}");
    }

    #[test]
    /// An empty item still gets a paragraph to hang its marker on. Dropping
    /// it shortens the list — and in a numbered one renumbers everything
    /// after it, so `1.` `2. second` prints "second" as item 1.
    fn an_empty_item_keeps_its_place() {
        let rendered = rendered("1.\n2. second\n3. third").expect("renders");
        assert_eq!(rendered.len(), 3, "{rendered:?}");
        assert!(
            rendered
                .iter()
                .all(|block| matches!(block.kind, Kind::Item(Item { marked: true, .. }))),
            "{rendered:?}"
        );
    }

    #[test]
    /// An item holding nothing but a nested list keeps its own marker, at
    /// its own level, before the nested one.
    fn an_item_holding_only_a_nested_list_keeps_its_marker() {
        let rendered = rendered("-\n  - inner").expect("renders");
        let levels: Vec<u8> = rendered
            .iter()
            .filter_map(|block| match block.kind {
                Kind::Item(item) => Some(item.level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, [0, 1], "{rendered:?}");
    }

    #[test]
    /// An ordered list authored as `5.` starts at five. The parser reports the
    /// authored value and the writer has to carry it through to a
    /// `w:startOverride`; dropping it renumbers the list to start at 1, which
    /// looks deliberate on the page.
    fn an_authored_start_value_reaches_the_numbering() {
        let mut refs = Refs::new(&[]);
        paragraphs("5. five\n6. six", &mut refs).expect("renders");
        let xml = refs.lists.to_xml();
        assert!(xml.contains(r#"<w:startOverride w:val="5"/>"#), "{xml}");
    }

    #[test]
    /// `nested` reports the index *past* the tag it closed, and stops at the
    /// end of the events when that tag is never closed. Nothing pulldown emits
    /// is unterminated, so the second case pins a guard the parser makes
    /// unnecessary — but this is the index `items` advances by, and one that
    /// fails to advance turns that walk into a loop that never ends rather
    /// than a page that is merely wrong.
    fn nested_always_advances_past_what_it_consumed() {
        let closed = [
            Event::Start(Tag::Item),
            Event::Text("only".into()),
            Event::End(TagEnd::Item),
            Event::SoftBreak,
        ];
        let (inner, next) = nested(&closed, 0);
        assert_eq!(inner.len(), 1, "{inner:?}");
        assert_eq!(next, 3, "the closing tag would be walked a second time");

        let dangling = [Event::Start(Tag::Item), Event::Text("dangling".into())];
        let (rest, end) = nested(&dangling, 0);
        assert_eq!(rest.len(), 1, "{rest:?}");
        assert_eq!(end, dangling.len(), "an unterminated item did not finish");
    }

    #[test]
    /// An item holding a *block* this writer cannot lay out — a fenced code
    /// block, a quote, a table — falls the whole list back to source, the same
    /// as an item holding an unrenderable inline does. It reaches that
    /// decision by a different route: the block's events ride along inside a
    /// text segment rather than becoming a segment of their own.
    fn an_item_holding_a_block_falls_the_whole_list_back() {
        let blocks = rendered("- run this:\n\n  ```\n  make all\n  ```\n").expect("renders");
        let block = blocks.first().expect("one paragraph");
        assert!(matches!(block.kind, Kind::Prose), "{block:?}");
        assert!(block.runs.contains("- run this:"), "{block:?}");
        assert!(
            block.runs.contains("make all"),
            "the code was lost: {block:?}"
        );
        // Deliberate: only a *top-level* code block or table is set
        // preformatted. Here the whole list falls back together, and setting
        // an entire list in the code face to carry one fenced block in it
        // would misreport the prose items around it as code.
        assert!(
            !block.runs.contains(r#"<w:rStyle w:val="Code"/>"#),
            "a list falling back is body face, not code: {block:?}"
        );
    }

    /// The `w:ilvl` of every list-item paragraph in `blocks`, in order.
    fn levels_of(blocks: &[Paragraph]) -> Vec<u8> {
        blocks
            .iter()
            .filter_map(|block| match block.kind {
                Kind::Item(item) => Some(item.level),
                _ => None,
            })
            .collect()
    }

    #[test]
    /// Nesting deeper than Word allows clamps onto the deepest defined level
    /// — and *only* the levels past it. An item naming a level the numbering
    /// part does not define loses its marker *and* its indent, printing flush
    /// against the margin. The levels are asserted in full rather than as
    /// `level <= MAX_LEVEL`, which a clamp that flattened every level onto 0
    /// would satisfy just as well while losing the shape of the list.
    fn nesting_deeper_than_word_allows_clamps_only_past_the_deepest() {
        let deep = (0..12).fold(String::new(), |mut source, depth| {
            source.push_str(&" ".repeat(depth * 2));
            source.push_str("- item\n");
            source
        });
        let levels = levels_of(&rendered(&deep).expect("renders"));
        assert_eq!(levels, [0, 1, 2, 3, 4, 5, 6, 7, 8, 8, 8, 8]);
        assert!(levels.iter().all(|level| *level <= MAX_LEVEL));
    }

    #[test]
    /// An item's text *after* a nested list is still that item's second
    /// paragraph, not a new item. The marker state has to survive the
    /// recursion into the nested list; resetting it there prints a second
    /// marker halfway down the item.
    fn text_after_a_nested_list_is_still_the_same_item() {
        let blocks = rendered("- outer\n\n  - inner\n\n  back to outer").expect("renders");
        let shape: Vec<(u8, bool)> = blocks
            .iter()
            .filter_map(|block| match block.kind {
                Kind::Item(item) => Some((item.level, item.marked)),
                _ => None,
            })
            .collect();
        assert_eq!(shape, [(0, true), (1, true), (0, false)], "{blocks:?}");
    }

    #[test]
    /// An item that cannot be rendered sends the *outer* list back to source
    /// too, not just the list it sits in. Laying out the outer bullets around
    /// a nested list printed as `- see [a](b)` would read as an item whose
    /// text happens to begin with a dash.
    fn a_bad_nested_item_falls_the_whole_outer_list_back() {
        let blocks = rendered("- outer\n\n  - see [a](b)").expect("renders");
        let block = blocks.first().expect("one paragraph");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert!(matches!(block.kind, Kind::Prose), "{block:?}");
        assert!(block.runs.contains("- outer"), "{block:?}");
        assert!(
            block.runs.contains("[a](b)"),
            "the nested list was lost: {block:?}"
        );
    }

    #[test]
    /// A list item renders its own inline marks and math, which is the whole
    /// point of laying lists out rather than printing them as source.
    fn a_list_item_renders_its_inline_content() {
        let rendered = rendered("- solve $x^2$ **now**").expect("renders");
        let item = rendered.first().expect("one item");
        assert!(item.runs.contains("<m:oMath>"), "{item:?}");
        assert!(item.runs.contains("<w:b/>"), "{item:?}");
    }

    #[test]
    /// One unrenderable item sends the *whole* list back to source. Half a
    /// list laid out and half printed as Markdown is worse than either.
    fn one_bad_item_falls_the_whole_list_back() {
        let rendered = rendered("- fine\n- see [link](u)").expect("renders");
        assert_eq!(rendered.len(), 1, "{rendered:?}");
        let block = rendered.first().expect("one paragraph");
        assert!(matches!(block.kind, Kind::Prose), "{block:?}");
        assert!(block.runs.contains(">- fine<"), "{block:?}");
    }

    #[test]
    /// A code block and a pipe table are set in the monospace character
    /// style. This is the whole of what makes the fallback usable: a pipe
    /// table in a proportional face loses column alignment entirely, and so
    /// does the indentation of a code block.
    fn a_preformatted_block_is_set_in_the_code_style() {
        for source in [
            "```\nlet x = 1;\n```",
            // An info string is part of the fence line, which prints too.
            "```rust\nlet x = 1;\n```",
            "    let x = 1;",
            "```\n```",
        ] {
            let runs = only(source);
            assert!(
                runs.contains(r#"<w:rStyle w:val="Code"/>"#),
                "{source} is not monospace: {runs}"
            );
        }
    }

    #[test]
    /// Every line of a preformatted block carries the style, not just the
    /// first. Word applies a `w:rStyle` per run, and the block is one run per
    /// line, so styling only the opening run leaves a table with its first
    /// row aligned and the rest not.
    fn every_line_of_a_preformatted_block_carries_the_style() {
        let runs = only("```\nlet x = 1;\nlet y = 2;\n```");
        assert_eq!(
            runs.matches(r#"<w:rStyle w:val="Code"/>"#).count(),
            4,
            "{runs}"
        );
    }

    #[test]
    /// A block that merely *falls back* to source is not preformatted. A
    /// quote or a link is prose the writer cannot lay out, not something
    /// whose alignment carries meaning, and setting it in Consolas would
    /// misreport it to the reader as code.
    fn a_block_that_only_falls_back_is_not_monospace() {
        for source in [
            "> recall the rule",
            "see [the handout](http://a.example)",
            "see ![alt](plot.png)",
            "<div>see the handout</div>",
        ] {
            let runs = only(source);
            assert!(
                !runs.contains(r#"<w:rStyle w:val="Code"/>"#),
                "{source} was set as code: {runs}"
            );
        }
    }

    #[test]
    /// Math inside a fenced code block prints as source, and that is correct:
    /// inside a fence `$x^2$` *is* the code. The no-degrading rule is about
    /// math the author meant as math, and pulldown reports a fence's content
    /// as plain text, so `refuse_math` never sees a math event here. Pinned
    /// because a change that re-parsed a fence's content would start refusing
    /// exports that are fine today.
    fn math_inside_a_fence_is_code_not_math() {
        let runs = only("```\n$x^2$\n```");
        assert!(runs.contains("$x^2$"), "{runs}");
    }

    #[test]
    /// A tab in a preformatted block reaches `w:t` as a tab character.
    ///
    /// Pinned rather than endorsed. `WordprocessingML` represents a tab stop
    /// with `<w:tab/>`, and Word's own writer never puts a raw tab in run
    /// text, so a tab-indented code block may not align the way a
    /// space-indented one does. `escape_xml` passes tabs through deliberately,
    /// and a block is one run per line, so emitting `<w:tab/>` would mean
    /// splitting the line. Left as is until a real page says it is wrong —
    /// this test is here so the change is noticed when it happens.
    fn a_tab_in_a_preformatted_block_survives_as_a_tab() {
        let runs = only("```\n\tlet x = 1;\n```");
        assert!(runs.contains(">\tlet x = 1;<"), "{runs:?}");
    }

    #[test]
    /// A pipe table keeps its rows on separate lines, which is the whole of
    /// what makes it readable as a table.
    fn a_code_block_falls_back_line_by_line() {
        let runs = only("```\nlet x = 1;\nlet y = 2;\n```");
        assert_eq!(runs.matches("<w:r><w:br/></w:r>").count(), 3, "{runs}");
        assert!(runs.contains(">let x = 1;<"), "{runs}");
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
        let rendered = rendered("intro\n\n    let x = 1;").expect("renders");
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
        let rendered = rendered("text\n\n    indented code").expect("renders");
        assert_eq!(rendered.len(), 2, "{rendered:?}");
        let block = rendered.last().expect("two paragraphs");
        // The text of the first run, whatever `w:rPr` it carries: the block
        // is set preformatted, so asserting the bare `<w:r><w:t>` shape here
        // would pin the face rather than the boundary this test is about.
        let first = block
            .runs
            .split_once("</w:t>")
            .and_then(|(head, _)| head.rsplit_once('>'))
            .map_or_else(String::new, |(_, text)| text.to_owned());
        assert_eq!(
            first, "    indented code",
            "the paragraph bled into the block: {block:?}"
        );
    }

    #[test]
    /// A gap between two blocks starts with the newline that ended the last
    /// one. Printing it would open the paragraph with a blank line the
    /// author never wrote.
    fn a_gap_does_not_open_with_a_blank_line() {
        let rendered = rendered("a\n\n[ref]: https://a.example\n\nb").expect("renders");
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
            rendered("See [the syllabus][syl].\n\n[syl]: https://a.example/s").expect("renders");
        let printed: String = rendered.iter().map(|block| block.runs.clone()).collect();
        assert!(printed.contains("[the syllabus][syl]"), "{rendered:?}");
        assert!(printed.contains("https://a.example/s"), "{rendered:?}");
    }

    #[test]
    /// A heading is a heading, not a paragraph beginning with a hash.
    fn headings_carry_their_level() {
        let kinds: Vec<String> = rendered("# One\n\n### Three\n\nprose")
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
        let mut rendered = rendered("## See ![fig](f.png)").expect("renders");
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
        assert_eq!(rendered("one\n\ntwo").expect("renders").len(), 2);
    }

    #[test]
    /// A `\r` cannot survive in `<w:t>`: XML line-ending normalisation
    /// rewrites it on read, so the document would not round-trip itself.
    fn carriage_returns_do_not_reach_the_document() {
        let rendered = rendered("one\r\n\r\ntwo\rthree").expect("renders");
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
        assert!(rendered("").expect("renders").is_empty());
        assert!(rendered("   \n\n  ").expect("renders").is_empty());
    }

    #[test]
    /// A supplied image is placed as a drawing, carrying its alt text as the
    /// description a screen reader announces.
    fn a_supplied_image_becomes_a_drawing() {
        let runs = only_with_figure("Look: ![a red maple](figure.png)");
        assert!(runs.contains("<w:drawing>"), "{runs}");
        assert!(runs.contains(r#"descr="a red maple""#), "{runs}");
        assert!(runs.contains(r#"r:embed="rIdImage1""#), "{runs}");
    }

    #[test]
    /// The alt text describes the picture; it is not also printed beside it.
    /// The walk steps over the whole image span, and one that did not would
    /// set every caption twice — once as a description, once as prose.
    fn alt_text_is_not_printed_as_well_as_described() {
        let runs = only_with_figure("![a red maple](figure.png)");
        assert!(runs.contains(r#"descr="a red maple""#), "{runs}");
        assert!(!runs.contains("<w:t"), "the alt text printed too: {runs}");
    }

    #[test]
    /// Text either side of an image keeps its place, and is rendered once.
    fn text_around_an_image_survives_it() {
        let runs = only_with_figure("before ![f](figure.png) after");
        let text: Vec<&str> = runs.split("<w:t").skip(1).collect();
        assert_eq!(text.len(), 2, "{runs}");
        assert!(runs.contains(">before <"), "{runs}");
        assert!(runs.contains("> after<"), "{runs}");
    }

    #[test]
    /// An image nobody supplied bytes for falls back to its Markdown source,
    /// so the path stays visible on the page for the author to chase.
    fn an_unsupplied_image_falls_back_to_its_source() {
        let mut rendered = rendered_with("![fig](missing.png)", &figure()).expect("renders");
        let runs = rendered.pop().map(|block| block.runs).unwrap_or_default();
        assert!(!runs.contains("<w:drawing>"), "{runs}");
        assert!(runs.contains("![fig](missing.png)"), "{runs}");
    }

    #[test]
    /// A remote image falls back too: the library never fetches, so the URL
    /// prints rather than vanishing.
    fn a_remote_image_falls_back_to_its_source() {
        let source = "![fig](https://example.test/f.png)";
        let mut rendered = rendered_with(source, &figure()).expect("renders");
        let runs = rendered.pop().map(|block| block.runs).unwrap_or_default();
        assert!(!runs.contains("<w:drawing>"), "{runs}");
        assert!(runs.contains("example.test"), "{runs}");
    }

    #[test]
    /// An image in a list item is placed, and the item keeps its numbering.
    /// The list is the case that used to send a whole list to source because
    /// one item held a picture.
    fn an_image_in_a_list_item_is_placed() {
        let rendered =
            rendered_with("- see ![f](figure.png)\n- and this", &figure()).expect("renders");
        assert_eq!(rendered.len(), 2, "{rendered:?}");
        let first = rendered.first().expect("an item");
        assert!(matches!(first.kind, Kind::Item(_)), "{first:?}");
        assert!(first.runs.contains("<w:drawing>"), "{first:?}");
    }

    #[test]
    /// Math beside an image that *can* be placed is rendered, not refused.
    /// The refusal exists because the fallback prints LaTeX as source; once
    /// the paragraph no longer falls back, there is nothing to refuse.
    fn math_beside_a_placed_image_is_rendered() {
        let runs = only_with_figure(r"Given ![f](figure.png), find $x^2$.");
        assert!(runs.contains("<w:drawing>"), "{runs}");
        assert!(runs.contains("<m:oMath>"), "{runs}");
    }

    #[test]
    /// Math beside an image that cannot be placed is still refused. The
    /// paragraph falls back to source, which is the path that would print
    /// `$x^2$` on the page for a student to read as a question.
    fn math_beside_an_unplaceable_image_is_still_refused() {
        let refused = rendered_with(r"Given ![f](missing.png), find $x^2$.", &figure());
        assert!(
            matches!(refused, Err(Error::UnsupportedMath { .. })),
            "{refused:?}"
        );
    }

    #[test]
    /// Two images in one paragraph are both placed, each with its own
    /// relationship — the walk must not stop at the first.
    fn two_images_in_one_paragraph_are_both_placed() {
        let images = vec![
            (
                "one.png".to_owned(),
                super::super::media::tests::png(96, 96, None),
            ),
            (
                "two.png".to_owned(),
                super::super::media::tests::png(48, 48, None),
            ),
        ];
        let mut rendered =
            rendered_with("![a](one.png) then ![b](two.png)", &images).expect("renders");
        let runs = rendered.pop().map(|block| block.runs).unwrap_or_default();
        assert_eq!(runs.matches("<w:drawing>").count(), 2, "{runs}");
        assert!(runs.contains(r#"r:embed="rIdImage1""#), "{runs}");
        assert!(runs.contains(r#"r:embed="rIdImage2""#), "{runs}");
    }

    #[test]
    /// A paragraph that places an image and *then* falls back registers
    /// nothing. The runs are thrown away, so the bundled bytes have to go
    /// with them — otherwise the package carries a picture it never draws.
    fn an_image_in_a_paragraph_that_falls_back_is_not_registered() {
        let images = figure();
        let mut refs = Refs::new(&images);
        let source = "see ![f](figure.png) and [the rubric](https://x.example)";
        let rendered = paragraphs(source, &mut refs).expect("renders");
        let runs = rendered
            .first()
            .map(|block| block.runs.clone())
            .unwrap_or_default();
        assert!(!runs.contains("<w:drawing>"), "{runs}");
        assert!(runs.contains("![f](figure.png)"), "{runs}");
        assert!(
            refs.media.parts().is_empty(),
            "an image was bundled for a paragraph that prints as source"
        );
    }

    #[test]
    /// The same, for a list: one item's link sends the whole list to source,
    /// and another item's image must not stay bundled.
    fn an_image_in_a_list_that_falls_back_is_not_registered() {
        let images = figure();
        let mut refs = Refs::new(&images);
        let source = "- see ![f](figure.png)\n- read [this](https://x.example)";
        paragraphs(source, &mut refs).expect("renders");
        assert!(refs.media.parts().is_empty(), "the list's image stayed");
    }

    #[test]
    /// An image inside a link falls back with the link, and bundles nothing.
    fn an_image_wrapped_in_a_link_falls_back() {
        let images = figure();
        let mut refs = Refs::new(&images);
        let source = "[![alt](figure.png)](https://x.example)";
        let rendered = paragraphs(source, &mut refs).expect("renders");
        let runs = rendered
            .first()
            .map(|block| block.runs.clone())
            .unwrap_or_default();
        assert!(!runs.contains("<w:drawing>"), "{runs}");
        assert!(refs.media.parts().is_empty());
    }

    #[test]
    /// Alt text authored with emphasis reaches the description as its text.
    /// Dropping the whole alt because it held a `*` would leave the figure
    /// undescribed for the reader who most needs it described.
    fn marked_up_alt_text_becomes_its_plain_text() {
        let runs = only_with_figure("![a *red* maple](figure.png)");
        assert!(runs.contains(r#"descr="a red maple""#), "{runs}");
    }

    #[test]
    /// An image in a heading is placed, and the heading stays a heading.
    fn an_image_in_a_heading_is_placed() {
        let mut rendered = rendered_with("# See ![f](figure.png)", &figure()).expect("renders");
        let block = rendered.pop().expect("one paragraph");
        assert!(matches!(block.kind, Kind::Heading(1)), "{block:?}");
        assert!(block.runs.contains("<w:drawing>"), "{block:?}");
    }

    #[test]
    /// Bytes that are not a PNG leave the reference visible as source. This
    /// is what the signature check buys: an injected diagram renderer that
    /// ignored the format it was asked for lands SVG behind a `.png` path,
    /// and the author sees the path rather than a document that will not
    /// open.
    fn a_non_png_behind_a_png_path_falls_back_visibly() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>"#;
        let images = vec![("generated/mermaid-ab.png".to_owned(), svg.to_vec())];
        let mut rendered =
            rendered_with("![flow](generated/mermaid-ab.png)", &images).expect("renders");
        let runs = rendered.pop().map(|block| block.runs).unwrap_or_default();
        assert!(!runs.contains("<w:drawing>"), "{runs}");
        assert!(runs.contains("generated/mermaid-ab.png"), "{runs}");
    }

    #[test]
    /// An emphasised image is still an image. The marks around it apply to
    /// text, and a picture carries none — but the walk must not lose the
    /// picture while tracking them.
    fn an_image_inside_emphasis_is_still_placed() {
        let runs = only_with_figure("**![f](figure.png)**");
        assert!(runs.contains("<w:drawing>"), "{runs}");
    }
}
