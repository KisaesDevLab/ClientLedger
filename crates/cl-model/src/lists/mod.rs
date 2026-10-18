//! List rules without egui (P11): ordering, hierarchy, filters and form
//! validation for the master lists. The engine owns the records; these
//! types only arrange and check what the screen loaded.

pub mod accounts;
pub mod contacts;
pub mod csv_map;
pub mod items;
pub mod tags;
pub mod terms;

/// Characters counted the way the engine's schemas count them (UTF-16 code
/// units, as JavaScript's `String.length`), so a local length check never
/// disagrees with the engine's.
#[must_use]
pub fn engine_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_length_counts_utf16_units() {
        assert_eq!(engine_len("abc"), 3);
        assert_eq!(engine_len("caf\u{e9}"), 4);
        assert_eq!(engine_len("\u{1F33F}"), 2, "outside the BMP counts twice");
    }
}
