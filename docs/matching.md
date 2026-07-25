# Matching (`kind: matching`)

Pair each left-hand prompt with its correct right-hand answer.

## Fields

Plus the [common metadata](README.md#common-metadata):

| Field         | Required? | Default | Meaning                                              |
|---------------|-----------|---------|------------------------------------------------------|
| `pairs`       | **Required** | —    | The left-to-right pairs, in presentation order.      |
| `distractors` | Optional  | `[]`    | Extra right-hand options that match no left prompt.  |

Each entry of `pairs` is:

| Field   | Required? | Default | Meaning                              |
|---------|-----------|---------|--------------------------------------|
| `left`  | **Required** | —    | The left-hand prompt.                |
| `right` | **Required** | —    | The correct right-hand answer.       |

**Rules:** at least two pairs, and each side non-empty.

## Example

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

## Export behavior

- **Print sheet:** the prompts numbered, and all right-hand options lettered —
  sorted, so they don't line up with the prompts — no key.
- **Canvas:** a `matching_question` (verified against Canvas's own export
  structure), scored partial credit per pair. Left/right cells are plain text.

### Canvas caveats

Only `general` feedback is wired (answer-level feedback is multiple-choice
only).

## Samples

[`../samples/matching/`](../samples/matching/)
