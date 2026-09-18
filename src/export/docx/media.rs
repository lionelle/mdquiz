//! Images in a Word document: the media parts, and the drawings that point at
//! them.
//!
//! [`super`] owns the container and [`super::inline`] owns the runs; this owns
//! the third thing an image needs, which is a *part* in the package and a
//! relationship pointing at it. A `w:drawing` does not carry an image, it
//! carries an `r:embed` naming a relationship, so a picture only appears if
//! three files agree — and this module is the one place all three are decided.
//!
//! # PNG only
//!
//! [`Media::drawing`] measures the bytes it is given and refuses anything that
//! is not a PNG, so the caller falls back to printing the Markdown source. SVG
//! is out of scope deliberately: Word renders it through `asvg:svgBlip`, which
//! is an *extension* to the drawing element and requires a rasterised fallback
//! alongside it for readers that do not understand the extension — so shipping
//! SVG means shipping a PNG anyway. The diagram pass is run in
//! [`DiagramFormat::Png`](crate::diagram::DiagramFormat::Png) for this writer
//! for the same reason.
//!
//! Refusing by *signature* rather than by file extension is the point: the
//! diagram renderer is injected, and one that ignores the format it was asked
//! for lands SVG bytes behind a `generated/….png` path. An extension check
//! would pass that through and produce a document Word cannot open.
//!
//! # Sizing
//!
//! A PNG states its pixel size, and may state its resolution in a `pHYs`
//! chunk. Both are needed: pixels alone are not a physical size, and assuming
//! 96 DPI prints a 300 DPI screenshot three times too wide. Anything larger
//! than the text column is scaled down to fit, in one step so it keeps its
//! proportions.

use std::fmt::Write as _;

use crate::export::escape_xml;

/// The PNG magic number every PNG file opens with.
///
/// The trailing four bytes are a deliberate transport check — a CRLF pair, a
/// DOS end-of-file and a bare LF — so bytes mangled in transit fail here
/// rather than halfway through decoding.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// English Metric Units per inch — the unit every OOXML drawing measurement is
/// in.
const EMU_PER_INCH: u32 = 914_400;

/// English Metric Units per twip, the unit [`super`] sets pages in.
const EMU_PER_TWIP: u32 = 635;

/// The resolution assumed for a PNG that does not declare one.
///
/// Word's own assumption, so an image with no `pHYs` chunk prints here at the
/// size it would print at if it had been pasted in.
const DEFAULT_DPI: u64 = 96;

/// Bytes of chunk header — a four-byte length and a four-byte type — that
/// precede every PNG chunk's data.
const CHUNK_HEADER: usize = 8;

/// Bytes of CRC that follow every PNG chunk's data.
const CRC_LEN: usize = 4;

/// The `pHYs` unit code meaning "per metre". Any other value states a bare
/// pixel ratio with no physical size.
const PHYS_UNIT_METRE: u8 = 1;

/// The widest an image may print, in EMUs: the text column.
///
/// Derived from the page setup [`super`] owns rather than restated, so moving
/// a margin moves the images with the text.
const MAX_WIDTH: u32 = (super::PAGE_WIDTH - 2 * super::MARGIN) * EMU_PER_TWIP;

/// The tallest an image may print, in EMUs: the text height of one page.
///
/// A picture taller than the page does not paginate — it is clipped at the
/// bottom margin — so this is the difference between a scaled-down diagram and
/// half a diagram.
const MAX_HEIGHT: u32 = (super::PAGE_HEIGHT - 2 * super::MARGIN) * EMU_PER_TWIP;

/// The directory image parts live in, relative to the document part.
///
/// Relative, because that is the form a relationship `Target` takes — and the
/// package path is derived from it rather than spelled separately. Two
/// spellings that only happen to agree is exactly how an `r:embed` comes to
/// point at a part that is not there.
const MEDIA_DIR: &str = "media";

/// The relationship type that marks a target as an embedded image.
const IMAGE_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";

/// The images a document uses, in the order it first used them.
///
/// Threaded through the writers as part of [`super::Refs`] rather than
/// collected by a separate scan of the same Markdown. The relationship id is
/// minted at the moment the drawing that cites it is written, so an `r:embed`
/// cannot name a relationship the package does not define — which is the one
/// image mistake Word refuses to open a document over.
#[derive(Debug)]
pub(super) struct Media<'a> {
    /// Every image the caller supplied, by its authored path.
    supplied: &'a [(String, Vec<u8>)],
    /// The images drawn so far, in the order the document first drew them.
    ///
    /// The entries themselves rather than indices into `supplied`, so
    /// [`Self::parts`] cannot fail to resolve one and silently emit fewer
    /// parts than [`Self::relationships`] declares.
    used: Vec<&'a (String, Vec<u8>)>,
    /// How many drawings have been written.
    ///
    /// Separate from `used` because `w:docPr/@id` must be unique per
    /// *drawing*, not per image: the same diagram placed twice is two.
    drawings: u32,
}

