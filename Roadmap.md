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

### Math — settled (Part 5 spike)

Math is a first-class part of many questions, so the print path must render it
properly rather than pass `$…$` through as source.

**Decision: `math-core` 0.8.2 for LaTeX→MathML, with an in-crate MathML→OMML
mapping.** This reverses the earlier plan to use `pulldown-latex`, which was
chosen before it was tested. Measured side by side:

| | `pulldown-latex` 0.8.0 | `math-core` 0.8.2 |
|---|---|---|
| `\char é` | **panics** (`lex.rs`, char-boundary slice) | `Err: Unknown command` |
| `\Big( x` | **malformed XML** (`stretchy="true"minsize=`) | well-formed |
| arrays / `cases` | **private CSS classes** (`menv-arraylike`, `menv-hline`) | standard inline `style=` |
| 20 000 adversarial inputs | **185 panics, 1 906 malformed** | **0 panics, 0 malformed** |
| transitive crates | 2 | 24 (many build-time only) |

The panic alone is disqualifying: CLAUDE.md forbids uncaught panics and the
input is an authored `.md` file. The private CSS classes were the deeper
problem — they made the "bounded, spec-defined vocabulary" argument for owning
the OMML mapping false, because the array semantics lived in an undocumented
convention of a pre-1.0 crate. `math-core` emits spec MathML, so that argument
holds again.

