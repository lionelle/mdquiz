# mdquiz

Author quiz questions as **Markdown + YAML**, then assemble a directory of them
into an **item bank** and export it to either a **print-ready Markdown sheet**
(no answer key) or a **Canvas *New Quizzes* item bank** (QTI package) you can
upload to Canvas.

> Status: early scaffold. The pipeline shape, tooling, and lint policy are in
> place; question types are being rolled out one at a time (see
> [`CLAUDE.md`](CLAUDE.md)).

## Idea

Each question is one Markdown file: **YAML front-matter** (a `---`-delimited
block at the top) carries the machine-readable parts (choices, correct answers,
scoring, matching pairs, regex for fill-in-the-blank, …), and everything below
it is the prompt as plain Markdown. A directory of question files — say
`module01/` with 10–20 questions — is assembled into a single item bank.
`mdquiz` parses those files into a typed model and renders the export you ask
for.

```
module01/*.md  ─▶  parse  ─▶  ItemBank model  ─▶  export ─┬─ print Markdown (no solutions)
                                                          └─ Canvas New Quizzes item bank (QTI)
```

An item bank is the only collection today. A future `Quiz` target can build on
the same model: in Canvas *New Quizzes* a quiz draws its questions from item
banks, so the bank is the natural first artifact.

Question types follow the Canvas **New Quizzes** model. Planned, in roll-out
order: true/false, multiple choice, multiple select, fill in the blank (literal
or regex), matching, ordering.

## Usage

```bash
# The bank name defaults to the directory name ("module01"); override with --name.
cargo run -- export module01/ --output module01.md     --format markdown
cargo run -- export module01/ --output module01.imscc  --format canvas
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