impl<'a> Media<'a> {
    /// A registry over the images `supplied` by the caller.
    pub(super) const fn new(supplied: &'a [(String, Vec<u8>)]) -> Self {
        Self {
            supplied,
            used: Vec::new(),
            drawings: 0,
        }
    }

    /// The run drawing the image at `url`, described to a reader by `alt`.
    ///
    /// `None` when there is no such image, or its bytes are not a PNG this
    /// writer can measure. The caller prints the Markdown source instead, so
    /// an unrenderable image leaves its path visible on the page rather than
    /// vanishing from it.
    pub(super) fn drawing(&mut self, url: &str, alt: &str) -> Option<String> {
        let entry = self.supplied.iter().find(|(path, _)| path == url)?;
        let extent = extent(&read_png(&entry.1)?);
        let slot = self
            .used
            .iter()
            .position(|(path, _)| path == url)
            .unwrap_or_else(|| {
                self.used.push(entry);
                self.used.len().saturating_sub(1)
            });
        self.drawings = self.drawings.saturating_add(1);
        Some(drawing(
            &relationship_id(slot),
            self.drawings,
            &part_name(slot),
            alt,
            extent,
        ))
    }

    /// Where the registry stands, so a caller can rewind to it.
    pub(super) const fn mark(&self) -> Mark {
        Mark {
            used: self.used.len(),
            drawings: self.drawings,
        }
    }

    /// Discard every image registered since `mark`.
    ///
    /// A paragraph is written speculatively: the runs are built, and only a
    /// later event reveals that the whole paragraph must print as Markdown
    /// source instead. Those runs are thrown away, so the images they drew
    /// have to go with them — otherwise the package carries a part and a
    /// relationship that nothing cites, which on a four-variant exam with a
    /// screenshot is megabytes of payload no reader will ever see.
    ///
    /// Exact, because `used` only ever grows by one `push` per image first
    /// drawn: an image an *earlier* paragraph already placed is found by the
    /// lookup in [`Self::drawing`] and never pushed again, so truncating
    /// cannot discard it.
    pub(super) fn rewind(&mut self, mark: Mark) {
        self.used.truncate(mark.used);
        self.drawings = mark.drawings;
    }

    /// Every image part to put in the package, as `(path, bytes)`.
    pub(super) fn parts(&self) -> Vec<(String, &'a [u8])> {
        self.used
            .iter()
            .enumerate()
            .map(|(slot, (_, bytes))| {
                (
                    format!("word/{MEDIA_DIR}/{}", part_name(slot)),
                    bytes.as_slice(),
                )
            })
            .collect()
    }

    /// The `<Relationship/>` entries the document part needs to resolve its
    /// drawings, ready to splice into `word/_rels/document.xml.rels`.
    pub(super) fn relationships(&self) -> String {
        (0..self.used.len()).fold(String::new(), |mut xml, slot| {
            let _ = write!(
                xml,
                r#"<Relationship Id="{}" Type="{IMAGE_REL_TYPE}" Target="{MEDIA_DIR}/{}"/>"#,
                relationship_id(slot),
                part_name(slot),
            );
            xml
        })
    }
}

/// Where a [`Media`] registry stood at some point, so it can be rewound.
#[derive(Debug, Clone, Copy)]
pub(super) struct Mark {
    /// How many distinct images had been drawn.
    used: usize,
    /// How many drawings had been written.
    drawings: u32,
}

/// The relationship id for the image in `slot`.
fn relationship_id(slot: usize) -> String {
    format!("rIdImage{}", slot + 1)
}

/// The part name for the image in `slot`, relative to [`MEDIA_DIR`].
///
/// Numbered by slot rather than named after the authored path: a part name is
/// a URI, and an authored `figures/a b.png` would need escaping that a reader
/// and a writer can disagree about.
fn part_name(slot: usize) -> String {
    format!("image{}.png", slot + 1)
}

