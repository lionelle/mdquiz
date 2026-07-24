# mdquiz

Author quiz questions as **Markdown + YAML**, then export a whole quiz to either
a **print-ready Markdown sheet** (no answer key) or a **Canvas *New Quizzes* QTI
package** you can upload to Canvas.

> Status: early scaffold. The pipeline shape, tooling, and lint policy are in
> place; question types are being rolled out one at a time (see
> [`CLAUDE.md`](CLAUDE.md)).

## Idea

Each question is a Markdown file: the prompt is plain Markdown, and a YAML block
carries the machine-readable parts (choices, correct answers, scoring, matching
pairs, regex for fill-in-the-blank, …). `mdquiz` parses those into a typed model
and renders the export you ask for.

```
source .md  ─▶  parse  ─▶  Quiz model  ─▶  export ─┬─ print Markdown (no solutions)
                                                   └─ Canvas New Quizzes QTI
```

Question types follow the Canvas **New Quizzes** model. Planned, in roll-out
order: true/false, multiple choice, multiple select, fill in the blank (literal
or regex), matching, ordering.

## Usage

```bash
cargo run -- export questions.md --output quiz.md --format markdown
cargo run -- export questions.md --output quiz.imscc --format canvas
```

## Development

```bash
pre-commit install          # wire up the git hooks (once)
cargo test
pre-commit run --all-files  # fmt + clippy (-D warnings) + tests
```

Quality bar (enforced by clippy, the pre-commit hook, and CI): functions ≤ 30
lines, docs on every function, zero warnings, no uncaught panics, no `unsafe`.
See [`CLAUDE.md`](CLAUDE.md) for the full working agreement.

## License

Licensed under either of Apache-2.0 or MIT at your option.
