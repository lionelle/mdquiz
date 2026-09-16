# Roadmap

## Quiz Types / Markdown

## Canvas Format

## Markdown Format

- [x] Randomly pull questions from various directories to form the main quiz
      (`--sample N`, `--random-order`; see `docs/exporting.md`).

## Printable quizzes (planned)

Turn a tree of question files into a printable exam — sampled per topic folder,
assembled with a header/footer, laid out with answer space, and emitted as
`.docx` (and `.pdf` via a converter), in several **variants** of one blueprint.

This plan was reviewed by an architecture critic and a feasibility pre-mortem.
Findings that survived independent verification are folded in below; claims that
did not survive are recorded under "Corrections" so they are not re-proposed.

### Settled decisions

| Decision | Choice | Rationale |
|---|---|---|
| Print format | Native DOCX written in-crate; PDF via injected converter | Pure bytes, unit-testable like `to_qti`; reuses the existing `zip` dep. |
| Blueprint | YAML spec file, with CLI overrides | Per-folder counts are unwieldy as flags; a spec is reviewable and checked in. |
| Header/footer | Document blocks + repeating page footer | A stapled paper exam needs "Page 2 of 6". |
| Variants | Independent draws, **with a collision check** | See "Variants actually have to differ". |
| Exam type | `quiz::Exam`, **not** `model::Quiz` | `model.rs` reserves `Quiz` for the Canvas New Quizzes concept. |
| Math | **Undecided — spike first.** See "Math". | The original justification did not survive review. |

### Corrections to earlier findings

- **`<m:d>` is *not* broken in LibreOffice.** The earlier "renders as thin
  vertical bars" finding was an artifact of the prototype forcing `m:d` around
  *non-stretchy* parens. A pandoc-built document with genuinely stretchy
  `\binom`, `pmatrix`, `cases` and `\left(1+\frac{1}{n}\right)^n` renders with
  correct tall delimiters. **No workaround is needed**: map non-stretchy `mo` to
  literal character runs and stretchy `mo` to `m:d`, which is simply faithful.
  (pandoc independently does exactly this.)
- **`m:func` is probably right but is not settled.** pandoc uses a plain upright
  run for `\log` and spaces it correctly. Do not treat it as a constraint.
- **The test bed lacks Cambria Math** (`fc-match "Cambria Math"` → XITS Math).
  OMML delimiters, radicals and accents are assembled from the font's OpenType
  MATH table, so *any* rendering conclusion drawn here is font-conditional.
  Part 4 must set `w:rFonts`/`m:mathFont` explicitly and be re-checked in Word.

### Verified by prototype

A hand-built DOCX converted with LibreOffice and round-tripped through pandoc
confirmed: a repeating page footer with `PAGE`/`NUMPAGES` resolving correctly;
per-question answer space via `w:spacing/@w:line`; `w:keepLines`; explicit page
breaks; a clean `soffice --headless --convert-to pdf`; and OMML math rendering
with italic variables, upright function names, super/subscripts, summations and
fractions.

### Math — open, and more expensive than first estimated

The original plan claimed LaTeX→MathML could be delegated to `pulldown-latex`
while an in-crate MathML→OMML mapping stayed "bounded and frozen at ~25
elements, a weekend of work". Three verified findings kill that framing:

1. **It panics.** `\char é` panics inside `pulldown-latex 0.8.0`
   (`parser/lex.rs:331`, char-boundary slice). A fuzz run hit ~0.26% of inputs.
   CLAUDE.md denies uncaught panics, and this is reachable from an authored
   `.md`. The crate's 0.8.0 changelog shows this is a recurring bug class.
2. **It emits malformed XML.** `\Big( x \bigg] y` produces
   `stretchy="true"minsize="1.8em"` — no separating space, so `xmllint` rejects
   it. Any mapper that parses the MathML *string* dies on ordinary input.
3. **The vocabulary is not spec MathML.** Array/matrix semantics arrive as
   private CSS classes (`class="menv-arraylike"`, `menv-cells-left menv-cases`,
   `menv-hline`), not `columnalign`/`columnlines`. That is an undocumented
   convention of a pre-1.0 crate, free to change in a patch release — so the
   "bounded and frozen" argument does not apply to this library.

Additional constraints: `push_mathml` returns `Ok(())` for unsupported commands
and leaks their arguments into the output as stray content, so a "warn and fall
back" policy has no signal to trigger on. And some constructs have **no OMML
representation at all** — pandoc's mature `texmath` drops `|` column rules and
`\hline`, and gives up entirely on `\cancel`/`\phantom`/`\textcolor`. A silently
wrong equation on a printed exam is the worst failure mode this tool has.

