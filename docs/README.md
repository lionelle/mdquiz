# mdquiz documentation

`mdquiz` turns a directory of **Markdown + YAML** question files into a Canvas
*New Quizzes* **item bank**. The same questions also print — as a Markdown
sheet, or as a Word exam in several variants. This folder is
the reference for the authoring format; [`../samples/`](../samples/) holds
runnable examples you can export as-is.

> New here? Start with the [project README](../README.md) for installation and a
> first-run quick start, then come back here for the authoring format.

## How a question file is shaped

Every question lives in its own `*.md` file: a **YAML front-matter** block
(delimited by `---` at the very top) carries the machine-readable data, and
everything below the closing `---` is the prompt as ordinary Markdown
(including GitHub-flavored pipe tables and `~~strikethrough~~`).

```markdown
---
id: tf-binary-search
kind: true_false
answer: true
---

Binary search requires its input array to be sorted.
```

A directory of these files becomes one item bank; files are ordered by filename,
so numeric prefixes (`01-…`, `02-…`) control question order.

## Common metadata

These fields may appear in **any** question's front-matter, regardless of
`kind`:

| Field      | Required? | Default        | Meaning                                                                 |
|------------|-----------|----------------|-------------------------------------------------------------------------|
| `id`       | **Required** | —           | Stable identifier, unique within the bank.                              |
| `kind`     | **Required** | —           | The question type (see the table below).                                |
| `title`    | Optional  | the `id`       | Instructor-facing name, shown in the Canvas bank editor, never to students. |
| `points`   | Optional  | `1`            | Points awarded for a fully correct answer.                              |
| `tags`     | Optional  | `[]`           | mdquiz-side organizational tags. **Not** exported to Canvas.            |
| `feedback` | Optional  | none           | Post-answer messages (`general` / `correct` / `incorrect`). Canvas only. |

Each `kind` then adds its own type-specific fields, documented on its page
below.

### Naming a question with a heading

Instead of `title:`, you can open the prompt with a level-1 heading; mdquiz uses
it as the title and strips it from the prompt so students never see it. An
explicit `title:` field always wins and leaves a body heading as ordinary
prompt text.

```markdown
---
id: tf-stack-fifo
kind: true_false
answer: false
---

# Stack ordering

A stack is a first-in, first-out (FIFO) data structure.
```

### Feedback

`feedback` is a nested block; every message is optional and authored as
Markdown:

```yaml
feedback:
  general: Shown no matter what.
  correct: Shown when the student is right.
  incorrect: Shown when the student is wrong.
```

Feedback is exported to Canvas only — the print sheet carries no answer key, so
it is omitted there. Canvas keeps *answer-level* (`correct`/`incorrect`)
feedback for multiple choice only; other types wire `general` only (see each
type's caveats). Any feedback message may use `{ file: … }` to load its content
from a separate Markdown file — see [partials.md](partials.md).

## Question types

| `kind`             | Type                         | Documentation                              | Samples                                        |
|--------------------|------------------------------|--------------------------------------------|------------------------------------------------|
| `true_false`       | True / false                 | [true-false.md](true-false.md)             | [`../samples/true-false/`](../samples/true-false/)         |
| `multiple_choice`  | Multiple choice (one answer) | [multiple-choice.md](multiple-choice.md)   | [`../samples/multiple-choice/`](../samples/multiple-choice/) |
| `multiple_select`  | Multiple select (choose all) | [multiple-select.md](multiple-select.md)   | [`../samples/multiple-select/`](../samples/multiple-select/) |
| `fill_in_blank`    | Fill in the blank            | [fill-in-the-blank.md](fill-in-the-blank.md) | [`../samples/fill-in-blank/`](../samples/fill-in-blank/)   |
| `matching`         | Matching                     | [matching.md](matching.md)                 | [`../samples/matching/`](../samples/matching/)             |
| `ordering`         | Ordering                     | [ordering.md](ordering.md)                 | [`../samples/ordering/`](../samples/ordering/)             |

See also:

- [images.md](images.md) — adding pictures to prompts, answers, and feedback.
- [math.md](math.md) — LaTeX math (`$…$` / `$$…$$`) via Canvas's native
  equation rendering.
- [diagrams.md](diagrams.md) — ` ```mermaid ` and ` ```dot ` (Graphviz) diagrams
  rendered to bundled images on Canvas export.
- [partials.md](partials.md) — pointing a choice, ordering item, or feedback
  message at a separate Markdown file (`file:`) instead of inline text.
- [exporting.md](exporting.md) — the CLI and the Canvas import workflow.
- [quizzes.md](quizzes.md) — quiz specs: the YAML blueprint for a printable
  exam, sampled per topic folder and produced in several variants.
