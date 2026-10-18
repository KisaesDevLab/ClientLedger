//! `cl-import`: readers for the files owners bring in. P11 adds the CSV
//! reader and writer the list import/export uses; bank files (OFX, QFX,
//! QIF, bank CSV profiles, Q-11) arrive in P25.
#![forbid(unsafe_code)]

pub mod csv;

/// Returns the crate name; kept so the skeleton test stays meaningful.
#[must_use]
pub fn crate_name() -> &'static str {
    "cl-import"
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches() {
        assert_eq!(super::crate_name(), "cl-import");
    }
}
