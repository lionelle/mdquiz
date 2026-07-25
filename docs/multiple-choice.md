# Multiple choice (`kind: multiple_choice`)

A single-answer question: the student picks exactly one option.

## Fields

Plus the [common metadata](README.md#common-metadata):

| Field     | Required? | Default | Meaning                                        |
|-----------|-----------|---------|------------------------------------------------|
| `choices` | **Required** | —    | The options, in presentation order (order is preserved). |

Each entry of `choices` is:

| Field     | Required? | Default | Meaning                                  |
|-----------|-----------|---------|------------------------------------------|
| `text`    | **Required** | —    | The option text, as Markdown.            |
| `correct` | Optional  | `false` | Whether this option is the correct one.  |

**Rules (checked at parse time):** at least two choices, and **exactly one**
marked `correct: true`.

A choice may use `file:` instead of `text:` to pull its content from a separate
Markdown file — see [partials.md](partials.md).

## Example

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

## Export behavior

- **Print sheet:** the prompt then lettered options (A, B, C…), no key.
- **Canvas:** a `multiple_choice_question`.

### Canvas caveats

Unlike true/false, New Quizzes **does** keep answer-level feedback for multiple
choice, so `correct:`/`incorrect:` messages survive the item-bank import.

## Samples

[`../samples/multiple-choice/`](../samples/multiple-choice/)
