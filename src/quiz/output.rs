//! The files one quiz run produces, as bytes with the names to write them to.
//!
//! The last stage of the printable-exam pipeline. [`assemble`](super::assemble)
//! decides *what* each variant holds and [`export::docx`](crate::export::docx)
//! renders one variant; this pairs every variant with its answer key, adds the
//! run manifest, and names the results — so the CLI does nothing but write
//! bytes to paths.
//!
//! # Every variant gets its own key
//!
//! A variant is a different paper: different questions, or the same questions
//! with their options in a different order. One key cannot grade two of them —
//! the letter a student circled means something different on each — so a key
//! is produced per variant, from the same [`Exam`](crate::quiz::exam::Exam) the sheet was rendered
//! from. Handing out variant B and grading it against key A is the failure
//! this pairing exists to make impossible.

use crate::Result;
use crate::export::docx;
use crate::quiz::assemble::Assembly;
use crate::quiz::manifest::Manifest;

/// The extension every file here is written with.
const EXTENSION: &str = "docx";

/// The suffix marking a file as the instructor's copy.
const KEY_SUFFIX: &str = "key";

/// The name of the run record, and its extension.
const MANIFEST_SUFFIX: &str = "manifest.yaml";

/// One file a quiz run produces.
#[derive(Debug, Clone)]
pub struct OutputFile {
    /// The file's name, extension included.
    ///
    /// A bare name, not a path: where these land is the caller's decision, and
    /// the library has no business reaching outside the directory it was given.
    pub name: String,
    /// The file's contents.
    pub bytes: Vec<u8>,
}

/// Every file `assembly` should be written as, named from `stem`.
///
/// Two per variant — the student's sheet and the instructor's key — and one
/// manifest recording what the run drew. A single-variant quiz is named for
/// its stem alone, because an `-A` implies a `-B` that does not exist.
///
/// `seed` is the one thing here that is not derivable from `assembly`: it is
/// what reproduces the run, so the manifest has to be told it.
///
/// `images` supplies the bytes behind every local image the questions
/// reference, keyed by the authored path — rendered diagrams included. The
/// library never reads a file, so an image with no entry here is one the
/// writer prints the Markdown source of instead. The same set is handed to
/// every variant; each document embeds only the images it actually drew.
///
/// # Errors
///
/// Returns [`crate::Error`] if any variant holds content the Word writer
/// cannot render, or if the manifest cannot be serialised.
pub fn render(
    assembly: &Assembly,
    stem: &str,
    seed: u64,
    images: &[(String, Vec<u8>)],
) -> Result<Vec<OutputFile>> {
    let mut files = Vec::new();
    for exam in &assembly.exams {
        files.push(OutputFile {
            name: file_name(stem, exam.variant.as_deref(), false),
            bytes: docx::to_docx(exam, images)?,
        });
        files.push(OutputFile {
            name: file_name(stem, exam.variant.as_deref(), true),
            bytes: docx::to_answer_key(exam, images)?,
        });
    }
    files.push(OutputFile {
        name: format!("{stem}-{MANIFEST_SUFFIX}"),
        bytes: Manifest::of(assembly, seed).to_yaml()?.into_bytes(),
    });
    Ok(files)
}

