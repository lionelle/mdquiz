//! The `numbering.xml` part: what list markers look like, and which list each
//! item counts in.
//!
//! A `w:p` does not say "bullet" or "3."; it says `w:numId` and `w:ilvl`, and
//! this part is what those resolve against. So the writer has to decide, while
//! rendering, which list every item belongs to — hence a [`Numbering`] threaded
//! through the render rather than a constant emitted afterwards.
//!
//! # Why each ordered list needs its own `w:numId`
//!
//! A `w:numId` carries the counter. Two ordered lists sharing one do not both
//! start at 1: the second continues the first, so a sheet with "1. 2. 3." and
//! then "1. 2." prints "4. 5." — plausible enough on screen to survive review
//! and wrong on paper. Each ordered list therefore opens a `w:num` of its own
//! with a `w:startOverride`, written at the `w:ilvl` that list's items sit on
//! — an override naming a level no item carries is ignored. Bullets carry no
//! counter, so every bulleted list shares one.

use std::fmt::Write as _;

use super::W_NS;

/// How a list marks its items.
#[derive(Debug, Clone, Copy)]
pub(super) enum Marker {
    /// `- item`.
    Bullet,
    /// `1. item`, counting from this value.
    Ordered(u64),
}

/// The `w:abstractNumId` of the bulleted definition.
const BULLET_ABSTRACT: u32 = 0;

/// The `w:abstractNumId` of the numbered definition.
const ORDERED_ABSTRACT: u32 = 1;

/// How many nesting levels the definitions cover.
///
/// Nine is Word's own convention. Markdown deeper than this is rare enough
/// that clamping beats a variable-length part.
const LEVELS: usize = 9;

/// The deepest `w:ilvl` any item may carry.
///
/// Word allows 0–8, and only the levels defined here resolve at all: an item
/// naming a level the part does not define loses its marker *and* its
/// indent, printing as an unmarked paragraph at the left margin. Deeper
/// nesting is therefore clamped onto this level rather than emitted.
pub(super) const MAX_LEVEL: u8 = 8;

/// Twips of indent each nesting level adds.
const INDENT: u32 = 720;

/// The left indent, in twips, of a list item's text at `level`.
///
/// Shared with the paragraph writer because a *continuation* paragraph — an
/// item's second paragraph, which must not repeat the marker — carries no
/// `w:numPr`, and so inherits none of the level's indent. It has to be told
/// the same number, or it hangs out to the left of the item it belongs to.
pub(super) fn indent(level: usize) -> u32 {
    INDENT.saturating_mul(u32::try_from(level).unwrap_or(u32::MAX).saturating_add(1))
}

/// How far the marker hangs to the left of its text.
const HANGING: u32 = 360;

/// The `w:numId` of the instance held at `index`.
///
/// One rule, called from both the hand-out in `push` and the part written by
/// `to_xml`: computing it twice lets the id an item carries drift from the id
/// the part defines, and a `w:numId` resolving to nothing is a document Word
/// will not open.
fn id_of(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1)
}

/// The bullet characters Word cycles through, with the font each needs.
///
/// The first and third are private-use code points that mean something only
/// in the named font — that is how Word itself writes them, and a reader
/// substituting the font gets a sensible glyph rather than a missing one.
const BULLETS: [(char, &str); 3] = [
    ('\u{F0B7}', "Symbol"),
    ('o', "Courier New"),
    ('\u{F0A7}', "Wingdings"),
];

/// The number formats Word cycles through for an ordered list.
const ORDERED: [&str; 3] = ["decimal", "lowerLetter", "lowerRoman"];

/// One `w:num`: which definition its levels come from, and where its counter
/// restarts.
struct Instance {
    /// The `w:abstractNumId` whose levels this instance draws its markers from.
    definition: u32,
    /// The value the counter restarts at and the `w:ilvl` that restart binds
    /// to, or `None` for a bulleted list, which has no counter to restart.
    ///
    /// The level matters: `w:lvlOverride` names one, and an override naming a
    /// level no item carries is ignored outright — a nested list authored `5.`
    /// would quietly start at the definition's 1.
    start: Option<(u64, u8)>,
}

