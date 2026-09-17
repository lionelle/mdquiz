//! Render an [`Exam`] to a Word (`.docx`) document for printing.
//!
//! A `.docx` is a zip of XML parts, so this builds the same way the QTI package
//! does — as bytes, through the shared `zip_package` helper, with no process and no temporary
//! directory. That keeps the whole writer unit-testable.
//!
//! This module owns the *container*: the parts a Word document must have
//! before any question appears, plus the page furniture that is the same on
//! every sheet. Authored content is delegated — [`inline`] renders Markdown to
//! runs, [`omml`] renders LaTeX to math, and [`numbering`] owns the list
//! definitions a `w:numPr` resolves against. The per-question answer
//! structures arrive in a later change and slot into the body builder.
//!
//! # What Word actually requires
//!
//! Word refuses a package outright — "unreadable content" — rather than
//! degrading, so every part below is load-bearing:
//!
//! * `[Content_Types].xml` must declare a type for *every* part, by extension
//!   or by name.
//! * `_rels/.rels` points at the main document; `word/_rels/document.xml.rels`
//!   points at everything the document references, and every `r:id` used in the
//!   document must exist there.
//! * `w:sectPr` carries the page size, margins and the footer reference, and
//!   must be the last child of `w:body`.
//! * Children of `w:pPr` are a *sequence*, not a set: out-of-order elements are
//!   a schema violation even though they look harmless.

mod inline;
mod numbering;
pub mod omml;

use std::fmt::Write as _;

use crate::Result;
use crate::export::docx::inline::Kind;
use crate::export::docx::numbering::{Item, Numbering};
use crate::export::zip_package;
use crate::quiz::exam::{Exam, ExamItem};
use crate::quiz::spec::{TEMPLATE_CLOSE, TEMPLATE_OPEN, TemplateKey};

/// The OOXML main-document namespace.
const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// The OOXML math namespace, declared up front so later math work needs no
/// container change.
const M_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

/// The relationship namespace, used for `r:id` references.
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// The relationship id of the page footer.
const FOOTER_REL: &str = "rIdFooter";

/// Page geometry, all in twips (twentieths of a point).
///
/// US Letter with one-inch margins. `EDGE_GAP` is the distance from the paper
/// edge to the header and footer.
const PAGE_WIDTH: u32 = 12_240;
/// US Letter height.
const PAGE_HEIGHT: u32 = 15_840;
/// One-inch page margins.
const MARGIN: u32 = 1_440;

/// Twips (twentieths of a point) per blank line of answer space.
///
/// Word's default single line height for 11pt Calibri is about 14.65pt, i.e.
/// ~293 twips. 320 is a little over that, so a requested line is genuinely a
/// line with room to write in — a smaller value would quietly give the student
/// *less* space than the spec asked for.
const TWIPS_PER_LINE: u32 = 320;

/// Distance from the page edge to the header and footer.
const EDGE_GAP: u32 = 720;

/// The heading styles the document defines, as (style id, half-point size).
///
/// Three, not six: past the third level a printed exam has no useful
/// distinction left to draw, and deeper headings clamp onto the last one.
/// `w:sz` is in half-points, so 28 is 14pt.
///
/// The ids are OOXML's *built-in* heading ids, and [`heading_styles`] gives
/// each its built-in `w:name` (`heading 1`, lower-case and spaced) and a
/// `w:outlineLvl`. Word matches a style to its built-in by that name, so a
/// plausible-looking `Heading1` instead makes a private style that looks
/// right on the page but never reaches the navigation pane or a contents
/// table.
const HEADING_STYLES: [(&str, u32); 3] = [("Heading1", 28), ("Heading2", 26), ("Heading3", 24)];

/// The tallest answer block that can be laid out: the page's text column.
///
/// An exact line height taller than the column cannot be placed on any page.
/// `LibreOffice` spills it onto blank pages and Word clips it, so the two
/// disagree and neither is the sheet the author wanted.
const MAX_ANSWER_TWIPS: u32 = PAGE_HEIGHT - 2 * MARGIN;

/// Render `exam` as the bytes of a `.docx` file.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if a prompt contains math this
/// writer cannot convert, [`crate::Error::Export`] if the zip container cannot
/// be written, or [`crate::Error::Io`] from the underlying writer.
pub fn to_docx(exam: &Exam) -> Result<Vec<u8>> {
    let mut lists = Numbering::new();
    let document = document_xml(exam, &mut lists)?;
    let numbering = lists.to_xml();
    let footer = footer_xml(exam);
    let rels = document_rels();
    let styles = styles();
    let parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("word/_rels/document.xml.rels", rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/styles.xml", styles.as_bytes()),
        ("word/numbering.xml", numbering.as_bytes()),
        ("word/settings.xml", SETTINGS.as_bytes()),
        ("word/footer1.xml", footer.as_bytes()),
    ];
    zip_package("Word document", &parts)
}

/// Every part's content type. A part missing from here makes Word reject the
/// whole package.
const CONTENT_TYPES: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
    r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
    r#"<Default Extension="xml" ContentType="application/xml"/>"#,
    r#"<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#,
    r#"<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>"#,
    r#"<Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>"#,
    r#"<Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/>"#,
    r#"<Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/>"#,
    r"</Types>",
);

/// The package-level relationship: where the main document lives.
const ROOT_RELS: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>"#,
    r"</Relationships>",
);

/// What the document part references. Every `r:id` in `document.xml` must
/// appear here or Word rejects the file.
///
/// Built at runtime rather than as a `const` so [`FOOTER_REL`] is the single
/// source of that id: `concat!` takes literals only, which would mean spelling
/// it twice and relying on a test to catch them drifting apart.
fn document_rels() -> String {
    let relationship = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            r#"<Relationship Id="rIdStyles" Type="{0}/styles" Target="styles.xml"/>"#,
            r#"<Relationship Id="rIdNumbering" Type="{0}/numbering" Target="numbering.xml"/>"#,
            r#"<Relationship Id="rIdSettings" Type="{0}/settings" Target="settings.xml"/>"#,
            r#"<Relationship Id="{1}" Type="{0}/footer" Target="footer1.xml"/>"#,
            r"</Relationships>",
        ),
        relationship, FOOTER_REL
    )
}

/// Document defaults plus the named styles the writers rely on.
///
/// Built at runtime, like [`document_rels`], so [`inline::CODE_STYLE`] is the
/// single source of the code style's id: a `w:rStyle` naming a style that is
/// not defined here silently loses its formatting.
///
/// `w:sz` is in half-points: 22 is the 11pt body face that [`TWIPS_PER_LINE`]
/// reasons about, 32 the 16pt title. `w:spacing` is in twips, so 120 is 6pt and
/// 240 is 12pt.
///
/// Fonts are spelled inline because `concat!` takes literals only. Calibri is
/// the body face and Consolas the monospace one — Word has shipped both since
/// 2007, and a reader without them substitutes by class rather than failing.
fn styles() -> String {
    format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
            r#"<w:docDefaults><w:rPrDefault><w:rPr>"#,
            r#"<w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri"/>"#,
            r#"<w:sz w:val="22"/><w:szCs w:val="22"/>"#,
            r#"</w:rPr></w:rPrDefault></w:docDefaults>"#,
            r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal">"#,
            r#"<w:name w:val="Normal"/></w:style>"#,
            r#"<w:style w:type="character" w:styleId="{code}">"#,
            r#"<w:name w:val="{code}"/>"#,
            r#"<w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/></w:rPr>"#,
            r"</w:style>",
            "{title}{headings}",
            r"</w:styles>",
        ),
        code = inline::CODE_STYLE,
        title = paragraph_style(
            "Title",
            "Title",
            // `w:pPr` children are a sequence: `spacing` must precede `jc`.
            // Emitting them the other way round is a schema violation
            // `xmllint --noout` cannot see and Word rejects the document for.
            r#"<w:spacing w:after="240"/><w:jc w:val="center"/>"#,
            r#"<w:b/><w:sz w:val="32"/>"#,
        ),
        headings = heading_styles(),
    )
}

