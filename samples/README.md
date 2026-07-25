# mdquiz samples

Each subdirectory here is a ready-to-export **item bank**: a folder of
per-question Markdown files. Point `mdquiz` at a directory to build one bank
from every `*.md` file inside it (files are ordered by filename, so the numeric
prefixes control question order).

```bash
# Print-ready sheet (no answer key):
cargo run -- export samples/true-false/ --output true-false.md --format markdown

# Canvas New Quizzes item bank (zipped QTI package):
cargo run -- export samples/true-false/ --output true-false.zip --format canvas
```

The Canvas output is a zipped QTI package; import it through Canvas's **"QTI
.zip file"** option, so use a `.zip` extension.

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

### Multiple choice (`kind: multiple_choice`)

A single-answer question. List the options under `choices` (order is preserved),
and mark the one correct option with `correct: true`:

```markdown
---
id: mc-binary-search-complexity
title: Binary search complexity
kind: multiple_choice
choices:
  - text: O(log n)
    correct: true
  - text: O(n)
  - text: O(1)
feedback:
  correct: Right — each comparison halves the range.
  incorrect: How much of the array is discarded per comparison?
---

What is the worst-case time complexity of binary search?
```

Rules (checked at parse time): at least two choices, and **exactly one** marked
`correct`. Unlike true/false, Canvas New Quizzes *does* keep answer-level
feedback for multiple choice, so `correct:`/`incorrect:` messages survive the
item-bank import.

### Multiple select (`kind: multiple_select`)

"Choose all that apply" — the exact same `choices` shape as multiple choice, but
**one or more** may be marked `correct`. Scored all-or-nothing in Canvas: the
student must select every correct option and no incorrect one.

```markdown
---
id: ms-sorted-input
kind: multiple_select
choices:
  - text: Binary search
    correct: true
  - text: Linear search
  - text: Interpolation search
    correct: true
---

Select all algorithms that require sorted input.
```

Rules: at least two choices, and **at least one** marked `correct`.

Remaining types (fill in the blank, matching, ordering) are rolled out one at a
time; each will get its own samples folder here as it lands.

## Canvas caveats

- **True/false feedback:** New Quizzes only keeps *answer-level* feedback for
  multiple choice. For true/false, `correct:`/`incorrect:` are dropped on
  item-bank import (a Canvas limitation); only `general:` may survive.
- **Tags** are for mdquiz-side organization only — they are never written into
  the QTI, and Canvas would not import them anyway.
