# File includes (partials)

Authoring rich Markdown or HTML inside a YAML string is awkward. Instead, a field
can point to a **separate Markdown file** — a *partial* — whose rendered content
is used in its place.

## Where `file:` is allowed

Replace the inline value with `{ file: path.md }` in any of:

- **Choices** (multiple choice / multiple select) — `file:` instead of `text:`.
- **Ordering items** — a `{ file: … }` entry instead of a bare string.
- **Feedback** messages — `general` / `correct` / `incorrect`.

```yaml
kind: multiple_choice
choices:
  - file: parts/balanced.md      # this choice = the rendered partial
    correct: true
  - text: Neither tree is balanced
feedback:
  general:
    file: parts/hint.md

kind: ordering
items:
  - Inline step
  - file: steps/second.md

feedback:
  correct: Nice work                # inline still works everywhere
  incorrect:
    file: fb/try-again.md
```

A choice must have **exactly one** of `text:` or `file:` (giving both, or
neither, is an error). Paths are resolved **relative to the bank directory** (the
folder you export).

## Images inside a partial

A partial is ordinary Markdown, so it can contain images. Their paths are written
**relative to the partial's own folder**, and mdquiz automatically **rebases**
them to the bank directory so they bundle into the Canvas package exactly like a
prompt image (see [images.md](images.md)):

```
samples/partials/
├── 01-tree-balance.md          # uses `file: parts/balanced.md`
└── parts/
    ├── balanced.md             # contains ![a balanced tree](binary-tree.png)
    └── binary-tree.png         # sits next to the partial
```

On export, `binary-tree.png` (referenced from `parts/balanced.md`) is rebased to
`parts/binary-tree.png`, bundled under `web_resources/parts/`, and rewritten to
Canvas's `$IMS-CC-FILEBASE$` reference. See
[`../samples/partials/`](../samples/partials/) for this exact runnable example.

## Limits (by design)

- **No escaping the bank directory.** A `file:` (or a partial's image) path must
  be relative and stay inside the directory: a `..` component, an absolute path
  (`/etc/passwd`) or a Windows drive prefix is refused. Keep a partial's images
  under the partial's own folder — a partial in `parts/` that points at
  `../shared/x.png` would rebase to a `..` path and be skipped.
- **Inline image syntax only.** Rebasing rewrites inline `![alt](path)` images.
  Reference-style images (`![alt][ref]` with a separate `[ref]: path` definition)
  are **not** rebased — use the inline form inside partials.
- **No nesting.** A partial is plain Markdown with no YAML front-matter, so it has
  no `file:` of its own to expand; includes are one level deep by construction.
- **Print export.** Partials resolve for the Markdown sheet too, but its images
  are only bundled for Canvas (the print sheet just references them).
