//! List rules without egui (P11): ordering, hierarchy, filters and form
//! validation for the master lists. The engine owns the records; these
//! types only arrange and check what the screen loaded.

pub mod accounts;
pub mod class_map;
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

/// Runs `f` `runs` times and returns its last answer with the fastest
/// time, so a timing test does not fail on one slow run of a busy machine.
#[cfg(test)]
pub(crate) fn fastest_of<T>(runs: usize, mut f: impl FnMut() -> T) -> (T, std::time::Duration) {
    let mut best = std::time::Duration::MAX;
    let mut out = None;
    for _ in 0..runs.max(1) {
        let start = std::time::Instant::now();
        let v = f();
        best = best.min(start.elapsed());
        out = Some(v);
    }
    // `runs.max(1)` ran at least once.
    (out.expect("ran once"), best)
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
