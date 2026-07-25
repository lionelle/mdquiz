# Multiple select (`kind: multiple_select`)

"Choose all that apply" — the same `choices` shape as
[multiple choice](multiple-choice.md), but **one or more** options may be
correct.

## Fields

Plus the [common metadata](README.md#common-metadata):

| Field     | Required? | Default          | Meaning                                     |
|-----------|-----------|------------------|---------------------------------------------|
| `choices` | **Required** | —             | The options, in presentation order.         |
| `scoring` | Optional  | `all_or_nothing` | How the selection is scored (see below).    |

Each entry of `choices` is `text` (**required**) and `correct` (optional,
defaults `false`) — identical to multiple choice.

**Rules:** at least two choices, and **at least one** marked `correct: true`.

## Scoring

| `scoring`         | Meaning                                                                                          |
|-------------------|--------------------------------------------------------------------------------------------------|
| `all_or_nothing`  | Default. Full marks only when every correct option is selected and no incorrect one is.          |
| `partial`         | Each correct selection adds an even share of the marks; each incorrect one subtracts a share (clamped to zero). |

```yaml
kind: multiple_select
scoring: partial   # or all_or_nothing (the default)
choices: [...]
```

## Example

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

## Export behavior

- **Print sheet:** the prompt then lettered options with checkboxes, no key.
- **Canvas:** a `multiple_answers_question`. All-or-nothing matches Canvas's own
  export exactly; partial credit uses the standard per-choice `Add`/`Subtract`
  QTI encoding.

## Samples

[`../samples/multiple-select/`](../samples/multiple-select/)
