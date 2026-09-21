//! Command-line interface for the `mdquiz` binary.
//!
//! The CLI is a thin shell over the library: it walks a directory of question
//! files, hands their contents to [`mdquiz::parse`], and writes the chosen
//! export target to disk. All real work lives in the library so it can be
//! tested without a process.

use std::collections::hash_map::RandomState;
use std::ffi::OsStr;
use std::fs;
use std::hash::BuildHasher as _;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::SystemTime;

use anyhow::Context;
use clap::{Args, Parser, Subcommand, ValueEnum};

use mdquiz::diagram::{self, DiagramFormat, DiagramLanguage};
use mdquiz::export::{self, canvas, markdown};
use mdquiz::model::{ItemBank, Question};
use mdquiz::parse;
use mdquiz::path::escapes_dir;
use mdquiz::quiz::assemble::{Assembly, assemble};
use mdquiz::quiz::output::{self, OutputFile};
use mdquiz::quiz::sample::{self, SampleRng, Source};
use mdquiz::quiz::spec::Spec;

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
    Export(ExportArgs),
    /// Assemble a printable exam from a quiz spec: one Word sheet and one
    /// answer key per variant.
    Quiz(QuizArgs),
}

/// Arguments for `mdquiz export`.
#[derive(Debug, Args)]
pub(crate) struct ExportArgs {
    /// Directory of Markdown question files (one question per file).
    dir: PathBuf,
    /// Path to write the exported item bank to.
    #[arg(short, long)]
    output: PathBuf,
    /// Export format to produce; defaults to the Canvas package.
    #[arg(short, long, value_enum, default_value_t = FormatArg::Canvas)]
    format: FormatArg,
    /// Bank name; defaults to the directory's own name.
    #[arg(short, long)]
    name: Option<String>,
    /// Recurse into subdirectories, gathering every question into one bank.
    #[arg(short, long)]
    recursive: bool,
    /// Keep at most N randomly-chosen questions from each directory (handy
    /// with `-r` to build a print quiz that samples every topic).
    #[arg(long, value_name = "N")]
    sample: Option<usize>,
    /// Also write a matching answer key (markdown export only): a second
    /// file, `<output>-key.md`, with each question's correct answer.
    #[arg(long)]
    include_key: bool,
    /// Shuffle the questions into a random order after selection (markdown
    /// export only).
    #[arg(long)]
    random_order: bool,
    /// Seed the random draw so `--sample`/`--random-order` reproduce
    /// exactly; omit for a different draw each run.
    #[arg(long, value_name = "N")]
    seed: Option<u64>,
    /// Image format for rendered diagrams (Canvas export only).
    #[arg(long, value_enum, default_value_t = DiagramFormatArg::Png)]
    diagram_format: DiagramFormatArg,
}

/// Arguments for `mdquiz quiz`.
#[derive(Debug, Args)]
pub(crate) struct QuizArgs {
    /// The quiz spec (YAML) describing what to draw and how to lay it out.
    spec: PathBuf,
    /// Directory to write the sheets and keys into.
    #[arg(short, long, default_value = ".")]
    out_dir: PathBuf,
    /// Base name for the files; defaults to the spec file's own stem.
    #[arg(short, long)]
    name: Option<String>,
    /// Seed the draw so a run reproduces exactly; omit for a fresh draw.
    ///
    /// The seed is printed either way, so a run worth keeping can be repeated
    /// — without it a sheet handed out can never be rebuilt.
    #[arg(long, value_name = "N")]
    seed: Option<u64>,
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

/// The diagram image format as selected on the command line.
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
        Command::Export(args) => run_export(args),
        Command::Quiz(args) => run_quiz(args),
    }
}

/// Export an item bank in the requested format.
///
/// # Errors
///
/// Returns an error if the sources cannot be read or parsed, the bank cannot be
/// exported, or the output file cannot be written. Also rejects flags that
/// apply only to the Markdown sheet being passed with `--format canvas`.
fn run_export(args: ExportArgs) -> anyhow::Result<()> {
    let ExportArgs {
        dir,
        output,
        format,
        name,
        recursive,
        sample: sample_size,
        include_key,
        random_order,
        seed,
        diagram_format,
    } = args;
    if (include_key || random_order) && matches!(format, FormatArg::Canvas) {
        anyhow::bail!("--include-key and --random-order apply only to `--format markdown`");
    }
    let mut bank = assemble_bank(&dir, name, recursive, sample_size, random_order, seed)?;
    // Diagram rendering and image bundling apply only to the Canvas package.
    let images = match format {
        FormatArg::Canvas => canvas_images(&dir, &mut bank.items, diagram_format.into()),
        FormatArg::Markdown => Vec::new(),
    };
    let reminders = write_export(&bank, format.into(), &output, &images)?;
    if include_key {
        write_answer_key(&bank, &output)?;
    }
    print_import_reminders(&reminders);
    Ok(())
}

/// Assemble a quiz spec and write every variant's sheet and key.
///
/// # Errors
///
/// Returns an error if the spec cannot be read or parsed, a question folder
/// cannot be listed, a variant cannot be rendered, or a file cannot be written.
fn run_quiz(args: QuizArgs) -> anyhow::Result<()> {
    let QuizArgs {
        spec: spec_path,
        out_dir,
        name,
        seed,
    } = args;
    let spec_path = spec_path.as_path();
    let out_dir = out_dir.as_path();
    let yaml = fs::read_to_string(spec_path)
        .with_context(|| format!("cannot read quiz spec {}", spec_path.display()))?;
    let spec = Spec::from_yaml(&yaml)?;
    // Folders in the spec are relative to the spec itself, not to the shell's
    // working directory: a spec is checked in beside the questions it draws.
    let root = spec_path.parent().unwrap_or(Path::new("."));
    let seed = seed.unwrap_or_else(entropy_seed);
    let mut assembly = assemble(
        &spec,
        &|dir: &str| list_group_sources(root, dir),
        &partial_reader(root),
        seed,
    )?;
    for warning in &assembly.warnings {
        eprintln!("warning: {warning}");
    }
    // After assembly, so the diagram pass sees the questions the variants
    // actually drew — and before rendering, because a `w:drawing` needs the
    // image bytes the pass produces.
    let images = quiz_images(root, &mut assembly);
    let stem = name.unwrap_or_else(|| spec_stem(spec_path));
    let files = output::render(&assembly, &stem, seed, &images)?;
    write_output_files(out_dir, &files)?;
    // Printed even when it was given, so the line in the terminal is the whole
    // record of how this paper was built.
    eprintln!("seed: {seed}");
    Ok(())
}

