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

## Machine limits — read before running anything heavy

**Everything that runs this crate's code goes through `memcap`.** This is not
a style preference; an uncapped run has taken the whole desktop down three
times.

```
memcap cargo test --all-features
memcap cargo mutants --jobs 1 --file src/export/docx/inline.rs
```

From the kernel log, not from guesswork: on 2026-09-16 at 19:25 and
2026-09-17 at 10:51 and 10:59, the lib test binary `mdquiz-c96b716a` reached
11.2, 10.6 and 11.0 GB of anonymous RSS and was killed by the **global** OOM
killer. Swap here is zram-only — compressed in RAM, so there is nowhere to
page out to — and each kill landed in the cgroup of the VS Code window the
session was running in (`app-code-*.scope: Failed with result 'oom-kill'`),
taking the editor down with the test.

`cargo mutants` is what triggers it. Mutating the `list` / `item_paragraphs`
recursion in `docx::inline` yields mutants that recurse or loop while
allocating, and 11 GB arrives within seconds — sooner than any wall-clock
timeout can help, which is why `.cargo/mutants.toml` is a backstop and not
the fix.

`memcap` (in `~/.local/bin`) runs the command in its own cgroup with
MemoryMax=6G and swap off, so a runaway mutant is SIGKILLed on its own and
the rest of the run carries on. `MEMCAP=12G memcap …` raises it for one run.
A PreToolUse hook in `.claude/settings.json` refuses `cargo test`,
`cargo mutants` and `cargo nextest` without it. Do not route around the hook
by running the binary in `target/debug/deps` directly — that is the exact
process that died three times.

### Build memory is the smaller, separate problem

This box has 20 cores but a busy desktop: it idles around **23 GB of 31 GB**
used (rust-analyzer alone holds ~2.7 GB across two instances). That leaves
roughly **6 GB** to build in. Measured on this crate:

| Workload | Peak RSS |
|---|---|
| cold `cargo build`, default 20 jobs | 1886 MB |
| cold `cargo build`, 4 jobs | 899 MB |
| cold `clippy --all-targets`, 20 jobs | 1586 MB |
| cold `clippy --all-targets`, 4 jobs | 811 MB |
| running the test suite, **unmutated** | 38 MB |
| `cargo mutants --jobs 1`, shared target dir | +1.1 GB |

Note what that last-but-one row does and does not say: the suite as written
is tiny, so a bare `cargo test` looks harmless. It is *mutated* code that
allocates without bound, and no build-side cap touches it.

Rules:

1. **Never run two cargo commands at once.** Not in parallel tool calls, not
   one agent while another builds. `.cargo/config.toml` caps jobs at 4, which
   halves peak memory for about 1.5 s more wall time — do not raise it here.
2. **At most one subagent at a time**, and only when a genuinely fresh view is
   worth it. Sequential is fine; slower is fine.
3. **Agents share one `CARGO_TARGET_DIR`, and not one under `/tmp`.** A
   per-agent target directory rebuilds all 114 dependencies from cold, for
   maximum memory and maximum disk. Worse, `/tmp` here is **tmpfs**, so a
   target tree put in a scratch directory is held in RAM: the 4.8 GB two
   sessions left behind was 4.8 GB of the 31 GB this box has, spent to save
   memory. Point it at a real filesystem, or just use `target/`.
4. **`cargo mutants`**: always under `memcap`, always `--jobs 1`, and point
   `CARGO_TARGET_DIR` at the shared tree so dependencies stay warm.

## Lint & style policy (enforced, not aspirational)

Declared in `Cargo.toml` `[lints]` and `clippy.toml`; the hook/CI promote every
warning to an error with `-D warnings`.

- **Functions ≤ 30 lines** (`clippy::too_many_lines`, threshold in `clippy.toml`).
  Split anything larger.
- **Cognitive complexity ≤ 15** (`clippy::cognitive_complexity`, threshold in
  `clippy.toml`). The lint belongs to `clippy::restriction`, so it is enabled by
  name — that group is not one to enable wholesale. `clippy::nursery` is *not*
  enabled: it does not carry this lint, and its unstable lints can break CI on
  unchanged code.
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
source .md ──▶ parse ──▶ model::ItemBank ──▶ export::{markdown,canvas}
                 │
                 └──▶ quiz::{spec,assemble} ──▶ quiz::exam::Exam ──▶ export::docx
```

Two pipelines share the parsing stage. The bank path is the original export; the
exam path builds a printable, sampled, multi-variant sheet and does **not** go
through `ItemBank`. Note `parse::item_bank_from_sources` is the inline-content
entry point — the recursive paths resolve each question's partials against its
own folder, which that helper cannot do.

- `src/model.rs` — the typed bank (`ItemBank`, `Question`, `QuestionKind`). The
  source of truth for what a question *is*.
- `src/parse.rs` — Markdown + YAML front-matter → `ItemBank` (`parse_question`,
  `item_bank_from_sources`). Resolves `file:` partials via an injected reader.
- `src/diagram.rs` — pre-export pass that renders ` ```mermaid ` and ` ```dot `
  blocks to bundled images via an injected renderer (the CLI shells out to
  `mmdc` and `dot`).
- `src/export/markdown.rs` — print sheet (no solutions).
- `src/export/canvas.rs` — Canvas New Quizzes QTI bytes.
- `src/export/docx.rs` — Word document bytes for printing, built from a
  `quiz::exam::Exam`. Owns the *container* — parts, styles, page furniture —
  and delegates content: `docx::inline` turns authored Markdown into `w:r`
  runs, `docx::omml` turns LaTeX into OOXML math, `docx::numbering` owns the
  list definitions a `w:numPr` resolves against. (`docx.rs` still writes runs
  of its own for page furniture — page breaks, answer space, footer fields —
  which carry no authored text.) Shares the zip/XML-escape helpers in
  `export.rs` with the Canvas exporter.
- `src/quiz/` — the printable-exam pipeline: `spec` (the authored YAML
  blueprint and its validation) → `assemble` (the one place randomness happens)
  → `exam` (the frozen `Exam` the print writers consume), with `sample` for the
  seeded draws.
- `src/label.rs` — `A`..`Z`-then-number sequence labels, shared by answer
  choices and quiz variants.
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
