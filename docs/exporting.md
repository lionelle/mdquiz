# Exporting an item bank

`mdquiz` reads a **directory** of question files (one question per `*.md` file)
and assembles them into a single item bank. Files are ordered by filename, so
numeric prefixes control question order.

## Command

```bash
mdquiz export <DIR> --output <OUTPUT> --format <markdown|canvas> [--name <NAME>]
```

| Option            | Required? | Meaning                                                       |
|-------------------|-----------|---------------------------------------------------------------|
| `<DIR>`           | **Required** | Directory of Markdown question files.                      |
| `-o`, `--output`  | **Required** | Path to write the exported bank to.                        |
| `-f`, `--format`  | **Required** | `markdown` (print sheet) or `canvas` (QTI package).        |
| `-n`, `--name`    | Optional  | Bank name; defaults to the directory's own name.              |

## Formats

### `markdown` — print-ready sheet

A single Markdown document for paper distribution. It deliberately omits the
answer key and any scoring hints. Questions are numbered under the bank name.

```bash
mdquiz export samples/true-false/ --output true-false.md --format markdown
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
