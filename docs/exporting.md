# Exporting an item bank

`mdquiz` reads a **directory** of question files (one question per `*.md` file)
and assembles them into a single item bank. Files are ordered by filename, so
numeric prefixes control question order.

## Command

```bash
mdquiz export <DIR> --output <OUTPUT> \
  [--format <markdown|canvas>] [--name <NAME>] [--recursive] [--sample <N>] \
  [--include-key] [--random-order] [--diagram-format <png|svg>]
```

| Option              | Required? | Meaning                                                       |
|---------------------|-----------|---------------------------------------------------------------|
| `<DIR>`             | **Required** | Directory of Markdown question files.                      |
| `-o`, `--output`    | **Required** | Path to write the exported bank to.                        |
| `-f`, `--format`    | Optional  | `canvas` (QTI package, **default**) or `markdown` (print sheet). |
| `-n`, `--name`      | Optional  | Bank name; defaults to the directory's own name.              |
| `-r`, `--recursive` | Optional  | Descend into subdirectories, gathering every question into one bank (see below). |
| `--sample <N>`      | Optional  | Keep at most `N` randomly-chosen questions **from each directory** (see below). |
| `--include-key`     | Optional  | Also write a matching answer key (markdown export only; see below). |
| `--random-order`    | Optional  | Shuffle the questions into a random order after selection (markdown export only). |
| `--diagram-format`  | Optional  | Diagram image format, `png` (default) or `svg`; Canvas export only. See [diagrams.md](diagrams.md). |

## Which files become questions

By default the export reads only the `*.md` files **directly inside** `<DIR>`;
subdirectories are skipped. A file counts as a question only if it opens with a
YAML front-matter block, so **`README.md`, partials, and other prose Markdown
are ignored** (not errors).

With `-r` / `--recursive`, the walk descends into subdirectories and gathers
every question into a single bank, ordered by relative path. Each question's
images and `file:` partials resolve relative to **its own folder**, so a
question at `module01/q.md` referencing `diagram.png` uses
`module01/diagram.png`. Question `id`s must be unique across the whole tree.

```bash
# One bank from an entire course tree:
mdquiz export course/ --recursive --output course.zip --format canvas
```

## Sampling a random subset

`--sample <N>` keeps at most `N` randomly-chosen questions **from each
directory** (questions are grouped by the folder they live in). Combined with
`-r`, it draws `N` from every topic folder — handy for generating a print quiz
that covers each topic without using every question. A folder with fewer than
`N` questions contributes all of them.

```bash
# A practice sheet: 3 random questions from each topic folder.
mdquiz export course/ --recursive --sample 3 --format markdown --output practice.md
```

The selection is random on every run (re-run for a different draw); the chosen
questions are laid out in their normal path order.

## Formats

### `markdown` — print-ready sheet

A single Markdown document for paper distribution. It deliberately omits the
answer key and any scoring hints. Questions are numbered under the bank name.

```bash
mdquiz export samples/true-false/ --output true-false.md --format markdown
```

#### Answer key (`--include-key`)

Add `--include-key` to also write a **second** file — `<output>-key.md` (e.g.
`quiz.md` → `quiz-key.md`) — an instructor answer key. It uses the same
numbering as the sheet, and each entry shows **only** the correct answer(s):
`True`/`False`, the correct choice(s) by letter, each blank's accepted answers,
the correct matching pairs, or the items in their correct order.

```bash
mdquiz export quiz/ --format markdown --include-key --output quiz.md
# writes quiz.md (student sheet) and quiz-key.md (answer key)
```

`--include-key` applies only to the markdown format (the Canvas package already
carries the answers).

#### Random order (`--random-order`)

Add `--random-order` to shuffle the questions into a random order **after**
selection (so it composes with `--sample`). When combined with `--include-key`,
the answer key is shuffled to match, keeping the numbering aligned. Like
`--include-key`, it applies only to the markdown format.

```bash
# Sample 3 per topic, shuffle them, and print a keyed practice quiz.
mdquiz export course/ -r --sample 3 --random-order --include-key \
  --format markdown --output practice.md
```

### `canvas` — Canvas New Quizzes item bank

A zipped QTI package (an `imsmanifest.xml` plus one QTI 1.2 assessment
document). Use a **`.zip`** extension.

```bash
mdquiz export samples/true-false/ --output true-false.zip --format canvas
```

Import it into Canvas through the **"QTI .zip file"** option. The recommended
flow is to **create an item bank first**, then import the QTI into it.

If the bank contains any `regex` fill-in-the-blank blanks, mdquiz prints
post-export reminders naming the blanks to switch to "Regular Expression Match"
in the New Quizzes editor (Canvas can't set that mode via import).

## What is and isn't exported

- **Feedback** is exported to Canvas only; the print sheet has no answer key.
- **Tags** are for mdquiz-side organization only — they are never written into
  the QTI, and Canvas would not import them anyway.
- See each type's page for per-type Canvas caveats.
