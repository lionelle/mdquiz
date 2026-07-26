# Math (LaTeX)

Write LaTeX math in any prompt, choice, ordering item, or feedback message:

- **Inline:** `$…$` — e.g. `Binary search is $O(\log n)$.`
- **Display:** `$$…$$` — e.g. `$$\sum_{i=1}^{n} i = \frac{n(n+1)}{2}$$`

## How it exports

- **Canvas:** each expression becomes Canvas's **native equation image**
  (`<img class="equation_image" …>`), rendered by Canvas's own equation service
  from the LaTeX in `data-equation-content`. Nothing is bundled and no external
  tool is required — the equation is selectable, scalable, and re-renders on
  whichever Canvas instance imports the bank. This matches how Canvas stores math
  in its own exports.
- **Print sheet:** the literal `$…$` source is kept. A Markdown viewer with math
  support renders it; on paper it shows the LaTeX source.

```markdown
---
id: mc-merge-sort
kind: multiple_choice
choices:
  - text: $O(n \log n)$
    correct: true
  - text: $O(n^2)$
feedback:
  correct: Each of the $\log n$ levels does $O(n)$ work.
---

What is the worst-case time complexity of merge sort?
```

See [`../samples/math/`](../samples/math/) for a runnable example.

## Notes

- **Escaping a literal dollar sign.** Since `$` delimits math, write a literal
  dollar as `\$` (e.g. `costs \$5`). An unpaired `$` is left alone, but two
  dollars on a line are read as a math span, so escape currency amounts.
- **LaTeX is not validated.** mdquiz passes the LaTeX through untouched; Canvas
  renders it. A malformed expression renders as Canvas would show it.
