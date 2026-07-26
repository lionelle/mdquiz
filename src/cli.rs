//! Command-line interface for the `mdquiz` binary.
//!
//! The CLI is a thin shell over the library: it walks a directory of question
//! files, hands their contents to [`mdquiz::parse`], and writes the chosen
//! export target to disk. All real work lives in the library so it can be
//! tested without a process.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};

use mdquiz::export::{self, canvas, markdown};
use mdquiz::mermaid::{self, DiagramFormat};
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
        /// Image format for rendered mermaid diagrams (Canvas export only).
        #[arg(long, value_enum, default_value_t = DiagramFormatArg::Png)]
        diagram_format: DiagramFormatArg,
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

/// The mermaid diagram image format as selected on the command line.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum DiagramFormatArg {
    /// Raster PNG (most reliably rendered inside Canvas).
    Png,
    /// Scalable SVG.
    Svg,
}

impl From<DiagramFormatArg> for DiagramFormat {
    /// Map the CLI-facing diagram format onto the library's [`DiagramFormat`].
    fn from(arg: DiagramFormatArg) -> Self {
        match arg {
            DiagramFormatArg::Png => Self::Png,
            DiagramFormatArg::Svg => Self::Svg,
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
            diagram_format,
        } => {
            let sources = read_question_sources(&dir)?;
            let reader = partial_reader(&dir);
            let mut bank =
                parse::item_bank_from_sources_with(bank_name(&dir, name), sources, &reader)?;
            // Mermaid rendering and image bundling apply only to the Canvas package.
            let images = match format {
                FormatArg::Canvas => canvas_images(&dir, &mut bank, diagram_format.into()),
                FormatArg::Markdown => Vec::new(),
            };
            let reminders = write_export(&bank, format.into(), &output, &images)?;
            print_import_reminders(&reminders);
            Ok(())
        }
    }
}

/// Render mermaid diagrams in `bank`, then gather every image the Canvas package
/// needs: the generated diagrams plus the local images read from `dir`.
fn canvas_images(
    dir: &Path,
    bank: &mut ItemBank,
    diagram_format: DiagramFormat,
) -> Vec<(String, Vec<u8>)> {
    let renderer = move |source: &str| render_mermaid(source, diagram_format);
    let outcome = mermaid::render_diagrams(bank, &renderer, diagram_format);
    for warning in &outcome.warnings {
        eprintln!("warning: {warning}");
    }
    let mut images = load_images(dir, bank);
    images.extend(outcome.images);
    images
}

/// Load the bytes of every local image referenced by the bank, resolved
/// relative to `dir`. Missing or unsafe paths are skipped with a warning.
///
/// Generated-diagram paths (under [`mermaid::GENERATED_DIR`]) are skipped here:
/// their bytes come from the diagram pass, not the question directory.
fn load_images(dir: &Path, bank: &ItemBank) -> Vec<(String, Vec<u8>)> {
    let mut images = Vec::new();
    for path in canvas::local_image_paths(bank) {
        if mermaid::is_generated_path(&path) {
            continue;
        }
        if escapes_dir(&path) {
            eprintln!("warning: skipping image outside the question directory: {path}");
            continue;
        }
        match fs::read(dir.join(&path)) {
            Ok(bytes) => images.push((path, bytes)),
            Err(error) => eprintln!("warning: cannot read image {path} ({error}); skipping"),
        }
    }
    images
}

/// Render one mermaid `source` to image bytes by shelling out to the mermaid CLI.
///
/// # Errors
///
/// Returns a message when the CLI is missing or the render fails; the caller
/// turns that into a warning and leaves the diagram as a code block.
fn render_mermaid(source: &str, format: DiagramFormat) -> std::result::Result<Vec<u8>, String> {
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let input = dir.path().join("diagram.mmd");
    let output = dir.path().join(format!("diagram.{}", format.extension()));
    fs::write(&input, source).map_err(|error| error.to_string())?;
    run_mmdc(&input, &output)?;
    fs::read(&output).map_err(|error| error.to_string())
}

