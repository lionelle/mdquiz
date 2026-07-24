# mdquiz samples

Each subdirectory here is a ready-to-export **item bank**: a folder of
per-question Markdown files. Point `mdquiz` at a directory to build one bank
from every `*.md` file inside it (files are ordered by filename, so the numeric
prefixes control question order).

```bash
# Print-ready sheet (no answer key):
cargo run -- export samples/true-false/ --output true-false.md --format markdown

# Canvas New Quizzes item bank (QTI package):
cargo run -- export samples/true-false/ --output true-false.imscc --format canvas
```

The bank name defaults to the directory name (`true-false`); override it with
`--name "Module 1 — Fundamentals"`.

## Question file format

A question file is **YAML front-matter** (a `---`-delimited block at the very
top of the file) carrying the machine-readable data, followed by the prompt as
ordinary Markdown. Everything below the closing `---` is the prompt, so you can
use normal Markdown — emphasis, inline `code`, Markdown headings, even fenced
code blocks in any language.

### Common fields

| Field    | Required | Default | Meaning                                                     |
|----------|----------|---------|-------------------------------------------------------------|
| `id`     | yes      | —       | Stable, unique id within the bank                           |
| `kind`   | yes      | —       | Question type (`true_false`, …)                             |
| `title`  | no       | `id`    | Instructor-facing name; shown in the Canvas bank editor     |
| `points` | no       | `1`     | Points for a fully correct answer                           |
| `tags`   | no       | `[]`    | Organizational tags (mdquiz-side; **not** exported to Canvas) |
| `feedback` | no     | —       | `general` / `correct` / `incorrect` messages (Markdown); Canvas only |

Feedback is written as a nested block and every message is optional:

```yaml
feedback:
  general: Shown no matter what.
  correct: Shown when the student is right.
  incorrect: Shown when the student is wrong.
```

Feedback appears in the Canvas export only — the print sheet carries no answer
key, so it is omitted there.

### Naming a question with a heading

Instead of the `title:` field, you can name a question by opening the body with
a level-1 heading. mdquiz uses it as the title and strips it from the prompt, so
it never shows to students:

```markdown
---
id: tf-stack-fifo
kind: true_false
answer: false
---

# Stack ordering

A stack is a first-in, first-out (FIFO) data structure.
```

An explicit `title:` field always wins; when it is present, a body heading is
left alone as ordinary prompt content.

## Question types

### True/false (`kind: true_false`)

One extra field, `answer` (`true` or `false`):

```markdown
---
id: tf-binary-search-sorted
title: Binary search prerequisites
kind: true_false
tags: [searching, algorithms]
answer: true
---

Binary search requires its input array to be sorted.
```

More types (multiple choice, multiple select, fill in the blank, matching,
ordering) are rolled out one at a time; each will get its own samples folder
here as it lands.
