# Images in prompts

Prompts are full Markdown, so images use the standard `![alt](src)` syntax.
There is no image-specific `kind`; any question type can include one.

```markdown
The tree below is balanced ![tree](binary-tree.png), unlike ![this one](https://ex.com/skew.png).
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
