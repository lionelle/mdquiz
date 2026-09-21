# mdquiz

Author quiz questions as **Markdown + YAML**, then export a whole directory of
them as a **Canvas *New Quizzes* item bank** (QTI package), a **print-ready
Markdown sheet**, or a **printable Word exam** — several shuffled variants, each
with its own answer key.

> **Status: alpha.** All six question types work on both exporters and the
> format is documented, but it hasn't been battle-tested across many Canvas
> instances yet. Expect rough edges; feedback welcome.

Each question is a single Markdown file: a `---`-delimited **YAML front-matter**
block carries the machine-readable parts (answers, choices, scoring…), and
everything below it is the prompt as ordinary Markdown.

```markdown
---
id: tf-binary-search
kind: true_false
answer: true
---

Binary search requires its input array to be sorted.
```

## Features

- **Six question types:** true/false, multiple choice, multiple select, fill in
  the blank (literal *and* regex), matching, and ordering.
- **Rich prompts and answers:** images (bundled into the package), LaTeX math
  (`$…$`, rendered by Canvas's native equation service), [Mermaid](https://mermaid.js.org)
  and [Graphviz DOT](https://graphviz.org) diagrams (rendered to images), and
  `file:` includes that pull content from separate Markdown files.
- **Three exports:** a Canvas New Quizzes **QTI `.zip`** (the default), a
  **print-ready Markdown** sheet with no answer key (add `--include-key` for a
  matching answer-key file), and a **Word `.docx` exam** for paper.
- **Printable exams from a blueprint:** `mdquiz quiz` reads a YAML spec — which
  topic folders to draw from, how many questions from each, how many variants —
  and writes one Word sheet *and its own answer key* per variant, plus a
  manifest recording the seed and what each paper actually said. Math becomes a
  real Word equation, not a picture; lists, tables, images and code blocks are
  laid out rather than printed as source.
- **One command:** point it at a directory; add `--recursive` to gather a whole
  tree of subfolders into one bank, or `--sample N` to draw N random questions
  from each folder. For print sheets, `--include-key` emits an answer key and
  `--random-order` shuffles the questions, and `--seed N` makes a sampled or
  shuffled sheet reproducible.

## Install

mdquiz is a Rust CLI. You need [Rust](https://rustup.rs) **1.96 or newer**
(`rustup update stable`). Build and install the `mdquiz` binary from source:

```bash
git clone https://github.com/lionelle/mdquiz
cd mdquiz
cargo install --path .
```

That puts `mdquiz` on your `PATH` (via `~/.cargo/bin`). Prefer not to install?
Run it in place with `cargo run -- <args>` instead of `mdquiz <args>`.

**Optional:** rendering diagrams needs the matching tool on your `PATH` —
Mermaid uses the [mermaid CLI](https://github.com/mermaid-js/mermaid-cli)
(`mmdc`, otherwise `npx @mermaid-js/mermaid-cli`;
`npm install -g @mermaid-js/mermaid-cli`) and Graphviz DOT uses
[`dot`](https://graphviz.org/download/) (`sudo apt install graphviz`,
`brew install graphviz`). Without it, those diagrams are left as code blocks and
a warning tells you which; everything else still exports.

## Quick start

```bash
# 1. Make a directory with one question.
mkdir quiz
cat > quiz/01-binary-search.md <<'EOF'
---
id: tf-binary-search
kind: true_false
answer: true
---

Binary search requires its input array to be sorted.
EOF

# 2. Export a Canvas package (canvas is the default format).
mdquiz export quiz/ --output quiz.zip

# ...or a print-ready sheet:
mdquiz export quiz/ --output quiz.md --format markdown
```

Then in Canvas: create a **New Quizzes item bank**, open it, and import
`quiz.zip` through the **"QTI .zip file"** option. The bank name defaults to the
directory name; override it with `--name`.

For a paper exam, write a blueprint beside your question folders and hand it to
`mdquiz quiz`:

```yaml
# exam.yaml
name: "CS 5001 — Midterm"
variants: 1                  # bump this once you have questions to spare
groups:
  - dir: quiz                # a folder of questions
    take: all                # or a number, drawn at random
```

```bash
mdquiz quiz exam.yaml --out-dir exam/ --seed 20260921
```

That writes `exam.docx`, `exam-key.docx` and `exam-manifest.yaml` — the manifest
recording the seed and what the paper actually said. Keep the seed and you can
rebuild the exact papers you handed out.

Ask for more `variants:` than your questions can fill and mdquiz tells you
instead of printing the same paper twice under different letters:

```
3 variants asked for but the groups can only make 1 distinct question set(s);
widen a `take:` range, add questions, or set `shuffle_choices: true`
```

With enough questions you get `exam-A.docx` and `exam-A-key.docx` through to
`C`. See [`docs/quizzes.md`](docs/quizzes.md) for the full spec format.

The [`samples/`](samples/) directory has a ready-to-export bank for every
question type — try `mdquiz export samples/multiple-choice/ --output mc.zip`.

## How it works

```
quiz/*.md ─▶ parse ─┬─▶ ItemBank ──────────▶ Canvas New Quizzes QTI (.zip)
                    │                    └─▶ print Markdown (no solutions)
                    │
                    └─▶ spec ─▶ assemble ─▶ Exam ─▶ Word .docx sheet + answer key
```

Every random decision — which questions a variant draws, what order its options
print in — is made once, while the exam is assembled, and frozen into the `Exam`
the writers read. A sheet and its answer key are two renderings of settled data,
so they cannot disagree about which option is `B`.

Questions follow the Canvas **New Quizzes** model. The parsing and export stages
are pure data transforms with no filesystem or process dependencies (the CLI
injects those), so the pipeline is easy to test and reason about.

## Documentation

- **[`docs/`](docs/)** — the authoring reference: [common
  metadata](docs/README.md) (which fields are required vs optional), a page per
  [question type](docs/README.md#question-types), and guides for
  [images](docs/images.md), [math](docs/math.md),
  [diagrams](docs/diagrams.md) (Mermaid and Graphviz),
  [file includes](docs/partials.md), [exporting](docs/exporting.md), and
  [quiz specs](docs/quizzes.md) (the blueprint for a printable exam).
- **[`samples/`](samples/)** — runnable example banks, one directory per feature.

## Development

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
pre-commit run --all-files   # the same gate the commit hook runs
```

The quality bar (enforced by clippy, the pre-commit hook, and CI): functions
≤ 30 lines, docs on every item, zero warnings, no uncaught panics, no `unsafe`.
See [`CLAUDE.md`](CLAUDE.md) for the full working agreement.

## License

Licensed under the [MIT License](LICENSE).
