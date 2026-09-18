//! Building a printable exam from a bank of questions.
//!
//! An [`ItemBank`](crate::model::ItemBank) is *every* question an author wrote;
//! an exam is a chosen subset, laid out. The pipeline runs in one direction:
//!
//! * [`spec`] — the authored YAML blueprint and its validation,
//! * [`assemble`] — the one place randomness happens,
//! * [`exam`] — the finished [`Exam`](exam::Exam) the print writers consume, and
//! * [`output`] — every variant paired with its answer key, named and ready to
//!   write, alongside the [`manifest`] recording what the run drew,
//!
//! with [`sample`] providing the seeded draws that [`assemble`] uses.
//!
//! The split matters because of one invariant: every random decision is made
//! during assembly and frozen into the `Exam`, so a sheet and its answer key
//! are two renderings of already-settled data and cannot disagree.

pub mod assemble;
pub mod exam;
pub mod manifest;
pub mod output;
pub mod sample;
pub mod spec;
