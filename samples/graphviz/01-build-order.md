---
id: mc-build-order
title: Build order from a dependency graph
kind: multiple_choice
tags: [graphs, build-systems]
choices:
  - text: util
    correct: true
  - text: parser
  - text: render
  - text: app
feedback:
  correct: Right — `util` has no outgoing edges, so nothing has to be built before it.
  incorrect: "Follow the arrows: a module can only be built once everything it points at exists."
---

In the dependency graph below an arrow from *A* to *B* means "*A* depends on
*B*". Which module must be compiled **first**?

```dot
digraph deps {
  rankdir=LR;
  node [shape=box, fontname="Helvetica"];
  app -> parser;
  app -> render;
  parser -> util;
  render -> util;
}
```
