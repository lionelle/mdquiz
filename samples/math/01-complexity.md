---
id: mc-merge-sort-complexity
title: Merge sort complexity
kind: multiple_choice
tags: [complexity, algorithms]
choices:
  - text: $O(n \log n)$
    correct: true
  - text: $O(n^2)$
  - text: $O(\log n)$
feedback:
  correct: Right — each of the $\log n$ levels does $O(n)$ work.
---

Merge sort divides the input in half each level and merges in linear time. What
is its worst-case time complexity?
