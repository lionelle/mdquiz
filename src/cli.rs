//! Command-line interface for the `mdquiz` binary.
//!
//! The CLI is a thin shell over the library: it reads source files, hands them
//! to [`mdquiz::parse`], and writes the chosen export target to disk. All real
//! work lives in the library so it can be tested without a process.

use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};

use mdquiz::export::{self, canvas, markdown};
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
    /// Export a quiz source file to the chosen format.
    Export {
        /// Path to the Markdown quiz source.
        input: PathBuf,
        /// Path to write the exported quiz to.
        #[arg(short, long)]
        output: PathBuf,
        /// Export format to produce.
        #[arg(short, long, value_enum)]
        format: FormatArg,
    },
}

/// The export format as selected on the command line.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum FormatArg {
    /// Print-ready Markdown with no answer key.
    Markdown,
    /// Canvas New Quizzes QTI package.
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
/// Returns any error from reading the source, parsing it, exporting it, or
/// writing the output file.
pub(crate) fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Export {
            input,
            output,
            format,
        } => {
            let source = fs::read_to_string(&input)
                .with_context(|| format!("reading quiz source {}", input.display()))?;
            let quiz = parse::parse_quiz(&source)?;
            write_export(&quiz, format.into(), &output)
        }
    }
}

/// Render `quiz` in `format` and write the result to `output`.
///
/// # Errors
///
/// Returns an error if rendering fails or the output file cannot be written.
fn write_export(
    quiz: &mdquiz::model::Quiz,
    format: export::Format,
    output: &PathBuf,
) -> anyhow::Result<()> {
    match format {
        export::Format::Markdown => {
            let rendered = markdown::to_print_markdown(quiz);
            fs::write(output, rendered)
                .with_context(|| format!("writing Markdown to {}", output.display()))
        }
        export::Format::Canvas => {
            let bytes = canvas::to_qti(quiz)?;
            fs::write(output, bytes)
                .with_context(|| format!("writing Canvas package to {}", output.display()))
        }
    }
}