/// One paragraph style based on `Normal`.
///
/// `properties` and `run_properties` are inserted verbatim, and both are
/// schema *sequences* — see [`ParagraphStyle::properties`] for `w:pPr`'s
/// order and `inline`'s module docs for `w:rPr`'s.
fn paragraph_style(id: &str, name: &str, properties: &str, run_properties: &str) -> String {
    format!(
        concat!(
            r#"<w:style w:type="paragraph" w:styleId="{id}"><w:name w:val="{name}"/>"#,
            r#"<w:basedOn w:val="Normal"/><w:pPr>{properties}</w:pPr>"#,
            r#"<w:rPr>{run_properties}</w:rPr></w:style>"#,
        ),
        id = id,
        name = name,
        properties = properties,
        run_properties = run_properties,
    )
}

/// The definitions for every style in [`HEADING_STYLES`].
///
/// `w:keepNext` on all of them: a heading stranded at the foot of a page,
/// with the section it names overleaf, is the one layout fault a heading can
/// have. `w:outlineLvl` is 0-based where the heading level is 1-based.
fn heading_styles() -> String {
    HEADING_STYLES
        .iter()
        .enumerate()
        .fold(String::new(), |mut xml, (index, (id, size))| {
            let _ = write!(
                xml,
                "{}",
                paragraph_style(
                    id,
                    &format!("heading {}", index + 1),
                    // `w:outlineLvl` comes after `w:spacing` in `CT_PPrBase`.
                    &format!(
                        r#"<w:keepNext/><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="{index}"/>"#
                    ),
                    &format!(r#"<w:b/><w:sz w:val="{size}"/>"#),
                )
            );
            xml
        })
}

/// Document settings, carrying the math font.
///
/// Cambria Math is set once here rather than on every run, so equations render
/// consistently and the later math work needs no container change. The font
/// matters: OOXML builds stretchy delimiters, radicals and accents out of a
/// font's OpenType MATH table, so substituting a different face changes how
/// equations are drawn.
const SETTINGS: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
    r#"xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">"#,
    r#"<m:mathPr><m:mathFont m:val="Cambria Math"/></m:mathPr>"#,
    r"</w:settings>",
);

/// The whole `word/document.xml` part.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if a prompt, header or footer
/// contains math that cannot be converted.
fn document_xml(exam: &Exam, lists: &mut Numbering) -> Result<String> {
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W_NS}" xmlns:m="{M_NS}" xmlns:r="{R_NS}"><w:body>"#
    );
    xml.push_str(&body_xml(exam, lists)?);
    xml.push_str(&section_xml());
    xml.push_str("</w:body></w:document>");
    Ok(xml)
}

/// Everything above the closing section properties: title, header, questions,
/// footer.
///
/// Question rendering is deliberately shallow for now — a numbered prompt and
/// its answer space. The per-kind answer structures land with the question
/// writers.
fn body_xml(exam: &Exam, lists: &mut Numbering) -> Result<String> {
    let mut xml = paragraph(&inline::text_run(&exam.title()), ParagraphStyle::Title);
    if let Some(header) = &exam.header {
        xml.push_str(&prose(header, lists)?);
    }
    for (index, item) in exam.items.iter().enumerate() {
        xml.push_str(&question_xml(
            index + 1,
            item,
            exam.layout.page_break_between,
            lists,
        )?);
    }
    if let Some(footer) = &exam.footer {
        xml.push_str(&prose(footer, lists)?);
    }
    Ok(xml)
}

/// The question number and the prompt's first paragraph, if it has one.
///
/// The number normally shares that paragraph. It cannot when the prompt opens
/// with a list item: the item brings its own marker and indent, so the number
/// would print inside the first bullet — and, displacing
/// [`ParagraphStyle::Question`], take the question's leading space with it.
/// There the number gets a line of its own and the list starts underneath.
fn opening_paragraphs(lead: &str, first: Option<inline::Paragraph>) -> String {
    let Some(first) = first else {
        return paragraph(lead, ParagraphStyle::Question);
    };
    if matches!(first.kind, Kind::Item(_)) {
        return paragraph(lead, ParagraphStyle::Question)
            + &paragraph(
                &first.runs,
                ParagraphStyle::Continuation.or_block(first.kind),
            );
    }
    // `runs`, not `centred`: the number shares this line, and setting a
    // display equation apart would take the number to the middle of the page
    // with it.
    paragraph(
        &(lead.to_owned() + &first.runs),
        ParagraphStyle::Question.or_block(first.kind),
    )
}

/// One numbered question: its prompt, then the blank space to answer in.
///
/// The number shares the first paragraph with the prompt rather than standing
/// alone, so a wrapped prompt still hangs off its own number — except where
/// [`opening_paragraphs`] cannot put it there.
fn question_xml(
    number: usize,
    item: &ExamItem,
    page_break: bool,
    lists: &mut Numbering,
) -> Result<String> {
    let mut xml = String::new();
    if page_break && number > 1 {
        xml.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
    }
    let mut prompt = inline::paragraphs(&item.question.prompt, lists)?.into_iter();
    let lead = inline::text_run(&format!("{number}. "));
    xml.push_str(&opening_paragraphs(&lead, prompt.next()));
    for block in prompt {
        let style = ParagraphStyle::Continuation.or_block(block.kind);
        xml.push_str(&paragraph(&block.centred(), style));
    }
    xml.push_str(&answer_space(item.answer_space));
    Ok(xml)
}

/// A blank run of `lines` for the student to write in.
///
/// One empty paragraph with an exact line height, rather than `lines` empty
/// paragraphs: an exact height makes the paragraph a single line box, and a
/// line box is indivisible, so the writing room is never broken across a page.
/// What keeps it *with* its question is not this paragraph but the
/// `w:keepNext` on [`ParagraphStyle::Question`]; remove that and the space
/// migrates to the next page on its own, orphaning the prompt.
///
/// The height is capped at [`MAX_ANSWER_TWIPS`] — see there for why.
fn answer_space(lines: usize) -> String {
    if lines == 0 {
        return String::new();
    }
    // Saturating, then clamped: `lines` is an unbounded `usize` off authored
    // YAML, and a plain multiply overflows — and panics in debug — long before
    // the conversion would fail.
    let height = u32::try_from(lines)
        .unwrap_or(u32::MAX)
        .saturating_mul(TWIPS_PER_LINE)
        .min(MAX_ANSWER_TWIPS);
    format!(
        r#"<w:p><w:pPr><w:spacing w:line="{height}" w:lineRule="exact" w:after="120"/></w:pPr><w:r><w:t/></w:r></w:p>"#
    )
}

/// Render an already-loaded Markdown block as body paragraphs.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if the block contains math that
/// cannot be converted.
fn prose(markdown: &str, lists: &mut Numbering) -> Result<String> {
    Ok(inline::paragraphs(markdown, lists)?
        .iter()
        // An empty *list item* is kept: it is a bullet with nothing after
        // it, and dropping it renumbers everything below. Anything else with
        // no runs is a blank paragraph nobody authored.
        .filter(|block| !block.runs.is_empty() || matches!(block.kind, Kind::Item(_)))
        .map(|block| paragraph(&block.centred(), ParagraphStyle::Body.or_block(block.kind)))
        .collect())
}

