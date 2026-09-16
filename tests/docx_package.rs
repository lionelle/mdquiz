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

use mdquiz::export::docx::to_docx;
use mdquiz::model::{Feedback, Question, QuestionKind, TrueFalse};
use mdquiz::quiz::exam::{Exam, ExamItem};
use mdquiz::quiz::spec::Layout;

/// A page-footer template exercising every placeholder.
const FOOTER: &str = "${name} (${variant}) — Page ${page} of ${pages}";

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
        header: Some("Answer every question.\n\nShow your working.".to_owned()),
        footer: Some("End of exam.".to_owned()),
        layout: Layout {
            page_footer: Some(FOOTER.to_owned()),
            ..Layout::default()
        },
        items: (0..3).map(sample_item).collect(),
    }
}

/// One true/false item whose prompt carries XML-significant characters.
fn sample_item(n: usize) -> ExamItem {
    ExamItem {
        question: Question {
            id: format!("q{n}"),
            title: None,
            prompt: format!("Is statement {n} true? Consider <a> & \"b\"."),
            points: 1.0,
            tags: Vec::new(),
            feedback: Feedback::default(),
            kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
        },
        answer_space: 4,
    }
}

/// Every `(name, contents)` part of the generated package.
fn package_parts() -> Vec<(String, Vec<u8>)> {
    let bytes = to_docx(&sample_exam()).expect("the exam renders");
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
    let parts = package_parts();
    assert!(!parts.is_empty(), "the package holds no parts");
    for (name, contents) in parts {
        assert_well_formed(dir.path(), &name, &contents);
    }
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
    assert!(rendered.contains("CS 3500"), "title missing: {rendered}");
    assert!(
        rendered.contains("Answer every question."),
        "header missing"
    );
    assert!(rendered.contains("End of exam."), "footer missing");
    assert!(rendered.contains("Page 1 of"), "page footer missing");
    // The prompt's XML-significant characters survive as themselves.
    assert!(
        rendered.contains("<a> & \"b\""),
        "escaping round-trip failed"
    );
}
