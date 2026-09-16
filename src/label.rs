//! Sequence labels for things a reader counts through.
//!
//! Answer choices and quiz variants are both presented as `A`, `B`, `C`, … and
//! both have to cope with running past `Z`. The rule lives here so the two
//! cannot drift apart.

/// The label for the item at `index`: `A`..`Z`, then a 1-based number.
///
/// Past twenty-six the letters run out and numbering is clearer than `AA`:
/// a twenty-seventh answer choice reads `27`, not a second alphabet.
#[must_use]
pub fn sequence(index: usize) -> String {
    if let Ok(offset) = u8::try_from(index)
        && offset < 26
    {
        return char::from(b'A' + offset).to_string();
    }
    (index + 1).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Letters while they last, then a 1-based number.
    fn letters_then_numbers() {
        assert_eq!(sequence(0), "A");
        assert_eq!(sequence(1), "B");
        assert_eq!(sequence(25), "Z");
        assert_eq!(sequence(26), "27");
        assert_eq!(sequence(100), "101");
    }

    #[test]
    /// Every label is distinct, so two choices can never share one.
    fn labels_are_unique() {
        let labels: std::collections::HashSet<String> = (0..200).map(sequence).collect();
        assert_eq!(labels.len(), 200);
    }
}