/// Invoke the mermaid CLI (`mmdc`, else `npx @mermaid-js/mermaid-cli`) on
/// `input`, writing `output`.
///
/// # Errors
///
/// Returns a message when no mermaid CLI is found or the render exits non-zero.
fn run_mmdc(input: &Path, output: &Path) -> std::result::Result<(), String> {
    let input = input.to_string_lossy();
    let output = output.to_string_lossy();
    let args = ["-i", input.as_ref(), "-o", output.as_ref()];
    for (program, pre_args) in [
        ("mmdc", &[][..]),
        ("npx", &["-y", "@mermaid-js/mermaid-cli"][..]),
    ] {
        let mut command = ProcessCommand::new(program);
        command.args(pre_args).args(args);
        match command.output() {
            Ok(result) if result.status.success() => return Ok(()),
            Ok(result) => return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned()),
            Err(_) => {} // program not found; try the next candidate
        }
    }
    Err("no mermaid CLI found (install @mermaid-js/mermaid-cli or `mmdc`)".to_owned())
}

/// A [`parse::PartialReader`] that reads `file:` partials relative to `dir`.
///
/// Refuses any path with a `..` component so an include cannot escape the
/// question directory; the resolved partial's local images are rebased by the
/// parser and bundled like any other local image.
fn partial_reader(dir: &Path) -> impl Fn(&str) -> std::result::Result<String, String> + use<'_> {
    move |path: &str| {
        if escapes_dir(path) {
            return Err(format!("path {path:?} escapes the question directory"));
        }
        fs::read_to_string(dir.join(path)).map_err(|error| error.to_string())
    }
}

