//! Export quiz content to a distribution format.
//!
//! Three targets are supported:
//!
//! * [`markdown`] — a single print-ready Markdown sheet with no answer key,
//! * [`canvas`] — a Canvas *New Quizzes* QTI package, and
//! * [`docx`] — a Word document for printing, built from a
//!   [`Exam`](crate::quiz::exam::Exam).
//!
//! Two of the three are zipped XML, so the packaging helpers they share live
//! here rather than in whichever exporter happened to need them first.

use std::io::{Cursor, Write as _};

use pulldown_cmark::Options;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::Result;

pub mod canvas;
pub mod docx;
pub mod markdown;

/// The Markdown dialect the exporters that parse Markdown read.
///
/// Shared rather than built per call site: what a prompt *means* cannot differ
/// between the Canvas package and the Word document. Turning an extension on
/// for one and not the other makes the same file two questions. The print
/// *Markdown* sheet is not a reader — it interpolates the authored source
/// verbatim — and `parse.rs` needs only front-matter, so neither uses this.
pub(crate) const MARKDOWN: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_MATH);

/// Escape `text` for inclusion in an XML text node or attribute value.
///
/// Characters XML 1.0 cannot represent *at all* are dropped rather than
/// escaped. Most control characters have no numeric-reference form either, so a
/// stray form feed pasted into a prompt would otherwise produce a part that no
/// reader can open — Word and `xmllint` both reject the whole document.
pub(crate) fn escape_xml(text: &str) -> String {
    text.chars().filter(|ch| is_xml_char(*ch)).fold(
        String::with_capacity(text.len()),
        |mut out, ch| {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                other => out.push(other),
            }
            out
        },
    )
}

/// Whether `ch` is legal in an XML 1.0 document.
///
/// Tab, newline and carriage return are the only control characters allowed;
/// `U+FFFE` and `U+FFFF` are permanently unassigned non-characters.
const fn is_xml_char(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\r')
        || matches!(ch, ' '..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// Zip `files` — `(path, contents)` pairs — into an in-memory archive.
///
/// Both the QTI package and the Word document are zip containers of XML parts,
/// so they build through here. `what` names the package in any error.
///
/// New exporters should follow the `docx` idiom — `concat!` constants composed
/// with `format!` — rather than the `canvas` one, which substitutes `{{TOKEN}}`
/// placeholders at runtime and so can leak an unreplaced token into the output.
///
/// # Errors
///
/// Returns [`crate::Error::Export`] if the archive cannot be written.
pub(crate) fn zip_package(what: &str, files: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut cursor);
        let options = SimpleFileOptions::default();
        for (path, content) in files {
            writer
                .start_file(*path, options)
                .map_err(|e| zip_err(what, &e))?;
            writer.write_all(content)?;
        }
        writer.finish().map_err(|e| zip_err(what, &e))?;
    }
    Ok(cursor.into_inner())
}

/// Map a zip-writer failure onto the crate export error, naming which package
/// was being built so a Canvas failure and a Word failure do not read alike.
fn zip_err(what: &str, error: &zip::result::ZipError) -> crate::Error {
    crate::Error::Export(format!("building {what}: {error}"))
}

/// The export formats the CLI can produce.
///
/// `docx` is deliberately absent: it is produced from an
/// [`Exam`](crate::quiz::exam::Exam) by the forthcoming `quiz` subcommand,
/// which does not route through this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Print-ready Markdown, no solutions.
    Markdown,
    /// Canvas New Quizzes QTI package.
    Canvas,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// The five predefined entities are escaped, `&` first so nothing is
    /// double-escaped.
    fn escapes_the_predefined_entities() {
        assert_eq!(
            escape_xml(r#"a & b < c > d " e ' f"#),
            "a &amp; b &lt; c &gt; d &quot; e &apos; f"
        );
    }

    #[test]
    /// Characters XML cannot represent are dropped, not passed through. A form
    /// feed in a prompt would otherwise make the whole part unreadable.
    fn drops_characters_xml_cannot_represent() {
        let escaped = escape_xml("a\u{0}b\u{8}c\u{B}d\u{C}e\u{1F}f\u{FFFE}g");
        assert_eq!(escaped, "abcdefg");
        // Tab, newline and carriage return are legal and survive.
        assert_eq!(escape_xml("a\tb\nc\rd"), "a\tb\nc\rd");
        // So does text outside the basic plane.
        assert_eq!(escape_xml("café 日本 🎓"), "café 日本 🎓");
    }
}
