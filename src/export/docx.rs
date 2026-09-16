//! Render an [`Exam`] to a Word (`.docx`) document for printing.
//!
//! A `.docx` is a zip of XML parts, so this builds the same way the QTI package
//! does — as bytes, through the shared `zip_package` helper, with no process and no temporary
//! directory. That keeps the whole writer unit-testable.
//!
//! This module owns the *container*: the parts a Word document must have before
//! any question appears, plus the page furniture that is the same on every
//! sheet. Question content, lists, tables and math arrive in later changes and
//! slot into the body builder.
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

use crate::Result;
use crate::export::{escape_xml, zip_package};
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
/// Returns [`crate::Error::Export`] if the zip container cannot be written, or
/// [`crate::Error::Io`] from the underlying writer.
pub fn to_docx(exam: &Exam) -> Result<Vec<u8>> {
    let document = document_xml(exam);
    let footer = footer_xml(exam);
    let rels = document_rels();
    let parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("word/_rels/document.xml.rels", rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/styles.xml", STYLES.as_bytes()),
        ("word/numbering.xml", NUMBERING.as_bytes()),
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

/// Document defaults plus the named styles later parts rely on.
///
/// `w:sz` is in half-points: 22 is the 11pt body face that [`TWIPS_PER_LINE`]
/// reasons about, 32 the 16pt title. `w:spacing` is in twips, so 120 is 6pt and
/// 240 is 12pt.
///
/// Fonts are spelled inline because `concat!` takes literals only. Calibri is
/// the body face; the monospace style preformatted blocks will need arrives
/// with them, since adding a style is a one-line change that needs no
/// coordinated edit elsewhere.
const STYLES: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
    r#"<w:docDefaults><w:rPrDefault><w:rPr>"#,
    r#"<w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri"/>"#,
    r#"<w:sz w:val="22"/><w:szCs w:val="22"/>"#,
    r#"</w:rPr></w:rPrDefault></w:docDefaults>"#,
    r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal">"#,
    r#"<w:name w:val="Normal"/></w:style>"#,
    r#"<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/>"#,
    // `w:pPr` children are a sequence: `spacing` must precede `jc`. Emitting
    // them the other way round is a schema violation `xmllint --noout` cannot
    // see and Word rejects the document for.
    r#"<w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="240"/>"#,
    r#"<w:jc w:val="center"/></w:pPr>"#,
    r#"<w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>"#,
    r"</w:styles>",
);

/// An empty numbering part.
///
/// Present from the start because lists need `numbering.xml` to exist *and* be
/// declared before any `w:numId` can resolve; adding the part later would mean
/// touching the content types and relationships again.
const NUMBERING: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>"#,
);

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
fn document_xml(exam: &Exam) -> String {
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W_NS}" xmlns:m="{M_NS}" xmlns:r="{R_NS}"><w:body>"#
    );
    xml.push_str(&body_xml(exam));
    xml.push_str(&section_xml());
    xml.push_str("</w:body></w:document>");
    xml
}

/// Everything above the closing section properties: title, header, questions,
/// footer.
///
/// Question rendering is deliberately shallow for now — a numbered prompt and
/// its answer space. The per-kind answer structures land with the question
/// writers.
fn body_xml(exam: &Exam) -> String {
    let mut xml = paragraph(&exam.title(), ParagraphStyle::Title);
    if let Some(header) = &exam.header {
        xml.push_str(&markdown_block(header));
    }
    for (index, item) in exam.items.iter().enumerate() {
        xml.push_str(&question_xml(
            index + 1,
            item,
            exam.layout.page_break_between,
        ));
    }
    if let Some(footer) = &exam.footer {
        xml.push_str(&markdown_block(footer));
    }
    xml
}

