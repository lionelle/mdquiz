# Quiz specs (blueprints)

A **quiz spec** is a YAML file describing a printable exam: which topic folders
to draw from, how many questions to take from each, and how the sheet is laid
out. It is the reproducible counterpart to a long command line — the same exam
can be rebuilt next term by re-running the file that made it.

> **Status:** the spec format and its validation ship now. Assembling and
> printing from a spec arrive with the `mdquiz quiz` command; see
> [`../Roadmap.md`](../Roadmap.md).

## A complete spec

```yaml
name: "CS 3500 — Exam 1"     # required
variants: 5                  # how many different sheets to produce (default 1)
seed: 20260915               # omit for a fresh draw each run
header: templates/header.md  # path to markdown rendered above the questions
footer: templates/footer.md  # path to markdown rendered below the questions

layout:
  answer_space: 3            # blank lines left after each question (default 3)
  page_break_between: false  # start every question on a fresh page
  shuffle_choices: false     # permute answer choices per variant
  page_footer: "${name} (${variant}) — Page ${page} of ${pages}"

groups:
  - dir: topics/trees
    take: 3
  - dir: topics/graphs
    take: 5
    answer_space: 6          # overrides layout.answer_space for this group
    shuffle_choices: true    # overrides layout.shuffle_choices
  - dir: topics/sorting
    take: all                # every question in the folder
```

Only `name` and `groups` are required. A group needs only `dir`; without a
`take:` it uses every question in the folder.

## Groups

Each group names one folder, searched **recursively**, and how much of it to use.
Groups appear on the sheet in the order they are written.

`take:` is either a number or the keyword `all`. Nothing else is accepted — a
misspelling like `take: al` is an error rather than being quietly read as "use
every question".

**Overlapping groups are rejected.** Because folders are searched recursively,
listing both `topics` and `topics/trees` would draw the same question twice and
collide on its `id`. Sibling folders that merely share a prefix (`topics/tree`
and `topics/trees`) are fine.

**A folder must be able to supply its `take:`.** If `take: 5` meets a folder
holding three questions, the build fails naming both numbers. A sheet quietly
missing two questions is worse than a build that stops. An empty folder is an
error too, whatever the `take:`, and `take: 0` is rejected outright.

**`dir` cannot escape the spec's own folder** — no `..`, absolute path, or drive
prefix. This is the same rule `file:` partials and images follow.

## Variants

`variants: N` produces N different sheets from one blueprint, so neighbouring
students get different questions. Each variant draws independently.

Independent draws can coincide, and with small folders they usually do. mdquiz
checks this for you against the real question counts:

- **Impossible** — fewer possible question sets than variants requested, and no
  group set to shuffle its choices. A folder of four questions with `take: 3`
  yields only four possible sheets, so five variants must repeat one. This is an
  **error**, and it names `shuffle_choices` as a way out.

  `shuffle_choices` is taken at its word here: a spec describes folders, not
  questions, so mdquiz cannot yet tell whether those questions *have* choices to
  shuffle. A folder of fill-in-the-blank items set to shuffle passes this check
  and still prints identical sheets.
- **Likely** — possible, but two sheets are apt to match. Six questions with
  `take: 3` gives twenty possible sheets, and five draws collide about 42% of the
  time. This is a **warning**; above a 5% chance mdquiz says so and continues.

Possible exams multiply across groups, so several shallow groups still support
many variants: three groups of three questions taking one each gives 27 sheets,
not three.

Two things worth knowing:

- **A `take: all` group draws the same questions on every variant** by
  definition. That is often what you want for a shared section; turn on
  `shuffle_choices` and the sheets still differ in the order of their options.
  A spec whose questions cannot vary but whose choices can is accepted, with a
  warning saying so.
- **Drawing different questions is not the only difference that matters.** With
  `shuffle_choices`, answer choices are permuted per variant too, so "answer B"
  is not the same option on every sheet.

## Layout

| Key | Default | Meaning |
|---|---|---|
| `answer_space` | `3` | Blank lines left after each question. |
| `page_break_between` | `false` | Start each question on a fresh page. |
| `shuffle_choices` | `false` | Permute answer choices per variant. |
| `page_footer` | none | Repeating footer on every printed page. |

