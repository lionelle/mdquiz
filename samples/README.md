# mdquiz samples

Each subdirectory here is a ready-to-export **item bank**: a folder of
per-question Markdown files. Point `mdquiz` at a directory to build one bank
from every `*.md` file inside it (files are ordered by filename, so the numeric
prefixes control question order). Add `-r`/`--recursive` to gather nested
subdirectories into a single bank — e.g. `export samples/ -r` builds one bank
from all of these; READMEs and partials (files without front-matter) are
skipped. See [`../docs/exporting.md`](../docs/exporting.md).

```bash
# Print-ready sheet (no answer key):
cargo run -- export samples/true-false/ --output true-false.md --format markdown

# Canvas New Quizzes item bank (zipped QTI package):
cargo run -- export samples/true-false/ --output true-false.zip --format canvas
```

The Canvas output is a zipped QTI package; import it through Canvas's **"QTI
.zip file"** option, so use a `.zip` extension.

The [`partials/`](partials/) bank shows **file includes**: choices and feedback
that pull their content (and an image) from separate Markdown files.

The bank name defaults to the directory name (`true-false`); override it with
`--name "Module 1 — Fundamentals"`.

> The authoring format is documented in full under [`../docs/`](../docs/) —
> including which metadata fields are required vs optional, a page per question
> type, and the Canvas caveats. The sections below are a quick tour; `docs/` is
> the reference.

## Question file format

A question file is **YAML front-matter** (a `---`-delimited block at the very
top of the file) carrying the machine-readable data, followed by the prompt as
ordinary Markdown. Everything below the closing `---` is the prompt, so you can
use normal Markdown — emphasis, inline `code`, Markdown headings, GitHub-flavored
pipe tables and `~~strikethrough~~`, even fenced code blocks in any language.

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

By default a multiple-select is graded **all-or-nothing** (full marks only for
selecting every correct option and no incorrect one). Add `scoring: partial` for
partial credit — each correct selection adds an even share of the marks and each
incorrect one subtracts a share (clamped to zero):

```yaml
kind: multiple_select
scoring: partial   # or all_or_nothing (the default)
choices: [...]
```

(All-or-nothing matches Canvas's own export exactly. Partial credit uses the
standard per-choice `Add`/`Subtract` QTI encoding.)

### Fill in the blank (`kind: fill_in_blank`)

One or more **inline** blanks, marked `{{name}}` in the prompt. Each name has an
entry under `blanks` listing its acceptable answers. Shorthand is a bare list;
the expanded form adds a `match:` mode (`case_insensitive` / `exact` / `regex`):

```markdown
---
id: fitb-http-status
kind: fill_in_blank
blanks:
  method: [GET, get]
  code:
    answers: ["404", "Not Found"]
    match: exact
---

An HTTP {{method}} request for a missing resource returns status {{code}}.
```

Rules: every `{{name}}` must have a `blanks` entry and vice versa, and each
blank needs at least one answer. Scored as partial credit per blank. On the
print sheet each blank becomes a fill-in line.

The expanded form takes a `match` mode:

| `match`            | Meaning                                                        |
|--------------------|----------------------------------------------------------------|
| `case_insensitive` | Default. `get` matches `GET`.                                  |
| `exact`            | Case-sensitive exact match.                                    |
| `regex`            | Answers are regex patterns (see caveats).                      |

```yaml
blanks:
  hex:
    match: regex
    answers: ['#[0-9A-Fa-f]{6}']
```

**Canvas caveats:** exports as `fill_in_multiple_blanks_question` (Open Entry
text blanks). New Quizzes' **dropdown** and **word bank** answer types, and the
**contains / close-enough** match modes, have no classic-QTI representation and
are not exported. Canvas fill-in-the-blank matches **case-insensitively**, so
the `exact` mode is recorded but may not be enforced on import. A **`regex`**
blank exports as a *literal-text* blank (Canvas can't set the regex mode via
import) — after importing, `mdquiz` reminds you which blanks to switch to
"Regular Expression Match" in the New Quizzes editor. Only `general` feedback is
wired (answer-level feedback is multiple-choice only).

### Matching (`kind: matching`)

Pair each left-hand prompt with its correct right-hand answer. Optional
`distractors` add extra right-hand options that match no prompt:

```markdown
---
id: mt-c-types
kind: matching
pairs:
  - left: char
    right: 1 byte
  - left: int
    right: 4 bytes
distractors: [8 bytes]
---

Match each C type to its size.
```

Rules: at least two pairs, each side non-empty. Scored partial credit per pair.
The print sheet lists the prompts numbered and all options lettered (sorted, so
they don't line up with the prompts). Exports as Canvas `matching_question`
(verified against Canvas's own export structure). Left/right cells are plain
text.

### Ordering (`kind: ordering`)

List the `items` in their **correct** order; mdquiz shuffles them for display so
the shown sequence is never the answer:

```markdown
---
id: ord-alloc
kind: ordering
items:
  - Declare a pointer
  - Call malloc
  - Check for NULL
  - Use the memory
  - Call free
---

# Heap allocation lifecycle

Put the steps of a typical C heap allocation in the order they should happen.
```

Rules: at least two items, each non-empty. Scored **all-or-nothing** — full
marks only when the entire sequence is correct (matching Canvas's own
`ordering_question`, verified against its export structure). On the print sheet
the items are listed sorted, each with a write-in blank for its position. Only
`general` feedback is wired.

## Images

Prompts are full Markdown, so images use standard `![alt](src)` syntax:

- **External URLs** (`![](https://…/x.png)`) pass through unchanged and render
  in both exports.
- **Local files** (`![](diagram.png)`, resolved relative to the question
  directory) are **bundled into the Canvas package** under `web_resources/` and
  rewritten to Canvas's `$IMS-CC-FILEBASE$` reference so they resolve on import.
  A referenced file that is missing is skipped with a warning.

```markdown
The tree below is balanced ![tree](binary-tree.png), unlike ![this one](https://ex.com/skew.png).
```

See [`images/`](images/) for a runnable example (`binary-tree.png` is bundled;
the Rust-logo URL is passed through). Local-image bundling follows Canvas's
Common Cartridge format; if an item-bank import doesn't show a bundled image,
host it and use a URL instead.

## Canvas caveats

- **True/false feedback:** New Quizzes only keeps *answer-level* feedback for
  multiple choice. For true/false, `correct:`/`incorrect:` are dropped on
  item-bank import (a Canvas limitation); only `general:` may survive.
- **Tags** are for mdquiz-side organization only — they are never written into
  the QTI, and Canvas would not import them anyway.
