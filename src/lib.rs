//! `mdquiz` core library.
//!
//! `mdquiz` represents each quiz question as a Markdown file with a YAML block
//! that carries the machine-readable parts (answers, choices, scoring). The
//! library assembles a directory of those files into a strongly typed
//! [`model::ItemBank`] and renders it to one of two export targets:
//!
//! * a single print-ready Markdown sheet with **no** answer key, and
//! * a Canvas *New Quizzes* item bank (QTI package) suitable for import.
//!
//! The crate is organised so the parsing and export stages never depend on the
//! CLI: everything below [`model`] is a plain data transform that is easy to
//! unit-test.

pub mod error;
pub mod export;
pub mod model;
pub mod parse;

pub use error::{Error, Result};
