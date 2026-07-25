---
id: mc-binary-search-complexity
title: Binary search complexity
kind: multiple_choice
tags: [complexity, searching]
choices:
  - text: O(log n)
    correct: true
  - text: O(n)
  - text: O(n log n)
  - text: O(1)
feedback:
  correct: Right — each comparison halves the remaining range.
  incorrect: Think about how much of the array is discarded per comparison.
---

What is the worst-case time complexity of binary search on a sorted array of
*n* elements?