/// The pixel size and resolution a PNG declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Png {
    /// Width in pixels.
    width: u32,
    /// Height in pixels.
    height: u32,
    /// Horizontal resolution in dots per inch.
    dpi_x: u64,
    /// Vertical resolution in dots per inch.
    dpi_y: u64,
}

/// The printed size of an image, in EMUs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Extent {
    /// Width.
    cx: u64,
    /// Height.
    cy: u64,
}

/// Read `bytes` as a PNG header.
///
/// `None` for anything that is not a PNG, and for a PNG declaring a zero
/// dimension — which is not an image, and would divide by zero when scaled.
fn read_png(bytes: &[u8]) -> Option<Png> {
    if bytes.get(..PNG_SIGNATURE.len())? != PNG_SIGNATURE {
        return None;
    }
    // IHDR is required to be the first chunk, so its fields are at fixed
    // offsets: 8 signature, 4 length, 4 type, then width and height.
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let (width, height) = (be_u32(bytes, 16)?, be_u32(bytes, 20)?);
    if width == 0 || height == 0 {
        return None;
    }
    let (dpi_x, dpi_y) = resolution(bytes).unwrap_or((DEFAULT_DPI, DEFAULT_DPI));
    Some(Png {
        width,
        height,
        dpi_x,
        dpi_y,
    })
}

/// The `(x, y)` resolution a PNG's `pHYs` chunk declares, in DPI.
fn resolution(bytes: &[u8]) -> Option<(u64, u64)> {
    let mut at = PNG_SIGNATURE.len();
    while let Some((kind, data, next)) = chunk(bytes, at) {
        match kind {
            b"pHYs" => return dots_per_inch(data),
            // `pHYs` is required to precede the image data, so by here there
            // is none — and walking the whole file to find that out would mean
            // stepping through every IDAT of a megabyte screenshot.
            b"IDAT" | b"IEND" => return None,
            _ => at = next,
        }
    }
    None
}

/// The chunk at `at`: its type, its data, and where the next chunk starts.
fn chunk(bytes: &[u8], at: usize) -> Option<(&[u8], &[u8], usize)> {
    let length = be_u32(bytes, at)? as usize;
    let kind_end = at.checked_add(CHUNK_HEADER)?;
    let kind = bytes.get(at.checked_add(CRC_LEN)?..kind_end)?;
    let data_end = kind_end.checked_add(length)?;
    let data = bytes.get(kind_end..data_end)?;
    // Past the four-byte CRC, which is not checked: a corrupt image is the
    // decoder's business, and refusing to *place* one would trade a visible
    // bad picture for a silently missing one.
    Some((kind, data, data_end.checked_add(CRC_LEN)?))
}

/// The `(x, y)` DPI a `pHYs` chunk's `data` states.
///
/// `None` when the chunk gives a bare pixel ratio (`unit == 0`): that corrects
/// the aspect of a non-square-pixel image but says nothing about physical
/// size, so the default resolution still applies.
fn dots_per_inch(data: &[u8]) -> Option<(u64, u64)> {
    if data.get(CHUNK_HEADER) != Some(&PHYS_UNIT_METRE) {
        return None;
    }
    let (x, y) = (
        per_metre_to_dpi(be_u32(data, 0)?),
        per_metre_to_dpi(be_u32(data, 4)?),
    );
    // A resolution rounding to zero would divide by zero downstream; it also
    // describes an image metres across, which no exam figure is.
    (x > 0 && y > 0).then_some((x, y))
}

/// Pixels per metre as dots per inch, rounded to nearest.
///
/// In `u64` because the widest input — `u32::MAX` pixels per metre, about
/// 109 million DPI — would otherwise need a saturating conversion that can
/// never fire, which reads as a guarded overflow that is not one.
fn per_metre_to_dpi(per_metre: u32) -> u64 {
    (u64::from(per_metre) * 254 + 5_000) / 10_000
}

/// The big-endian `u32` at offset `at` in `bytes`.
fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let field: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(field))
}

/// The size `png` should print at, scaled down to fit the text column.
fn extent(png: &Png) -> Extent {
    fit(Extent {
        cx: u64::from(png.width) * u64::from(EMU_PER_INCH) / png.dpi_x,
        cy: u64::from(png.height) * u64::from(EMU_PER_INCH) / png.dpi_y,
    })
}

