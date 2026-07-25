# Fill in the blank (`kind: fill_in_blank`)

One or more **inline** blanks, each marked `{{name}}` in the prompt and matched
against a list of acceptable answers.

## Fields

Plus the [common metadata](README.md#common-metadata):

| Field    | Required? | Default | Meaning                                                    |
|----------|-----------|---------|------------------------------------------------------------|
| `blanks` | **Required** | —    | A map from each `{{name}}` in the prompt to its answers.   |

Every `{{name}}` marker in the prompt must have a `blanks` entry and vice
versa; each blank needs at least one answer.

### Blank shorthand and expanded forms

A blank's value is **either** a bare list of acceptable answers (matched
case-insensitively):

```yaml
blanks:
  method: [GET, get]
```

**or** an expanded map with an explicit `match` mode:

| Field   | Required? | Default            | Meaning                                        |
|---------|-----------|--------------------|------------------------------------------------|
| `answers` | **Required** | —              | Acceptable answers (or regex patterns).        |
| `match` | Optional  | `case_insensitive` | How responses are matched (see table).         |

| `match`            | Meaning                                       |
|--------------------|-----------------------------------------------|
| `case_insensitive` | Default. `get` matches `GET`.                 |
| `exact`            | Case-sensitive exact match.                   |
| `regex`            | Answers are regex patterns (see caveats).     |

## Example

```markdown
---
id: fitb-http-status
kind: fill_in_blank
blanks:
  method: [GET, get]
  code:
    answers: ["404", "Not Found"]
    match: exact
---

An HTTP {{method}} request for a missing resource returns status {{code}}.
```

## Export behavior

- **Print sheet:** each `{{name}}` becomes a fill-in line.
- **Canvas:** a `fill_in_multiple_blanks_question` (Open Entry text blanks),
  scored as partial credit per blank.

### Canvas caveats

- New Quizzes' **dropdown** and **word bank** answer types, and the
  **contains / close-enough** match modes, have no classic-QTI representation
  and are not exported.
- Canvas fill-in-the-blank matches **case-insensitively**, so `exact` is
  recorded but may not be enforced on import.
- A **`regex`** blank exports as a *literal-text* blank (Canvas can't set the
  regex mode via import). After importing, mdquiz **prints a reminder** naming
  which blanks to switch to "Regular Expression Match" in the New Quizzes
  editor.
- Only `general` feedback is wired (answer-level feedback is multiple-choice
  only).

## Samples

[`../samples/fill-in-blank/`](../samples/fill-in-blank/)