/// How a paragraph is presented.
#[derive(Debug, Clone, Copy)]
enum ParagraphStyle {
    /// The exam title, centred and large.
    Title,
    /// Ordinary prose.
    Body,
    /// A question prompt: kept whole so it does not straddle a page break.
    Question,
    /// A prompt's second and later paragraphs: kept with the first, and
    /// separated from it, but without the wide gap that opens a question.
    Continuation,
    /// A Markdown heading, at the given level.
    Heading(u8),
    /// A list item, in the list [`Item`] names.
    ///
    /// `sticky` is inherited from the style the caller asked for, so a list
    /// inside a prompt holds together with its answer space while one in the
    /// exam's header is free to break across a page.
    Item {
        /// Which list the paragraph counts in, and how it is marked.
        item: Item,
        /// Whether the item is held to the paragraph after it.
        sticky: bool,
    },
}

impl ParagraphStyle {
    /// This style, or the one `kind` demands instead.
    ///
    /// A heading and a list item bring their own `w:pPr`; everything else
    /// keeps the style the caller chose. The heading styles carry
    /// `w:keepNext` of their own, and a list item inherits stickiness from
    /// the style it displaces, so either used partway through a prompt still
    /// holds together with what follows it.
    const fn or_block(self, kind: Kind) -> Self {
        match kind {
            Kind::Heading(level) => Self::Heading(level),
            Kind::Item(item) => Self::Item {
                item,
                sticky: self.is_sticky(),
            },
            Kind::Prose | Kind::Equation => self,
        }
    }

    /// Whether a list item displacing this style should inherit its
    /// `w:keepNext`.
    ///
    /// Only the styles carrying `w:keepNext` *inline* answer yes. A heading
    /// keeps with what follows too, but through its style definition, and
    /// `or_block` replaces a heading outright rather than asking — so a
    /// heading is never the `self` here.
    const fn is_sticky(self) -> bool {
        matches!(self, Self::Question | Self::Continuation)
    }

