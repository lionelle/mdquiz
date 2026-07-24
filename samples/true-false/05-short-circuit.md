---
id: tf-short-circuit
title: Short-circuit evaluation
kind: true_false
tags: [languages, rust, semantics]
answer: true
---

Consider the following Rust expression, where `check()` has a side effect:

```rust
let ready = false && check();
```

Because `&&` short-circuits, `check()` is never called and `ready` is `false`.
