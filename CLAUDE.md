# CLAUDE.md — working agreement for `mdquiz`

This file is the contract for how work happens in this repo. Follow it exactly.

## What `mdquiz` is

A Rust CLI that lets an instructor author quiz questions as **Markdown files with
an embedded YAML block** (Markdown for the human-readable prompt, YAML for the
machine-readable answers/choices/scoring), then export a whole quiz to one of:

- **print Markdown** — a single Markdown sheet for paper distribution, **no
  answer key**; or
- **Canvas New Quizzes QTI** — a package that imports into Canvas.

We follow the Canvas **New Quizzes** model for question options.

## Workflow — non-negotiable

1. **Run `/check-rs` between steps.** After each self-contained change (a new
   module, a question type, an exporter branch), run `/check-rs` before moving
   on. Do not batch several features and check once at the end.
2. **The gate is green or the step isn't done.** `cargo fmt --all -- --check`,
   `cargo clippy --all-targets --all-features -- -D warnings`, and
   `cargo test --all-features` must all pass. No warnings, ever.
3. **Small, reviewable commits.** One logical change per commit. Never commit
   with a failing gate or a skipped hook (`--no-verify` is off-limits).
4. **Tests land with the code.** Every question type and every exporter branch
   ships with unit tests in the same change. A type is not "rolled out" until it
   has tests on both export paths.
5. **New dependencies are curated only.** Prefer well-maintained, widely-used
   crates; justify anything unusual in the PR description. `cargo add` so
   versions resolve against the registry — don't hand-edit versions.

## Lint & style policy (enforced, not aspirational)

Declared in `Cargo.toml` `[lints]` and `clippy.toml`; the hook/CI promote every
warning to an error with `-D warnings`.

- **Functions ≤ 30 lines** (`clippy::too_many_lines`, threshold in `clippy.toml`).
  Split anything larger.
- **Docs on every function**, public *and* private (`missing_docs` +
  `clippy::missing_docs_in_private_items`). Public fallible fns document their
  `# Errors`.
- **No warnings** anywhere.
- **No uncaught panics.** `panic!`, `unwrap`, `expect`, `todo!`,
  `unimplemented!`, `unreachable!`, and unchecked indexing are denied. Return a
  `Result` and handle it. Scaffolding returns a typed error, it does not panic.
- **No `unsafe`** — it is `forbid`den.

If a lint genuinely must be suppressed, use a *narrowly scoped* `#[allow(...)]`
(or `#[expect(...)]`) with a `reason = "..."` on the specific item, never a
crate-wide relaxation.

## Architecture

The pipeline is a plain data transform so it is trivial to test without a
process:

```
source .md  ──▶  parse::item_bank_from_sources  ──▶  model::ItemBank  ──▶  export::{markdown,canvas}
```

- `src/model.rs` — the typed bank (`ItemBank`, `Question`, `QuestionKind`). The
  source of truth for what a question *is*.
- `src/parse.rs` — Markdown + YAML front-matter → `ItemBank` (`parse_question`,
  `item_bank_from_sources`). Resolves `file:` partials via an injected reader.
- `src/diagram.rs` — pre-export pass that renders ` ```mermaid ` and ` ```dot `
  blocks to bundled images via an injected renderer (the CLI shells out to
  `mmdc` and `dot`).
- `src/export/markdown.rs` — print sheet (no solutions).
- `src/export/canvas.rs` — Canvas New Quizzes QTI bytes.
- `src/path.rs` — path policy for authored, bank-relative paths (`escapes_dir`,
  `is_local_image`). Shared by the parser, the exporters and the CLI, which all
  have to agree on what a `file:`/image path may reach.
- `src/error.rs` — the crate `Error`/`Result`. Everything fallible flows through
  here.
- `src/cli.rs` + `src/main.rs` — thin CLI shell; **no logic** beyond wiring I/O
  to the library. Filesystem/process work is injected into the library as
  closures so the lower layers stay pure.

Keep the CLI thin. If you're tempted to put logic in `cli.rs`, it belongs in the
library.

## Question types

All six now ship, each with a payload struct and tests on both exporters, in the
roll-out order they were built:

1. **True/False**
2. **Multiple choice** (single answer)
3. **Multiple select** (choose all that apply)
4. **Fill in the blank** (literal text *and* regex matching, à la Canvas)
5. **Matching**
6. **Ordering**

On top of the types, prompts and answers support images, LaTeX math
(`$…$` → Canvas's native equation image), ` ```mermaid ` and ` ```dot `
(Graphviz) diagrams (rendered to bundled images), and `file:` partials. Any new
`QuestionKind` variant follows the same rule: a payload struct, both exporter
branches, and tests, in one change.

## Commands

```bash
cargo build
cargo run -- export input.md --output quiz.md --format markdown
cargo test
pre-commit run --all-files   # same gate the hook runs on commit
```