/// The lists one document holds.
pub(super) struct Numbering {
    /// The id shared by every bulleted list, once one has been opened.
    bullet: Option<u32>,
    /// One entry per `w:num`, in id order. The id is the position plus one,
    /// because `w:numId` 0 means "no numbering".
    instances: Vec<Instance>,
}

impl Numbering {
    /// A document with no lists in it yet.
    pub(super) const fn new() -> Self {
        Self {
            bullet: None,
            instances: Vec::new(),
        }
    }

    /// Open a list whose items sit at `level`, returning the `w:numId` they
    /// should carry.
    ///
    /// `level` is needed only to restart an ordered list's counter, which
    /// `w:lvlOverride` does per level; a bulleted list ignores it.
    pub(super) fn open(&mut self, marker: Marker, level: u8) -> u32 {
        match marker {
            Marker::Ordered(start) => self.push(ORDERED_ABSTRACT, Some((start, level))),
            Marker::Bullet => {
                if let Some(id) = self.bullet {
                    return id;
                }
                let id = self.push(BULLET_ABSTRACT, None);
                self.bullet = Some(id);
                id
            }
        }
    }

    /// Record one `w:num`, returning its id.
    fn push(&mut self, definition: u32, start: Option<(u64, u8)>) -> u32 {
        self.instances.push(Instance { definition, start });
        id_of(self.instances.len().saturating_sub(1))
    }

    /// The whole `word/numbering.xml` part.
    ///
    /// Both abstract definitions are always written, whether or not the
    /// document uses them: an unreferenced one costs a few hundred bytes,
    /// while a `w:numId` resolving to nothing is a document Word will not
    /// open.
    pub(super) fn to_xml(&self) -> String {
        let mut xml = format!(
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                r#"<w:numbering xmlns:w="{W_NS}">"#,
            ),
            W_NS = W_NS
        );
        xml.push_str(&definition(BULLET_ABSTRACT, &Marker::Bullet));
        xml.push_str(&definition(ORDERED_ABSTRACT, &Marker::Ordered(1)));
        for (index, entry) in self.instances.iter().enumerate() {
            xml.push_str(&instance(id_of(index), entry.definition, entry.start));
        }
        xml.push_str("</w:numbering>");
        xml
    }
}

/// One `w:abstractNum`: how a list of this kind looks at every level.
fn definition(id: u32, marker: &Marker) -> String {
    let levels = (0..LEVELS).fold(String::new(), |mut xml, level| {
        xml.push_str(&level_xml(level, marker));
        xml
    });
    format!(
        concat!(
            r#"<w:abstractNum w:abstractNumId="{id}">"#,
            r#"<w:multiLevelType w:val="hybridMultilevel"/>{levels}"#,
            r"</w:abstractNum>",
        ),
        id = id,
        levels = levels,
    )
}