/// One numbered question: its prompt, then the blank space to answer in.
fn question_xml(number: usize, item: &ExamItem, page_break: bool) -> String {
    let mut xml = String::new();
    if page_break && number > 1 {
        xml.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
    }
    let prompt = format!("{number}. {}", item.question.prompt);
    xml.push_str(&paragraph(&prompt, ParagraphStyle::Question));
    xml.push_str(&answer_space(item.answer_space));
    xml
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

/// Render an already-loaded Markdown block as plain paragraphs.
///
/// Inline formatting, lists and tables are the later writers' work; splitting
/// on blank lines is enough for a header or footer to reach the page intact.
fn markdown_block(markdown: &str) -> String {
    // Normalised first: a `header:` authored on Windows arrives CRLF, which
    // would not match the blank-line split and would collapse the whole block
    // into one paragraph. Stray `\r` also cannot survive inside `<w:t>`, which
    // XML line-ending normalisation rewrites on read.
    markdown
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .fold(String::new(), |mut xml, block| {
            xml.push_str(&paragraph(block, ParagraphStyle::Body));
            xml
        })
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
}

impl ParagraphStyle {
    /// The `w:pPr` contents for this style, in schema order.
    const fn properties(self) -> &'static str {
        match self {
            Self::Title => r#"<w:pStyle w:val="Title"/>"#,
            Self::Body => r#"<w:spacing w:after="120"/>"#,
            Self::Question => r#"<w:keepNext/><w:keepLines/><w:spacing w:before="240"/>"#,
        }
    }
}

/// One paragraph holding `text`.
fn paragraph(text: &str, style: ParagraphStyle) -> String {
    format!(
        "<w:p><w:pPr>{}</w:pPr>{}</w:p>",
        style.properties(),
        text_run(text)
    )
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
        push_text_run(&mut runs, before);
        let Some((key, after_close)) = after_open.split_once(TEMPLATE_CLOSE) else {
            // Unterminated. `rest` still holds the text before the `${`, so it
            // must be advanced past the marker or the trailing push below
            // emits that text a second time.
            push_text_run(&mut runs, TEMPLATE_OPEN);
            rest = after_open;
            break;
        };
        runs.push_str(&placeholder_run(key.trim(), exam));
        rest = after_close;
    }
    push_text_run(&mut runs, rest);
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
        Some(TemplateKey::Name) => text_run(&exam.name),
        Some(TemplateKey::Variant) => text_run(exam.variant.as_deref().unwrap_or_default()),
        Some(TemplateKey::Page) => field_run("PAGE"),
        Some(TemplateKey::Pages) => field_run("NUMPAGES"),
        None => String::new(),
    }
}

/// Append a text run for `text`, if there is any.
fn push_text_run(runs: &mut String, text: &str) {
    if !text.is_empty() {
        runs.push_str(&text_run(text));
    }
}

/// One literal text run.
fn text_run(text: &str) -> String {
    format!(
        r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
        escape_xml(text)
    )
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
    const PPR_ORDER: [&str; 6] = [
        "w:pStyle",
        "w:keepNext",
        "w:keepLines",
        "w:pageBreakBefore",
        "w:spacing",
        "w:jc",
    ];

    /// The `w:pPr` child elements of `xml`, in the order they appear.
    fn ppr_children(xml: &str) -> Vec<Vec<String>> {
        xml.split("<w:pPr>")
            .skip(1)
            .filter_map(|rest| rest.split_once("</w:pPr>"))
            .map(|(block, _)| {
                block
                    .split('<')
                    .filter_map(|tag| tag.split([' ', '/', '>']).next())
                    .filter(|name| name.starts_with("w:"))
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .collect()
    }

    #[test]
    /// `w:pPr` children are a schema *sequence*, not a set. Emitting them out
    /// of order is invalid OOXML that `xmllint --noout` cannot see and Word
    /// refuses the document for — so check the order here, in both the document
    /// and the styles part.
    fn paragraph_properties_follow_the_schema_sequence() {
        let bytes = to_docx(&exam(Some("A"), 2)).expect("renders");
        for name in ["word/document.xml", "word/styles.xml"] {
            for children in ppr_children(&part(&bytes, name)) {
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

    /// The `w:pPr` of the paragraph whose text begins with `lead`.
    fn properties_of(document: &str, lead: &str) -> String {
        document
            .split("<w:p><w:pPr>")
            .find(|block| block.contains(&format!(r#"<w:t xml:space="preserve">{lead}"#)))
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
    /// A placeholder written in a *prompt* is ordinary text. Only the page
    /// footer is a template; substituting in body text would let a question
    /// silently rewrite itself.
    fn a_placeholder_in_a_prompt_is_literal_text() {
        let mut quiz = exam(None, 1);
        if let Some(item) = quiz.items.first_mut() {
            item.question.prompt = "What does ${page} mean?".to_owned();
        }
        let document = part(&to_docx(&quiz).expect("renders"), "word/document.xml");
        assert!(document.contains("What does ${page} mean?"), "{document}");
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
