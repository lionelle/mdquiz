//! Command-line interface for the `mdquiz` binary.
//!
//! The CLI is a thin shell over the library: it walks a directory of question
//! files, hands their contents to [`mdquiz::parse`], and writes the chosen
//! export target to disk. All real work lives in the library so it can be
//! tested without a process.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};

use mdquiz::export::{self, canvas, markdown};
use mdquiz::model::ItemBank;
use mdquiz::parse;

/// Author quizzes in Markdown + YAML and export them for print or Canvas.
#[derive(Debug, Parser)]
#[command(name = "mdquiz", version, about)]
pub(crate) struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Assemble a directory of question files into an item bank and export it.
    Export {
        /// Directory of Markdown question files (one question per file).
        dir: PathBuf,
        /// Path to write the exported item bank to.
        #[arg(short, long)]
        output: PathBuf,
        /// Export format to produce.
        #[arg(short, long, value_enum)]
        format: FormatArg,
        /// Bank name; defaults to the directory's own name.
        #[arg(short, long)]
        name: Option<String>,
    },
}

/// The export format as selected on the command line.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum FormatArg {
    /// Print-ready Markdown with no answer key.
    Markdown,
    /// Canvas New Quizzes item bank (QTI package).
    Canvas,
}

impl From<FormatArg> for export::Format {
    /// Map the CLI-facing format onto the library's [`export::Format`].
    fn from(arg: FormatArg) -> Self {
        match arg {
            FormatArg::Markdown => Self::Markdown,
            FormatArg::Canvas => Self::Canvas,
        }
    }
}

/// Run the parsed CLI to completion.
///
/// # Errors
///
/// Returns any error from reading the sources, parsing them, exporting the
/// bank, or writing the output file.
pub(crate) fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Export {
            dir,
            output,
            format,
            name,
        } => {
            let sources = read_question_sources(&dir)?;
            let bank = parse::item_bank_from_sources(bank_name(&dir, name), sources)?;
            let reminders = write_export(&bank, format.into(), &output)?;
            print_import_reminders(&reminders);
            Ok(())
        }
    }
}

/// Print any post-import manual-fix reminders to stderr after a Canvas export.
fn print_import_reminders(reminders: &[String]) {
    if reminders.is_empty() {
        return;
    }
    eprintln!(
        "\nAfter importing into Canvas, fix {} item(s) manually:",
        reminders.len()
    );
    for reminder in reminders {
        eprintln!("  - {reminder}");
    }
}

/// Read every `*.md` file directly in `dir` as a `(filename, content)` pair.
///
/// The listing is not sorted here; deterministic ordering is the library's job
/// in [`parse::item_bank_from_sources`]. Subdirectories and non-Markdown files
/// are skipped.
///
/// # Errors
///
/// Returns an error if the directory or any question file cannot be read.
fn read_question_sources(dir: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let mut sources = Vec::new();
    let entries = fs::read_dir(dir)
        .with_context(|| format!("reading question directory {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading an entry in {}", dir.display()))?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(OsStr::to_str) != Some("md") {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading question file {}", path.display()))?;
        sources.push((entry.file_name().to_string_lossy().into_owned(), content));
    }
    Ok(sources)
}

/// Choose the bank name: the `--name` override, else the directory's own name.
fn bank_name(dir: &Path, name: Option<String>) -> String {
    name.unwrap_or_else(|| {
        dir.file_name().map_or_else(
            || "item-bank".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        )
    })
}

/// Render `bank` in `format` and write the result to `output`.
///
/// Returns any post-import manual-fix reminders for the chosen format (empty
/// for the print sheet; the Canvas caveats for a QTI export).
///
/// # Errors
///
/// Returns an error if rendering fails or the output file cannot be written.
fn write_export(
    bank: &ItemBank,
    format: export::Format,
    output: &Path,
) -> anyhow::Result<Vec<String>> {
    match format {
        export::Format::Markdown => {
            let rendered = markdown::to_print_markdown(bank)?;
            fs::write(output, rendered)
                .with_context(|| format!("writing Markdown to {}", output.display()))?;
            Ok(Vec::new())
        }
        export::Format::Canvas => {
            let bytes = canvas::to_qti(bank)?;
            fs::write(output, bytes)
                .with_context(|| format!("writing Canvas package to {}", output.display()))?;
            Ok(canvas::import_reminders(bank))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// The CLI format maps one-to-one onto the library's export format.
    fn format_arg_maps_to_export_format() {
        assert_eq!(
            export::Format::from(FormatArg::Markdown),
            export::Format::Markdown
        );
        assert_eq!(
            export::Format::from(FormatArg::Canvas),
            export::Format::Canvas
        );
    }

    #[test]
    /// An explicit `--name` overrides the directory-derived default.
    fn bank_name_prefers_explicit_override() {
        let name = bank_name(
            Path::new("/courses/module01"),
            Some("Final Exam".to_owned()),
        );
        assert_eq!(name, "Final Exam");
    }

    #[test]
    /// With no override the directory's own name becomes the bank name.
    fn bank_name_defaults_to_dir_name() {
        assert_eq!(
            bank_name(Path::new("/courses/cs3500/module01"), None),
            "module01"
        );
    }

    #[test]
    /// A path with no final component falls back to a fixed label.
    fn bank_name_falls_back_when_no_dir_name() {
        assert_eq!(bank_name(Path::new("/"), None), "item-bank");
    }

    #[test]
    /// Only `*.md` files are read; other files and subdirectories are skipped.
    fn read_question_sources_keeps_only_markdown_files() {
        let dir = tempfile::tempdir().expect("create temp dir");
        fs::write(dir.path().join("q1.md"), "one").expect("write q1");
        fs::write(dir.path().join("notes.txt"), "skip").expect("write notes");
        fs::create_dir(dir.path().join("nested.md")).expect("create dir named like md");

        let sources = read_question_sources(dir.path()).expect("read sources");
        let names: Vec<&str> = sources.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["q1.md"]);
        assert_eq!(sources.first().map(|(_, c)| c.as_str()), Some("one"));
    }

    #[test]
    /// Reading a directory that does not exist is an error, not a panic.
    fn read_question_sources_errors_on_missing_dir() {
        let err = read_question_sources(Path::new("/no/such/mdquiz/dir"))
            .expect_err("missing directory must fail");
        assert!(err.to_string().contains("reading question directory"));
    }

    /// A one-question true/false bank for exercising the write seam.
    fn true_false_bank() -> ItemBank {
        use mdquiz::model::{Feedback, Question, QuestionKind, TrueFalse};
        ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: "P?".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
            }],
        }
    }

    #[test]
    /// Markdown export writes a text sheet headed by the bank name.
    fn write_export_writes_markdown() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out = dir.path().join("quiz.md");
        write_export(&true_false_bank(), export::Format::Markdown, &out).expect("write");
        let text = fs::read_to_string(&out).expect("read back");
        assert!(text.starts_with("# M"));
    }

    #[test]
    /// Canvas export writes a zip package (starting with the PK signature).
    fn write_export_writes_canvas_zip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out = dir.path().join("quiz.zip");
        write_export(&true_false_bank(), export::Format::Canvas, &out).expect("write");
        let bytes = fs::read(&out).expect("read back");
        assert!(bytes.starts_with(b"PK"));
    }
}
