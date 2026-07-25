---
id: mc-rust-move
title: Ownership after move
kind: multiple_choice
points: 2
tags: [languages, rust, ownership]
choices:
  - text: A compile-time error — `s` was moved into `t`.
    correct: true
  - text: It prints an empty string.
  - text: It prints the string twice.
  - text: A runtime panic.
---

In this Rust snippet, what happens when you try to use `s` on the last line?

```rust
let s = String::from("hello");
let t = s;
println!("{s}");
```