**Therefore Part 5 is a spike, not an implementation**, with three deliverables:

- A **bake-off** between `pulldown-latex` (consumed as an `Event` stream, not as
  a MathML string) and `math-core` 0.8.2 — a maintained fork with snapshot and
  fuzz tests — measured against a fixed corpus plus the `\char é` case. Also
  price the hybrid: own the DOCX writer, shell out to pandoc for math only.
- A written **support matrix**: constructs that map faithfully / map lossily /
  cannot map.
- A **hard-error policy**: anything outside the faithful set fails the export
  with the offending LaTeX quoted (`Error::UnsupportedMath`). No best-effort
  rendering onto paper.

Whichever library wins is pinned (`=x.y.z`), wrapped so a panic becomes an
`Error`, and covered by snapshot tests so a version bump shows as a diff.

Still correct and not up for relitigation: the low-download direct LaTeX→OMML
crates (`tex2word-math` 134 downloads, `easydoc-math` 97, `ooxml-omml` a
290-download alpha) fail the curated-dependency bar.

### Variants actually have to differ

Independent draws can silently produce identical variants. Exact collision
probability for `variants: 5`:

| pool | take | combinations | P(two variants identical) |
|---|---|---|---|
| 4 | 3 | 4 | **100%** |
| 5 | 3 | 10 | 69.8% |
| 6 | 3 | 20 | 41.9% |
| 10 | 5 | 252 | 3.9% |

Topic folders in `samples/` hold 1–6 questions, so this is the normal case, not
an edge case. Two consequences:

- Spec validation **errors** when `C(pool, take) < variants` and **warns** when
  the collision probability is high, naming the group.
- Draw-level variation is not enough on its own: add **per-variant choice
  shuffling** (a `Vec<Choice>` permutation). Without it, "answer A" is the same
  option on every paper. Any `take: all` group is identical across variants by
  definition and the docs must say so.

**Invariant:** all randomization happens during assembly and is frozen into the
`Exam` before any writer sees it; writers are pure functions of the `Exam`. This
is what keeps the answer key in sync with a shuffled sheet.

### Spec shape

```yaml
name: "CS 3500 — Exam 1"
variants: 5
seed: 20260915              # omit for a fresh draw each run
header: templates/header.md
footer: templates/footer.md
layout:
  answer_space: 3           # blank lines after each question
  page_break_between: false
  page_footer: "${name} (${variant}) — Page ${page} of ${pages}"
groups:
  - dir: topics/trees
    take: 3
  - dir: topics/graphs
    take: 5
    answer_space: 6         # per-group override
    shuffle_choices: true
  - dir: topics/sorting
    take: all
```

Semantics that must be pinned down in Part 2, because each changes the code:

- `dir` **recurses** into subdirectories.
- **Overlapping groups** (`topics` and `topics/graphs`) are a validation error;
  assemble routes through the same id-uniqueness check as
  `parse::item_bank_from_questions`.
- `take: N` **errors** when the pool is smaller than `N`; `take: all` never does.
- `dir` is validated with the library's path-escape rule (`escapes_dir` moves
  from `cli.rs` into the library, where CLAUDE.md says policy belongs).
- `take` is **not** a naive untagged string-or-int enum, which would read
  `take: none` as `all`; the keyword arm is its own `#[serde(rename_all)]` enum
  so typos are rejected.
- Layout templating uses `${…}`, **not** `{{…}}`, which is already the
  fill-in-the-blank blank marker (`model::blank_marker`).
- `${page}`/`${pages}` compile to OOXML `PAGE`/`NUMPAGES` *fields*, while
  `${name}`/`${variant}` are literal text substitution. Unknown keys are
  rejected.
- Per-variant seeds are **derived** from the root seed, so adding a sixth
  variant leaves the first five unchanged and a single variant can be
  regenerated.

### Parts

Each part is one logical change ending in a green `/check-rs`, per CLAUDE.md.
Each part ships its own `docs/` page in the same commit, matching the repo's
existing pattern.

0. **Foundations.** Retarget the cross-cutting passes at the unit they actually
   walk: `render_diagrams(items: &mut [Question], …)` and
   `local_image_paths(items: &[Question])`; `ItemBank` callers pass
   `&mut bank.items`. Decide the `export::Format` shape now, before it dictates
   signatures — preferred: one document type reaches all four exporters
   (`Exam::from_bank(ItemBank)`), so `Format` stays flat with no invalid
   combinations. Move `escapes_dir` into the library. Add `Error::Spec { group,
   message }`. Behaviour-free, mechanical, reviewable in one commit.