/// The file name for one variant's sheet, or its key.
///
/// The variant comes before the key suffix so a directory listing groups a
/// sheet with the key that grades it (`exam-A`, `exam-A-key`) rather than
/// filing every key away from the paper it belongs to.
fn file_name(stem: &str, variant: Option<&str>, key: bool) -> String {
    let mut name = stem.to_owned();
    if let Some(variant) = variant {
        name.push('-');
        name.push_str(variant);
    }
    if key {
        name.push('-');
        name.push_str(KEY_SUFFIX);
    }
    name.push('.');
    name.push_str(EXTENSION);
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Feedback, Question, QuestionKind, TrueFalse};
    use crate::quiz::exam::{Exam, ExamItem};
    use crate::quiz::spec::Layout;

    /// An exam of one true/false question, labelled `variant`.
    fn exam(variant: Option<&str>) -> Exam {
        Exam {
            name: "Exam 1".to_owned(),
            variant: variant.map(ToOwned::to_owned),
            header: None,
            footer: None,
            layout: Layout::default(),
            items: vec![ExamItem {
                question: Question {
                    id: "q0".to_owned(),
                    title: None,
                    prompt: "True?".to_owned(),
                    points: 1.0,
                    tags: Vec::new(),
                    feedback: Feedback::default(),
                    kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
                },
                answer_space: 3,
                option_order: Vec::new(),
            }],
        }
    }

    /// An assembly of the given variants, with no warnings.
    fn assembly(variants: &[Option<&str>]) -> Assembly {
        Assembly {
            exams: variants.iter().copied().map(exam).collect(),
            warnings: Vec::new(),
        }
    }

    #[test]
    /// Every variant is written as a sheet *and* a key. One key cannot grade
    /// two papers — a letter circled on variant B means something else on A —
    /// so a missing key here is a pile of exams nobody can mark.
    fn every_variant_gets_a_sheet_and_a_key() {
        let files = render(
            &assembly(&[Some("A"), Some("B"), Some("C")]),
            "midterm",
            1,
            &[],
        )
        .expect("renders");
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "midterm-A.docx",
                "midterm-A-key.docx",
                "midterm-B.docx",
                "midterm-B-key.docx",
                "midterm-C.docx",
                "midterm-C-key.docx",
                "midterm-manifest.yaml",
            ]
        );
    }

    #[test]
    /// A lone sheet carries no variant in its name: an `-A` implies a `-B`
    /// that does not exist.
    fn a_single_variant_is_named_for_its_stem_alone() {
        let files = render(&assembly(&[None]), "quiz", 1, &[]).expect("renders");
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, ["quiz.docx", "quiz-key.docx", "quiz-manifest.yaml"]);
    }

    #[test]
    /// Each key is rendered from the variant it accompanies, not from the
    /// first. Rendering one key and copying it would pass a test that only
    /// counted files.
    fn each_key_belongs_to_its_own_variant() {
        let mut quiz = assembly(&[Some("A"), Some("B")]);
        if let Some(second) = quiz.exams.get_mut(1)
            && let Some(item) = second.items.first_mut()
        {
            item.question.kind = QuestionKind::TrueFalse(TrueFalse { answer: false });
        }
        let files = render(&quiz, "exam", 1, &[]).expect("renders");
        let key_of = |name: &str| {
            files
                .iter()
                .find(|file| file.name == name)
                .map(|file| String::from_utf8_lossy(&file.bytes).into_owned())
                .unwrap_or_default()
        };
        assert_ne!(
            key_of("exam-A-key.docx"),
            key_of("exam-B-key.docx"),
            "both keys came out identical for variants with different answers"
        );
    }

    #[test]
    /// A sheet and its key are named so a listing groups them together, and
    /// the key is never mistaken for another variant's paper.
    fn names_group_a_sheet_with_its_key() {
        assert_eq!(file_name("exam", Some("A"), false), "exam-A.docx");
        assert_eq!(file_name("exam", Some("A"), true), "exam-A-key.docx");
        assert_eq!(file_name("exam", None, false), "exam.docx");
        assert_eq!(file_name("exam", None, true), "exam-key.docx");
    }

    #[test]
    /// A file name is a bare name, never a path: the library has no business
    /// reaching outside the directory the caller chose.
    fn a_file_name_never_escapes_its_directory() {
        let files = render(&assembly(&[Some("A"), None]), "exam", 1, &[]).expect("renders");
        for file in &files {
            assert!(
                !file.name.contains('/') && !file.name.contains('\\'),
                "{} is a path, not a name",
                file.name
            );
            assert!(!file.name.contains(".."), "{} climbs out", file.name);
        }
    }

    #[test]
    /// An empty assembly produces no sheets rather than a blank document. The
    /// manifest is still written: a run that drew nothing is worth a record
    /// saying so, with the seed that did it.
    fn an_assembly_with_no_variants_produces_only_a_manifest() {
        let files = render(&assembly(&[]), "exam", 1, &[]).expect("renders");
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(
            names,
            ["exam-manifest.yaml"],
            "a spec that selected nothing left a blank exam behind"
        );
    }
}