/// List one spec group's question sources, named relative to the spec's folder.
///
/// Spec-relative, not group-relative, because that is what [`assemble`]
/// promises its questions: it recovers each one's own folder from the
/// directory part of its path, and rebases that question's images and `file:`
/// partials against it. A bare `q1.md` would send a group's
/// `figures/maple.png` looking beside the *spec* instead of beside the
/// question — and would make two groups' identically named figures collide.
///
/// # Errors
///
/// Returns the failure as a string, which is the contract [`assemble`] injects.
fn list_group_sources(root: &Path, dir: &str) -> std::result::Result<Vec<Source>, String> {
    if escapes_dir(dir) {
        return Err(format!("group folder {dir:?} escapes the spec's directory"));
    }
    let sources = read_question_sources(&root.join(dir), false).map_err(|e| e.to_string())?;
    Ok(sources
        .into_iter()
        .map(|(path, contents)| (mdquiz::path::join_dir(dir, &path), contents))
        .collect())
}

/// The base name for a spec's output files: the spec file's own stem.
fn spec_stem(spec_path: &Path) -> String {
    spec_path.file_stem().map_or_else(
        || "quiz".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// Write every rendered file into `out_dir`, creating it if needed.
///
/// # Errors
///
/// Returns an error if the directory cannot be created or a file not written.
fn write_output_files(out_dir: &Path, files: &[OutputFile]) -> anyhow::Result<()> {
    fs::create_dir_all(out_dir).with_context(|| format!("cannot create {}", out_dir.display()))?;
    for file in files {
        let path = out_dir.join(&file.name);
        fs::write(&path, &file.bytes)
            .with_context(|| format!("cannot write {}", path.display()))?;
        println!("{}", path.display());
    }
    Ok(())
}

/// Read a directory's questions into a bank, optionally recursing, sampling,
/// and shuffling the final order.
///
/// # Errors
///
/// Propagates any source-read, parse, or assembly failure.
fn assemble_bank(
    dir: &Path,
    name: Option<String>,
    recursive: bool,
    sample: Option<usize>,
    random_order: bool,
    seed: Option<u64>,
) -> anyhow::Result<ItemBank> {
    let mut rng = SampleRng::seeded(seed.unwrap_or_else(entropy_seed));
    let mut sources = read_question_sources(dir, recursive)?;
    if let Some(limit) = sample {
        sources = sample::per_directory(sources, limit, &mut rng);
    }
    let mut bank = build_bank(dir, bank_name(dir, name), sources)?;
    if random_order {
        sample::shuffle(&mut bank.items, &mut rng);
    }
    Ok(bank)
}

/// A seed drawn from operating-system entropy mixed with the current time.
///
/// Lives in the binary, not the library: seeding from ambient state is exactly
/// the impurity the library avoids, so the caller decides when a draw should be
/// unpredictable and when `--seed` should make it reproducible.
fn entropy_seed() -> u64 {
    RandomState::new().hash_one(SystemTime::now())
}

/// Render the diagrams in `items`, then gather every image the Canvas package
/// needs: the generated diagrams plus the local images read from `dir`.
fn canvas_images(
    dir: &Path,
    items: &mut [Question],
    diagram_format: DiagramFormat,
) -> Vec<(String, Vec<u8>)> {
    let renderer = move |language: DiagramLanguage, source: &str| {
        render_diagram(language, source, diagram_format)
    };
    let outcome = diagram::render_diagrams(&mut *items, &renderer, diagram_format);
    bundled(dir, &*items, outcome)
}

/// Render the diagrams across every variant of `assembly`, then gather every
/// image its sheets need.
///
/// Always [`DiagramFormat::Png`]: Word places SVG only through the
/// `asvg:svgBlip` extension, which needs a rasterised copy alongside it
/// anyway, so the Word path has no SVG mode to choose. One pass across all the
/// variants at once, so a figure four of them share is rendered once.
fn quiz_images(root: &Path, assembly: &mut Assembly) -> Vec<(String, Vec<u8>)> {
    let renderer = |language: DiagramLanguage, source: &str| {
        render_diagram(language, source, DiagramFormat::Png)
    };
    let outcome = diagram::render_diagrams(assembly.questions_mut(), &renderer, DiagramFormat::Png);
    bundled(root, assembly.questions(), outcome)
}

/// The images to bundle: the diagrams just rendered, plus the local images
/// `items` reference, read from `dir`. Diagram warnings are reported here
/// because a diagram left as code is the author's to fix, not an export
/// failure.
fn bundled<'a>(
    dir: &Path,
    items: impl IntoIterator<Item = &'a Question>,
    outcome: diagram::DiagramOutcome,
) -> Vec<(String, Vec<u8>)> {
    for warning in &outcome.warnings {
        eprintln!("warning: {warning}");
    }
    // Deliberately unsorted: both writers look an image up by its authored
    // path, and both sources are already deduplicated, so the order here
    // cannot reach the output. Sorting it only reshuffled the Canvas
    // manifest's `<resource>` list against every package built before.
    let mut images = load_images(dir, items);
    images.extend(outcome.images);
    images
}

/// Load the bytes of every local image referenced by `items`, resolved
/// relative to `dir`. Missing or unsafe paths are skipped with a warning.
///
/// Generated-diagram paths (under [`diagram::GENERATED_DIR`]) are skipped here:
/// their bytes come from the diagram pass, not the question directory.
fn load_images<'a>(
    dir: &Path,
    items: impl IntoIterator<Item = &'a Question>,
) -> Vec<(String, Vec<u8>)> {
    let mut images = Vec::new();
    for path in canvas::local_image_paths(items) {
        if diagram::is_generated_path(&path) {
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

/// Render one diagram `source` to image bytes by shelling out to the renderer
/// for its `language` (`mmdc` for mermaid, `dot` for Graphviz).
///
/// # Errors
///
/// Returns a message when the tool is missing, the render fails, or the
/// temporary files it works through cannot be written or read back. Every
/// message names what failed, because the caller turns it into a warning the
/// author sees instead of a diagram.
fn render_diagram(
    language: DiagramLanguage,
    source: &str,
    format: DiagramFormat,
) -> std::result::Result<Vec<u8>, String> {
    let dir = tempfile::tempdir()
        .map_err(|error| format!("could not create a temporary directory ({error})"))?;
    let input = dir
        .path()
        .join(format!("diagram.{}", language.source_extension()));
    let output = dir.path().join(format!("diagram.{}", format.extension()));
    fs::write(&input, source)
        .map_err(|error| format!("could not write the diagram source ({error})"))?;
    match language {
        DiagramLanguage::Mermaid => run_mmdc(&input, &output)?,
        DiagramLanguage::Graphviz => run_dot(&input, &output, format)?,
    }
    fs::read(&output).map_err(|error| format!("the renderer produced no image ({error})"))
}

/// Invoke the mermaid CLI (`mmdc`, else `npx @mermaid-js/mermaid-cli`) on
/// `input`, writing `output`.
///
/// The image format follows `output`'s extension, which `mmdc` reads — so
/// unlike [`run_dot`] this needs no [`DiagramFormat`].
///
/// # Errors
///
/// Returns a message when no mermaid CLI is found or the render exits non-zero.
fn run_mmdc(input: &Path, output: &Path) -> std::result::Result<(), String> {
    run_first_available(
        &[("mmdc", &[]), ("npx", &["-y", "@mermaid-js/mermaid-cli"])],
        &[
            OsStr::new("-i"),
            input.as_os_str(),
            OsStr::new("-o"),
            output.as_os_str(),
        ],
        "no mermaid CLI found; install it with `npm install -g @mermaid-js/mermaid-cli`",
    )
}

/// The message shown when Graphviz is not installed. Named so a test can tell
/// "`dot` is missing" apart from "`dot` rejected this diagram".
const DOT_MISSING: &str =
    "Graphviz `dot` not found; install Graphviz from https://graphviz.org/download/";

/// Invoke Graphviz (`dot`) on `input`, writing `output` in `format`.
///
/// # Errors
///
/// Returns a message when `dot` is not installed or the render exits non-zero
/// (a syntax error in the diagram, for instance).
fn run_dot(input: &Path, output: &Path, format: DiagramFormat) -> std::result::Result<(), String> {
    // Spelled out rather than derived from the file extension: `dot`'s output
    // format is its own vocabulary that only happens to agree with ours.
    let format_flag = match format {
        DiagramFormat::Png => "-Tpng",
        DiagramFormat::Svg => "-Tsvg",
    };
    run_first_available(
        &[("dot", &[])],
        &[
            OsStr::new(format_flag),
            OsStr::new("-o"),
            output.as_os_str(),
            input.as_os_str(),
        ],
        DOT_MISSING,
    )
}

/// Run the first of `candidates` that can be spawned, passing its own leading
/// arguments followed by `args`.
///
/// Candidates are `(program, leading arguments)` pairs tried in order; a program
/// that cannot be spawned is treated as "not installed" and the next one is
/// tried.
///
/// # Errors
///
/// Returns the failing program's stderr (named, since a language may have
/// several candidates), or `missing` if none of them could be spawned.
fn run_first_available(
    candidates: &[(&str, &[&str])],
    args: &[&OsStr],
    missing: &str,
) -> std::result::Result<(), String> {
    for (program, pre_args) in candidates {
        let mut command = ProcessCommand::new(program);
        command.args(*pre_args).args(args);
        match command.output() {
            Ok(result) if result.status.success() => return Ok(()),
            Ok(result) => return Err(tool_failure(program, &result)),
            Err(_) => {} // program not found; try the next candidate
        }
    }
    Err(missing.to_owned())
}

/// The message for a `program` that ran but exited non-zero: its stderr, or the
/// exit status when it said nothing (an empty message would leave the eventual
/// warning explaining nothing).
fn tool_failure(program: &str, result: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&result.stderr);
    match stderr.trim() {
        "" => format!("`{program}` failed ({})", result.status),
        message => format!("`{program}`: {message}"),
    }
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

/// Read every question `*.md` file under `dir` as a `(relative-path, content)`
/// pair; with `recursive`, descend into subdirectories.
///
/// Paths use `/` separators and are relative to `dir` (e.g. `matching/01.md`).
/// Skipped: `README.md`, hidden entries (starting with `.`), non-Markdown files,
/// and Markdown lacking a leading YAML front-matter block (partials and prose).
/// Ordering is [`build_bank`]'s job.
///
/// # Errors
///
/// Returns an error if the directory or any question file cannot be read.
fn read_question_sources(dir: &Path, recursive: bool) -> anyhow::Result<Vec<(String, String)>> {
    let mut sources = Vec::new();
    collect_sources(dir, "", recursive, &mut sources)?;
    Ok(sources)
}

/// Collect question sources under `dir`, prefixing their keys with `prefix`.
///
/// # Errors
///
/// Returns an error if a directory or file cannot be read.
fn collect_sources(
    dir: &Path,
    prefix: &str,
    recursive: bool,
    out: &mut Vec<(String, String)>,
) -> anyhow::Result<()> {
    let entries =
        fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading an entry in {}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if path.is_dir() {
            if recursive {
                collect_sources(&path, &rel, recursive, out)?;
            }
        } else if is_question_file(&path, &name) {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("reading question file {}", path.display()))?;
            // A partial or other prose `.md` has no front-matter and is not a
            // standalone question — skip it so recursion ignores partials.
            if looks_like_question(&content) {
                out.push((rel, content));
            }
        }
    }
    Ok(())
}

/// Whether `path` (with file `name`) is a candidate question file: a `*.md` that
/// is not a `README.md`.
fn is_question_file(path: &Path, name: &str) -> bool {
    path.extension().and_then(OsStr::to_str) == Some("md")
        && !name.eq_ignore_ascii_case("readme.md")
}

/// Whether `content` opens with a YAML front-matter block, marking it a question
/// (rather than a partial or other prose Markdown).
fn looks_like_question(content: &str) -> bool {
    content.trim_start().starts_with("---")
}

/// Parse every `(relative-path, content)` source into a question — resolving its
/// partials and images relative to its own subdirectory — and assemble the bank.
///
/// # Errors
///
/// Returns an error naming the source file if a question fails to parse, or if
/// two questions share an `id`.
fn build_bank(
    dir: &Path,
    name: String,
    mut sources: Vec<(String, String)>,
) -> anyhow::Result<ItemBank> {
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    let mut questions = Vec::with_capacity(sources.len());
    for (rel_path, content) in sources {
        let base = mdquiz::path::parent_dir(&rel_path);
        let question_dir = dir.join(base);
        let reader = partial_reader(&question_dir);
        let mut question = parse::parse_question_with(&content, &reader)
            .with_context(|| format!("in question file {rel_path}"))?;
        parse::rebase_local_paths(&mut question, base);
        questions.push(question);
    }
    Ok(parse::item_bank_from_questions(name, questions)?)
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

/// Write the answer key beside the markdown sheet, at the path [`key_path`]
/// derives from `output`.
///
/// # Errors
///
/// Returns an error if the key file cannot be written.
fn write_answer_key(bank: &ItemBank, output: &Path) -> anyhow::Result<()> {
    let path = key_path(output);
    fs::write(&path, markdown::to_answer_key(bank))
        .with_context(|| format!("writing answer key to {}", path.display()))?;
    eprintln!("wrote answer key to {}", path.display());
    Ok(())
}

/// The answer-key path for a sheet `output`: `<stem>-key.<ext>` in the same
/// directory (e.g. `quiz.md` → `quiz-key.md`).
fn key_path(output: &Path) -> PathBuf {
    let mut name = output
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    name.push_str("-key");
    if let Some(ext) = output.extension() {
        name.push('.');
        name.push_str(&ext.to_string_lossy());
    }
    output.with_file_name(name)
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
    /// A seed reproduces both the draw and the shuffled order exactly, and a
    /// different seed is free to differ. This is the promise `docs/exporting.md`
    /// makes for `--seed`.
    fn assemble_bank_is_reproducible_for_a_seed() {
        let dir = tempfile::tempdir().expect("create temp dir");
        for n in 0..6 {
            fs::write(
                dir.path().join(format!("q{n}.md")),
                format!("---\nid: q{n}\nkind: true_false\nanswer: true\n---\n\nQ{n}?\n"),
            )
            .expect("write question");
        }
        let ids = |seed| {
            assemble_bank(dir.path(), None, false, Some(3), true, seed)
                .expect("assemble")
                .items
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<String>>()
        };
        let drawn = ids(Some(7));
        assert_eq!(drawn.len(), 3);
        assert_eq!(drawn, ids(Some(7)), "same seed must reproduce the sheet");
        let others: Vec<Vec<String>> = (1..8).map(|seed| ids(Some(seed))).collect();
        assert!(
            others.iter().any(|other| *other != drawn),
            "no seed produced a different sheet"
        );
        // The unseeded branch is the default path and must stay unpredictable:
        // pinned to a constant, every "random" sheet would be the same one and
        // none of the seeded assertions above would notice.
        let unseeded: Vec<Vec<String>> = (0..5).map(|_| ids(None)).collect();
        assert_eq!(unseeded.first().map(Vec::len), Some(3));
        assert!(
            unseeded.windows(2).any(|pair| pair.first() != pair.get(1)),
            "five unseeded runs all produced the same sheet"
        );
    }

    #[test]
    /// Two unseeded runs get different seeds. This is the only thing worth
    /// asserting about a reader of ambient state: not *what* it returns, but
    /// that it is not a constant.
    fn entropy_seed_is_not_a_constant() {
        assert_ne!(entropy_seed(), entropy_seed());
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
    /// Only front-matter `*.md` questions are read; other files, non-question
    /// Markdown (README/partials), and subdirectories are skipped.
    fn read_question_sources_keeps_only_questions() {
        let dir = tempfile::tempdir().expect("create temp dir");
        fs::write(dir.path().join("q1.md"), "---\nid: q\n---\n\nQ?").expect("write q1");
        fs::write(dir.path().join("notes.txt"), "skip").expect("write notes");
        fs::write(dir.path().join("README.md"), "# readme").expect("write readme");
        fs::write(dir.path().join("partial.md"), "Just prose, no front-matter").expect("partial");
        // A hidden file is skipped even with valid front-matter.
        fs::write(dir.path().join(".draft.md"), "---\nid: d\n---\n\nD?").expect("draft");
        fs::create_dir(dir.path().join("nested.md")).expect("create dir named like md");

        let sources = read_question_sources(dir.path(), false).expect("read sources");
        let names: Vec<&str> = sources.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["q1.md"]);
    }

    #[test]
    /// With `--recursive`, questions in subdirectories are gathered (paths keep
    /// their subfolder), while non-recursive stays one level deep.
    fn read_question_sources_recurses_when_asked() {
        let dir = tempfile::tempdir().expect("create temp dir");
        fs::write(dir.path().join("top.md"), "---\nid: t\n---\n\nT?").expect("top");
        fs::create_dir(dir.path().join("sub")).expect("subdir");
        fs::write(dir.path().join("sub/deep.md"), "---\nid: d\n---\n\nD?").expect("deep");

        let flat = read_question_sources(dir.path(), false).expect("flat");
        assert_eq!(
            flat.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["top.md"]
        );

        let deep = read_question_sources(dir.path(), true).expect("recursive");
        let mut names: Vec<&str> = deep.iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["sub/deep.md", "top.md"]);
    }

    #[test]
    /// `build_bank` roots each subdir question's reader at its own folder and
    /// rebases its prompt image and its partial's image to root-relative paths.
    fn build_bank_rebases_subdir_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir(dir.path().join("topics")).expect("subdir");
        fs::write(dir.path().join("topics/pic.png"), b"png").expect("pic");
        fs::write(dir.path().join("topics/ans.md"), "![p](inpartial.png)").expect("partial");
        fs::write(dir.path().join("topics/inpartial.png"), b"png").expect("inpartial");
        let src = "---\nid: q\nkind: multiple_choice\nchoices:\n\
            \x20 - file: ans.md\n    correct: true\n  - text: No\n---\n\n![m](pic.png)\n";
        fs::write(dir.path().join("topics/01.md"), src).expect("question");

        let sources = read_question_sources(dir.path(), true).expect("sources");
        let bank = build_bank(dir.path(), "m".to_owned(), sources).expect("bank");
        let question = bank.items.first().expect("question");
        // Prompt image and the partial's image both rebased under `topics/`.
        assert!(question.prompt.contains("![m](topics/pic.png)"));
        assert!(matches!(
            &question.kind,
            mdquiz::model::QuestionKind::MultipleChoice(mc) if mc.choices.first()
                .is_some_and(|c| c.text.contains("![p](topics/inpartial.png)"))
        ));
        // Both rebased paths resolve on disk through the root-rooted loader.
        let names: Vec<String> = load_images(dir.path(), &bank.items)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        assert!(names.iter().any(|n| n == "topics/pic.png"));
        assert!(names.iter().any(|n| n == "topics/inpartial.png"));
    }

    #[test]
    /// Reading a directory that does not exist is an error, not a panic.
    fn read_question_sources_errors_on_missing_dir() {
        let err = read_question_sources(Path::new("/no/such/mdquiz/dir"), false)
            .expect_err("missing directory must fail");
        assert!(err.to_string().contains("no/such/mdquiz/dir"));
    }

    /// A one-question true/false bank whose prompt is `prompt`.
    fn true_false_bank(prompt: &str) -> ItemBank {
        use mdquiz::model::{Feedback, Question, QuestionKind, TrueFalse};
        ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: prompt.to_owned(),
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
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("here.png"), b"png-bytes").expect("write image");
        let bank = true_false_bank("![a](here.png) ![b](missing.png) ![c](../escape.png)");
        let images = load_images(dir.path(), &bank.items);
        assert_eq!(images.len(), 1);
        assert_eq!(
            images.first().map(|(path, _)| path.as_str()),
            Some("here.png")
        );
    }

    #[test]
    /// A `..` image path is refused by the guard even when the target exists.
    fn load_images_refuses_parent_escape() {
        let root = tempfile::tempdir().expect("temp dir");
        fs::write(root.path().join("secret.png"), b"x").expect("write outside");
        let qdir = root.path().join("q");
        fs::create_dir(&qdir).expect("qdir");
        let bank = true_false_bank("![e](../secret.png)");
        // The file exists outside `qdir`, so only the `..` guard can skip it.
        assert!(load_images(&qdir, &bank.items).is_empty());
    }

    #[test]
    /// Generated-diagram paths are skipped by `load_images` (bytes come elsewhere).
    fn load_images_skips_generated_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("real.png"), b"x").expect("write image");
        // The generated image exists on disk too, so only the reserved-prefix
        // guard — not a read error — can keep it out of the loaded set.
        fs::create_dir(dir.path().join("generated")).expect("generated dir");
        fs::write(dir.path().join("generated/mermaid-abc.png"), b"x").expect("write generated");
        let bank = true_false_bank("![d](generated/mermaid-abc.png) and ![r](real.png)");
        let images = load_images(dir.path(), &bank.items);
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
        let sources = read_question_sources(dir.path(), false).expect("sources");
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
        let sources = read_question_sources(dir.path(), false).expect("sources");
        let read = partial_reader(dir.path());
        let bank = parse::item_bank_from_sources_with("m", sources, &read).expect("bank");
        // The partial's `img.png` was rebased to `parts/img.png`, which exists.
        let images = load_images(dir.path(), &bank.items);
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
        let sources = read_question_sources(dir.path(), false).expect("sources");
        let read = partial_reader(dir.path());
        let err = parse::item_bank_from_sources_with("m", sources, &read).expect_err("missing");
        assert!(err.to_string().contains("01-q.md"));
    }

    #[test]
    /// Markdown export writes a text sheet headed by the bank name.
    fn write_export_writes_markdown() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out = dir.path().join("quiz.md");
        write_export(&true_false_bank("P?"), export::Format::Markdown, &out, &[]).expect("write");
        let text = fs::read_to_string(&out).expect("read back");
        assert!(text.starts_with("# M"));
    }

    /// The arguments a `mdquiz quiz` run would parse to.
    #[test]
    /// A group's sources are named relative to the *spec*, not the group. That
    /// prefix is what lets `assemble` recover a question's own folder and
    /// rebase its images against it — without it an image beside a question
    /// is looked for beside the spec, which is where it is not.
    fn group_sources_are_named_relative_to_the_spec() {
        let dir = tempfile::tempdir().expect("temp dir");
        quiz_fixture(dir.path(), 1);
        let sources = list_group_sources(dir.path(), "topics").expect("lists");
        let paths: Vec<&str> = sources.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(paths, ["topics/q.md"]);
    }

    #[test]
    /// An image beside a question in a group folder is found, placed, and
    /// bundled into the sheet. This is the whole chain: the source path names
    /// the group, the parser rebases the image onto it, the CLI reads it from
    /// there, and the writer embeds it.
    fn an_image_beside_a_question_reaches_the_sheet() {
        let dir = tempfile::tempdir().expect("temp dir");
        let spec = illustrated_fixture(dir.path());
        let out = dir.path().join("out");
        run_quiz(quiz_args(&spec, &out, None, 4)).expect("quiz runs");
        let bytes = fs::read(out.join("midterm.docx")).expect("the sheet is written");
        let names = part_names(&bytes);
        assert!(
            names.contains(&"word/media/image1.png".to_owned()),
            "no image was bundled: {names:?}"
        );
    }

    #[test]
    /// Two groups may each hold a `figures/fig.png`, and they are two
    /// different pictures. Group-relative source paths would have collapsed
    /// them into one, printing whichever was read last under both questions.
    fn identically_named_figures_in_two_groups_stay_distinct() {
        let dir = tempfile::tempdir().expect("temp dir");
        for (group, width) in [("trees", 96_u32), ("graphs", 48_u32)] {
            let figures = dir.path().join(group).join("figures");
            fs::create_dir_all(&figures).expect("create figures");
            fs::write(figures.join("fig.png"), sized_png(width)).expect("write image");
            fs::write(
                dir.path().join(group).join("q.md"),
                format!(
                    "---\nid: {group}\nkind: true_false\nanswer: true\n---\n\n\
                     See ![a figure](figures/fig.png)\n"
                ),
            )
            .expect("write question");
        }
        let spec = dir.path().join("midterm.yaml");
        fs::write(
            &spec,
            "name: M\nvariants: 1\ngroups:\n  - dir: trees\n    take: all\n\
             \x20 - dir: graphs\n    take: all\n",
        )
        .expect("write spec");
        let out = dir.path().join("out");
        run_quiz(quiz_args(&spec, &out, None, 9)).expect("quiz runs");
        let bytes = fs::read(out.join("midterm.docx")).expect("the sheet is written");
        let media: Vec<String> = part_names(&bytes)
            .into_iter()
            .filter(|name| name.starts_with("word/media/"))
            .collect();
        assert_eq!(media.len(), 2, "the two figures collided: {media:?}");
    }

    #[test]
    /// Every variant that draws an illustrated question carries the figure,
    /// and carries it once. A shared figure is bundled per *sheet* — each is
    /// a standalone file — but never twice within one.
    fn every_variant_carries_a_shared_figure_exactly_once() {
        let dir = tempfile::tempdir().expect("temp dir");
        let spec = illustrated_fixture(dir.path());
        fs::write(
            &spec,
            "name: M\nvariants: 3\nlayout:\n  shuffle_choices: true\n\
             groups:\n  - dir: topics\n    take: all\n",
        )
        .expect("write spec");
        let out = dir.path().join("out");
        run_quiz(quiz_args(&spec, &out, None, 3)).expect("quiz runs");
        for variant in ["A", "B", "C"] {
            let bytes =
                fs::read(out.join(format!("midterm-{variant}.docx"))).expect("sheet written");
            let media: Vec<String> = part_names(&bytes)
                .into_iter()
                .filter(|name| name.starts_with("word/media/"))
                .collect();
            assert_eq!(media, ["word/media/image1.png"], "variant {variant}");
        }
    }

    /// A one-question spec whose question embeds an image in its own folder.
    fn illustrated_fixture(dir: &Path) -> PathBuf {
        let figures = dir.join("topics").join("figures");
        fs::create_dir_all(&figures).expect("create figures");
        fs::write(figures.join("tree.png"), tiny_png()).expect("write image");
        fs::write(
            dir.join("topics").join("q.md"),
            "---
id: q
kind: true_false
answer: true
---

             Is this a maple? ![a maple](figures/tree.png)
",
        )
        .expect("write question");
        let spec = dir.join("midterm.yaml");
        fs::write(
            &spec,
            "name: M
variants: 1
groups:
  - dir: topics
    take: all
",
        )
        .expect("write spec");
        spec
    }

    /// A 1x1 PNG header, which is all the writer measures.
    ///
    /// Built here rather than shared with `export::docx::media`'s fixture:
    /// this is the binary crate, which cannot see that module's internals.
    fn tiny_png() -> Vec<u8> {
        sized_png(1)
    }

    /// A square PNG header `side` pixels across, so two fixtures can differ.
    fn sized_png(side: u32) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend(13_u32.to_be_bytes());
        bytes.extend(b"IHDR");
        bytes.extend(side.to_be_bytes());
        bytes.extend(side.to_be_bytes());
        bytes.extend([8, 2, 0, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    /// The part names inside a `.docx`.
    fn part_names(bytes: &[u8]) -> Vec<String> {
        let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("a zip");
        archive.file_names().map(ToOwned::to_owned).collect()
    }

    fn quiz_args(spec: &Path, out_dir: &Path, name: Option<&str>, seed: u64) -> QuizArgs {
        QuizArgs {
            spec: spec.to_path_buf(),
            out_dir: out_dir.to_path_buf(),
            name: name.map(ToOwned::to_owned),
            seed: Some(seed),
        }
    }

    /// A spec folder holding one multiple-choice question, plus the spec.
    fn quiz_fixture(dir: &Path, variants: usize) -> PathBuf {
        let topics = dir.join("topics");
        fs::create_dir_all(&topics).expect("create topics");
        fs::write(
            topics.join("q.md"),
            "---\nid: q\nkind: multiple_choice\nchoices:\n  \
             - text: alpha\n    correct: true\n  - text: beta\n---\n\nPick one.\n",
        )
        .expect("write question");
        let spec = dir.join("midterm.yaml");
        fs::write(
            &spec,
            format!(
                "name: M\nvariants: {variants}\nlayout:\n  shuffle_choices: true\n\
                 groups:\n  - dir: topics\n    take: all\n"
            ),
        )
        .expect("write spec");
        spec
    }

    #[test]
    /// A multi-variant run writes a sheet *and* a key for every variant. One
    /// key cannot grade two papers, so a variant without its own key is a pile
    /// of exams nobody can mark.
    fn quiz_writes_a_sheet_and_a_key_for_every_variant() {
        let dir = tempfile::tempdir().expect("temp dir");
        let spec = quiz_fixture(dir.path(), 3);
        let out = dir.path().join("out");
        run_quiz(quiz_args(&spec, &out, None, 7)).expect("quiz runs");
        for name in [
            "midterm-A.docx",
            "midterm-A-key.docx",
            "midterm-B.docx",
            "midterm-B-key.docx",
            "midterm-C.docx",
            "midterm-C-key.docx",
        ] {
            let path = out.join(name);
            assert!(path.is_file(), "{name} was not written");
            assert!(
                fs::metadata(&path).expect("metadata").len() > 0,
                "{name} is empty"
            );
        }
    }

    #[test]
    /// The run is recorded beside the sheets: the seed that drew it, every
    /// variant, and the options as printed. Without it a paper handed out
    /// months ago cannot be accounted for.
    fn quiz_writes_a_manifest_recording_the_seed_and_variants() {
        let dir = tempfile::tempdir().expect("temp dir");
        let spec = quiz_fixture(dir.path(), 2);
        let out = dir.path().join("out");
        run_quiz(quiz_args(&spec, &out, None, 77)).expect("quiz runs");
        let manifest =
            fs::read_to_string(out.join("midterm-manifest.yaml")).expect("manifest written");
        assert!(manifest.contains("seed: 77"), "{manifest}");
        assert!(manifest.contains("variant: A"), "{manifest}");
        assert!(manifest.contains("variant: B"), "{manifest}");
        assert!(manifest.contains("id: q"), "{manifest}");
        // The options as printed, so the record says what the paper said.
        assert!(manifest.contains("alpha"), "{manifest}");
        assert!(manifest.contains("beta"), "{manifest}");
    }

    #[test]
    /// The output directory is created rather than required to exist: the
    /// first run of a new quiz should not fail on a missing folder.
    fn quiz_creates_its_output_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let spec = quiz_fixture(dir.path(), 1);
        let out = dir.path().join("nested/sheets");
        run_quiz(quiz_args(&spec, &out, None, 1)).expect("quiz runs");
        assert!(out.join("midterm.docx").is_file());
        assert!(out.join("midterm-key.docx").is_file());
    }

    #[test]
    /// A seed reproduces a run exactly, byte for byte. A sheet handed out can
    /// otherwise never be rebuilt — which is why the seed is printed.
    fn quiz_is_reproducible_for_a_seed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let spec = quiz_fixture(dir.path(), 2);
        let read = |out: &Path| fs::read(out.join("midterm-B.docx")).expect("read sheet");
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        run_quiz(quiz_args(&spec, &first, None, 99)).expect("first run");
        run_quiz(quiz_args(&spec, &second, None, 99)).expect("second run");
        assert_eq!(
            read(&first),
            read(&second),
            "the same seed drew differently"
        );
    }

    #[test]
    /// `--name` overrides the spec's stem, so two specs can write into one
    /// folder without colliding.
    fn quiz_name_overrides_the_spec_stem() {
        let dir = tempfile::tempdir().expect("temp dir");
        let spec = quiz_fixture(dir.path(), 1);
        let out = dir.path().join("out");
        run_quiz(quiz_args(&spec, &out, Some("final"), 3)).expect("quiz runs");
        assert!(out.join("final.docx").is_file());
        assert!(!out.join("midterm.docx").exists());
    }

    #[test]
    /// A group folder cannot climb out of the spec's own directory. A spec is
    /// checked in beside its questions; one reaching `../../etc` is reading
    /// somewhere its author did not choose.
    fn quiz_group_folder_cannot_escape_the_spec_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let refused = list_group_sources(dir.path(), "../elsewhere");
        assert!(refused.is_err(), "the escape was allowed");
    }

    #[test]
    /// Output files are named from the spec, and a spec with no stem still
    /// gets a name rather than an extension-only file.
    fn spec_stem_names_the_output() {
        assert_eq!(spec_stem(Path::new("out/midterm.yaml")), "midterm");
        assert_eq!(spec_stem(Path::new("final.yml")), "final");
        assert_eq!(spec_stem(Path::new("")), "quiz");
    }

    #[test]
    /// The answer-key path inserts `-key` before the extension.
    fn key_path_inserts_key_suffix() {
        assert_eq!(key_path(Path::new("quiz.md")), Path::new("quiz-key.md"));
        assert_eq!(
            key_path(Path::new("out/quiz.md")),
            Path::new("out/quiz-key.md")
        );
        assert_eq!(key_path(Path::new("quiz")), Path::new("quiz-key"));
    }

    #[test]
    /// `write_answer_key` writes the key beside the sheet.
    fn write_answer_key_writes_key_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_answer_key(&true_false_bank("P?"), &dir.path().join("quiz.md")).expect("write key");
        let key = fs::read_to_string(dir.path().join("quiz-key.md")).expect("read key");
        assert!(key.contains("Answer Key"));
    }

    #[test]
    /// Canvas export writes a zip package (starting with the PK signature).
    fn write_export_writes_canvas_zip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out = dir.path().join("quiz.zip");
        write_export(&true_false_bank("P?"), export::Format::Canvas, &out, &[]).expect("write");
        let bytes = fs::read(&out).expect("read back");
        assert!(bytes.starts_with(b"PK"));
    }

    /// A program name no PATH will resolve, for exercising the "not installed"
    /// branch of [`run_first_available`].
    const MISSING_PROGRAM: &str = "mdquiz-no-such-program";

    #[test]
    /// An unspawnable candidate is skipped in favour of the next one.
    fn run_first_available_skips_missing_candidates() {
        let result = run_first_available(
            &[(MISSING_PROGRAM, &[]), ("cargo", &[])],
            &[OsStr::new("--version")],
            "none found",
        );
        assert!(result.is_ok());
    }

    #[test]
    /// When no candidate can be spawned, the caller's message is returned.
    fn run_first_available_reports_missing_tool() {
        let error = run_first_available(&[(MISSING_PROGRAM, &[])], &[], "none found")
            .expect_err("no such program");
        assert_eq!(error, "none found");
    }

    #[test]
    /// A candidate that runs but fails reports its own stderr, named — the
    /// difference between "not installed" and "bad diagram".
    fn run_first_available_reports_stderr_on_failure() {
        let error = run_first_available(
            &[("cargo", &[])],
            &[OsStr::new("--mdquiz-not-a-flag")],
            "none found",
        )
        .expect_err("bad flag");
        assert!(error.starts_with("`cargo`: ") && error != "none found");
    }

    #[test]
    /// A candidate's own leading arguments are passed before the shared ones —
    /// the mermaid fallback (`npx -y @mermaid-js/mermaid-cli`, then `-i`/`-o`)
    /// depends on that order. `test ok = ok` exits 0; any other order is a
    /// usage error.
    fn run_first_available_passes_leading_args_first() {
        // `test ok = ok` exits 0 and `false` exits 1, so this is `Ok` only if the
        // unspawnable candidate is skipped *and* the rest are tried in order.
        let result = run_first_available(
            &[
                (MISSING_PROGRAM, &[]),
                ("test", &["ok", "="]),
                ("false", &[]),
            ],
            &[OsStr::new("ok")],
            "none found",
        );
        assert!(result.is_ok());
    }

    #[test]
    /// A tool that fails silently still reports which program failed, rather
    /// than an empty message.
    fn tool_failure_names_program_when_stderr_is_empty() {
        let quiet = std::process::Command::new("cargo")
            .arg("--version")
            .output()
            .map(|mut output| {
                output.stderr.clear();
                output
            })
            .expect("cargo runs");
        let message = tool_failure("dot", &quiet);
        assert!(message.starts_with("`dot` failed ("));
    }

    /// Whether Graphviz is installed, so a `dot`-dependent test can skip
    /// deliberately rather than pass silently on a render failure.
    fn graphviz_available() -> bool {
        let available =
            run_first_available(&[("dot", &[])], &[OsStr::new("-V")], "missing").is_ok();
        if !available {
            eprintln!("skipping: Graphviz not installed");
        }
        available
    }

    #[test]
    /// With Graphviz installed, DOT source renders to real bytes in each format
    /// (so the `-T` flag matches the extension the pass will name the file).
    fn render_diagram_renders_dot_in_each_format() {
        if !graphviz_available() {
            return; // Graphviz is not installed on this machine.
        }
        let source = "digraph { a -> b; }";
        let png =
            render_diagram(DiagramLanguage::Graphviz, source, DiagramFormat::Png).expect("png");
        assert!(png.starts_with(b"\x89PNG"));
        let svg =
            render_diagram(DiagramLanguage::Graphviz, source, DiagramFormat::Svg).expect("svg");
        assert!(String::from_utf8_lossy(&svg).contains("<svg"));
    }

    #[test]
    /// Malformed DOT fails with Graphviz's own message rather than silently
    /// producing an image — the pass turns that message into a warning.
    fn render_diagram_reports_dot_syntax_error() {
        if !graphviz_available() {
            return; // Graphviz is not installed on this machine.
        }
        let error = render_diagram(DiagramLanguage::Graphviz, "digraph {", DiagramFormat::Png)
            .expect_err("malformed dot must fail");
        assert!(error.starts_with("`dot`") && error != DOT_MISSING);
    }

    #[test]
    /// The Canvas image set is the local images plus the pass's generated ones;
    /// an unrenderable diagram simply contributes nothing and keeps its fence.
    fn canvas_images_merges_local_and_generated() {
        use mdquiz::model::{Feedback, Question, QuestionKind, TrueFalse};
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("real.png"), b"x").expect("write image");
        let mut bank = ItemBank {
            name: "M".to_owned(),
            items: vec![Question {
                id: "q".to_owned(),
                title: None,
                prompt: "![r](real.png)\n\n```dot\ndigraph { a -> b; }\n```".to_owned(),
                points: 1.0,
                tags: Vec::new(),
                feedback: Feedback::default(),
                kind: QuestionKind::TrueFalse(TrueFalse { answer: true }),
            }],
        };
        let images = canvas_images(dir.path(), &mut bank.items, DiagramFormat::Png);
        let names: Vec<&str> = images.iter().map(|(path, _)| path.as_str()).collect();
        assert!(names.contains(&"real.png"));
        let prompt = bank.items.first().map_or("", |q| q.prompt.as_str());
        if graphviz_available() {
            assert!(prompt.contains("![diagram](generated/graphviz-"));
            assert!(names.iter().any(|n| n.starts_with("generated/graphviz-")));
        } else {
            assert!(prompt.contains("```dot"));
            assert!(!names.iter().any(|n| n.starts_with("generated/")));
        }
    }
}