1. **`src/quiz/sample.rs`.** Move `SampleRng`, `sample_per_directory`,
   `sample_group`, `shuffle` out of `cli.rs`. **Sort sources before sampling** —
   today `collect_sources` uses unordered `fs::read_dir` and the only sort
   happens afterwards in `build_bank`, so the same seed does *not* reproduce a
   draw. Library exposes `seeded(u64)` only; the CLI supplies entropy, keeping
   the library a pure function of (sources, seed). Fix `index`'s modulo bias and
   its 32-bit `unwrap_or(0)` collapse. Test: shuffle the input, assert identical
   survivors.
2. **`src/quiz/spec.rs`.** The blueprint, plus every validation listed above:
   collision check, `take` underflow, overlap, path escape, unknown template
   keys.
3. **`src/quiz/assemble.rs`.** Spec + sources → `Vec<Exam>`, one per variant,
   via an injected **source lister** (`dir → Vec<(path, content)>`) alongside the
   existing `PartialReader`, so it is testable without a filesystem. Resolves
   answer-space precedence *here* into `ExamItem { question, answer_space }`, so
   writers read a `usize` and never re-derive precedence. Performs all
   randomization, including choice shuffling. Hoists the diagram pass above the
   variant loop with a shared cache so `mmdc`/`dot` run once, not once per
   variant.
4. **DOCX container.** *Done.* `src/export/docx.rs` writes all eight parts
   (`[Content_Types].xml`, both `.rels`, `document.xml`, `styles.xml`,
   `numbering.xml`, `settings.xml` with the math font, `footer1.xml`), the page
   geometry, and the `PAGE`/`NUMPAGES` footer fields. `tests/docx_package.rs`
   checks every part with `xmllint` and converts through LibreOffice.
   **Still open: open a generated `.docx` in real Microsoft Word.** Everything
   so far is verified against LibreOffice and pandoc only, and `Cambria Math`
   is not installed on the dev box — so how equations and stretchy delimiters
   actually draw is unverified.

   **Two follow-ups recorded rather than done.** (a) Nothing in CI runs
   `cargo test -- --ignored`, so the LibreOffice "does it actually open" check
   never runs automatically; the unit tests now cover the three faults it used
   to be the sole killer of, but a nightly or `workflow_dispatch` job would
   restore the end-to-end signal. (b) No test produces a multi-page document,
   so `NUMPAGES` is never anything but 1 — only a `page_break_between: true`
   exam through the LibreOffice path would prove the field really counts.

   **Three copies of the sources→questions loop.** `parse::item_bank_from_sources_with`,
   `cli::build_bank`, and `assemble::load_pools` each sort by path, resolve a
   per-question base, parse with a scoped reader and name the failing file. The
   public helper cannot do the per-question base, which is why the other two
   exist. Fix: one `parse::questions_from_sources(sources, &read)` both callers
   build on. Deferred out of the cleanup pass deliberately — it rewires the
   CLI's `partial_reader`, which is security-relevant (`escapes_dir` would see
   `sub/../x.md` rather than `../x.md`; still rejected, since `Path::components`
   does not resolve `..`, but it changes an error message and needs
   `partial_reader_reads_and_refuses_escape` rewritten).

   **Path policy when the CLI is wired.** `assemble::parse_one` joins a
   question's folder onto the authored partial path and hands the result to the
   injected reader without checking it — correct layering, since the library
   stays pure and the reader owns policy. The `quiz` subcommand's reader must
   therefore run `escapes_dir` on the **joined** path, or a `file:` partial
   escapes the question tree.

   **Placeholder scanners.** `model::blank_markers` (`{{…}}`),
   `quiz::spec::check_template` and `export::docx::footer_runs` (both `${…}`)
   are the same scan written three times, and they handle an unterminated
   opener three different ways — one of which was a real bug. The key *set* is
   now shared (`quiz::spec::TemplateKey`), which closes the drift that mattered;
   collapsing the three scanners into one `src/template.rs` is still worth doing
   before a fourth appears.

   **Image dimensions — decided.** `wp:extent` needs EMU and the crate has no
   image dependency. When Part 11 lands, parse the PNG `IHDR` in-crate rather
   than adding one: the diagram path already forces PNG, `IHDR` is a fixed
   24-byte header, and a curated dependency is hard to justify for one struct
   read. SVG stays out of scope, so no second decoder is implied.
