//! End-to-end checks on the generated Word package.
//!
//! The unit tests in `export::docx` assert on XML substrings, which is fine for
//! catching a wrong attribute but cannot tell you the document opens. Word
//! refuses a malformed package outright, so these tests exercise the two things
//! substring matching misses: that every part is well-formed XML, and that a
//! real word processor can read the result.
//!
//! The well-formedness check runs in the normal gate — CI installs `xmllint`
//! for it, and the test fails rather than skips when `CI` is set. The
//! conversion check is `#[ignore]`d: it needs `LibreOffice`, so it is opt-in
//! via `cargo test -- --ignored` and does **not** run automatically. Asking for
//! it means run it or fail; it will not quietly skip.
#![expect(
    clippy::expect_used,
    reason = "clippy.toml allows expect in tests, but that allowance does not \
              reach helper fns in an integration-test crate; this file is all test code"
)]

use std::io::Read as _;
use std::path::Path;
use std::process::Command;

use mdquiz::export::docx::{to_answer_key, to_docx};
use mdquiz::model::{
    Choice, Feedback, MatchPair, Matching, MultipleSelect, Question, QuestionKind, ScoringMode,
    TrueFalse,
};
use mdquiz::quiz::exam::{Exam, ExamItem};
use mdquiz::quiz::spec::Layout;

/// A page-footer template exercising every placeholder.
const FOOTER: &str = "${name} (${variant}) — Page ${page} of ${pages}";

/// A header block exercising a heading, inline marks, and bulleted, nested and
/// ordered lists — including two ordered lists that must each start at 1.
const HEADER: &str = "\
# Instructions

Answer every question. Show your *working* and mark the final answer in `ink`.

- No calculators.
- ~~No~~ notes, except:
  - one index card;
  - a calculator with no $\\log$ key.

1. Write in ink.
2. Sign every page.";

