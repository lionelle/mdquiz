//! Markdown pipe tables → `w:tbl`.
//!
//! A table is the one authored construct that is neither a run nor a
//! paragraph: `w:tbl` is a block-level sibling of `w:p`, so it cannot be
//! carried inside one. That is why this is its own module rather than another
//! arm of [`super::inline`], which turns inline structure into runs.
//!
//! # What Word requires
//!
//! * A `w:tc` must hold **at least one** `w:p`. An empty cell is an empty
//!   paragraph, not an absent one — omit it and Word rejects the document.
//! * `w:tblGrid` declares the columns. A row with fewer cells than the grid
//!   renders ragged, so short rows are padded.
//! * A `w:tbl` needs a `w:p` after it. Two adjacent tables with nothing
//!   between them merge into one, and a document ending on a table has an
//!   empty paragraph appended for it anyway — so [`table`] emits its own and
//!   the result is what Word would have made regardless.
//!
//! # Widths
//!
//! The columns divide the text width equally, in a fixed layout. Autofit
//! reads better for prose, but these are exam tables: a column the student
//! writes the answer into is usually the empty one, and autofit collapses an
//! empty column to nothing. A fixed share is predictable, leaves room to
//! write, and does not depend on what the reader decides to measure.

use std::fmt::Write as _;

use pulldown_cmark::{Alignment, Event, Tag, TagEnd};

use super::Refs;
use super::inline::{Marks, runs_for};
use crate::Result;

/// The width the columns divide between them, in twips: the text column.
///
/// Read from the page setup [`super`] owns rather than restated, so moving a
/// margin moves the table with the text.
const TABLE_WIDTH: u32 = super::PAGE_WIDTH - 2 * super::MARGIN;

/// A single hairline border on every edge, inside and out.
///
/// Spelled on each table rather than defined as a table *style*: a `w:tblStyle`
/// naming a style the package does not define loses its borders silently, and
/// a borderless table of blank cells is an invisible table.
const BORDERS: &str = concat!(
    "<w:tblBorders>",
    r#"<w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/>"#,
    r#"<w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/>"#,
    r#"<w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/>"#,
    r#"<w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/>"#,
    r#"<w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/>"#,
    r#"<w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/>"#,
    "</w:tblBorders>",
);

/// The empty paragraph that closes a table off from whatever follows it.
const SEPARATOR: &str = "<w:p/>";