`math-core` also rejects far more than it renders (2 029 of 20 000 adversarial
inputs, against `pulldown-latex`'s 19 815). For a printed exam that is the right
default: it *is* the hard-error policy, arriving for free.

Cost accepted: 24 crates in the build graph against `pulldown-latex`'s 2. In
context that is proportionate — the crate already pulls 55, of which `clap`
alone is 17 — and most of `math-core`'s are compile-time proc-macros.

**Support matrix.** A 41-case corpus of realistic instructor math produced 20
distinct MathML elements and 13 attributes. That is the whole mapping surface.

*Maps faithfully:*

| MathML | OMML |
|---|---|
| `mi` `mn` `mo` `mtext` | `m:r` + `m:t`, `m:sty` for italic/upright |
| `mrow` | `m:e` |
| `mfrac` | `m:f`; `linethickness="0"` → `m:type val="noBar"` (that is `\binom`) |
| `msup` `msub` `msubsup` | `m:sSup` `m:sSub` `m:sSubSup` |
| `msqrt` `mroot` | `m:rad` |
| `munderover` on ∑ ∏ ∫ | `m:nary` |
| `mover` `munder` with `accent` | `m:acc` |
| `munder` `mover` otherwise | `m:limLow` `m:limUpp` |
| `mphantom` | `m:phant` |
| `mspace` | spacing run |
| `mtable` `mtr` `mtd` | `m:m` `m:mr` `m:e`, alignment via `m:mcJc` |
| stretchy `mo` pair | `m:d`; non-stretchy → literal character runs |

Blackboard and script letters need no special handling: `math-core` emits the
Unicode codepoint (ℝ, 𝒫) rather than a `mathvariant` to interpret.

*Maps lossily (accepted):* `lspace`/`rspace` operator spacing and
`displaystyle`/`scriptlevel` have no per-element OMML equivalent. Cosmetic;
dropped silently.

*Cannot map (hard error):* table **column rules and `\hline`** — `mtd
style="border-…"` has no OMML element at all. pandoc's mature converter drops
them silently, which would print an augmented matrix or a truth table with its
rules missing and no warning. An exam is not a place for that.

**Hard-error policy.** Export fails, quoting the offending LaTeX, when
`math-core` rejects the input, or when the MathML contains a construct in the
"cannot map" tier. A new `Error::UnsupportedMath { latex, reason }` carries
both. Nothing renders best-effort onto paper.

**Panics from dependencies are caught, not assumed away.** A third-party crate
may panic; this crate's job is to stop it bubbling out as an abort. Conversion
runs inside `catch_unwind` and a caught panic becomes
`Error::UnsupportedMath`, the same as a rejection. That holds regardless of the
measured panic rate — `math-core` scored zero, but the guard is about not
trusting the measurement. (The release profile does not set `panic = "abort"`,
so unwinding is available.)

`math-core` is also pinned (`=0.8.2`), and the adversarial corpus lands as a
test, so a version bump that reintroduces a panic or malformed output is caught
here rather than discovered on an exam.

**Parsing the MathML.** `math-core` returns a string, so the mapper needs an XML
reader: `roxmltree` (73M downloads, MIT/Apache-2.0, 56 KiB, one transitive dep
already in the tree). A read-only DOM rather than a pull parser, because the
mapping is a tree transform — `mfrac` needs both children, `munderover` needs
three.

Rejected: hand-writing a LaTeX parser (unbounded maintenance); the direct
LaTeX→OMML crates (`tex2word-math` 134 downloads, `easydoc-math` 97,
`ooxml-omml` a 290-download alpha — none clears the curated-dependency bar);
rendering math to images (hard external dep for core content, no baseline
alignment, not editable); and shelling out to pandoc for math only (a hard
external dependency for core content, and pandoc itself drops `|` column rules
and gives up outright on `\cancel`, `\phantom` and `\textcolor`).

`prompt_html` already parses with `Options::ENABLE_MATH`, so the DOCX writer
walks the same `Event::InlineMath` / `Event::DisplayMath` events. The Canvas
path is untouched (its equation-service images already work); the markdown sheet
keeps emitting literal `$…$`.

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
5. **Math.** *Done* — see "Math — settled" above. `src/export/docx/omml.rs`
   converts LaTeX to OMML and is verified by rendering, not only by asserting
   on XML: every defect below was found by looking at a printed page.

   **Known gaps, deliberately left:**
   - **Accent marks are not normalised.** `math-core` returns *spacing*
     modifiers for some accents (`\hat` → U+02C6) and *combining* marks for
     others (`\vec` → U+20D7). Word positions accents from the combining
     forms, so a small mapping is owed.
   - **`\underline`/`\underbrace` take the wrong construct.** `MathML` marks
     these with `accentunder`, not `accent`, so they fall through to
     `m:limLow` with a bare combining character as the limit. They want
     `m:bar` and `m:groupChr`.
   - **Ragged tables declare the wrong column count.** `m:mcs` is built from
     row 0 only, and short rows are not padded.
   - **The "nothing best-effort" policy leaks.** `\color{red}{x}` silently
     drops the colour and `mstyle` attributes are discarded, where the module
     header promises a refusal.
   - **`⋃` renders as an error glyph in LibreOffice.** Environment-conditional:
     Cambria Math is not installed here, and pandoc's own output shows the same
     symptom for `\lim`. Needs checking in real Word before being treated as a
     defect in this crate.
6. **Inline runs.** *Done* — `src/export/docx/inline.rs`. Bold, italic,
   strikethrough, code spans and headings become `w:r` runs with the right
   `w:rPr`; headings use OOXML's built-in `Heading1`–`Heading3` so they reach
   the navigation pane and a contents table. Math becomes `m:oMath`. This is
   where math is finally wired in; the end-to-end check is in
   `tests/docx_package.rs`.

   **The rule that shaped it: nothing is dropped silently.** A block the
   writer cannot lay out (list, table, block quote, image, link, fenced or
   indented code, rule, raw HTML) is emitted as *its own Markdown source*,
   indentation and line breaks preserved. One `match` arm in `runs`
   enumerates what is renderable and its fallback refuses the whole
   paragraph, so that set cannot drift from what is handled. `blocks` makes
   the same call at block level, and anything the parser reports *no event
   for at all* is covered by comparing each block's span against the source —
   a link reference definition (`[id]: https://…`) is the live case, and a
   `***` rule falls out of the same mechanism rather than needing an arm of
   its own.

   **Math is the deliberate exception, and the rule is now enforced on every
   path**: unconvertible math fails, *and so does math inside a block that
   would fall back to source*. `- solve $x^2$` is an error rather than a
   sheet with `$x^2$` printed on it. `to_docx` is fallible for this reason.

   **Found by rendering and by review, not by assertion.** Every defect below
   passed the unit tests, `xmllint --noout`, *and* schema validation:
   - `m:oMathPara` centres the **whole paragraph**. Two separate bugs: `$$…$$`
     mid-sentence dragged the surrounding words to the middle of the page,
     and — after the first fix — a prompt that was *only* display math still
     dragged its question number there, because `question_xml` prepends the
     number after the decision is made. `to_omml` no longer emits the wrapper
     at all; setting an equation apart is now the paragraph writer's call, as
     only it knows what else shares the line.
   - **Same-kind emphasis cancelled itself.** Marks were a flag toggled per
     event, but CommonMark *nests*: `****very****` rendered with no bold, and
     in `**a __b__ c**` the inner word was the only plain one. Marks are
     depth counters now. (The Canvas path renders these correctly, so this
     was also a divergence between the two exporters.)
   - A multi-paragraph prompt had no space between its paragraphs.
   - A heading opening a prompt silently lost its level, while a heading
     later in the same prompt kept it.
   - An indented code block lost the indent on its first line only: the
     parser reports the block from its content, not its marker.
   - The gap covering opened its paragraph with a blank line, because a gap
     between two blocks begins with the newline that ended the last one.

   **Known gaps, deliberately left:**
   - **Inline marks inside a fallen-back block print their markers** — a
     `~~struck~~` list item shows its tildes, because the block falls back
     whole. Part 7 closes it for lists, Part 8 for tables.
   - **No hyperlinks.** A link paragraph falls back to source, so the URL
     prints for inline links; a *reference* link prints its definition line
     instead. Real `w:hyperlink` needs a relationship per link and belongs
     with Part 10, which already has to write rels.
   - **Emphasis around math is dropped** — `**$x$**` renders unbolded, since
     marks are not carried into `m:r`.
   - **Headings are capped at three levels**, deeper ones clamping onto
     `Heading3` rather than naming a style the document does not define.

7. **Lists.** *Done* — `src/export/docx/numbering.rs` plus the list walk in
   `inline.rs`. Bulleted and numbered, nested to `w:ilvl` 8, with each item's
   inline formatting and math rendered rather than printed as source.

   **Each ordered list opens its own `w:numId`.** The counter lives on the
   `w:numId`, so two lists sharing one do not both start at 1 — the second
   continues the first, printing "3. 4." where the author wrote two lists of
   two. Plausible enough on screen to survive review, and wrong on paper.
   Bullets carry no counter, so every bulleted list shares one id.

   **Found by rendering:** an item's *second* paragraph was marked like a
   fresh item, so `- one` followed by an indented continuation printed two
   bullets. A continuation now carries no `w:numPr` — and therefore inherits
   no indent from the level either, so it is indented by hand to the same
   place. `numbering::indent` is shared by both calculations, with a test
   that they agree; drifting apart leaves an item's second paragraph hanging
   to the left of its first.

   **What a list-item paragraph is lives in one place.** `numbering::Item`
   holds the three fields — `w:numId`, `w:ilvl`, and whether the marker
   prints — and emits its own `w:numPr` and `w:ind`. They were previously
   spelled out in four (`inline::Kind::Item`, `ParagraphStyle::Item`,
   `item_properties`, and an `item_kind` constructor), so adding an attribute
   to a list item meant four edits that nothing made fail together. `docx.rs`
   keeps only the `CT_PPrBase` ordering, which is the container's business;
   `indent`, `INDENT` and `HANGING` are private to `numbering` again.

   The fallback rule is unchanged and now has a sharper edge: one item the
   writer cannot lay out sends the *whole* list back to source. Half a list
   formatted and half printed as Markdown is worse than either.

   This also narrows the Part 6 math refusal — math in a list now renders, so
   only tables, quotes, links and images still refuse it. A list still reaches
   the refusal when it *falls back*, which is what the restored test pins.

   **Found by review**, both of them the quiet kind — the page looks
   plausible and is wrong only on paper:

   - **The question number printed inside the first bullet.** `question_xml`
     prepends "6. " to the prompt's first paragraph so a wrapped prompt hangs
     off its own number. When the prompt *opens* with a list, that paragraph
     carries a `w:numPr` of its own, so the number landed inside the item —
     and, displacing `ParagraphStyle::Question`, took the question's leading
     space with it. `opening_paragraphs` now gives the number its own line
     there and lets the list start underneath.
   - **`w:startOverride` was pinned to `w:ilvl="0"`.** `w:lvlOverride` names a
     level, and one naming a level no item carries is ignored outright, so a
     nested list authored `5.` quietly started at the definition's 1. The
     level an ordered list's items sit on is now threaded through
     `Numbering::open` and the override is written against it.

   The `w:numId` a paragraph carries and the one the part defines were also
   computed by two separate expressions; both now go through `numbering::id_of`,
   because drift between them resolves a `w:numPr` to nothing, which is a
   document Word will not open.

8. **Preformatted blocks.** *Done* — code blocks are emitted as literal text
   in the monospace `Code` character style. Tables were too, until item 13
   laid them out; the monospace path is now their *fallback*. See "Tables"
   below. Folding the two together is what removed a whole part from this
   plan, and it is also what gave tables a fallback worth having.

   Half of this had already landed for Part 7's list fallback: `literal` joins
   lines with explicit `<w:r><w:br/></w:r>` runs, because a `w:p` collapses
   newlines and a multi-row table would otherwise arrive as one run-on line.
   What Part 8 adds is the face. `Block::Preformatted` is the two constructs
   that are *set* this way rather than falling back to it, so a quote or a
   link still prints in the body face rather than being misreported as code.
   The run properties are built through the same `Marks` path an inline code
   span takes, so the block face and the span face resolve to one `w:rStyle`.

   **Only a top-level block is set this way.** A code block or table inside a
   list item rides along in the list's own fallback, which prints the whole
   list as source in the body face — setting an entire list in the code face
   to carry one fenced block would misreport the prose items around it. Both
   the test and `docs/quizzes.md` say so, so it is a decision rather than an
   accident.

   **Known gap: tabs.** `WordprocessingML` represents a tab stop with
   `<w:tab/>`, and a block is written one run per line, so a tab-indented code
   block reaches `w:t` as a raw tab character. `escape_xml` passes tabs
   through deliberately and the document is well-formed, but a tab may not
   align the way the equivalent spaces do. Pinned by
   `a_tab_in_a_preformatted_block_survives_as_a_tab` rather than changed,
   because emitting `<w:tab/>` means splitting a line into several runs and no
   real page has yet said the current output is wrong.

   **Whitespace is pinned on the XML, not on the converted page.**
   `LibreOffice`'s plain-text filter collapses runs of spaces, so the
   conversion shows a code block is *present* and says nothing about its
   indent — it reported `total += i` for a line authored with four leading
   spaces, and `|3|` for a padded table cell. The document itself is correct;
   `a_preformatted_block_keeps_its_whitespace_in_the_xml` checks it where it
   is observable, and runs in the gate rather than behind `#[ignore]`.
9. **The six question kinds** + the answer-key document.

   **The freeze gap is closed.** `ExamItem::option_order` holds the presented
   order as indices into the list a kind shows — an ordering question's items,
   or a matching question's shared right-hand options — decided in `assemble`
   with every other random choice. `shuffle_choices` now reaches both kinds,
   so variants differ; nothing shuffles the payload itself, because for these
   two the authored order *is* the answer.

   **Found doing it: the text sort could print the answer.** Both
   `display_order`s sorted by text and the doc claimed that "never hands the
   student the correct order" — but items authored alphabetically sort
   straight back to the authored order, and a shuffle can land on it by
   chance. The repo's own fixtures are the bad case: `Compile, Link, Run` is
   alphabetical, and a matching question's options sort into the order that
   pairs each with the prompt it answers. `model::hides_the_answer` rotates a
   presented order off the identity, applied inside both `display_order`s so
   the *bank* print sheet is fixed too, and reused by `assemble` after a
   shuffle. Rotation consumes no randomness, so an unshuffled sheet does not
   depend on the seed at all.

   **The answer structures ship.** `src/export/docx/answers.rs` owns what is
   printed under a prompt, returning runs for the container to set — the same
   division `inline` uses. All six kinds, following the bank sheet's idioms so
   the two artifacts read alike: a lettered list for a single answer, a `[ ]`
   box where more than one may be picked, a `____` blank wherever the student
   writes something in. Matching and ordering read `ExamItem::option_order`
   and sort nothing.

   Brackets rather than `☐` deliberately: the ballot-box code point is missing
   from some faces a reader may substitute, and a missing glyph prints as a
   box that looks deliberate — the sheet would be wrong in a way nobody can
   see.

   **Found doing it: the DOCX writer printed `{{city}}`.** Fill-in-the-blank
   markers are an instruction to the exporter, and only the bank sheet
   substituted them; the exam sheet put the marker on the paper. `answers::
   prompt` now fills them with writing room on this path too.

   `answer_space` is still emitted for every kind, including those that now
   print options. It is an authored layout value, and having the writer decide
   "a choice question needs no room" would be exactly the re-derivation
   `ExamItem` exists to prevent — set `answer_space: 0` on the group instead.

   **Found by review: the bank sheet was handing away a matching answer.**
   `markdown::render_matching` sorted the options itself
   (`options.sort_unstable()`) instead of going through
   `Matching::display_order`, so it skipped `hides_the_answer` — and the two
   print paths' tests then asserted *opposite* things about identical data,
   one of them that option A is prompt 1's answer. It uses the model's order
   now. `render_ordering` was already correct; only matching had its own sort.

   Blank-marker substitution was also duplicated verbatim in both paths,
   `"________"` literal included. It lives in `model::fill_blanks` with
   `model::BLANK_FILL` now — the marker syntax's own module — so a blank is
   the same width whichever sheet a student is handed.

   **The answer-key document ships.** `docx::to_answer_key` pairs with
   `to_docx`: same container, same numbering, a different body. The key is one
   line per question — a title, then the answer — with no header, no writing
   room and no page breaks, because those are the sheet's instructions to the
   student and its room to write.

   The key lives in `answers.rs` beside the sheet's options, and that is the
   point: **the sheet letters its options from the presented order and the key
   names those letters back.** A key built from a different order than the
   sheet it grades would mark every correct paper wrong, and neither document
   can reveal that on its own — so one module owns both, and
   `the_key_letters_matching_answers_as_the_sheet_did` asserts them together.
   This is what closing the freeze gap was for; the bank key prints answers as
   *text* precisely because it could not trust a label.

   An ordering key gives the authored sequence, not the presented one: there
   the authored order is the answer.

   Part 9 is complete. `export/markdown.rs` keeps deriving its own order for
   the bank path, correctly — it never sees an `ExamItem`. What it must not do
   is derive a *different* order, which is what the bug above was.
10. **Images and diagrams.** *Done* — PNG only, placed as real drawings.

    `export::docx::media` owns the third thing an image needs beyond a run: a
    *part* in the package and a relationship pointing at it. A `w:drawing`
    carries no image, it carries an `r:embed` naming a relationship, so a
    picture only appears if three files agree — and one module decides all
    three.

    **The id is minted at the moment the drawing is written.** That is the
    whole reason `Media` is a mutable registry threaded through the writers
    rather than a scan of the same Markdown done separately: a second scan
    that disagreed with the renderer by one image would leave a dangling
    `r:embed`, and Word does not lose the picture for that — it refuses to
    open the document. `relationships_resolve_in_both_directions` now checks
    `r:embed` as well as `r:id`; it only checked the latter, which is exactly
    the half that images do not use.

    **`Refs` replaced the threaded `Numbering`.** Lists and images are the
    same kind of thing — an id minted while the body is written, redeemed by
    a part written afterwards — and both are needed together, since authored
    Markdown can hold a list holding an image. Bundling them kept one
    parameter across the twenty-three signatures that had it rather than
    growing a second.

    **SVG-in-DOCX is out of scope, and enforced by signature.** Word places
    SVG through `asvg:svgBlip`, an *extension* that needs a rasterised copy
    beside it for readers that do not understand it — so shipping SVG means
    shipping a PNG anyway. `media` therefore reads the PNG magic number
    rather than trusting the file extension, which is load-bearing: the
    diagram renderer is injected, and one that ignores the format it was
    asked for lands SVG bytes behind a `generated/….png` path. An extension
    check would pass that straight through into a document that will not
    open. The quiz path has no `--diagram-format` flag at all; it forces PNG
    by not offering the choice.

    **Sizing reads the PNG.** Pixels are not a physical size, so a `pHYs`
    chunk is honoured where present and 96 DPI — Word's own assumption —
    where it is not. Without that a 300 DPI screenshot prints three times too
    wide. Anything larger than the text column is scaled in one step, by
    whichever dimension overruns by more; clamping width and height
    separately is the tempting version and it squashes the picture. The
    height cap matters too: a `wp:inline` picture does not paginate, it is
    cut off at the bottom margin.

    Alt text becomes `descr` on both `wp:docPr` and `pic:cNvPr`, which is what
    a screen reader announces. An image whose bytes were not supplied — a
    remote URL, a missing file, a non-PNG — falls back to its Markdown source
    with the rest, so the path stays visible for the author to chase.

    **A real bug came out of the first end-to-end run.** `cli.rs` was handing
    `assemble` *group*-relative source paths (`q1.md`) where the library
    documents spec-relative ones (`topics/q1.md`). `assemble` recovers a
    question's own folder from the directory part of its path and rebases its
    images and `file:` partials against it, so every image beside a question
    in a group folder was looked for beside the *spec* — and two groups'
    identically named figures would have collided. Both spellings of the join
    now go through one `path::join_dir`, replacing a private copy in
    `assemble`.

    Verified end to end, not just in the suite: a two-variant spec with an
    authored PNG and a ` ```mermaid ` block produced sheets holding both
    images, and `pdfimages -list` on the LibreOffice conversion reports them
    on the page at 96 ppi, unscaled. The `#[ignore]`d conversion test now
    asserts that — `pdftotext` cannot see a picture, so every text assertion
    would have passed with the drawing silently dropped.

    **Three more defects came out of the review pass.**

    *An image placed in a paragraph that then fell back stayed bundled.* A
    block is laid out speculatively — the runs are built, and a later link or
    raw HTML still sends the whole block to Markdown source. Those runs are
    discarded; the registered image was not, so the package carried a part
    and a relationship nothing cited. `Media::mark`/`rewind` now bracket the
    attempt, and `Block::render` rewinds on each fallback arm. Exact, because
    `used` only grows by one push per image first drawn — an image an earlier
    paragraph placed is found by the lookup and never pushed again, so
    truncating cannot discard it.

    *`fit` returned an unclamped extent when a dimension rounded to zero.*
    A `pHYs` chunk states its two resolutions independently, so a corrupt one
    reaches the sizing with a width that rounds away and a height that does
    not. The zero shortcut returned both untouched: `cx="0"`, which Word
    draws as nothing, beside a `cy` past what the schema admits, which Word
    rejects the file over. Each axis is now clamped on its own there, and
    `shrink`'s one-EMU floor applies on every path.

    *A failing diagram was retried once per variant.* `render_diagrams`
    remembered successes but not failures, so a missing `mmdc` was shelled
    out to — and warned about — once per copy of the question. `Seen` now
    holds both. This reverses a documented decision (`Failures are not cached
    the way renders are`), and deliberately: that rationale was "each block
    needing attention is named", which variants broke. The warning names the
    *question*, so four clones produce four byte-identical lines reporting
    one problem. Two genuinely different failing diagrams still warn twice.

    Also from the review: the relationship `Target` and the media part path
    are now derived from one `MEDIA_DIR` rather than two spellings that
    happened to agree; the registry stores the supplied entries rather than
    indices into them, so `parts` cannot resolve fewer than `relationships`
    declares; `Assembly::questions`/`questions_mut` moved the variant walk
    out of `cli.rs`; and `parse::rebase_images` joins through `path::join_dir`
    like `assemble` does, because the Word writer matches a supplied image by
    its authored path *as a string* — a `./` one adds and the other does not
    is a picture that silently fails to place.
11. **`mdquiz quiz` subcommand.** *Done* — sheets, keys and the run manifest.

    `quiz::output::render` pairs every variant with its answer key and names
    both, returning `Vec<OutputFile>` of bare names and bytes; `cli.rs` creates
    the directory and writes them. Variant naming reuses `Exam::variant_label`
    (and so `label::sequence`'s A..Z-then-number rule) rather than re-deriving
    it, and the variant goes *before* the key suffix — `exam-A`, `exam-A-key` —
    so a listing groups a sheet with the key that grades it. A lone sheet takes
    the stem alone, because an `-A` implies a `-B` that does not exist.

    **Every variant gets its own key**, which is the point of the whole freeze
    chain: a letter circled on variant B means something else on A, so one key
    cannot grade two papers. Verified end to end on a three-variant spec —
    variant A showed `C. 1 byte` and its key said `char → C`; variant B showed
    `B. 1 byte` and its key said `char → B`.

    Group folders resolve against the *spec's* directory, not the shell's
    working directory (a spec is checked in beside the questions it draws), and
    `escapes_dir` refuses one that climbs out. The seed is printed on every
    run, given or generated: without it a sheet handed out can never be
    rebuilt.

    `Command` now holds `clap::Args` structs rather than inline fields, so
    `run` is a two-arm dispatch and each subcommand destructures its own
    arguments.

    **The manifest ships.** `quiz::manifest` writes `<stem>-manifest.yaml`
    beside the sheets: the exam name, the seed, and per variant every question
    id with its options *as printed*. YAML because a spec is YAML — an author
    reading the record of a run should not have to change notation to do it,
    and `serde_norway` was already a dependency.

    It records presented **texts**, not a permutation of the authored choices:
    assembly shuffles a choice payload in place and discards the order the
    author wrote, so there is nothing left to diff against. Texts are
    self-contained anyway — the record needs no second file to interpret.

    **That costs nothing in correctness.** For multiple choice and multiple
    select the `correct` flag rides on the `Choice` itself, so the shuffle
    carries the answer along with the text and both writers letter by position
    in the same frozen vector; there is no mapping to lose. The two kinds where
    the answer *cannot* ride along — matching and ordering, whose authored order
    is the answer — are exactly the two that store `option_order`. Every kind is
    covered, by whichever of the two mechanisms suits it.

    **One accessor, so the record cannot lie.**
    `ExamItem::presented_options` is now the single source of the printed
    order, read by the docx writer, the answer key *and* the manifest;
    `answers.rs` lost its private copy. A manifest computing the order itself
    would be a record of a paper nobody sat.

    Two things this turned up:

    - The key letters a correct choice by its position in the *whole* option
      list, and every fixture put its correct answer first — where "position
      among all options" and "position among the correct ones" agree, so
      swapping `enumerate` and `filter` in `correct_choices` would have passed
      the entire suite.
      `the_key_letters_a_choice_by_its_place_in_the_whole_list` uses correct
      answers at positions 1 and 3 and fails on exactly that change.
    - Verified against generated files, not just tests: for a three-variant
      spec the manifest's option lists equal what each sheet printed, item for
      item, and each key names its own sheet's letters.

12. **Per-question points on the sheet.** *Done* — both print paths.

    `Question::points` had existed since the first question kind and only the
    Canvas exporter ever read it (`points_possible`). On paper that meant a
    student could not tell a twenty-mark question from a two-mark one, which
    is exactly the judgement a student makes when budgeting an hour.

    Every question now leads `3. (5 points) `, and each sheet states its
    total under the title. The *same* lead on the sheet and on the key, so a
    grader reading the two side by side is never comparing different marks
    against the same number — `the_sheet_and_the_key_lead_each_question_alike`
    asserts them together, in the spirit of the Part 9 key/option pairing.

    `export::points_label` is shared by the two writers so a Word sheet and a
    Markdown one word it the same way. `f64::to_string` already gives the
    shortest form that round-trips — `1` not `1.0`, `2.5` not `2.5000001` —
    so only the plural needed deciding, and only an exact 1 is singular.

    **`f64` sums from `-0.0`.** That is the only identity preserving the sign
    of everything it adds, so an exam of no questions totals negative zero
    and a sheet reads `Total: -0 points`. Found by the empty-exam test, fixed
    in `points_label` rather than at each call site, and pinned on both
    paths.

    `Layout::show_points` (default true) turns the per-question marks off for
    a quiz where every question counts the same and they would only be
    repetition; the total still prints, and is then the one place the
    weighting appears at all. The Markdown bank sheet has no such knob — a
    bank is not an exam and has no layout — so its marks always print.

    The six Markdown renderers took `number: usize` and each spelled
    `{number}. ` themselves; they now take the lead the dispatcher built, so
    the six cannot disagree about how a question is introduced.

    Verified on converted pages, not just in the suite: `Total: 7.5 points`
    with `1. (5 points)` and `2. (2.5 points)` on both the sheet and the key.


13. **Real Word tables.** *Done* — `w:tbl`, not text that looks like one.

    `export::docx::table` owns it. A table is the one authored construct that
    is neither a run nor a paragraph — `w:tbl` is a block-level *sibling* of
    `w:p` — so it could not travel as an `inline::Paragraph`'s runs without
    somebody eventually wrapping it in a `w:p`. That mistake produces
    well-formed XML, so neither `xmllint` nor a substring assertion catches
    it; only Word refusing the file does. Hence `Kind::Table` and a single
    `docx::set`, which every rendered block now goes through: the wrapping
    decision is made in one place and cannot be forgotten in another.

    Three things Word refuses a document over, each pinned by a test:

    - **A `w:tc` must hold at least one `w:p`.** The empty cell is the one a
      student writes the answer in, so this is not a corner case.
    - **A row shorter than the grid renders ragged**, and Markdown permits
      one, so short rows are padded.
    - **Two adjacent tables merge** with nothing between them, and a document
      ending on a table gets an empty paragraph appended anyway — so each
      table emits its own.

    **Fixed equal column widths, not autofit.** Autofit reads better for
    prose and is wrong here: the column a student writes into is the empty
    one, and autofit collapses an empty column to nothing. An equal share of
    the text width is predictable, leaves room to write, and does not depend
    on what the reader decides to measure. The grid and every cell state the
    same width, because under a fixed layout a cell that disagrees with its
    column leaves the reader to arbitrate.

    Borders are spelled on each table rather than defined as a table *style*,
    for the reason `styles.xml` already documents for `w:rStyle`: a style
    reference the package does not define loses its formatting silently, and
    a borderless table of blank cells is an invisible table. The header row
    carries `w:tblHeader` so it repeats when the table breaks across a page —
    columns with no names on page two are columns nobody can answer — and its
    cells open bold, so a cell's own `**…**` stacks rather than replacing it.
    Per-column alignment reaches the cells as `w:jc`.

    **Math in a cell now renders** rather than being refused. The refusal
    existed because the table printed as source and the LaTeX would have
    printed with it; once the table is laid out there is nothing to refuse.
    Math in a table that *falls back* is still refused.

    One cell the writer cannot lay out sends the whole table to source, in
    the monospace face — the same rule a list follows, for the same reason.
    An answer option holding a table falls back too: an option is folded into
    one line, and a `w:tbl` is not something that folds.

    Verified on a converted page: a four-column table with left, centred and
    right alignment, math and inline marks in its cells, and an empty column
    laid out for writing.


14. **Print formatting for real exams.** *Done* — three changes driven by a
    real course bank (CS 5001, 357 questions).

    **Marks a student can actually mark.** `[ ] ` is two characters wide; a
    pen needs more. The marks are now round where exactly one answer is right
    (multiple choice *and* true/false, which is a pick-one whatever its
    payload is called) and square where several may be, so the shape says how
    many to give before the instruction is read. Multiple choice had no mark
    at all before — only a letter.

    All three marks are set in the **monospace face**. A box built from
    body-face spaces is a different size in every substituted font, and too
    small to mark in most; in Consolas it is the same box everywhere. They
    live in `export.rs` beside `points_label`, shared by both print paths, so
    a student handed either sheet sees the same affordance.

    **Code blocks are blocks.** They were printed from their *source span*,
    which is why the ``` ``` ``` fences and the info string reached the page.
    Built from the block's own events instead, the parser hands over the code
    without them. `ParagraphStyle::CodeBlock` then sets it on a shaded panel
    (`w:shd`, `F2F2F2`) inset from both margins — the panel does the job the
    backticks used to do, marking where the block starts and ends. Verified
    by rendering a page to PNG: 30,800 pixels of exactly `F2F2F2`.

    Note `w:shd` sits between `w:numPr` and `w:spacing` in `CT_PPrBase`;
    `PPR_ORDER` grew an entry so a misfiled panel fails the schema test
    rather than Word.

    **Matching is two columns**, so a student can draw between them. A
    borderless `w:tbl` rather than tab stops, because a tab-stop layout wraps
    a long prompt *under* the option column — and the real bank's prompts are
    sentences ("Accessing an element of an array by its index"). The split is
    60/40, not even: a prompt is a sentence and an option is a word.

    That needed `table.rs` split in two — `of_cells` takes already-rendered
    `Row`s and owns the Word requirements, while the Markdown path renders
    cells first and calls it. `answers::lines` returns `Answers::Lines` or
    `Answers::Block`, mirroring `Kind::Table`/`set`: a `w:tbl` is not a `w:p`
    and cannot travel as one. The Markdown sheet uses a pipe table, the only
    thing Markdown has that puts two things on one line.

    The write-in blank stays on the left, so the sheet works whether a course
    grades drawn lines or written letters.

    **The conversion test now reads `pdftotext -layout`.** Without it a wide
    mark is read as its own column and lands on a line of its own, and a
    matching question's two columns interleave — so the old flat read could
    not see either change. It now asserts a prompt and an option share a
    line, which is the whole point of the layout and the one thing only a
    converted page proves.


### Tables

A Markdown pipe table is a real `w:tbl` (see item 13). The section that stood
here described the v1 stopgap — literal text in a monospace face — which is
now only the *fallback*, taken when a cell holds something the writer cannot
lay out.

The stopgap's two requirements still apply to that fallback, and to code
blocks, which is why the two share one path:

- **Monospace.** A pipe table in a proportional font loses column alignment
  entirely. In a monospace style the columns still line up on paper.
- **Preserved line breaks.** A DOCX paragraph collapses newlines, so a
  multi-row table would otherwise render as one run-on line. Each row needs an
  explicit `<w:br/>`.

No warning is emitted for the fallback. This is deliberate and is *not*
inconsistent with the hard-error policy for math: a table rendered as text is
visibly text, and the instructor sees exactly what the student sees. Wrong
math, by contrast, looks right. The failure modes are not comparable.

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

**Schema validation is not automated, and should be.** `xmllint --noout`
proves only well-formedness; it cannot see that `w:pPr` children are a
*sequence*, which is how two of this plan's bugs got in. Validation against
ECMA-376's `wml.xsd` does catch them, but it has only ever been run by hand
against a vendored copy of the schema, which is not in the repo. The schema is
~19 MB of generated XSD across twenty files and its licence terms have not
been checked, so vendoring it is a decision, not an oversight — but until it
is made, every claim that a part "validates" is a claim about a manual run,
not about the gate. The `w:pPr` and `w:rPr` orderings do at least have unit
tests asserting the sequence directly (`paragraph_properties_follow_the_schema_sequence`).

**Mutation testing is what found the Part 6 defects the other checks
missed** — 118 of 119 viable mutants are now caught across `docx.rs` and
`docx/inline.rs`; the one survivor is equivalent (pointing `line_start` at
the newline instead of just past it changes nothing, because `literal` trims
leading newlines — verified by diffing both versions over twelve inputs).

**`--ignored` now runs in CI** (Part 6): the workflow installs
`libreoffice-writer` and `poppler-utils` and runs the round-trip as its own
step, so the only check that the document actually opens is no longer
optional. **Mutation testing is still manual**, and it has found most of the
real test gaps in Parts 3–6 — including two in Part 6 that every other check
passed.

### Deferred

- **Tag filtering.** `Question::tags` is documented for exactly this; a
  `tags: [trees]` group filter is the natural next knob.
- **Markdown header/footer.** Only the DOCX writer consumes header/footer;
  extending the Markdown sheet is a small follow-on.
- **Table sizing beyond equal columns.** Every column takes an equal share of
  the text width (see item 13). A table of one narrow column and one wide one
  wastes the page; an authored width hint, or autofit for tables with no empty
  cells, would fix it.
- **SVG in DOCX.** Needs `asvg:svgBlip` inside `a:blip`'s extension list
  *plus* a rasterised `r:embed` fallback, so it is strictly more work than PNG
  and never less — see Part 10. The Canvas path still offers
  `--diagram-format svg`, where it is a plain `<img>`.
- **Forcing the point marks on for a uniform sheet.** `show_points` is a
  plain bool, so an instructor who wants `(1 point)` on all forty questions
  gets it and one who does not turns it off. What is missing is a *per-group*
  override, for a spec whose sections differ in weight.