/// One `w:lvl`: the marker and indent at one nesting level.
///
/// `CT_Lvl` is a sequence — `start`, `numFmt`, `lvlText`, `lvlJc`, `pPr`,
/// `rPr` — and writing them in any other order is a schema violation Word
/// refuses the document for.
fn level_xml(level: usize, marker: &Marker) -> String {
    let (format, text, fonts) = match *marker {
        Marker::Bullet => {
            let (glyph, font) = BULLETS
                .get(level % BULLETS.len())
                .copied()
                .unwrap_or(('*', ""));
            let fonts = format!(r#"<w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}"/></w:rPr>"#);
            ("bullet".to_owned(), glyph.to_string(), fonts)
        }
        // `%N` counts *this* level, so each nesting level numbers itself
        // rather than compounding into "1.a.i".
        Marker::Ordered(_) => (
            (*ORDERED.get(level % ORDERED.len()).unwrap_or(&"decimal")).to_owned(),
            format!("%{}.", level + 1),
            String::new(),
        ),
    };
    format!(
        concat!(
            r#"<w:lvl w:ilvl="{level}"><w:start w:val="1"/><w:numFmt w:val="{format}"/>"#,
            r#"<w:lvlText w:val="{text}"/><w:lvlJc w:val="left"/>"#,
            r#"<w:pPr><w:ind w:left="{indent}" w:hanging="{hanging}"/></w:pPr>{fonts}</w:lvl>"#,
        ),
        level = level,
        format = format,
        text = crate::export::escape_xml(&text),
        indent = indent(level),
        hanging = HANGING,
        fonts = fonts,
    )
}

/// One `w:num`: a list, and where its counter starts.
///
/// The restart is written at the `w:ilvl` the list's items carry, not at 0:
/// `w:lvlOverride` names a level, and one naming a level no item uses is
/// ignored, so a nested list authored `5.` would start at the definition's 1.
fn instance(id: u32, definition: u32, start: Option<(u64, u8)>) -> String {
    let mut xml = format!(r#"<w:num w:numId="{id}"><w:abstractNumId w:val="{definition}"/>"#);
    if let Some((start, level)) = start {
        let _ = write!(
            xml,
            r#"<w:lvlOverride w:ilvl="{level}"><w:startOverride w:val="{start}"/></w:lvlOverride>"#
        );
    }
    xml.push_str("</w:num>");
    xml
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    /// Every bulleted list shares one `w:numId`. Bullets carry no counter, so
    /// a second id would only add a definition nothing needs.
    fn bulleted_lists_share_one_numbering() {
        let mut numbering = Numbering::new();
        assert_eq!(numbering.open(Marker::Bullet, 0), 1);
        assert_eq!(numbering.open(Marker::Bullet, 0), 1);
        assert_eq!(numbering.to_xml().matches("<w:num ").count(), 1);
    }

    #[test]
    /// Each ordered list gets its own. Sharing one makes the second continue
    /// the first — a sheet that prints "1. 2." then "3. 4." where the author
    /// wrote two lists of two.
    fn each_ordered_list_gets_its_own_numbering() {
        let mut numbering = Numbering::new();
        assert_eq!(numbering.open(Marker::Ordered(1), 0), 1);
        assert_eq!(numbering.open(Marker::Ordered(1), 0), 2);
        let xml = numbering.to_xml();
        assert_eq!(xml.matches("<w:num ").count(), 2, "{xml}");
        assert_eq!(xml.matches("w:startOverride").count(), 2, "{xml}");
    }

    #[test]
    /// A list authored as `3.` starts at three, not at one.
    fn an_ordered_list_starts_where_it_says() {
        let mut numbering = Numbering::new();
        numbering.open(Marker::Ordered(3), 0);
        assert!(
            numbering
                .to_xml()
                .contains(r#"<w:startOverride w:val="3"/>"#),
            "{}",
            numbering.to_xml()
        );
    }

    #[test]
    /// Both definitions are always present, so a `w:numId` can never resolve
    /// to nothing — which is a document Word refuses to open.
    fn both_definitions_are_always_written() {
        let xml = Numbering::new().to_xml();
        assert!(xml.contains(r#"w:abstractNumId="0""#), "{xml}");
        assert!(xml.contains(r#"w:abstractNumId="1""#), "{xml}");
        assert!(!xml.contains("<w:num "), "an unused instance: {xml}");
    }

    #[test]
    /// Bullets and numbers use different definitions, or every ordered list
    /// would print as bullets.
    fn bullets_and_numbers_use_different_definitions() {
        let mut numbering = Numbering::new();
        numbering.open(Marker::Bullet, 0);
        numbering.open(Marker::Ordered(1), 0);
        let xml = numbering.to_xml();
        assert!(xml.contains(r#"<w:abstractNumId w:val="0"/>"#), "{xml}");
        assert!(xml.contains(r#"<w:abstractNumId w:val="1"/>"#), "{xml}");
    }

    #[test]
    /// The deepest level items may use is the deepest one defined. If these
    /// drift apart, a clamped item names a level that resolves to nothing.
    fn the_clamp_matches_the_definitions() {
        assert_eq!(usize::from(MAX_LEVEL) + 1, LEVELS);
    }

    #[test]
    /// Every level a `w:ilvl` may name is defined. An item deeper than the
    /// definitions reach loses its marker and its indent silently.
    fn every_level_word_allows_is_defined() {
        let xml = Numbering::new().to_xml();
        for level in 0..LEVELS {
            assert!(
                xml.contains(&format!(r#"<w:lvl w:ilvl="{level}">"#)),
                "level {level} is undefined: {xml}"
            );
        }
    }

    #[test]
    /// The id `open` hands out is the id the part defines, against the right
    /// definition and start. `push` and `to_xml` both go through `id_of`, and
    /// drift between them would point items at the wrong list — a bulleted
    /// item that numbers itself, or a `w:numId` resolving to nothing, which is
    /// a document Word will not open. A bulleted instance carries no
    /// `w:startOverride`: bullets have no counter to restart.
    fn the_id_open_returns_is_the_one_the_part_defines() {
        let mut numbering = Numbering::new();
        let bullet = numbering.open(Marker::Bullet, 0);
        let first = numbering.open(Marker::Ordered(1), 0);
        let second = numbering.open(Marker::Ordered(5), 2);
        assert_eq!(
            (bullet, first, second, numbering.open(Marker::Bullet, 1)),
            (1, 2, 3, 1),
            "a later bulleted list opened a second id"
        );
        let xml = numbering.to_xml();
        assert!(
            xml.contains(&format!(
                r#"<w:num w:numId="{bullet}"><w:abstractNumId w:val="0"/></w:num>"#
            )),
            "a bulleted list restarted a counter it does not have: {xml}"
        );
        assert!(
            xml.contains(&format!(
                concat!(
                    r#"<w:num w:numId="{id}"><w:abstractNumId w:val="1"/>"#,
                    r#"<w:lvlOverride w:ilvl="2"><w:startOverride w:val="5"/></w:lvlOverride>"#,
                ),
                id = second
            )),
            "the start landed on the wrong list or level: {xml}"
        );
    }

    /// The values of every `w:{attribute}` in `xml`, in document order.
    ///
    /// Shared with `docx::tests`, which reads `w:numId` out of `document.xml`
    /// the same way.
    pub(in crate::export::docx) fn values_of(xml: &str, attribute: &str) -> Vec<String> {
        xml.split(&format!(r#"<w:{attribute} w:val=""#))
            .skip(1)
            .filter_map(|rest| rest.split_once('"'))
            .map(|(value, _)| value.to_owned())
            .collect()
    }

    #[test]
    /// Each level draws its marker from the right place in the cycle. An
    /// off-by-one in the index silently replaces *every* bullet with the
    /// fallback glyph — a page of asterisks set in the body font, which no
    /// test asserting only that the levels exist would notice.
    fn each_level_takes_its_own_marker() {
        // Spelled out, not recomputed from `BULLETS` and `%`: an expectation
        // built the same way as the code agrees with it however it is
        // written, including a way that gives every level the same marker.
        let expected = [
            "\u{F0B7}", "o", "\u{F0A7}", "\u{F0B7}", "o", "\u{F0A7}", "\u{F0B7}", "o", "\u{F0A7}",
            "%1.", "%2.", "%3.", "%4.", "%5.", "%6.", "%7.", "%8.", "%9.",
        ];
        assert_eq!(values_of(&Numbering::new().to_xml(), "lvlText"), expected);
    }

    #[test]
    /// …and its number format, so nesting an ordered list gives letters and
    /// roman numerals rather than nine levels of `1.`.
    fn each_level_takes_its_own_format() {
        let expected = [
            "bullet",
            "bullet",
            "bullet",
            "bullet",
            "bullet",
            "bullet",
            "bullet",
            "bullet",
            "bullet",
            "decimal",
            "lowerLetter",
            "lowerRoman",
            "decimal",
            "lowerLetter",
            "lowerRoman",
            "decimal",
            "lowerLetter",
            "lowerRoman",
        ];
        assert_eq!(values_of(&Numbering::new().to_xml(), "numFmt"), expected);
    }

    #[test]
    /// A bullet glyph is a private-use code point that means something only
    /// in its own font, so the font must travel with it.
    fn every_bullet_level_names_its_font() {
        let xml = Numbering::new().to_xml();
        for (_, font) in BULLETS {
            assert!(xml.contains(&format!(r#"w:ascii="{font}""#)), "{xml}");
        }
    }

    #[test]
    /// A continuation paragraph is indented by hand, so the two calculations
    /// must agree: if they drift, an item's second paragraph hangs out to the
    /// left of the first.
    fn the_shared_indent_matches_the_definitions() {
        let xml = Numbering::new().to_xml();
        for level in 0..LEVELS {
            let expected = format!(r#"<w:ind w:left="{}" "#, indent(level));
            assert!(
                xml.contains(&expected),
                "level {level} indents to something else: {xml}"
            );
        }
    }
}
