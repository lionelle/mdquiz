# mdquiz

Write your question bank once, as **Markdown + YAML**, and keep it in version
control alongside everything else you teach.

**mdquiz builds Canvas *New Quizzes* item banks.** Point it at a directory of
questions and it produces a QTI package you import straight into Canvas — that
is what it is for, and what the authoring format is shaped around.

The same questions also **print**, because a course that runs online quizzes
usually needs a paper exam too, and maintaining the questions twice is how the
two drift apart. A print-ready Markdown sheet, or a Word `.docx` exam in
several shuffled variants, each with its own answer key.

> **Status: alpha.** All six question types work on every exporter and the
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
- **Canvas item banks:** a New Quizzes **QTI `.zip`**, the default export.
  Import it through Canvas's "QTI .zip file" option; per-question and
  per-answer feedback, scoring modes and regex-matched blanks all come across.
- **Also prints:** a **Markdown sheet** for quick handouts (`--format
  markdown`, with `--include-key` for a matching key), and a **Word `.docx`
  exam** via `mdquiz quiz` — a YAML blueprint says which folders to draw from,
  how many questions from each, and how many variants, and you get one sheet
  *and its own answer key* per variant plus a manifest of what each paper
  said. Math becomes a real Word equation, not a picture.
- **One command:** point it at a directory; add `--recursive` to gather a whole
  tree of subfolders into one bank, or `--sample N` to draw N random questions
  from each folder. For print sheets, `--include-key` emits an answer key and
  `--random-order` shuffles the questions, and `--seed N` makes a sampled or
  shuffled sheet reproducible.

## Install

mdquiz is a Rust CLI. You need [Rust](https://rustup.rs) **1.96 or newer**
(`rustup update stable`). Build and install the `mdquiz` binary from source:

```bash
cargo install mdquiz
```
The above pulls the crate mdquiz, and installs the mdquiz command line tool. If you want the latest version or a development version, you can pull source directly from github and compile locally.


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
                      ┌─▶ ItemBank ─┬─▶ Canvas New Quizzes QTI (.zip)   ← the main path
quiz/*.md ─▶ parse ─┤              └─▶ print Markdown sheet (+ key)
                      │
                      └─▶ spec ─▶ assemble ─▶ Exam ─▶ Word .docx + key, per variant
```

Questions follow the Canvas **New Quizzes** model — that is what the fields
mean, and where any disagreement is resolved. The print paths render the same
questions. It is possible to take all questions or sample subsets, or even
sample subsets within dub directories (often used for print ones).


## Documentation

- **[`docs/`](docs/)** — the authoring reference: [common
  metadata](docs/README.md) (which fields are required vs optional), a page per
  [question type](docs/README.md#question-types), and guides for
  [images](docs/images.md), [math](docs/math.md),
  [diagrams](docs/diagrams.md) (Mermaid and Graphviz),
  [file includes](docs/partials.md), [exporting](docs/exporting.md), and
  [quiz specs](docs/quizzes.md) (the blueprint for a printable exam).
- **[`samples/`](samples/)** — runnable example banks, one directory per
  feature, plus [`samples/exam.yaml`](samples/exam.yaml): a worked quiz spec
  you can build a three-variant paper exam from.
- **[`CHANGELOG.md`](CHANGELOG.md)** — what changed in each release, and the
  known limitations of the current one.

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