    /// The `w:pPr` contents for this style, in schema order.
    fn properties(self) -> String {
        match self {
            Self::Title => r#"<w:pStyle w:val="Title"/>"#.to_owned(),
            Self::Body => r#"<w:spacing w:after="120"/>"#.to_owned(),
            Self::Question => format!(r#"{STICKY}<w:spacing w:before="240"/>"#),
            Self::Continuation => {
                format!(r#"{STICKY}<w:spacing w:before="120" w:after="120"/>"#)
            }
            Self::Heading(level) => format!(r#"<w:pStyle w:val="{}"/>"#, heading_style(level)),
            Self::Item { item, sticky } => item_properties(item, sticky),
        }
    }
}

/// The `w:pPr` tags that hold a paragraph to the one after it.
///
/// Named because [`ParagraphStyle::is_sticky`] reports which styles carry
/// them: spelling them out at each site lets a style gain `w:keepNext` while
/// `is_sticky` goes on saying it has none, and a list item in a prompt then
/// silently stops keeping with its answer space.
const STICKY: &str = "<w:keepNext/><w:keepLines/>";

/// The `w:pPr` of one list-item paragraph.
///
/// The marker and the indent are [`Item`]'s to describe; what belongs here is
/// the order they go in. `CT_PPrBase` is a sequence — `keepNext`, `keepLines`,
/// `numPr`, `spacing`, `ind` — so the container's own spacing is interleaved
/// between the two pieces rather than appended after them.
fn item_properties(item: Item, sticky: bool) -> String {
    let mut properties = if sticky {
        STICKY.to_owned()
    } else {
        String::new()
    };
    properties.push_str(&item.marker());
    properties.push_str(r#"<w:spacing w:after="60"/>"#);
    properties.push_str(&item.indent());
    properties
}

/// The style id for a Markdown heading at `level`.
///
/// Levels below the deepest defined style are clamped rather than dropped: a
/// `w:pStyle` naming a style the document does not define is ignored, so an
/// `######` heading would print as plain body text.
fn heading_style(level: u8) -> &'static str {
    let deepest = HEADING_STYLES.len().saturating_sub(1);
    let index = usize::from(level).saturating_sub(1).min(deepest);
    HEADING_STYLES.get(index).map_or("", |(id, _)| *id)
}

/// One paragraph holding already-rendered `runs`.
fn paragraph(runs: &str, style: ParagraphStyle) -> String {
    format!("<w:p><w:pPr>{}</w:pPr>{runs}</w:p>", style.properties())
}

/// The section properties: page geometry and the footer reference.
///
/// Must be the final child of `w:body`.
fn section_xml() -> String {
    // Named arguments, not positional: `concat!` rules out inline captures, and
    // a run of bare `{}` across four lines is where a transposed width/height
    // or a margin in the wrong slot hides.
    format!(
        concat!(
            r#"<w:sectPr><w:footerReference w:type="default" r:id="{footer}"/>"#,
            r#"<w:pgSz w:w="{width}" w:h="{height}"/>"#,
            r#"<w:pgMar w:top="{margin}" w:right="{margin}" w:bottom="{margin}" "#,
            r#"w:left="{margin}" w:header="{gap}" w:footer="{gap}" w:gutter="0"/>"#,
            r"</w:sectPr>",
        ),
        footer = FOOTER_REL,
        width = PAGE_WIDTH,
        height = PAGE_HEIGHT,
        margin = MARGIN,
        gap = EDGE_GAP,
    )
}

/// The repeating page footer.
///
/// `${page}` and `${pages}` are not text substitution: they become OOXML
/// *fields*, which Word and `LibreOffice` evaluate per printed page. `${name}`
/// and `${variant}` are ordinary text and are substituted here.
fn footer_xml(exam: &Exam) -> String {
    // No guard for an absent footer: `footer_runs` over an empty template
    // produces no runs, which is exactly the empty paragraph we want.
    let runs = footer_runs(exam.layout.page_footer.as_deref().unwrap_or_default(), exam);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:ftr xmlns:w="{W_NS}"><w:p><w:pPr><w:jc w:val="center"/></w:pPr>{runs}</w:p></w:ftr>"#
    )
}

/// Expand a page-footer template into runs, turning `${page}`/`${pages}` into
/// fields and everything else into text.
fn footer_runs(template: &str, exam: &Exam) -> String {
    let mut runs = String::new();
    let mut rest = template;
    while let Some((before, after_open)) = rest.split_once(TEMPLATE_OPEN) {
        runs.push_str(&inline::text_run(before));
        let Some((key, after_close)) = after_open.split_once(TEMPLATE_CLOSE) else {
            // Unterminated. `rest` still holds the text before the `${`, so it
            // must be advanced past the marker or the trailing push below
            // emits that text a second time.
            runs.push_str(&inline::text_run(TEMPLATE_OPEN));
            rest = after_open;
            break;
        };
        runs.push_str(&placeholder_run(key.trim(), exam));
        rest = after_close;
    }
    runs.push_str(&inline::text_run(rest));
    runs
}

/// The runs for one `${...}` placeholder.
///
/// An unrecognised key renders as nothing, rather than leaking its own name
/// onto every printed page. That is a fallback, not a contract: an [`Exam`]
/// built through [`Spec`](crate::quiz::spec::Spec) cannot carry one, because
/// spec validation rejects unknown keys — but this function is reachable from
/// any hand-built [`Exam`], so it does not assume that.
fn placeholder_run(key: &str, exam: &Exam) -> String {
    // Matched exhaustively on the shared key type, so adding a placeholder to
    // `TemplateKey` fails to compile here rather than silently printing nothing.
    match TemplateKey::parse(key) {
        Some(TemplateKey::Name) => inline::text_run(&exam.name),
        Some(TemplateKey::Variant) => inline::text_run(exam.variant.as_deref().unwrap_or_default()),
        Some(TemplateKey::Page) => field_run("PAGE"),
        Some(TemplateKey::Pages) => field_run("NUMPAGES"),
        None => String::new(),
    }
}

/// One field run, e.g. `PAGE` or `NUMPAGES`.
///
/// A field is five runs: a begin marker, the instruction, a separator, a cached
/// value for readers that do not evaluate fields themselves, and an end marker.
fn field_run(instruction: &str) -> String {
    format!(
        concat!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
            r#"<w:r><w:instrText xml:space="preserve"> {} </w:instrText></w:r>"#,
            r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
            r"<w:r><w:t>1</w:t></w:r>",
            r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        ),
        instruction
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Feedback, Question, QuestionKind, TrueFalse};
    use crate::quiz::spec::Layout;
    use std::io::Cursor;

    /// A page-footer template exercising every placeholder. Held as a const so
    /// the `${...}` markers are not mistaken for format arguments.
    const FOOTER_TEMPLATE: &str = "${name} (${variant}) — Page ${page} of ${pages}";

    /// An exam with `items` questions and the given variant/header/footer.
    fn exam(variant: Option<&str>, items: usize) -> Exam {
        Exam {
            name: "CS 3500 — Exam 1".to_owned(),
            variant: variant.map(ToOwned::to_owned),
            header: Some("Answer every question.\n\nShow your working.".to_owned()),
            footer: Some("End of exam.".to_owned()),
            layout: Layout {
                page_footer: Some(FOOTER_TEMPLATE.to_owned()),
                ..Layout::default()
            },
            items: (0..items)
                .map(|n| ExamItem {
                    question: Question {
                        id: format!("q{n}"),
                        title: None,
                        prompt: format!("Question {n} & <prompt>"),
                        points: 1.0,
                        tags: Vec::new(),
                        feedback: Feedback::default(),
                        kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
                    },
                    answer_space: 3,
                })
                .collect(),
        }
    }

    /// The package's part names.
    fn part_names(bytes: &[u8]) -> Vec<String> {
        let archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
        archive.file_names().map(ToOwned::to_owned).collect()
    }

    /// One part's contents as text.
    fn part(bytes: &[u8], name: &str) -> String {
        use std::io::Read as _;
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
        let mut file = archive.by_name(name).expect("part present");
        let mut text = String::new();
        file.read_to_string(&mut text).expect("part is text");
        text
    }

    #[test]
    /// Every part Word requires is present. A missing one makes Word reject the
    /// whole document rather than degrade, so this is the floor.
    fn the_package_holds_every_required_part() {
        let bytes = to_docx(&exam(Some("A"), 2)).expect("renders");
        let names = part_names(&bytes);
        for required in [
            "[Content_Types].xml",
            "_rels/.rels",
            "word/_rels/document.xml.rels",
            "word/document.xml",
            "word/styles.xml",
            "word/numbering.xml",
            "word/settings.xml",
            "word/footer1.xml",
        ] {
            assert!(
                names.iter().any(|name| name == required),
                "missing {required}"
            );
        }
    }

    #[test]
    /// Every part has a content type, by extension or by name. This is the
    /// check Word applies before it will open the file at all.
    fn every_part_has_a_content_type() {
        let bytes = to_docx(&exam(None, 1)).expect("renders");
        let types = part(&bytes, "[Content_Types].xml");
        for name in part_names(&bytes) {
            if name == "[Content_Types].xml" {
                continue;
            }
            let extension = name
                .rsplit_once('.')
                .map(|(_, ext)| ext)
                .unwrap_or_default();
            let by_extension = types.contains(&format!(r#"Extension="{extension}""#));
            let by_name = types.contains(&format!(r#"PartName="/{name}""#));
            assert!(by_extension || by_name, "no content type declares {name}");
        }
    }

    #[test]
    /// Every relationship the document references exists, and every
    /// relationship points at a part that is actually in the package. A
    /// dangling `r:id` is the classic "unreadable content" cause.
    fn relationships_resolve_in_both_directions() {
        let bytes = to_docx(&exam(Some("B"), 1)).expect("renders");
        let document = part(&bytes, "word/document.xml");
        let rels = part(&bytes, "word/_rels/document.xml.rels");
        let names = part_names(&bytes);
        // Each `r:id` used in the document is declared.
        for reference in document.split(r#"r:id=""#).skip(1) {
            let id = reference.split('"').next().unwrap_or_default();
            assert!(
                rels.contains(&format!(r#"Id="{id}""#)),
                "undeclared r:id {id}"
            );
        }
        // Each declared target exists as a part.
        let mut declared = 0_usize;
        for entry in rels.split(r#"Target=""#).skip(1) {
            declared += 1;
            let target = entry.split('"').next().unwrap_or_default();
            let path = format!("word/{target}");
            assert!(names.contains(&path), "missing target {path}");
        }
        assert!(declared >= 4, "every document part should be declared");
    }

    #[test]
    /// The section properties close the body and carry the page furniture.
    fn the_section_closes_the_body_with_page_setup() {
        let bytes = to_docx(&exam(None, 1)).expect("renders");
        let document = part(&bytes, "word/document.xml");
        assert!(document.ends_with("</w:sectPr></w:body></w:document>"));
        assert!(document.contains(r#"<w:pgSz w:w="12240" w:h="15840"/>"#));
        assert!(document.contains(r#"w:type="default" r:id="rIdFooter""#));
    }

    #[test]
    /// Page numbers are OOXML fields, not text: they have to count real pages,
    /// which the writer cannot know.
    fn page_numbers_are_fields_not_text() {
        let bytes = to_docx(&exam(Some("C"), 1)).expect("renders");
        let footer = part(&bytes, "word/footer1.xml");
        assert!(footer.contains("<w:instrText xml:space=\"preserve\"> PAGE </w:instrText>"));
        assert!(footer.contains("<w:instrText xml:space=\"preserve\"> NUMPAGES </w:instrText>"));
        // ...while the literal placeholders are substituted as text.
        assert!(footer.contains("CS 3500 — Exam 1"));
        // Each placeholder becomes its own run, so the substituted values are
        // not contiguous in the XML — assert on the run, not the sentence.
        assert!(
            footer.contains(r#"<w:t xml:space="preserve">C</w:t>"#),
            "the variant label is missing: {footer}"
        );
        assert!(!footer.contains("${"), "a placeholder reached the page");
    }

    #[test]
    /// A quiz with no page footer configured still produces a valid footer part
    /// rather than a dangling relationship.
    fn an_absent_page_footer_still_yields_a_valid_part() {
        let mut quiz = exam(None, 1);
        quiz.layout.page_footer = None;
        let bytes = to_docx(&quiz).expect("renders");
        let footer = part(&bytes, "word/footer1.xml");
        assert!(footer.contains("<w:ftr"));
        assert!(footer.ends_with("</w:ftr>"));
    }

    #[test]
    /// Authored text is XML-escaped, so a prompt containing markup cannot
    /// corrupt the document.
    fn authored_text_is_escaped() {
        let bytes = to_docx(&exam(None, 1)).expect("renders");
        let document = part(&bytes, "word/document.xml");
        assert!(document.contains("Question 0 &amp; &lt;prompt&gt;"));
        assert!(!document.contains("<prompt>"));
    }

    #[test]
    /// Answer space is one exact-height paragraph rather than several empty
    /// ones, so the writing room cannot be split from its question by a page
    /// break. Zero lines produces none at all.
    fn answer_space_is_a_single_sized_paragraph() {
        assert_eq!(answer_space(0), "");
        let three = answer_space(3);
        assert!(three.contains(r#"w:line="960""#), "{three}");
        assert!(three.contains(r#"w:lineRule="exact""#));
        assert_eq!(three.matches("<w:p>").count(), 1);
    }

    #[test]
    /// An absurd answer space neither panics nor overflows: it is capped at the
    /// page's text column, because a line box taller than the column cannot be
    /// laid out on any page.
    fn answer_space_is_capped_rather_than_overflowing() {
        // A literal, not `PAGE_HEIGHT - 2 * MARGIN` again: an expectation
        // written as the constant's own expression agrees with it however it
        // is defined, including a cap taller than the paper it must fit on.
        assert_eq!(
            MAX_ANSWER_TWIPS, 12_960,
            "the cap is no longer US Letter less its margins"
        );
        let capped = format!(r#"w:line="{MAX_ANSWER_TWIPS}""#);
        assert!(answer_space(usize::MAX).contains(&capped));
        assert!(answer_space(100_000).contains(&capped));
        // A sane request is untouched.
        assert!(answer_space(4).contains(r#"w:line="1280""#));
    }

    #[test]
    /// An unterminated `${` emits the text before it exactly once and leaves
    /// the marker visible, rather than silently repeating the preceding run.
    fn an_unterminated_placeholder_does_not_duplicate_text() {
        let mut quiz = exam(None, 1);
        quiz.layout.page_footer = Some("Sheet ${name} tail ${oops".to_owned());
        let footer = part(&to_docx(&quiz).expect("renders"), "word/footer1.xml");
        assert_eq!(footer.matches(" tail ").count(), 1, "{footer}");
    }

    #[test]
    /// A header authored with CRLF still splits into paragraphs, and no stray
    /// carriage return reaches the XML.
    fn crlf_blocks_split_into_paragraphs() {
        let mut quiz = exam(None, 1);
        quiz.header = Some("Para one.\r\n\r\nPara two.".to_owned());
        let document = part(&to_docx(&quiz).expect("renders"), "word/document.xml");
        assert!(document.contains("<w:t xml:space=\"preserve\">Para one.</w:t>"));
        assert!(document.contains("<w:t xml:space=\"preserve\">Para two.</w:t>"));
        assert!(
            !document.contains('\r'),
            "a carriage return reached the XML"
        );
    }

    /// The `CT_PPrBase` child sequence, in schema order. Only the elements this
    /// writer emits need listing; an unknown one fails the check loudly.
    const PPR_ORDER: [&str; 9] = [
        "w:pStyle",
        "w:keepNext",
        "w:keepLines",
        "w:pageBreakBefore",
        "w:numPr",
        "w:spacing",
        "w:ind",
        "w:jc",
        "w:outlineLvl",
    ];

    /// The *direct* `w:pPr` child elements of `xml`, in the order they appear.
    ///
    /// Direct children only: `w:numPr` carries `w:ilvl` and `w:numId` of its
    /// own, and those belong to `CT_NumPr` rather than to the `CT_PPrBase`
    /// sequence [`PPR_ORDER`] describes.
    fn ppr_children(xml: &str) -> Vec<Vec<String>> {
        xml.split("<w:pPr>")
            .skip(1)
            .filter_map(|rest| rest.split_once("</w:pPr>"))
            .map(|(block, _)| top_level_tags(block))
            .collect()
    }

    /// The names of the elements at depth zero in `xml`.
    fn top_level_tags(xml: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut depth = 0_usize;
        for piece in xml.split('<').skip(1) {
            let Some((tag, _)) = piece.split_once('>') else {
                continue;
            };
            let closing = tag.starts_with('/');
            let name = tag
                .trim_start_matches('/')
                .split([' ', '/'])
                .next()
                .unwrap_or_default();
            if !closing && depth == 0 && name.starts_with("w:") {
                names.push(name.to_owned());
            }
            if closing {
                depth = depth.saturating_sub(1);
            } else if !tag.ends_with('/') {
                depth = depth.saturating_add(1);
            }
        }
        names
    }

    #[test]
    /// `w:pPr` children are a schema *sequence*, not a set. Emitting them out
    /// of order is invalid OOXML that `xmllint --noout` cannot see and Word
    /// refuses the document for — so check the order here, in both the document
    /// and the styles part.
    ///
    /// Checked against an exam *with* a list as well as one without: a list
    /// item is the only paragraph that emits `w:numPr` or `w:ind` at all, so
    /// two of the nine positions go unvisited otherwise.
    fn paragraph_properties_follow_the_schema_sequence() {
        let mut listed = exam(Some("A"), 2);
        // A marked item, an unmarked continuation and a nested item: between
        // them they emit every `w:pPr` child the list writer can produce, and
        // an exam without a list leaves `w:numPr` and `w:ind` unvisited.
        listed.header = Some("- one\n\n  still one\n\n  - deeper".to_owned());
        let listed = to_docx(&listed).expect("renders");
        let document = part(&listed, "word/document.xml");
        assert!(
            document.contains("<w:numPr>") && document.contains("<w:ind "),
            "the fixture no longer lays a list out, so two of the nine \
             positions go unchecked again: {document}"
        );
        for bytes in [to_docx(&exam(Some("A"), 2)).expect("renders"), listed] {
            for name in ["word/document.xml", "word/styles.xml"] {
                assert_schema_order(&part(&bytes, name), name);
            }
        }
    }

    /// Assert every `w:pPr` in `xml` lists its children in `PPR_ORDER` order.
    fn assert_schema_order(xml: &str, name: &str) {
        for children in ppr_children(xml) {
            let positions: Vec<Option<usize>> = children
                .iter()
                .map(|child| PPR_ORDER.iter().position(|known| known == child))
                .collect();
            assert!(
                positions.iter().all(Option::is_some),
                "{name}: w:pPr child outside the known order: {children:?}"
            );
            let ordered = positions
                .windows(2)
                .all(|pair| matches!((pair.first(), pair.get(1)), (Some(a), Some(b)) if a < b));
            assert!(
                ordered,
                "{name}: w:pPr children out of schema order: {children:?}"
            );
        }
    }

    #[test]
    /// `w:pgMar` must carry all seven attributes; the schema marks every one
    /// required, and omitting any makes the document invalid.
    fn page_margins_carry_every_required_attribute() {
        let document = part(
            &to_docx(&exam(None, 1)).expect("renders"),
            "word/document.xml",
        );
        let margins = document
            .split_once("<w:pgMar ")
            .and_then(|(_, rest)| rest.split_once("/>"))
            .map(|(attrs, _)| attrs.to_owned())
            .unwrap_or_default();
        for attribute in [
            "top", "right", "bottom", "left", "header", "footer", "gutter",
        ] {
            assert!(
                margins.contains(&format!("w:{attribute}=")),
                "w:pgMar is missing {attribute}: {margins}"
            );
        }
    }

    /// The visible text of `xml`: every `w:t`, concatenated.
    ///
    /// Runs split at every formatting boundary, so a sentence that reads as
    /// one phrase on the page is several runs in the XML, and asserting on the
    /// raw markup would test where the writer happened to break them.
    fn text_of(xml: &str) -> String {
        xml.split("<w:t")
            // `<w:t>` and `<w:t …>`, but not `<w:tbl>` or the self-closing
            // `<w:t/>` that holds answer space open.
            .filter(|rest| rest.starts_with('>') || rest.starts_with(' '))
            .filter_map(|rest| rest.split_once('>'))
            .filter_map(|(_, body)| body.split_once("</w:t>"))
            .map(|(text, _)| text)
            .collect()
    }

    /// The `w:pPr` of the paragraph whose text begins with `lead`.
    fn properties_of(document: &str, lead: &str) -> String {
        document
            .split("<w:p><w:pPr>")
            .find(|block| text_of(block).starts_with(lead))
            .and_then(|block| block.split_once("</w:pPr>"))
            .map_or_else(String::new, |(properties, _)| properties.to_owned())
    }

    #[test]
    /// A prompt is sticky and prose is not. `w:keepNext` is the only thing
    /// holding the answer space on the same page as the question it belongs
    /// to; losing it, or putting it on body prose instead, orphans the prompt.
    fn the_prompt_paragraph_keeps_its_answer_space_with_it() {
        let document = part(
            &to_docx(&exam(None, 1)).expect("renders"),
            "word/document.xml",
        );
        let prompt = properties_of(&document, "1. Question 0");
        assert!(
            prompt.contains("<w:keepNext/>"),
            "prompt lost keepNext: {prompt}"
        );
        assert!(
            prompt.contains("<w:keepLines/>"),
            "prompt lost keepLines: {prompt}"
        );
        assert_eq!(
            properties_of(&document, "Answer every question."),
            r#"<w:spacing w:after="120"/>"#,
            "body prose should not be sticky: {document}"
        );
        assert_eq!(document.matches("<w:keepNext/>").count(), 1, "{document}");
    }

    #[test]
    /// The body opens with the title, and the title names the variant this
    /// sheet is — a student holding form B must be able to see that.
    fn the_body_opens_with_the_titled_variant() {
        let titled = part(
            &to_docx(&exam(Some("B"), 1)).expect("renders"),
            "word/document.xml",
        );
        assert!(
            titled.contains(concat!(
                r#"<w:body><w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr>"#,
                r#"<w:r><w:t xml:space="preserve">CS 3500 — Exam 1 (B)</w:t></w:r></w:p>"#
            )),
            "{titled}"
        );
        let single = part(
            &to_docx(&exam(None, 1)).expect("renders"),
            "word/document.xml",
        );
        assert!(
            single.contains(r#"<w:t xml:space="preserve">CS 3500 — Exam 1</w:t>"#),
            "{single}"
        );
    }

    #[test]
    /// The fields appear in the order the template asks for them. Swapping
    /// `PAGE` and `NUMPAGES` prints "Page 3 of 1" on every sheet, which no
    /// substring assertion on the two instructions can see.
    fn footer_fields_follow_the_template_order() {
        let footer = part(
            &to_docx(&exam(Some("C"), 1)).expect("renders"),
            "word/footer1.xml",
        );
        let fields: Vec<&str> = footer
            .split(r#"<w:instrText xml:space="preserve"> "#)
            .skip(1)
            .filter_map(|rest| rest.split_once(' ').map(|(name, _)| name))
            .collect();
        assert_eq!(fields, ["PAGE", "NUMPAGES"], "{footer}");
    }

    #[test]
    /// Every field is balanced. An unclosed field is well-formed XML, so
    /// `xmllint` passes it, but the reader swallows the rest of the footer
    /// into the field's result.
    fn every_field_is_balanced() {
        let footer = part(
            &to_docx(&exam(Some("C"), 1)).expect("renders"),
            "word/footer1.xml",
        );
        for marker in ["begin", "separate", "end"] {
            assert_eq!(
                footer
                    .matches(&format!(r#"w:fldCharType="{marker}""#))
                    .count(),
                2,
                "unbalanced {marker} markers: {footer}"
            );
        }
    }

    #[test]
    /// Each Word part is declared by name, not merely by extension. The
    /// `Default Extension="xml"` entry satisfies an extension check for every
    /// part in the package, so a missing `Override` is invisible to one — and
    /// a missing `Override` on the main document is exactly what makes Word
    /// call the file unreadable.
    fn each_word_part_declares_its_own_content_type() {
        let bytes = to_docx(&exam(None, 1)).expect("renders");
        let types = part(&bytes, "[Content_Types].xml");
        for (name, content_type) in [
            ("/word/document.xml", "wordprocessingml.document.main+xml"),
            ("/word/styles.xml", "wordprocessingml.styles+xml"),
            ("/word/numbering.xml", "wordprocessingml.numbering+xml"),
            ("/word/settings.xml", "wordprocessingml.settings+xml"),
            ("/word/footer1.xml", "wordprocessingml.footer+xml"),
        ] {
            let declared = types
                .split(r#"<Override PartName=""#)
                .skip(1)
                .filter_map(|entry| entry.split_once('"'))
                .any(|(declared, rest)| declared == name && rest.contains(content_type));
            assert!(
                declared,
                "no Override declares {name} as {content_type}: {types}"
            );
        }
    }

    #[test]
    /// The package relationship resolves too. Its target is relative to the
    /// package root rather than to `word/`, so the document-level check in
    /// `relationships_resolve_in_both_directions` never looks at it.
    fn the_package_relationship_points_at_a_real_part() {
        let bytes = to_docx(&exam(None, 1)).expect("renders");
        let names = part_names(&bytes);
        let root = part(&bytes, "_rels/.rels");
        let mut targets = 0_usize;
        for entry in root.split(r#"Target=""#).skip(1) {
            let target = entry.split('"').next().unwrap_or_default();
            targets += 1;
            assert!(
                names.iter().any(|name| name == target),
                "missing root target {target}"
            );
        }
        assert_eq!(targets, 1, "the package must name the main document");
    }

    #[test]
    /// A page break separates questions when asked for, and never precedes the
    /// first one.
    fn page_breaks_fall_between_questions_only() {
        let mut quiz = exam(None, 3);
        quiz.layout.page_break_between = true;
        let document = part(&to_docx(&quiz).expect("renders"), "word/document.xml");
        assert_eq!(document.matches(r#"<w:br w:type="page"/>"#).count(), 2);
    }

    #[test]
    /// A quiz with nothing in it still produces a document Word can open. The
    /// `None` branches for header, footer and an empty item list are otherwise
    /// never taken, because the fixture always fills them.
    fn an_empty_exam_still_renders_a_document() {
        let mut quiz = exam(None, 0);
        quiz.header = None;
        quiz.footer = None;
        let bytes = to_docx(&quiz).expect("renders");
        let document = part(&bytes, "word/document.xml");
        assert!(
            document.contains(r#"<w:pStyle w:val="Title"/>"#),
            "no title"
        );
        assert!(document.ends_with("</w:sectPr></w:body></w:document>"));
        assert_eq!(part_names(&bytes).len(), 8, "a part went missing");
    }

    #[test]
    /// Zero answer space leaves no blank paragraph behind, all the way through
    /// the document rather than just in the helper.
    fn zero_answer_space_emits_no_blank_paragraph() {
        let mut quiz = exam(None, 1);
        for item in &mut quiz.items {
            item.answer_space = 0;
        }
        let document = part(&to_docx(&quiz).expect("renders"), "word/document.xml");
        assert!(!document.contains(r#"w:lineRule="exact""#), "{document}");
    }

    #[test]
    /// A prompt that opens with a list keeps the question number out of the
    /// first bullet. The number shares the first prompt paragraph, so that
    /// paragraph carrying a `w:numPr` of its own prints "1. bubble sort"
    /// *inside* the item, indented under a bullet, and the question's own
    /// leading space goes with it.
    fn a_prompt_opening_with_a_list_numbers_the_question_not_the_bullet() {
        let document = part(
            &to_docx(&exam_asking("- bubble sort\n- merge sort")).expect("renders"),
            "word/document.xml",
        );
        let numbered = properties_of(&document, "1. ");
        assert!(
            !numbered.contains("<w:numPr>"),
            "the question number is inside a bullet: {document}"
        );
        assert!(
            numbered.contains(r#"<w:spacing w:before="240"/>"#),
            "the question lost its leading space: {document}"
        );
        assert_eq!(
            document.matches("<w:numPr>").count(),
            2,
            "an item lost its marker: {document}"
        );
    }

    #[test]
    /// A start value on a *nested* ordered list is overridden at the `w:ilvl`
    /// its items actually sit on. `w:lvlOverride` names a level, and one
    /// naming a level no item uses is ignored outright — the authored `5.`
    /// then prints as `a.`, which reads as a perfectly ordinary list.
    fn a_nested_ordered_lists_start_is_overridden_at_its_own_level() {
        let package = to_docx(&exam_asking("Steps:\n\n- outer\n\n  5. five")).expect("renders");
        let document = part(&package, "word/document.xml");
        let inner = document
            .split(r#"<w:ilvl w:val="1"/><w:numId w:val=""#)
            .nth(1)
            .and_then(|rest| rest.split_once('"'))
            .map_or_else(String::new, |(id, _)| id.to_owned());
        assert!(!inner.is_empty(), "no nested item at all: {document}");
        let numbering = part(&package, "word/numbering.xml");
        let instance = numbering
            .split(&format!(r#"<w:num w:numId="{inner}">"#))
            .nth(1)
            .and_then(|rest| rest.split_once("</w:num>"))
            .map_or_else(String::new, |(body, _)| body.to_owned());
        assert!(
            instance.contains(r#"<w:lvlOverride w:ilvl="1">"#),
            "the start is overridden at a level no item uses: {instance}"
        );
        assert!(
            instance.contains(r#"<w:startOverride w:val="5"/>"#),
            "{instance}"
        );
    }

    #[test]
    /// Two ordered lists in one document count separately. The counter lives
    /// on the `w:numId`, so sharing one makes the second continue the first —
    /// a header ending "1. 2." and a prompt opening "1." prints "3.". The
    /// end-to-end version of this lives behind `#[ignore]` in
    /// `tests/docx_package.rs`, so without this one the whole-document wiring
    /// for the module's headline invariant never runs in the gate.
    fn two_ordered_lists_do_not_continue_each_other() {
        let mut quiz = exam_asking("Then:\n\n1. weigh it\n2. record it");
        quiz.header = Some("1. read the rules\n2. sign the sheet".to_owned());
        let package = to_docx(&quiz).expect("renders");
        let document = part(&package, "word/document.xml");
        let mut ids = numbering_ids(&document);
        ids.dedup();
        assert_eq!(ids.len(), 2, "the two lists share a counter: {document}");
        let numbering = part(&package, "word/numbering.xml");
        for id in ids {
            assert!(
                numbering.contains(&format!(
                    r#"<w:num w:numId="{id}"><w:abstractNumId w:val="1"/>"#
                )),
                "numId {id} is not an ordered list: {numbering}"
            );
        }
    }

    #[test]
    /// An empty item in body prose keeps its paragraph. It is a marker with
    /// nothing after it, and dropping it as a runless paragraph shortens the
    /// list and renumbers everything below, so `1.` followed by `2. second`
    /// prints "second" as item 1.
    fn an_empty_list_item_is_not_filtered_out_of_the_body() {
        let mut quiz = exam(None, 1);
        quiz.header = Some("1.\n2. second\n3. third".to_owned());
        let document = part(&to_docx(&quiz).expect("renders"), "word/document.xml");
        assert_eq!(document.matches("<w:numPr>").count(), 3, "{document}");
    }

    #[test]
    /// A continuation paragraph inside a *nested* item is indented to its own
    /// level. Indenting every continuation to level 0 puts an inner item's
    /// second paragraph out at the outer list's margin, under the wrong
    /// marker — and the existing level-0 test passes either way.
    fn a_nested_continuation_is_indented_to_its_own_level() {
        let mut quiz = exam(None, 1);
        quiz.header = Some("- outer\n\n  - inner\n\n    still inner".to_owned());
        let document = part(&to_docx(&quiz).expect("renders"), "word/document.xml");
        let tail = properties_of(&document, "still inner");
        assert!(!tail.contains("<w:numPr>"), "a second marker: {tail}");
        assert!(tail.contains(r#"<w:ind w:left="1440"/>"#), "{tail}");
    }

    /// `(level, marked, sticky)` and the `w:pPr` it must produce, for list id
    /// 7. Spelled out rather than rebuilt from the writer's own pieces: an
    /// expectation assembled the way the code assembles it agrees with any
    /// bug the code has.
    const ITEM_SHAPES: [(u8, bool, bool, &str); 5] = [
        (
            0,
            true,
            true,
            concat!(
                r#"<w:keepNext/><w:keepLines/>"#,
                r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="7"/></w:numPr>"#,
                r#"<w:spacing w:after="60"/>"#,
            ),
        ),
        (
            3,
            true,
            false,
            concat!(
                r#"<w:numPr><w:ilvl w:val="3"/><w:numId w:val="7"/></w:numPr>"#,
                r#"<w:spacing w:after="60"/>"#,
            ),
        ),
        (
            0,
            false,
            false,
            r#"<w:spacing w:after="60"/><w:ind w:left="720"/>"#,
        ),
        (
            2,
            false,
            true,
            concat!(
                r#"<w:keepNext/><w:keepLines/>"#,
                r#"<w:spacing w:after="60"/><w:ind w:left="2160"/>"#,
            ),
        ),
        (
            8,
            false,
            false,
            r#"<w:spacing w:after="60"/><w:ind w:left="6480"/>"#,
        ),
    ];

    #[test]
    /// The shapes a list-item `w:pPr` comes in, asserted together. A marked
    /// paragraph takes its indent from the numbering definition and must not
    /// repeat it; an unmarked one has no definition to inherit from and must
    /// carry *its own level's* indent by hand. Stickiness is orthogonal to
    /// both, and `CT_PPrBase` fixes the order they appear in.
    fn an_item_paragraph_is_marked_or_indented_but_never_both() {
        for (level, marked, sticky, expected) in ITEM_SHAPES {
            let item = Item {
                list: 7,
                level,
                marked,
            };
            assert_eq!(
                item_properties(item, sticky),
                expected,
                "level {level}, marked {marked}, sticky {sticky}"
            );
        }
    }

    /// The exam of [`exam`] with its single prompt replaced by `prompt`.
    fn exam_asking(prompt: &str) -> Exam {
        let mut quiz = exam(None, 1);
        if let Some(item) = quiz.items.first_mut() {
            item.question.prompt = prompt.to_owned();
        }
        quiz
    }

    #[test]
    /// Math authored in a prompt reaches the page as math. Everything below
    /// `omml` is tested against LaTeX; this is the one test that the writer
    /// is actually wired to a prompt at all.
    fn math_in_a_prompt_reaches_the_document() {
        let quiz = exam_asking(r"Sort in $O(n \log n)$ time.");
        let document = part(&to_docx(&quiz).expect("renders"), "word/document.xml");
        assert!(document.contains("<m:oMath>"), "{document}");
        assert!(
            !text_of(&document).contains(r"\log"),
            "LaTeX source printed: {document}"
        );
    }

    #[test]
    /// Math the writer cannot convert fails the export outright rather than
    /// producing a sheet with a wrong or missing formula on it.
    fn unconvertible_math_fails_the_export() {
        let quiz = exam_asking(r"Evaluate $\begin{unknown}x\end{unknown}$.");
        assert!(matches!(
            to_docx(&quiz),
            Err(crate::Error::UnsupportedMath { .. })
        ));
    }

    #[test]
    /// A prompt spanning several paragraphs keeps its number on the first and
    /// holds the rest together: `w:keepNext` on every part is what stops the
    /// tail, and the answer space after it, sliding onto the next page.
    fn a_multi_paragraph_prompt_stays_together() {
        let document = part(
            &to_docx(&exam_asking("First half.\n\nSecond half.")).expect("renders"),
            "word/document.xml",
        );
        assert!(text_of(&document).contains("1. First half."), "{document}");
        let tail = properties_of(&document, "Second half.");
        assert!(tail.contains("<w:keepNext/>"), "tail lost keepNext: {tail}");
    }

    #[test]
    /// Every style a run or paragraph names must be defined, or Word drops
    /// the formatting silently — the document still opens, just wrong.
    fn every_style_referenced_is_defined() {
        let package = to_docx(&exam_asking("`code` and\n\n# A heading")).expect("renders");
        let styles = part(&package, "word/styles.xml");
        let used = referenced_styles(&part(&package, "word/document.xml"));
        // Named, not just counted: a scanner that silently found nothing
        // would assert over an empty list and pass. Containment rather than
        // equality, so adding a style elsewhere does not fail this test.
        for expected in ["Title", "Code", "Heading1"] {
            assert!(used.iter().any(|id| id == expected), "{used:?}");
        }
        for id in used {
            assert!(
                styles.contains(&format!(r#"w:styleId="{id}""#)),
                "{id} is used but not defined: {styles}"
            );
        }
    }

    /// Every style id the document references, from `w:pStyle` and `w:rStyle`.
    fn referenced_styles(document: &str) -> Vec<String> {
        document
            .split("Style w:val=\"")
            .skip(1)
            .filter_map(|rest| rest.split_once('"'))
            .map(|(id, _)| id.to_owned())
            .collect()
    }

    #[test]
    /// A prompt that is nothing but a display equation keeps its number at
    /// the margin. `m:oMathPara` centres everything on its line, so wrapping
    /// the paragraph that also holds "1." puts the number in the middle of
    /// the page.
    fn a_numbered_equation_is_not_centred() {
        let document = part(
            &to_docx(&exam_asking(r"$$\frac{a}{b}$$")).expect("renders"),
            "word/document.xml",
        );
        assert!(document.contains("<m:oMath>"), "the equation is missing");
        assert!(
            !document.contains("oMathPara"),
            "the question number was centred with the equation: {document}"
        );
    }

    #[test]
    /// A display equation in a later paragraph of a prompt *is* set apart:
    /// nothing else shares its line, so centring costs nothing.
    fn a_standalone_equation_is_centred() {
        let document = part(
            &to_docx(&exam_asking("Prove:\n\n$$e^{i\\pi} + 1 = 0$$")).expect("renders"),
            "word/document.xml",
        );
        assert!(document.contains("<m:oMathPara>"), "{document}");
    }

    #[test]
    /// A heading opening a prompt is set as one, exactly as a heading later
    /// in the same prompt is. Honouring it in one place and not the other
    /// made the same markup mean two things.
    fn a_heading_opening_a_prompt_keeps_its_level() {
        let document = part(
            &to_docx(&exam_asking("## Part A\n\nWhat is x?")).expect("renders"),
            "word/document.xml",
        );
        assert_eq!(
            properties_of(&document, "1. Part A"),
            r#"<w:pStyle w:val="Heading2"/>"#,
            "{document}"
        );
    }

    #[test]
    /// The heading styles are OOXML's built-ins, not private styles that
    /// merely look like them. Word matches by `w:name`, so `Heading1` where
    /// `heading 1` belongs gives a style that prints correctly but never
    /// reaches the navigation pane or a contents table.
    fn heading_styles_are_the_built_in_ones() {
        let styles = part(
            &to_docx(&exam(None, 1)).expect("renders"),
            "word/styles.xml",
        );
        for (level, id) in HEADING_STYLES.iter().enumerate() {
            let name = format!(r#"<w:name w:val="heading {}"/>"#, level + 1);
            assert!(styles.contains(&name), "{} lacks {name}: {styles}", id.0);
        }
        assert!(
            styles.contains(r#"<w:outlineLvl w:val="0"/>"#),
            "no outline level: {styles}"
        );
    }

    #[test]
    /// A list reaches the document as real Word numbering — a `w:numPr`
    /// pointing at a defined `w:num` — rather than as bullet characters
    /// typed into the text.
    fn a_list_becomes_word_numbering() {
        let package = to_docx(&exam_asking("- first\n- second")).expect("renders");
        let document = part(&package, "word/document.xml");
        assert_eq!(document.matches("<w:numPr>").count(), 2, "{document}");
        // Every id used must be defined, or Word opens the file with the
        // list silently flattened to plain paragraphs.
        let numbering = part(&package, "word/numbering.xml");
        for id in numbering_ids(&document) {
            assert!(
                numbering.contains(&format!(r#"<w:num w:numId="{id}">"#)),
                "numId {id} is used but not defined: {numbering}"
            );
        }
    }

    /// Every `w:numId` the document references.
    fn numbering_ids(document: &str) -> Vec<String> {
        numbering::tests::values_of(document, "numId")
    }

    #[test]
    /// An item's second paragraph carries no marker, and is indented by hand
    /// to where the marker would have put it. Without the indent it hangs
    /// out to the left of the item it belongs to.
    fn an_item_continuation_is_indented_but_unmarked() {
        let document = part(
            &to_docx(&exam_asking("- one\n\n  still one")).expect("renders"),
            "word/document.xml",
        );
        let tail = properties_of(&document, "still one");
        assert!(!tail.contains("<w:numPr>"), "a second marker: {tail}");
        assert!(tail.contains(r#"<w:ind w:left="720"/>"#), "{tail}");
    }

    #[test]
    /// A list inside a prompt is held to its answer space, exactly as the
    /// prompt's own paragraphs are; one in the header is free to break
    /// across a page.
    fn a_list_in_a_prompt_is_sticky_and_one_in_the_header_is_not() {
        let mut quiz = exam_asking("Pick one:\n\n- alpha");
        quiz.header = Some("- loose".to_owned());
        let document = part(&to_docx(&quiz).expect("renders"), "word/document.xml");
        assert!(
            properties_of(&document, "alpha").contains("<w:keepNext/>"),
            "{document}"
        );
        assert!(
            !properties_of(&document, "loose").contains("<w:keepNext/>"),
            "{document}"
        );
    }

    #[test]
    /// A placeholder written in a *prompt* is ordinary text. Only the page
    /// footer is a template; substituting in body text would let a question
    /// silently rewrite itself.
    fn a_placeholder_in_a_prompt_is_literal_text() {
        let mut quiz = exam(None, 1);
        if let Some(item) = quiz.items.first_mut() {
            item.question.prompt = "What does ${page} mean?".to_owned();
        }
        let document = part(&to_docx(&quiz).expect("renders"), "word/document.xml");
        assert!(
            text_of(&document).contains("What does ${page} mean?"),
            "{document}"
        );
        assert!(!document.contains("PAGE"), "a prompt became a field");
    }

    #[test]
    /// An unknown placeholder renders as nothing rather than printing its own
    /// name on every page. Spec validation rejects these, but a hand-built
    /// exam can still carry one.
    fn an_unknown_placeholder_renders_as_nothing() {
        let mut quiz = exam(None, 1);
        quiz.layout.page_footer = Some("A${bogus}B".to_owned());
        let footer = part(&to_docx(&quiz).expect("renders"), "word/footer1.xml");
        assert!(footer.contains(">A<"), "{footer}");
        assert!(footer.contains(">B<"), "{footer}");
        assert!(!footer.contains("bogus"), "the key leaked onto the page");
    }

    #[test]
    /// The header and footer blocks reach the document, split into paragraphs.
    fn header_and_footer_blocks_are_rendered() {
        let document = part(
            &to_docx(&exam(None, 1)).expect("renders"),
            "word/document.xml",
        );
        assert!(document.contains("Answer every question."));
        assert!(document.contains("Show your working."));
        assert!(document.contains("End of exam."));
    }
}