/// Scale `size` down until it fits the text area, keeping its proportions.
///
/// One step, by whichever dimension overruns by more. Clamping width and
/// height independently is the tempting version and it squashes the picture.
fn fit(size: Extent) -> Extent {
    let (max_width, max_height) = (u128::from(MAX_WIDTH), u128::from(MAX_HEIGHT));
    let (cx, cy) = (u128::from(size.cx), u128::from(size.cy));
    if cx == 0 || cy == 0 {
        // No ratio to preserve, so each axis is clamped on its own. Returning
        // the size untouched here would leak both an extent of zero — a
        // picture Word draws as nothing — and, on the other axis, a value
        // past what the schema admits, which Word rejects the file over. A
        // `pHYs` chunk states its two resolutions independently, so a corrupt
        // one reaches this with a rounded-away width and an unbounded height.
        return Extent {
            cx: shrink(cx, MAX_WIDTH),
            cy: shrink(cy, MAX_HEIGHT),
        };
    }
    if cx <= max_width && cy <= max_height {
        return size;
    }
    if cx * max_height > cy * max_width {
        Extent {
            cx: u64::from(MAX_WIDTH),
            cy: shrink(cy * max_width / cx, MAX_HEIGHT),
        }
    } else {
        Extent {
            cx: shrink(cx * max_height / cy, MAX_WIDTH),
            cy: u64::from(MAX_HEIGHT),
        }
    }
}

/// A scaled dimension as a `u64`, never larger than the `limit` it was scaled
/// to fit and never zero — a zero extent is a picture Word draws as nothing.
fn shrink(scaled: u128, limit: u32) -> u64 {
    u64::try_from(scaled)
        .unwrap_or_else(|_| u64::from(limit))
        .clamp(1, u64::from(limit))
}