5. **Math spike** (see above): bake-off, support matrix, hard-error policy.
6. **Inline runs** — bold, italic, code, strikethrough.
7. **Lists** — including `numbering.xml` abstract/concrete definitions.
8. **Preformatted blocks** — code blocks *and* tables, both emitted as literal
   text in a monospace style. See "Tables in v1" below; folding them together is
   what removes a whole part from this plan.
9. **The six question kinds** + the answer-key document. This is where the
   matching/ordering freeze gap closes: their presented order is currently
   derived inside `export/markdown.rs` at render time rather than stored, so
   `shuffle_choices` cannot vary them between variants and a key that printed
   labels rather than text would have to re-derive that sort. Put the derived
   order on `ExamItem` and have the writers read it.
10. **Images and diagrams.** PNG only; declare SVG-in-DOCX out of scope
    (it needs `asvg:svgBlip` plus a raster fallback) and force PNG diagrams.
11. **`mdquiz quiz` subcommand.** Library returns
    `Vec<OutputFile>` + warnings; `cli.rs` only writes bytes — it is already
    1122 lines. Variant naming (A/B/C, reusing `choice_label`'s A..Z-then-number
    rule; `key_path` only appends `-key` today and nothing produces the `-A`).
    Manifest records per-variant question ids, the shuffled choice order, and
    the derived seed.

### Tables in v1

A Markdown pipe table is emitted as **literal text**, not as `w:tbl`. Real table
rendering is deferred (see "Deferred").

This matches what the print sheet already does — `export/markdown.rs` never
parses Markdown, it interpolates the authored string verbatim, and its own test
asserts a pipe table stays literal. It does *not* match Canvas, which renders
real bordered HTML tables; that divergence is accepted for v1 and must be
documented in `docs/`.

Two requirements make the difference between a usable stopgap and an unreadable
one, and both are the same requirements code blocks have — which is why the two
share one part:

- **Monospace.** A pipe table in a proportional font loses column alignment
  entirely. In a monospace style the columns still line up on paper.
- **Preserved line breaks.** A DOCX paragraph collapses newlines, so a
  multi-row table would otherwise render as one run-on line. Each row needs an
  explicit `<w:br/>` (or its own paragraph with zero spacing).

No warning is emitted. This is deliberate and is *not* inconsistent with the
hard-error policy for math: a table rendered as text is visibly text, and the
instructor sees exactly what the student sees. Wrong math, by contrast, looks
right. The failure modes are not comparable.

### Converter contract

`soffice` returns **exit 0 while producing no file** when its profile is locked
by another instance — and a 5-variant run invokes it 10 times. The injected
converter must therefore: assert the output exists and is non-empty rather than
trusting the exit status; run with a private profile
(`-env:UserInstallation=…`, using the existing `tempfile` dep); and never run
conversions concurrently.

Absence of a converter is **not** the `diagram.rs` contract — there, the
requested artifact still exists with a code block in it. Here, `--format pdf`
with no converter would leave nothing at the requested path. Decide explicitly:
write the `.docx` beside the requested path and warn loudly with an install hint
(mirroring `DOT_MISSING`), or fail hard.

### Testing

Substring assertions on XML — the existing style in `canvas.rs` — are tolerable
for QTI but produce green tests against a DOCX that will not open. Word rejects
a dangling `r:embed`, a missing content-type override, out-of-order `w:pPr`
children, or a `numId` with no matching `w:num`; none of that is visible to
`contains()`. Land this in Part 4, before any content:

1. A `tests/` directory (there is none today): generate → open as zip →
   `xmllint --noout` every part.
2. A relationship-integrity test: every referenced `r:id` exists in the matching
   `.rels`; every `.rels` target exists as a zip entry; every part has a
   content-type entry.
3. Snapshot tests over OMML fragments, so a math-library bump is a reviewable
   diff rather than silent drift.
4. An `#[ignore]`d, tool-gated test that converts via `soffice` and asserts
   `pdftotext` output contains expected strings. CI, not every `cargo test`.

### Deferred

- **Tag filtering.** `Question::tags` is documented for exactly this; a
  `tags: [trees]` group filter is the natural next knob.
- **Markdown header/footer.** Only the DOCX writer consumes header/footer;
  extending the Markdown sheet is a small follow-on.
- **Real DOCX tables.** `w:tbl`/`w:tblGrid`/`w:tblPr`/`w:tblBorders`, to reach
  parity with the Canvas exporter's bordered tables. v1 emits table text
  instead; see "Tables in v1".
- **Per-question points on the sheet.** `Question::points` exists but no
  exporter prints it. A paper exam wants per-question points and a per-variant
  total.