/// Render a Markdown table as a `w:tbl`, followed by the paragraph Word needs
/// after one.
///
/// `Ok(None)` means a cell held something this writer cannot lay out, and the
/// caller should print the whole table as Markdown source — the same rule a
/// list follows, and for the same reason: half a table formatted and half
/// printed as pipes reads worse than either.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if a cell holds math that cannot
/// be converted.
pub(super) fn table(
    alignments: &[Alignment],
    events: &[Event<'_>],
    refs: &mut Refs<'_>,
) -> Result<Option<String>> {
    let rows = rows(events);
    let columns = rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(alignments.len());
    if columns == 0 {
        return Ok(None);
    }
    let mut xml = format!("<w:tbl>{}{}", properties(), grid(columns));
    for (index, row) in rows.iter().enumerate() {
        let Some(cells) = row_xml(row, alignments, columns, index == 0, refs)? else {
            return Ok(None);
        };
        xml.push_str(&cells);
    }
    xml.push_str("</w:tbl>");
    xml.push_str(SEPARATOR);
    Ok(Some(xml))
}

/// The table-wide properties: full width, fixed layout, and borders.
fn properties() -> String {
    format!(
        concat!(
            "<w:tblPr>",
            r#"<w:tblW w:w="{width}" w:type="dxa"/>"#,
            r#"<w:tblLayout w:type="fixed"/>"#,
            "{borders}",
            "</w:tblPr>",
        ),
        width = TABLE_WIDTH,
        borders = BORDERS,
    )
}

/// The column grid: `columns` equal shares of the text width.
fn grid(columns: usize) -> String {
    let each = column_width(columns);
    let mut xml = String::from("<w:tblGrid>");
    for _ in 0..columns {
        let _ = write!(xml, r#"<w:gridCol w:w="{each}"/>"#);
    }
    xml.push_str("</w:tblGrid>");
    xml
}

/// One column's width in twips, an equal share of the text column.
fn column_width(columns: usize) -> u32 {
    u32::try_from(columns)
        .ok()
        .filter(|columns| *columns > 0)
        .map_or(TABLE_WIDTH, |columns| TABLE_WIDTH / columns)
}

/// One row as a `w:tr`, padded to `columns` cells.
///
/// `Ok(None)` when a cell cannot be laid out.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if a cell holds math that cannot
/// be converted.
fn row_xml(
    row: &[Vec<Event<'_>>],
    alignments: &[Alignment],
    columns: usize,
    header: bool,
    refs: &mut Refs<'_>,
) -> Result<Option<String>> {
    // A header row repeats when the table breaks across a page. Without it a
    // student reading the second page has columns and no names for them.
    let width = column_width(columns);
    let mut xml = String::from("<w:tr>");
    if header {
        xml.push_str("<w:trPr><w:tblHeader/></w:trPr>");
    }
    for column in 0..columns {
        let events = row.get(column).map(Vec::as_slice).unwrap_or_default();
        let Some(cell) = cell_xml(events, alignments.get(column), header, width, refs)? else {
            return Ok(None);
        };
        xml.push_str(&cell);
    }
    xml.push_str("</w:tr>");
    Ok(Some(xml))
}

/// One cell as a `w:tc`, holding exactly one paragraph.
///
/// `Ok(None)` when the cell's content cannot be laid out.
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedMath`] if the cell holds math that
/// cannot be converted.
fn cell_xml(
    events: &[Event<'_>],
    alignment: Option<&Alignment>,
    header: bool,
    width: u32,
    refs: &mut Refs<'_>,
) -> Result<Option<String>> {
    // Header cells open bold, so the marks a cell's own `**…**` applies stack
    // on top rather than replacing it.
    let marks = if header {
        Marks::bold()
    } else {
        Marks::default()
    };
    let Some(runs) = runs_for(events, marks, refs)? else {
        return Ok(None);
    };
    // Stated in twips to match the grid: under a fixed layout an `auto`
    // cell width leaves the reader to guess, and readers guess differently.
    let sizing = format!(r#"<w:tcPr><w:tcW w:w="{width}" w:type="dxa"/></w:tcPr>"#);
    let properties = justification(alignment);
    let properties = if properties.is_empty() {
        String::new()
    } else {
        format!("<w:pPr>{properties}</w:pPr>")
    };
    // Always a paragraph, even with no runs: a `w:tc` holding no `w:p` is the
    // one table mistake Word refuses to open a document over.
    Ok(Some(format!(
        "<w:tc>{sizing}<w:p>{properties}{runs}</w:p></w:tc>"
    )))
}

/// The `w:jc` a column's Markdown alignment asks for, or nothing for the
/// default.
fn justification(alignment: Option<&Alignment>) -> String {
    match alignment {
        Some(Alignment::Left) => r#"<w:jc w:val="left"/>"#.to_owned(),
        Some(Alignment::Center) => r#"<w:jc w:val="center"/>"#.to_owned(),
        Some(Alignment::Right) => r#"<w:jc w:val="right"/>"#.to_owned(),
        Some(Alignment::None) | None => String::new(),
    }
}

/// Split a table's events into rows of cells.
///
/// The head row and the body rows are gathered the same way: pulldown reports
/// the head as `TableHead` rather than a `TableRow`, but a header is a row of
/// cells like any other and only the *first* row is treated as one here.
fn rows<'e>(events: &[Event<'e>]) -> Vec<Vec<Vec<Event<'e>>>> {
    let mut rows: Vec<Vec<Vec<Event<'e>>>> = Vec::new();
    let mut cell: Option<Vec<Event<'e>>> = None;
    for event in events {
        match event {
            Event::Start(Tag::TableHead | Tag::TableRow) => rows.push(Vec::new()),
            Event::Start(Tag::TableCell) => cell = Some(Vec::new()),
            Event::End(TagEnd::TableCell) => {
                if let (Some(row), Some(events)) = (rows.last_mut(), cell.take()) {
                    row.push(events);
                }
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => cell = None,
            other => {
                if let Some(events) = cell.as_mut() {
                    events.push(other.clone());
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::super::inline;
    use super::*;

    /// The XML `markdown` renders to, which must be exactly one table.
    fn only(markdown: &str) -> String {
        let images: Vec<(String, Vec<u8>)> = Vec::new();
        let mut refs = Refs::new(&images);
        let rendered = inline::paragraphs(markdown, &mut refs).expect("renders");
        assert_eq!(rendered.len(), 1, "expected one block: {rendered:?}");
        rendered
            .into_iter()
            .next()
            .map(|block| block.runs)
            .unwrap_or_default()
    }

    /// The text inside every `w:t` of `xml`, in order.
    fn texts(xml: &str) -> Vec<String> {
        xml.split("<w:t")
            .skip(1)
            .filter_map(|rest| rest.split_once('>'))
            .filter_map(|(_, body)| body.split_once("</w:t>"))
            .map(|(text, _)| text.to_owned())
            .collect()
    }

    #[test]
    /// A pipe table becomes a real table, with its cells in reading order.
    fn a_table_becomes_rows_and_cells() {
        let xml = only("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(xml.starts_with("<w:tbl>"), "{xml}");
        assert_eq!(xml.matches("<w:tr>").count(), 2, "{xml}");
        assert_eq!(xml.matches("<w:tc>").count(), 4, "{xml}");
        assert_eq!(texts(&xml), ["a", "b", "1", "2"]);
    }

    #[test]
    /// The columns divide the text width, and the grid and the cells agree
    /// about it — under a fixed layout a cell that disagrees with its column
    /// is a cell the reader has to arbitrate.
    fn the_columns_divide_the_text_width() {
        let xml = only("| a | b | c |\n|---|---|---|\n| 1 | 2 | 3 |");
        let each = TABLE_WIDTH / 3;
        assert_eq!(
            xml.matches(&format!(r#"<w:gridCol w:w="{each}"/>"#))
                .count(),
            3,
            "{xml}"
        );
        assert_eq!(
            xml.matches(&format!(r#"<w:tcW w:w="{each}" w:type="dxa"/>"#))
                .count(),
            6,
            "{xml}"
        );
    }

    #[test]
    /// An empty cell is an empty paragraph, not an absent one. A `w:tc`
    /// holding no `w:p` is well-formed XML that Word refuses to open — and
    /// the empty cell is the one a student writes the answer in.
    fn an_empty_cell_still_holds_a_paragraph() {
        let xml = only("| n | total |\n|---|-------|\n| 3 |       |");
        for cell in xml.split("<w:tc>").skip(1) {
            let body = cell.split("</w:tc>").next().unwrap_or_default();
            assert!(body.contains("<w:p>"), "a cell holds no paragraph: {body}");
        }
        assert_eq!(xml.matches("<w:tc>").count(), 4, "{xml}");
    }

    #[test]
    /// The header row is marked to repeat when the table breaks across a
    /// page, and only the header row is. Columns with no names on page two
    /// are columns a student cannot answer.
    fn the_header_row_repeats_across_pages() {
        let xml = only("| a |\n|---|\n| 1 |\n| 2 |");
        assert_eq!(xml.matches("<w:tblHeader/>").count(), 1, "{xml}");
        let first = xml.split("<w:tr>").nth(1).unwrap_or_default();
        assert!(first.contains("<w:tblHeader/>"), "{xml}");
    }

    #[test]
    /// Header cells are bold, and their own emphasis stacks on top rather
    /// than replacing it.
    fn header_cells_are_bold() {
        let xml = only("| plain | *stressed* |\n|---|---|\n| a | b |");
        let header = xml.split("</w:tr>").next().unwrap_or_default();
        assert_eq!(header.matches("<w:b/>").count(), 2, "{header}");
        assert!(header.contains("<w:i/>"), "emphasis was lost: {header}");
        let body = xml.split("</w:tr>").nth(1).unwrap_or_default();
        assert!(!body.contains("<w:b/>"), "a body cell is bold: {body}");
    }

    #[test]
    /// A column's Markdown alignment reaches its cells.
    fn column_alignment_reaches_the_cells() {
        let xml = only("| l | c | r |\n|:--|:-:|--:|\n| 1 | 2 | 3 |");
        for value in ["left", "center", "right"] {
            assert_eq!(
                xml.matches(&format!(r#"<w:jc w:val="{value}"/>"#)).count(),
                2,
                "{value} missing: {xml}"
            );
        }
    }

    #[test]
    /// A table with no stated alignment sets none, rather than guessing one.
    fn an_unaligned_table_sets_no_justification() {
        let xml = only("| a |\n|---|\n| 1 |");
        assert!(!xml.contains("<w:jc"), "{xml}");
    }

    #[test]
    /// A short row is padded to the column count. A row with fewer cells
    /// than the grid renders ragged, and a Markdown table is allowed one.
    fn a_short_row_is_padded_to_the_grid() {
        let xml = only("| a | b | c |\n|---|---|---|\n| 1 |");
        assert_eq!(xml.matches("<w:tc>").count(), 6, "{xml}");
        assert_eq!(texts(&xml), ["a", "b", "c", "1"]);
    }

    #[test]
    /// Marks and inline code inside a cell survive.
    fn a_cell_keeps_its_inline_marks() {
        let xml = only("| a |\n|---|\n| **bold** and `code` |");
        let body = xml.split("</w:tr>").nth(1).unwrap_or_default();
        assert!(body.contains("<w:b/>"), "{body}");
        assert!(body.contains(r#"<w:rStyle w:val="Code"/>"#), "{body}");
    }

    #[test]
    /// Math in a cell is laid out as math. It used to be refused, because
    /// the table printed as source and the LaTeX would have printed with it;
    /// now that the table is laid out there is nothing to refuse.
    fn math_in_a_cell_is_rendered() {
        let xml = only("| f | $x^2$ |\n|---|---|\n| a | b |");
        assert!(xml.contains("<m:oMath>"), "{xml}");
    }

    #[test]
    /// A cell holding something this writer cannot lay out sends the *whole*
    /// table to source, in the monospace face that keeps its columns lined
    /// up. Half a table formatted and half printed as pipes reads worse than
    /// either.
    fn a_cell_that_cannot_be_laid_out_falls_the_table_back() {
        let xml = only("| a | [link](https://x.example) |\n|---|---|\n| 1 | 2 |");
        assert!(!xml.contains("<w:tbl>"), "{xml}");
        assert!(xml.contains(r#"<w:rStyle w:val="Code"/>"#), "{xml}");
        assert!(xml.contains(">| a | [link](https://x.example) |<"), "{xml}");
    }

    #[test]
    /// Math in a table that falls back is still refused: the fallback prints
    /// the source, and `$x^2$` on an exam paper is what that rule exists for.
    fn math_in_a_table_that_falls_back_is_refused() {
        let images: Vec<(String, Vec<u8>)> = Vec::new();
        let mut refs = Refs::new(&images);
        let source = "| $x^2$ | [a](https://x.example) |\n|---|---|\n| 1 | 2 |";
        let refused = inline::paragraphs(source, &mut refs);
        assert!(
            matches!(refused, Err(crate::Error::UnsupportedMath { .. })),
            "{refused:?}"
        );
    }

    #[test]
    /// A table is closed off by a paragraph. Two adjacent tables with
    /// nothing between them merge into one.
    fn a_table_is_followed_by_a_paragraph() {
        let images: Vec<(String, Vec<u8>)> = Vec::new();
        let mut refs = Refs::new(&images);
        let source = "| a |\n|---|\n| 1 |\n\n| b |\n|---|\n| 2 |";
        let rendered = inline::paragraphs(source, &mut refs).expect("renders");
        let xml: String = rendered.iter().map(|block| block.runs.clone()).collect();
        assert_eq!(xml.matches("<w:tbl>").count(), 2, "{xml}");
        assert!(!xml.contains("</w:tbl><w:tbl>"), "the tables merged: {xml}");
        assert_eq!(xml.matches("</w:tbl><w:p/>").count(), 2, "{xml}");
    }

    #[test]
    /// Cell text is escaped. An authored `&` reaching the XML raw makes the
    /// whole document unreadable.
    fn cell_text_is_escaped() {
        let xml = only("| a |\n|---|\n| Tom & <Jerry> |");
        assert!(xml.contains("Tom &amp; &lt;Jerry&gt;"), "{xml}");
        assert!(!xml.contains("<Jerry>"), "{xml}");
    }
}