/// One inline picture: a `w:drawing` run sized `extent`, resolving through
/// relationship `rel`.
///
/// `id` numbers the drawing within the document and `name` identifies the part
/// it came from; `alt` is the authored alt text, which is what a screen reader
/// announces and the only description a reader of the file ever gets.
fn drawing(rel: &str, id: u32, name: &str, alt: &str, extent: Extent) -> String {
    let alt = escape_xml(alt);
    let (cx, cy) = (extent.cx, extent.cy);
    format!(
        concat!(
            r#"<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">"#,
            r#"<wp:extent cx="{cx}" cy="{cy}"/>"#,
            r#"<wp:effectExtent l="0" t="0" r="0" b="0"/>"#,
            r#"<wp:docPr id="{id}" name="Picture {id}" descr="{alt}"/>"#,
            r#"<wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="{A_NS}" noChangeAspect="1"/></wp:cNvGraphicFramePr>"#,
            r#"<a:graphic xmlns:a="{A_NS}"><a:graphicData uri="{PIC_NS}">"#,
            r#"<pic:pic xmlns:pic="{PIC_NS}">"#,
            r#"<pic:nvPicPr><pic:cNvPr id="{id}" name="{name}" descr="{alt}"/><pic:cNvPicPr/></pic:nvPicPr>"#,
            r#"<pic:blipFill><a:blip r:embed="{rel}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>"#,
            r#"<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>"#,
            r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr>"#,
            r"</pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>",
        ),
        cx = cx,
        cy = cy,
        id = id,
        alt = alt,
        name = name,
        rel = rel,
        A_NS = super::A_NS,
        PIC_NS = super::PIC_NS,
    )
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A PNG header declaring `width` by `height` pixels, and `dpi` if given.
    ///
    /// Only the header is built: nothing here decodes an image, and a real
    /// one would hide the numbers under test behind a blob.
    pub(in crate::export::docx) fn png(width: u32, height: u32, dpi: Option<u32>) -> Vec<u8> {
        let mut bytes = Vec::from(PNG_SIGNATURE);
        bytes.extend(chunk_bytes(*b"IHDR", &ihdr(width, height)));
        if let Some(dpi) = dpi {
            let per_metre = dpi.saturating_mul(10_000) / 254;
            let mut data = Vec::new();
            data.extend(per_metre.to_be_bytes());
            data.extend(per_metre.to_be_bytes());
            data.push(PHYS_UNIT_METRE);
            bytes.extend(chunk_bytes(*b"pHYs", &data));
        }
        bytes.extend(chunk_bytes(*b"IDAT", &[0]));
        bytes
    }

    /// The IHDR payload for a `width` by `height` 8-bit truecolour image.
    fn ihdr(width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend(width.to_be_bytes());
        data.extend(height.to_be_bytes());
        data.extend([8, 2, 0, 0, 0]);
        data
    }

    /// One PNG chunk: length, type, data, and a CRC placeholder.
    ///
    /// The CRC is zeroed because nothing in this module checks it — a corrupt
    /// image is the reader's business, and refusing to *place* one would
    /// trade a visibly bad picture for a silently missing one.
    fn chunk_bytes(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(u32::try_from(data.len()).unwrap_or_default().to_be_bytes());
        bytes.extend(kind);
        bytes.extend(data);
        bytes.extend(vec![0_u8; CRC_LEN]);
        bytes
    }

    /// The images a test supplies, as one entry named `figure.png`.
    fn supplied(bytes: Vec<u8>) -> Vec<(String, Vec<u8>)> {
        vec![("figure.png".to_owned(), bytes)]
    }

    /// The `cx`/`cy` a drawing of `bytes` would print at.
    fn printed(bytes: &[u8]) -> Extent {
        let png = read_png(bytes).expect("a PNG header");
        extent(&png)
    }

    #[test]
    /// A PNG that states no resolution is placed at 96 DPI, which is what
    /// Word assumes when an image is pasted in.
    fn an_image_without_a_resolution_is_placed_at_the_default() {
        let size = printed(&png(96, 48, None));
        assert_eq!(size.cx, u64::from(EMU_PER_INCH));
        assert_eq!(size.cy, u64::from(EMU_PER_INCH) / 2);
    }

    #[test]
    /// A stated resolution is honoured. A 300 DPI screenshot is not three
    /// times the size of a 96 DPI one of the same subject, and placing it at
    /// the default would print it three times too wide — or, once the column
    /// clamp caught it, at a size that has nothing to do with the original.
    fn a_stated_resolution_is_honoured() {
        let size = printed(&png(600, 300, Some(300)));
        assert_eq!(size.cx, u64::from(EMU_PER_INCH) * 2);
        assert_eq!(size.cy, u64::from(EMU_PER_INCH));
    }

    #[test]
    /// A `pHYs` chunk with no unit gives a pixel *ratio*, not a size, so the
    /// default resolution still applies. Reading it as DPI would place a
    /// 1:1-ratio image at a hundredth of an inch.
    fn a_pixel_ratio_without_a_unit_is_not_a_resolution() {
        let mut bytes = Vec::from(PNG_SIGNATURE);
        bytes.extend(chunk_bytes(*b"IHDR", &ihdr(96, 96)));
        bytes.extend(chunk_bytes(*b"pHYs", &[0, 0, 0, 1, 0, 0, 0, 1, 0]));
        assert_eq!(printed(&bytes).cx, u64::from(EMU_PER_INCH));
    }

    #[test]
    /// An image wider than the text column is scaled down to it, and keeps
    /// its proportions. Clamping the width alone stretches the picture.
    fn an_image_wider_than_the_column_is_scaled_to_fit() {
        let size = printed(&png(96 * 20, 96 * 10, None));
        assert_eq!(size.cx, u64::from(MAX_WIDTH));
        assert_eq!(size.cy, u64::from(MAX_WIDTH) / 2, "proportions changed");
    }

    #[test]
    /// A tall narrow image is capped by the page, not the column. A picture
    /// taller than the text area does not paginate — it is cut off at the
    /// bottom margin — so the cap is the difference between a small diagram
    /// and half a diagram.
    fn an_image_taller_than_the_page_is_scaled_to_fit() {
        let size = printed(&png(96, 96 * 40, None));
        assert_eq!(size.cy, u64::from(MAX_HEIGHT));
        assert_eq!(size.cx, u64::from(MAX_HEIGHT) / 40, "proportions changed");
        assert!(size.cx <= u64::from(MAX_WIDTH));
    }

    #[test]
    /// An image already inside the text area is left at its authored size.
    fn an_image_that_fits_is_not_resized() {
        let size = printed(&png(96, 96, None));
        assert_eq!(size.cx, u64::from(EMU_PER_INCH));
        assert_eq!(size.cy, u64::from(EMU_PER_INCH));
    }

    #[test]
    /// Anything that is not a PNG is refused, so the caller prints the
    /// Markdown source instead of writing a drawing Word cannot resolve.
    fn bytes_that_are_not_a_png_are_refused() {
        for (what, bytes) in [
            ("a JPEG", vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F']),
            ("empty", Vec::new()),
            ("a truncated header", Vec::from(PNG_SIGNATURE)),
            ("a zero-width PNG", png(0, 10, None)),
            ("a zero-height PNG", png(10, 0, None)),
        ] {
            assert!(read_png(&bytes).is_none(), "{what} was accepted");
        }
    }

    #[test]
    /// SVG bytes behind a `.png` path are refused. The diagram renderer is
    /// injected, and one that ignores the format it was asked for produces
    /// exactly this; checking the signature rather than the extension is what
    /// turns a document Word cannot open into a visible fallback.
    fn an_svg_behind_a_png_path_is_refused() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>"#;
        let supplied = vec![("generated/mermaid-ab.png".to_owned(), svg.to_vec())];
        let mut media = Media::new(&supplied);
        assert!(media.drawing("generated/mermaid-ab.png", "flow").is_none());
        assert!(media.parts().is_empty(), "a part was written for an SVG");
    }

    #[test]
    /// An image nobody supplied bytes for yields no drawing — a remote URL,
    /// or a file that could not be read.
    fn an_unsupplied_image_yields_no_drawing() {
        let supplied = supplied(png(96, 96, None));
        let mut media = Media::new(&supplied);
        assert!(media.drawing("https://example.test/f.png", "").is_none());
        assert!(media.drawing("missing.png", "").is_none());
        assert!(media.parts().is_empty());
    }

    #[test]
    /// The same image placed twice is bundled once but drawn twice, with a
    /// distinct `docPr` id each time. Word rejects a document whose drawings
    /// share an id, and bundling the bytes twice would double the file for
    /// nothing.
    fn one_image_placed_twice_is_one_part_and_two_drawings() {
        let supplied = supplied(png(96, 96, None));
        let mut media = Media::new(&supplied);
        let first = media.drawing("figure.png", "a").expect("draws");
        let second = media.drawing("figure.png", "a").expect("draws");
        assert_eq!(media.parts().len(), 1, "the bytes were bundled twice");
        assert!(first.contains(r#"r:embed="rIdImage1""#), "{first}");
        assert!(second.contains(r#"r:embed="rIdImage1""#), "{second}");
        assert!(first.contains(r#"<wp:docPr id="1""#), "{first}");
        assert!(second.contains(r#"<wp:docPr id="2""#), "{second}");
    }

    #[test]
    /// Every `r:embed` a drawing cites is a relationship the package declares,
    /// pointing at a part the package holds. A dangling one is not a missing
    /// picture — Word refuses to open the document at all.
    fn every_drawing_resolves_to_a_part_the_package_holds() {
        let supplied = vec![
            ("one.png".to_owned(), png(96, 96, None)),
            ("two.png".to_owned(), png(48, 48, Some(300))),
        ];
        let mut media = Media::new(&supplied);
        let drawings = ["two.png", "one.png", "two.png"]
            .map(|url| media.drawing(url, "figure").expect("draws"))
            .concat();
        let relationships = media.relationships();
        let parts = media.parts();
        assert_eq!(parts.len(), 2);
        for embed in drawings.split(r#"r:embed=""#).skip(1) {
            let id = embed.split('"').next().unwrap_or_default();
            assert!(relationships.contains(&format!(r#"Id="{id}""#)), "{id}");
        }
        for (path, _) in &parts {
            let target = path.strip_prefix("word/").unwrap_or_default();
            let cited = relationships.contains(&format!(r#"Target="{target}""#));
            assert!(cited, "{path} is in the package but nothing points at it");
        }
    }

    #[test]
    /// Slots are assigned in the order the document first drew them, not in
    /// the order the caller happened to supply them.
    fn slots_follow_the_order_the_document_drew_them() {
        let supplied = vec![
            ("one.png".to_owned(), png(96, 96, None)),
            ("two.png".to_owned(), png(48, 48, None)),
        ];
        let mut media = Media::new(&supplied);
        let second = media.drawing("two.png", "").expect("draws");
        let first = media.drawing("one.png", "").expect("draws");
        assert!(second.contains(r#"r:embed="rIdImage1""#), "{second}");
        assert!(first.contains(r#"r:embed="rIdImage2""#), "{first}");
        let parts = media.parts();
        let paths: Vec<&str> = parts.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(paths, ["word/media/image1.png", "word/media/image2.png"]);
    }

    #[test]
    /// A part carries the bytes of the image it stands for, not of whichever
    /// one happened to be first.
    fn a_part_carries_its_own_image_bytes() {
        let supplied = vec![
            ("one.png".to_owned(), png(96, 96, None)),
            ("two.png".to_owned(), png(48, 48, None)),
        ];
        let mut media = Media::new(&supplied);
        media.drawing("two.png", "").expect("draws");
        media.drawing("one.png", "").expect("draws");
        let parts = media.parts();
        let bytes: Vec<&[u8]> = parts.iter().map(|(_, bytes)| *bytes).collect();
        assert_eq!(bytes, [png(48, 48, None), png(96, 96, None)]);
    }

    #[test]
    /// A chunk declaring an impossible length is bailed out of, not walked
    /// into. The length field is attacker-controlled in the sense that
    /// matters here: a corrupt or truncated file reaches this, and a reader
    /// that trusted the field would hang, panic, or allocate four gigabytes.
    fn a_chunk_longer_than_the_file_ends_the_walk() {
        let mut bytes = Vec::from(PNG_SIGNATURE);
        bytes.extend(chunk_bytes(*b"IHDR", &ihdr(96, 96)));
        bytes.extend(u32::MAX.to_be_bytes());
        bytes.extend(b"pHYs");
        let png = read_png(&bytes).expect("the header is still readable");
        assert_eq!((png.dpi_x, png.dpi_y), (DEFAULT_DPI, DEFAULT_DPI));
    }

    #[test]
    /// A `pHYs` after the image data is not looked for. The format requires
    /// it before `IDAT`, and walking past there would step through every
    /// data chunk of a megabyte screenshot to learn nothing.
    fn a_resolution_after_the_image_data_is_not_searched_for() {
        let mut bytes = Vec::from(PNG_SIGNATURE);
        bytes.extend(chunk_bytes(*b"IHDR", &ihdr(96, 96)));
        bytes.extend(chunk_bytes(*b"IDAT", &[0]));
        bytes.extend(chunk_bytes(
            *b"pHYs",
            &[0, 0, 0xB1, 0x2F, 0, 0, 0xB1, 0x2F, 1],
        ));
        assert_eq!(printed(&bytes).cx, u64::from(EMU_PER_INCH));
    }

    #[test]
    /// A resolution of zero, or a truncated `pHYs`, falls back to the default
    /// rather than dividing by it.
    fn an_unusable_resolution_falls_back_to_the_default() {
        for (what, data) in [
            ("zero per metre", vec![0, 0, 0, 0, 0, 0, 0, 0, 1]),
            ("zero on one axis", vec![0, 0, 0xB1, 0x2F, 0, 0, 0, 0, 1]),
            ("truncated", vec![0, 0, 0xB1, 0x2F]),
        ] {
            let mut bytes = Vec::from(PNG_SIGNATURE);
            bytes.extend(chunk_bytes(*b"IHDR", &ihdr(96, 96)));
            bytes.extend(chunk_bytes(*b"pHYs", &data));
            bytes.extend(chunk_bytes(*b"IDAT", &[0]));
            let size = printed(&bytes);
            assert_eq!(size.cx, u64::from(EMU_PER_INCH), "{what}");
            assert_eq!(size.cy, u64::from(EMU_PER_INCH), "{what}");
        }
    }

    #[test]
    /// A resolution so high that a dimension rounds away still prints inside
    /// the page, and never at zero. A `pHYs` chunk states its two
    /// resolutions independently, so a corrupt one reaches the sizing with a
    /// vanishing width and an unbounded height — and Word draws a zero
    /// extent as nothing while rejecting an oversized one outright.
    fn a_dimension_that_rounds_away_is_still_placed() {
        let mut bytes = Vec::from(PNG_SIGNATURE);
        bytes.extend(chunk_bytes(*b"IHDR", &ihdr(100, 2_000_000_000)));
        // 4e9 pixels per metre on x is about 101 million DPI; 20 on y is
        // under one, so the two axes round in opposite directions.
        let mut phys = Vec::new();
        phys.extend(4_000_000_000_u32.to_be_bytes());
        phys.extend(20_u32.to_be_bytes());
        phys.push(PHYS_UNIT_METRE);
        bytes.extend(chunk_bytes(*b"pHYs", &phys));
        bytes.extend(chunk_bytes(*b"IDAT", &[0]));
        let size = printed(&bytes);
        assert!(size.cx >= 1, "a zero-width picture draws as nothing");
        assert!(size.cx <= u64::from(MAX_WIDTH), "{size:?}");
        assert!(size.cy <= u64::from(MAX_HEIGHT), "{size:?}");
    }

    #[test]
    /// When both dimensions overrun, the one that overruns by more decides
    /// the scale — otherwise fitting the width would leave the height over.
    fn the_worse_overrun_decides_the_scale() {
        let size = printed(&png(96 * 20, 96 * 40, None));
        assert_eq!(size.cy, u64::from(MAX_HEIGHT), "height should bind");
        assert!(size.cx < u64::from(MAX_WIDTH), "{size:?}");
        assert_eq!(size.cx, u64::from(MAX_HEIGHT) / 2, "proportions changed");
    }

    #[test]
    /// A shape so extreme that its minor axis scales to nothing keeps one
    /// EMU of it. Zero is not a thin picture, it is no picture.
    fn a_scaled_away_minor_axis_keeps_one_emu() {
        let size = printed(&png(6_000_000, 1, None));
        assert_eq!(size.cx, u64::from(MAX_WIDTH));
        assert_eq!(size.cy, 1);
    }

    #[test]
    /// Pixels per metre round to the nearest DPI rather than truncating:
    /// 3780 per metre is the 96 DPI almost every tool writes, and truncating
    /// it yields 95, which shows up as an image 1% too large.
    fn a_resolution_rounds_to_the_nearest_dot_per_inch() {
        assert_eq!(per_metre_to_dpi(3_780), 96);
        assert_eq!(per_metre_to_dpi(3_779), 96);
        assert_eq!(per_metre_to_dpi(11_811), 300);
        assert_eq!(per_metre_to_dpi(0), 0);
    }

    #[test]
    /// An image with no alt text still carries the attribute. Absent and
    /// empty are different to a screen reader: absent invites it to read the
    /// file name instead.
    fn an_image_without_alt_text_still_carries_the_attribute() {
        let supplied = supplied(png(96, 96, None));
        let mut media = Media::new(&supplied);
        let xml = media.drawing("figure.png", "").expect("draws");
        assert!(xml.contains(r#"descr="""#), "{xml}");
    }

    #[test]
    /// Rewinding discards what was drawn since the mark, and only that. A
    /// paragraph that falls back after placing an image must not leave the
    /// bytes behind in a package that no longer draws them.
    fn rewinding_discards_only_what_came_after_the_mark() {
        let supplied = vec![
            ("kept.png".to_owned(), png(96, 96, None)),
            ("dropped.png".to_owned(), png(48, 48, None)),
        ];
        let mut media = Media::new(&supplied);
        media.drawing("kept.png", "").expect("draws");
        let mark = media.mark();
        media.drawing("dropped.png", "").expect("draws");
        media.rewind(mark);
        assert_eq!(media.parts().len(), 1, "the kept image went too");
        // The next slot is the one the rewind freed, not one past it.
        let next = media.drawing("dropped.png", "").expect("draws");
        assert!(next.contains(r#"r:embed="rIdImage2""#), "{next}");
        assert!(next.contains(r#"<wp:docPr id="2""#), "{next}");
    }

    #[test]
    /// An image an earlier paragraph placed survives a later rewind: the
    /// lookup finds it rather than pushing it again, so there is nothing
    /// after the mark to truncate.
    fn a_rewind_cannot_discard_an_earlier_paragraphs_image() {
        let supplied = supplied(png(96, 96, None));
        let mut media = Media::new(&supplied);
        media.drawing("figure.png", "").expect("draws");
        let mark = media.mark();
        media.drawing("figure.png", "").expect("draws");
        media.rewind(mark);
        assert_eq!(media.parts().len(), 1, "the first placement was lost");
    }

    #[test]
    /// A relationship target and the part it names are derived from one
    /// constant, so they cannot drift into an `r:embed` pointing at nothing.
    fn a_relationship_target_names_the_part_that_is_written() {
        let supplied = supplied(png(96, 96, None));
        let mut media = Media::new(&supplied);
        media.drawing("figure.png", "").expect("draws");
        let relationships = media.relationships();
        for (path, _) in &media.parts() {
            let target = path.strip_prefix("word/").unwrap_or_default();
            assert!(
                relationships.contains(&format!(r#"Target="{target}""#)),
                "{path}"
            );
        }
    }

    #[test]
    /// Alt text reaches the description, escaped. It is what a screen reader
    /// announces, and an unescaped `&` makes the whole document unreadable.
    fn alt_text_is_escaped_into_the_description() {
        let supplied = supplied(png(96, 96, None));
        let mut media = Media::new(&supplied);
        let xml = media.drawing("figure.png", "A & B <tree>").expect("draws");
        assert!(xml.contains(r#"descr="A &amp; B &lt;tree&gt;""#), "{xml}");
        assert!(!xml.contains("<tree>"), "{xml}");
    }
}