/// Whether `path` walks out of its base directory via a `..` component.
///
/// The single source of truth for the path-escape rule shared by
/// [`partial_reader`] (which errors) and [`load_images`] (which skips).
fn escapes_dir(path: &str) -> bool {
    path.split('/').any(|part| part == "..")
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
    images: &[(String, Vec<u8>)],
) -> anyhow::Result<Vec<String>> {
    match format {
        export::Format::Markdown => {
            let rendered = markdown::to_print_markdown(bank)?;
            fs::write(output, rendered)
                .with_context(|| format!("writing Markdown to {}", output.display()))?;
            Ok(Vec::new())
        }
        export::Format::Canvas => {
            let bytes = canvas::to_qti(bank, images)?;
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
    /// The CLI diagram format maps onto the library's `DiagramFormat`.
    fn diagram_format_arg_maps() {
        assert!(matches!(
            DiagramFormat::from(DiagramFormatArg::Png),
            DiagramFormat::Png
        ));
        assert!(matches!(
            DiagramFormat::from(DiagramFormatArg::Svg),
            DiagramFormat::Svg
        ));
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
    /// `load_images` reads present images and skips missing/unsafe ones.
    fn load_images_reads_present_skips_others() {
        use mdquiz::model::{Feedback, Question, QuestionKind, TrueFalse};
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("here.png"), b"png-bytes").expect("write image");
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: "![a](here.png) ![b](missing.png) ![c](../escape.png)".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
            }],
        };
        let images = load_images(dir.path(), &bank);
        assert_eq!(images.len(), 1);
        assert_eq!(
            images.first().map(|(path, _)| path.as_str()),
            Some("here.png")
        );
    }

    #[test]
    /// A `..` image path is refused by the guard even when the target exists.
    fn load_images_refuses_parent_escape() {
        use mdquiz::model::{Feedback, Question, QuestionKind, TrueFalse};
        let root = tempfile::tempdir().expect("temp dir");
        fs::write(root.path().join("secret.png"), b"x").expect("write outside");
        let qdir = root.path().join("q");
        fs::create_dir(&qdir).expect("qdir");
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: "![e](../secret.png)".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
            }],
        };
        // The file exists outside `qdir`, so only the `..` guard can skip it.
        assert!(load_images(&qdir, &bank).is_empty());
    }

    #[test]
    /// Generated-diagram paths are skipped by `load_images` (bytes come elsewhere).
    fn load_images_skips_generated_paths() {
        use mdquiz::model::{Feedback, Question, QuestionKind, TrueFalse};
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("real.png"), b"x").expect("write image");
        let bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: "![d](generated/mermaid-abc.png) and ![r](real.png)".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
            }],
        };
        let images = load_images(dir.path(), &bank);
        assert_eq!(images.len(), 1);
        assert_eq!(
            images.first().map(|(path, _)| path.as_str()),
            Some("real.png")
        );
    }

    #[test]
    /// The partial reader reads files under the directory and refuses escapes.
    fn partial_reader_reads_and_refuses_escape() {
        let root = tempfile::tempdir().expect("temp dir");
        fs::create_dir(root.path().join("partials")).expect("subdir");
        fs::write(root.path().join("partials/a.md"), "Hello").expect("write partial");
        fs::write(root.path().join("secret.md"), "nope").expect("write secret");
        let qdir = root.path().join("q");
        fs::create_dir(&qdir).expect("qdir");
        let read = partial_reader(root.path());
        assert_eq!(read("partials/a.md").expect("reads"), "Hello");
        assert!(read("missing.md").is_err());
        let escaping = partial_reader(&qdir);
        assert!(escaping("../secret.md").is_err());
    }

    #[test]
    /// Exporting a directory resolves a `file:` choice partial end to end.
    fn export_resolves_file_choice_partial() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir(dir.path().join("partials")).expect("subdir");
        fs::write(dir.path().join("partials/right.md"), "The **right** one").expect("partial");
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: partials/right.md\n    correct: true\n  - text: Wrong\n---\n\nPick?\n";
        fs::write(dir.path().join("01-q.md"), source).expect("question");
        let sources = read_question_sources(dir.path()).expect("sources");
        let read = partial_reader(dir.path());
        let bank = parse::item_bank_from_sources_with("m", sources, &read).expect("bank assembles");
        assert!(matches!(
            bank.items.first().map(|q| &q.kind),
            Some(mdquiz::model::QuestionKind::MultipleChoice(mc))
                if mc.choices.first().is_some_and(|c| c.text == "The **right** one")
        ));
    }

    #[test]
    /// A partial's embedded image resolves on disk at its rebased bank path.
    fn partial_image_resolves_through_load_images() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir(dir.path().join("parts")).expect("subdir");
        fs::write(dir.path().join("parts/ans.md"), "![x](img.png)").expect("partial");
        fs::write(dir.path().join("parts/img.png"), b"png").expect("image");
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: parts/ans.md\n    correct: true\n  - text: No\n---\n\nPick?\n";
        fs::write(dir.path().join("01-q.md"), source).expect("question");
        let sources = read_question_sources(dir.path()).expect("sources");
        let read = partial_reader(dir.path());
        let bank = parse::item_bank_from_sources_with("m", sources, &read).expect("bank");
        // The partial's `img.png` was rebased to `parts/img.png`, which exists.
        let images = load_images(dir.path(), &bank);
        assert_eq!(
            images.first().map(|(p, _)| p.as_str()),
            Some("parts/img.png")
        );
    }

    #[test]
    /// A missing partial fails assembly, naming the offending question file.
    fn missing_partial_names_question_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: parts/nope.md\n    correct: true\n  - text: No\n---\n\nPick?\n";
        fs::write(dir.path().join("01-q.md"), source).expect("question");
        let sources = read_question_sources(dir.path()).expect("sources");
        let read = partial_reader(dir.path());
        let err = parse::item_bank_from_sources_with("m", sources, &read).expect_err("missing");
        assert!(err.to_string().contains("01-q.md"));
    }

    #[test]
    /// Markdown export writes a text sheet headed by the bank name.
    fn write_export_writes_markdown() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out = dir.path().join("quiz.md");
        write_export(&true_false_bank(), export::Format::Markdown, &out, &[]).expect("write");
        let text = fs::read_to_string(&out).expect("read back");
        assert!(text.starts_with("# M"));
    }

    #[test]
    /// Canvas export writes a zip package (starting with the PK signature).
    fn write_export_writes_canvas_zip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out = dir.path().join("quiz.zip");
        write_export(&true_false_bank(), export::Format::Canvas, &out, &[]).expect("write");
        let bytes = fs::read(&out).expect("read back");
        assert!(bytes.starts_with(b"PK"));
    }
}
