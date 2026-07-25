# True / false (`kind: true_false`)

A statement the student marks true or false.

## Fields

Plus the [common metadata](README.md#common-metadata):

| Field    | Required? | Default | Meaning                        |
|----------|-----------|---------|--------------------------------|
| `answer` | **Required** | —    | The correct answer (`true` or `false`). |

## Example

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

## Export behavior

- **Print sheet:** the prompt followed by blank True / False checkboxes.
- **Canvas:** a `true_false_question`.

### Canvas caveats

New Quizzes keeps *answer-level* feedback for multiple choice only. For
true/false, `correct:`/`incorrect:` are dropped on item-bank import (a Canvas
limitation); only `general:` may survive.

## Samples

[`../samples/true-false/`](../samples/true-false/)
