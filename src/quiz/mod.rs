//! Quiz assembly: choosing which of a bank's questions go on one sheet.
//!
//! Selection is kept apart from the item bank on purpose. An
//! [`ItemBank`](crate::model::ItemBank) is *every* question an author wrote; a
//! printable quiz is a chosen subset. [`sample`] owns that choice, as a pure
//! function of its inputs and a seed, so a draw can be reproduced exactly.

pub mod sample;
pub mod spec;