`answer_space` and `shuffle_choices` can be overridden per group.

`header` and `footer` are **paths** to Markdown files, resolved relative to the
spec's own folder. Like `dir`, and like `file:` partials, they may not escape it
with `..` or an absolute path.

### Placeholders

`page_footer` may use `${...}` placeholders. Only these are accepted, and an
unknown one is an error rather than being printed literally on every page:

| Placeholder | Becomes |
|---|---|
| `${name}` | The exam's `name`. |
| `${variant}` | The variant label (`A`, `B`, …). |
| `${page}` | The current page number. |
| `${pages}` | The total page count. |

`${page}` and `${pages}` become real page-number fields in the output, so they
resolve per page; they are only meaningful in `page_footer`.

The delimiter is `${...}`, **not** `{{...}}`, because `{{name}}` is already the
[fill-in-the-blank](fill-in-the-blank.md) blank marker — a header that later
became a question partial would otherwise sprout blanks where it meant to name
the exam.

## What your Markdown looks like on the printed sheet

The spec's `header:` and `footer:` files and every question prompt are
Markdown, and the Word document renders them as formatting rather than
printing the markers. (`page_footer` is the exception: it is plain text plus
`${...}` placeholders, not Markdown.)

**Rendered as formatting:**

| You write | You get |
|---|---|
| `**bold**` | **bold** |
| `*italic*` | *italic* |
| `~~struck~~` | struck-through text |
| `` `code` `` | a monospace span |
| `# Heading` … `### Heading` | Word's built-in Heading 1–3 styles |
| `- item`, `1. item` | a real Word list, nesting and all |
| `$x^2$`, `$$…$$` | a real Word equation |

Headings deeper than `###` are set as Heading 3. Because the built-in styles
are used, headings also appear in Word's navigation pane and in a generated
table of contents.

Math becomes a genuine Word equation, not a picture: it is selectable,
searchable, and editable in Word. Display math (`$$…$$`) written on a line of
its own is centred on that line. Written mid-sentence, or as the whole of a
question's first line, it stays inline — centring it there would drag the
sentence, or the question's number, into the middle of the page.

Lists become Word lists: bulleted or numbered, nested to any depth, with the
formatting and math inside each item rendered. Each numbered list restarts at
1 — or at whatever number you wrote — rather than continuing the one before
it. An item may hold more than one paragraph; only the first carries the
number.

**Printed as you typed it** — tables, block quotes, links, images, fenced and
indented code blocks, horizontal rules and raw HTML are not laid out yet, so
the block appears as its own Markdown source, indentation and line breaks
intact:

```markdown
| Algorithm | Cost |
|---|---|
| merge sort | n log n |
```

prints as three lines still carrying their pipes. This is deliberate: a table
that is *visibly* unformatted is something you can see and work around, where
one silently flattened into a run-on line is not. The same blocks render
properly on the Canvas export; the print sheet catches up over the releases
tracked in [`../Roadmap.md`](../Roadmap.md).

Note that inline formatting inside such a block is printed too — a
`~~struck~~` table cell shows its tildes — because the whole block falls back
together. A list holding a link is the same: one item the writer cannot lay
out sends the whole list back to source, rather than formatting half of it.

**Math is the exception, and it is never printed as source.** The export
**fails**, naming the expression, if:

- the formula uses something the Word writer cannot render (see
  [math.md](math.md)); or
- the formula sits inside one of the blocks above — a table, quote, link or
  image — which would otherwise print it as raw LaTeX.

So `| $x^2$ |` in a table is an error today, not a sheet with `$x^2$` printed
on it. (Math in a *list* is fine: lists are laid out.) The reason math alone
is treated this way: an unformatted table is visibly unformatted, but a
student who meets `$\frac{a}{b}$` on a printed page sees something that is
not a formula, and nobody finds out until the copies are run off. Move the
math out of the table until tables land.

## Unknown keys are errors

A spec is validated strictly: a misspelled key is rejected rather than ignored.
A blueprint that silently dropped `variants: 5` would print one sheet and say
nothing about it.
