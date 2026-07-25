# Images

Any authored text that Canvas renders as rich HTML can hold an image, using the
standard Markdown `![alt](src)` syntax. There is no image-specific `kind`.

Images are supported in:

- **Prompts** — every question type.
- **Answers** — multiple-choice / multiple-select `choices` and `ordering`
  `items` (these export as rich HTML).
- **Feedback** — `general` / `correct` / `incorrect` messages.

They are **not** supported in **matching** cells or **fill-in-the-blank**
answers: those export as plain text, so an image there would show as literal
`![alt](src)` and its file would not bundle.

```markdown
The tree below is balanced ![tree](binary-tree.png), unlike ![this one](https://ex.com/skew.png).
```

```yaml
# An image inside a multiple-choice answer:
choices:
  - text: "![balanced tree](binary-tree.png)"
    correct: true
  - text: Neither
```

## External URLs

`![](https://…/x.png)` passes through unchanged and renders in both the print
sheet and the Canvas export. Nothing is bundled.

## Local files

`![](diagram.png)` is resolved **relative to the question directory** and, for
the Canvas export, **bundled into the package** under `web_resources/` and
rewritten to Canvas's `$IMS-CC-FILEBASE$` reference so it resolves on import. A
referenced file that is missing is skipped with a warning.

Local-image bundling follows Canvas's Common Cartridge format. If an item-bank
import doesn't show a bundled image, host it somewhere and use a URL instead.

## Samples

[`../samples/images/`](../samples/images/) has a runnable example:
`binary-tree.png` is bundled locally, and a Rust-logo URL is passed through.