/// Whether `tool` can be run on this machine.
///
/// Spawning is the test, not the exit status: `pdftotext --version` reads
/// `--version` as a filename and exits non-zero, so a status check would
/// report a perfectly good tool as missing.
fn available(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

/// A small exam covering the container's moving parts.
fn sample_exam() -> Exam {
    Exam {
        name: "CS 3500 — Exam 1".to_owned(),
        variant: Some("A".to_owned()),
        header: Some(HEADER.to_owned()),
        footer: Some("End of exam.".to_owned()),
        layout: Layout {
            page_footer: Some(FOOTER.to_owned()),
            ..Layout::default()
        },
        items: (0..PROMPTS.len()).map(sample_item).collect(),
    }
}

/// The prompts the sample exam asks, one per construct worth rendering.
///
/// Every entry is something the writer handles differently: escaping, inline
/// math, display math and a multi-paragraph prompt. Written as prompts on a
/// real exam rather than as a test fixture, so the converted PDF can be read.
const PROMPTS: [&str; 8] = [
    "Is statement 0 true? Consider <a> & \"b\".",
    r"Sort in $O(n \log n)$ time and say **why** it is not $O(n)$.",
    r"Evaluate $$\sum_{i=1}^{n} i^2$$ in closed form.",
    "State the invariant.\n\nThen prove it is *maintained*.",
    "Prove the identity:\n\n$$e^{i\\pi} + 1 = 0$$",
    "Answer both parts:\n\n1. State the rule.\n\n   Then name it.\n2. Apply it to $n = 2^{10}$.",
    "Rank these, slowest first:\n\n- bubble sort\n- merge sort\n- counting sort",
    // A code block and a pipe table: the two constructs set preformatted
    // rather than laid out. Both are here so the converter reads a document
    // holding them, and so the rows and the indent are checked on a real page.
    "Trace the loop and complete the table:\n\n```\nfor i in 0..n:\n    total += i\n```\n\n\
     | n | total |\n|---|-------|\n| 3 |       |",
];

/// One item asking the `n`th prompt, of the `n`th kind.
fn sample_item(n: usize) -> ExamItem {
    ExamItem {
        question: Question {
            id: format!("q{n}"),
            title: None,
            prompt: PROMPTS.get(n).copied().unwrap_or_default().to_owned(),
            points: 1.0,
            tags: Vec::new(),
            feedback: Feedback::default(),
            kind: sample_kind(n),
        },
        answer_space: 4,
        option_order: Vec::new(),
    }
}

/// The question kind for item `n`.
///
/// Varied rather than all true/false so more than one answer structure reaches
/// the checks in this file: substring assertions in `export::docx` cannot tell
/// you the document *opens*, and a matching question is the one that emits an
/// empty paragraph as a separator — the likeliest thing to come out wrong on a
/// real page. Every other index stays true/false so the prompts above keep
/// exercising what they were written for.
fn sample_kind(n: usize) -> QuestionKind {
    match n {
        1 => QuestionKind::MultipleSelect(MultipleSelect {
            choices: vec![
                Choice {
                    text: "*merge* sort".to_owned(),
                    correct: true,
                },
                Choice {
                    text: "bubble sort".to_owned(),
                    correct: false,
                },
            ],
            scoring: ScoringMode::default(),
        }),
        2 => QuestionKind::Matching(Matching {
            pairs: vec![
                MatchPair {
                    left: "char".to_owned(),
                    right: "1 byte".to_owned(),
                },
                MatchPair {
                    left: "int".to_owned(),
                    right: "4 bytes".to_owned(),
                },
            ],
            distractors: vec!["8 bytes".to_owned()],
        }),
        _ => QuestionKind::TrueFalse(TrueFalse { answer: true }),
    }
}

/// Every `(name, contents)` part of the generated package.
fn package_parts() -> Vec<(String, Vec<u8>)> {
    parts_of(to_docx(&sample_exam()).expect("the exam renders"))
}

/// Every `(name, contents)` part of the generated answer key.
fn key_parts() -> Vec<(String, Vec<u8>)> {
    parts_of(to_answer_key(&sample_exam()).expect("the key renders"))
}

/// Every `(name, contents)` part of the package in `bytes`.
fn parts_of(bytes: Vec<u8>) -> Vec<(String, Vec<u8>)> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("the package is a zip");
    // By index rather than by name: `file_names` would borrow the archive
    // immutably while reading each part needs it mutably.
    (0..archive.len())
        .map(|index| {
            let mut file = archive.by_index(index).expect("the part is readable");
            let name = file.name().to_owned();
            let mut contents = Vec::new();
            file.read_to_end(&mut contents)
                .expect("the part is readable");
            (name, contents)
        })
        .collect()
}

