# Changelog

Notable changes to mdquiz. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) — while the version
is `0.x`, a minor bump may break the library API.

## [0.2.0] — 2026-09-21

The first release published to crates.io. `0.1.0` existed only in the
repository, so if you have been building from source, everything below is what
changed; if you are arriving from crates.io, this is simply what mdquiz does.

Canvas item banks remain the point of the tool. What this release adds is that
the same questions now **print**, so a course running online quizzes does not
have to maintain a second copy of its questions for the paper exam.

### Added

- **`mdquiz quiz <spec>`** — build a printable exam from a YAML blueprint:
  which topic folders to draw from, how many questions from each, how many
  variants. Writes one Word `.docx` sheet **and its own answer key** per
  variant, plus a manifest recording the seed and the options each paper
  actually printed. See [`docs/quizzes.md`](docs/quizzes.md) and the worked
  [`samples/exam.yaml`](samples/exam.yaml).
- **Word (`.docx`) export.** Math becomes a real Word equation — selectable,
  searchable, editable — not a picture. Lists, headings, PNG images, pipe
  tables and code blocks are laid out rather than printed as source. Page
  furniture supports `${name}`, `${variant}`, `${page}` and `${pages}`.
- **`--seed <N>` on `export`**, so `--sample` and `--random-order` reproduce
  exactly. A seeded run now selects the same questions on any machine.
- **Per-question points and a total** on both print sheets, from the existing
  `points:` field. `layout.show_points: false` prints the total only, for a
  quiz where every question is worth the same.

### Changed

- **Matching prints as two columns** on both print sheets, so a student can
  draw between them, with a write-in rule beside each prompt for courses that
  grade written letters instead.
- **Answer marks are bigger and say how many answers to give**: round `(    )`
  where exactly one option is right, square `[    ]` where several may be.
  Multiple choice previously printed no mark at all. Write-in rules are longer.
  All are set in a monospace face so they are the same size in any font.
- **Code blocks on the Word sheet** print on a shaded panel in a monospace
  face, without their ``` ``` ``` fences.
- The minimum supported Rust version is now declared: **1.96**.
- Canvas QTI output is unchanged. Verified by diffing this release against
  `0.1.0` over the whole `samples/` tree: the packages are byte-identical.

### Fixed

- **A matching question could print its own answer key.** Options were sorted,
  which on a bank whose answers happened to sort that way lined every option up
  with its prompt — `samples/matching/02-c-types.md` produced a perfect
  diagonal. Both print paths now rotate the presented order off the authored
  one.
- **`--sample` depended on filesystem enumeration order.** Sampling ran before
  the sources were sorted, so the same seed could draw different questions on
  a different machine or filesystem. Sources are now sorted first.
- Random index selection was debiased (modulo → rejection sampling). This
  changes which questions a given seed draws; `0.1.0` had no `--seed` on
  `export`, so no reproducible draw is affected.

### Known limitations

- **Block quotes print as their Markdown source** on the Word sheet, `>` and
  all, as do links, horizontal rules and raw HTML.
- Images are **PNG only**. SVG in a `.docx` needs `asvg:svgBlip` plus a
  rasterised fallback, so it is strictly more work than PNG and never less;
  diagrams are rendered to PNG for this path. The Canvas export still offers
  `--diagram-format svg`.
- Matching and table columns use fixed proportions rather than measuring their
  content.
- A code block or image inside a list item falls the whole list back to source.

## [0.1.0]

Canvas *New Quizzes* item banks and print-ready Markdown sheets, from a
directory of Markdown + YAML question files. Six question types — true/false,
multiple choice, multiple select, fill in the blank (literal and regex),
matching, ordering — with images, LaTeX math, Mermaid and Graphviz diagrams,
and `file:` includes.

Never published to crates.io.

[0.2.0]: https://github.com/lionelle/mdquiz/releases/tag/v0.2.0
[0.1.0]: https://github.com/lionelle/mdquiz/tree/v0.1.0
