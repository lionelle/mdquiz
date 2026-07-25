# Ordering (`kind: ordering`)

Arrange a set of items into a correct sequence.

## Fields

Plus the [common metadata](README.md#common-metadata):

| Field   | Required? | Default | Meaning                                       |
|---------|-----------|---------|-----------------------------------------------|
| `items` | **Required** | —    | The items, authored in their **correct** order. |

List `items` in the order you want them graded. mdquiz shuffles them for
display (sorted by text), so the presented sequence is never the answer.

**Rules:** at least two items, and each non-empty.

## Example

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

## Export behavior

- **Print sheet:** the items listed sorted (never in the correct order), each
  with a write-in blank for its position, no key.
- **Canvas:** an `ordering_question` (verified against Canvas's own export
  structure), scored **all-or-nothing** — full marks only when the entire
  sequence is correct.

### Canvas caveats

Only `general` feedback is wired (answer-level feedback is multiple-choice
only).

## Samples

[`../samples/ordering/`](../samples/ordering/)