/// Assert that `contents` parses as XML, naming `part` if it does not.
fn assert_well_formed(dir: &Path, part: &str, contents: &[u8]) {
    let path = dir.join(part.replace('/', "_"));
    std::fs::write(&path, contents).expect("the part is written");
    let check = Command::new("xmllint")
        .arg("--noout")
        .arg(&path)
        .output()
        .expect("xmllint runs");
    assert!(
        check.status.success(),
        "{part} is not well-formed XML: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
/// Every XML part in the package parses. A substring assertion happily passes
/// on a document with an unclosed tag; Word does not.
fn every_part_is_well_formed_xml() {
    if !available("xmllint") {
        // Skipping locally is a courtesy; skipping in CI would mean this check
        // silently never runs, and libtest swallows the note on a green test.
        assert!(
            std::env::var_os("CI").is_none(),
            "xmllint is required in CI — see the install step in ci.yml"
        );
        eprintln!("skipping: xmllint not installed");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    // Both documents: the key is a second body through the same container, and
    // Word refuses a malformed one just as readily as a malformed sheet.
    for (label, parts) in [("sheet", package_parts()), ("key", key_parts())] {
        assert!(!parts.is_empty(), "the {label} holds no parts");
        for (name, contents) in parts {
            assert_well_formed(dir.path(), &format!("{label}-{name}"), &contents);
        }
    }
}

#[test]
/// A preformatted block reaches `document.xml` with its whitespace intact and
/// in the code style.
///
/// Pinned on the XML rather than on the converted page because `LibreOffice`'s
/// plain-text filter collapses runs of spaces: the conversion can show that
/// the code block is *there* and says nothing about its indent. Alignment is
/// the whole reason these blocks are set preformatted, so it is checked where
/// it is actually observable — and this test runs in the gate, while the
/// conversion is `#[ignore]`d.
fn a_preformatted_block_keeps_its_whitespace_in_the_xml() {
    let document = sample_document();
    assert!(
        document.contains(">    total += i<"),
        "the code block lost its indent: {document}"
    );
    assert!(
        document.contains("| 3 |       |"),
        "the table lost its column padding: {document}"
    );
    assert!(
        document.contains(r#"<w:rStyle w:val="Code"/>"#),
        "the preformatted block is not monospace: {document}"
    );
}

/// Convert `docx` to a PDF beside it, returning that path.
///
/// Runs with a private profile: a `LibreOffice` already open on this machine
/// otherwise makes the conversion exit zero having written nothing.
fn convert_to_pdf(dir: &Path, docx: &Path) -> std::path::PathBuf {
    let profile = dir.join("profile");
    let convert = Command::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.display()
        ))
        .args(["--headless", "--convert-to", "pdf", "--outdir"])
        .arg(dir)
        .arg(docx)
        .output()
        .expect("soffice runs");
    assert!(convert.status.success(), "conversion failed");
    dir.join("exam.pdf")
}

#[test]
#[ignore = "requires LibreOffice; run with `cargo test -- --ignored`"]
/// A real word processor opens the document and produces a PDF whose text is
/// the exam. This is the closest thing to opening it in Word that can be
/// automated here.
fn libreoffice_converts_the_document() {
    assert!(
        available("soffice"),
        "LibreOffice is required: this test is `#[ignore]`d, so asking for it \
         with `--ignored` means run it or fail, not skip it"
    );
    let dir = tempfile::tempdir().expect("temp dir");
    let docx = dir.path().join("exam.docx");
    let bytes = to_docx(&sample_exam()).expect("the exam renders");
    std::fs::write(&docx, bytes).expect("the document is written");

    let pdf = convert_to_pdf(dir.path(), &docx);
    // Exit status alone is not enough: soffice returns 0 having written nothing
    // when its profile is locked.
    let size = std::fs::metadata(&pdf).map_or(0, |meta| meta.len());
    assert!(size > 0, "conversion produced no PDF at {}", pdf.display());

    assert!(
        available("pdftotext"),
        "pdftotext is required to check what the converted page actually says"
    );
    assert_pdf_reads_as_the_exam(&pdf);
}

/// Assert the converted PDF's text is the exam we rendered.
fn assert_pdf_reads_as_the_exam(pdf: &Path) {
    let text = Command::new("pdftotext")
        .arg(pdf)
        .arg("-")
        .output()
        .expect("pdftotext runs");
    let rendered = String::from_utf8_lossy(&text.stdout);
    assert_furniture(&rendered);
    assert_prompts(&rendered);
}

/// Assert the page furniture — title, header, footer, page numbers — printed.
fn assert_furniture(rendered: &str) {
    assert!(rendered.contains("CS 3500"), "title missing: {rendered}");
    assert!(rendered.contains("Instructions"), "header heading missing");
    assert!(
        rendered.contains("No calculators."),
        "the header list lost its text: {rendered}"
    );
    assert!(
        rendered.contains("one index card"),
        "the nested list lost its text: {rendered}"
    );
    // Both ordered lists must start at 1. Sharing a `w:numId` would make the
    // second continue the first, printing "3." and "4." in the prompt.
    for opener in ["1. Write in ink.", "1. State the rule."] {
        assert!(
            rendered.contains(opener),
            "the ordered list opening {opener:?} did not restart: {rendered}"
        );
    }
    assert!(rendered.contains("End of exam."), "footer missing");
    assert!(rendered.contains("Page 1 of"), "page footer missing");
    assert_page_numbering(rendered);
}

/// Assert the page footer's `${page}` and `${pages}` fields both resolved.
///
/// Checked as a relationship rather than against a literal count: `${pages}`
/// is resolved by the word processor, and the proof it updated is that the
/// last page's own number equals the total. Pinning "Page 2 of 2" instead made
/// this fail whenever the sample exam grew — which it does every time a part
/// of the plan lands — for a reason that had nothing to do with the footer.
fn assert_page_numbering(rendered: &str) {
    let footers = page_footers(rendered);
    assert!(!footers.is_empty(), "no page footer resolved: {rendered}");
    let total = footers.iter().map(|(_, total)| *total).max().unwrap_or(0);
    assert!(
        footers.iter().all(|(_, each)| *each == total),
        "the footers disagree on the total: {footers:?}"
    );
    // The numbers themselves, not just the largest: `${page}` resolving to 1
    // on every page of a three-page sheet satisfies any check on the maximum,
    // and a stale `${page}` is precisely what this is here to catch.
    assert_eq!(
        footers.iter().map(|(page, _)| *page).collect::<Vec<_>>(),
        (1..=total).collect::<Vec<_>>(),
        "the page numbers are not 1..={total}, so a field went stale: {footers:?}"
    );
}

/// Every `Page N of M` the footer resolved to, in the order they appear.
fn page_footers(rendered: &str) -> Vec<(usize, usize)> {
    rendered
        .split("Page ")
        .skip(1)
        .filter_map(|rest| rest.split_once(" of "))
        .filter_map(|(page, rest)| {
            let total = rest.split(|ch: char| !ch.is_ascii_digit()).next()?;
            Some((page.parse().ok()?, total.parse().ok()?))
        })
        .collect()
}

/// Assert every prompt printed as the writer claims to render it.
///
/// Math is checked from both sides: that the LaTeX source did *not* print,
/// and that the symbols it stands for did. Either alone passes on a page
/// where the equation silently vanished.
fn assert_prompts(rendered: &str) {
    // The prompt's XML-significant characters survive as themselves.
    assert!(
        rendered.contains("<a> & \"b\""),
        "escaping round-trip failed"
    );
    for source in [r"\log", r"\sum", r"\pi"] {
        assert!(
            !rendered.contains(source),
            "LaTeX source printed: {rendered}"
        );
    }
    for symbol in ['∑', 'π'] {
        assert!(
            rendered.contains(symbol),
            "{symbol} is missing from the page: {rendered}"
        );
    }
    assert!(
        rendered.contains("Then prove it is maintained."),
        "a prompt's second paragraph is missing: {rendered}"
    );
    // Preformatted blocks reach the page as separate lines rather than one
    // run-on line, which is what the explicit `w:br` runs buy. Only the
    // *content* is checked here: LibreOffice's plain-text filter collapses
    // runs of spaces, so the indent and the column padding are pinned on the
    // XML instead, by `a_preformatted_block_keeps_its_whitespace_in_the_xml`.
    assert!(
        rendered.contains("total += i"),
        "the code block is missing from the page: {rendered}"
    );
    assert!(
        rendered.contains("| n | total |"),
        "the table lost its row: {rendered}"
    );
    assert_answer_structures(rendered);
}

/// Assert the answer structures reached the page, on a document a word
/// processor actually read.
///
/// The per-kind shapes are asserted on the XML in `export::docx`; what this
/// adds is that they survive to a laid-out page — in particular the empty
/// paragraph a matching question puts between its prompts and its options,
/// which is the one line here that carries no text of its own.
fn assert_answer_structures(rendered: &str) {
    // Multiple select: a box per option, and the option's italics do not eat
    // its text.
    assert!(
        rendered.contains("[ ] A. merge sort"),
        "the multiple-select options are missing: {rendered}"
    );
    // Matching: numbered prompts with a blank each, then every option
    // lettered, distractor included.
    for line in ["____ 1. char", "____ 2. int"] {
        assert!(
            rendered.contains(line),
            "the matching prompt {line:?} is missing: {rendered}"
        );
    }
    for right in ["1 byte", "4 bytes", "8 bytes"] {
        assert!(
            rendered.contains(right),
            "the matching option {right:?} is missing: {rendered}"
        );
    }
    // True/false still prints both options and neither answer.
    assert!(
        rendered.contains("[ ] True") && rendered.contains("[ ] False"),
        "the true/false options are missing: {rendered}"
    );
}

/// The `word/document.xml` of the sample exam.
fn sample_document() -> String {
    package_parts()
        .into_iter()
        .find(|(name, _)| name == "word/document.xml")
        .map(|(_, contents)| String::from_utf8_lossy(&contents).into_owned())
        .expect("the package holds word/document.xml")
}
