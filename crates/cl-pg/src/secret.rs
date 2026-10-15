//! A string that refuses to print itself.

use crate::{PgError, Result};
use std::fmt;

/// A secret value (database password, connection URL with a password).
///
/// `Debug` and `Display` print `***`. Call [`Secret::expose`] at the single
/// place the value is really needed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap an existing value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// 256 random bits as 64 hex characters (safe inside URLs and SQL).
    ///
    /// # Errors
    /// Fails when the operating system has no random source.
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| PgError::Random(e.to_string()))?;
        Ok(Secret(hex(&bytes)))
    }

    /// The raw value. Never log it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Replace every occurrence of this secret in `text` with `***`.
    #[must_use]
    pub fn scrub(&self, text: &str) -> String {
        if self.0.is_empty() {
            return text.to_string();
        }
        text.replace(&self.0, "***")
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_secrets_are_long_hex_and_distinct() {
        let a = Secret::generate().unwrap();
        let b = Secret::generate().unwrap();
        assert_eq!(a.expose().len(), 64);
        assert!(a.expose().bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn formatting_never_shows_the_value() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s}"), "***");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert_eq!(format!("{:?}", Some(&s)), "Some(Secret(***))");
    }

    #[test]
    fn scrub_removes_the_value() {
        let s = Secret::new("abc123");
        assert_eq!(s.scrub("x abc123 y abc123"), "x *** y ***");
        assert_eq!(Secret::new("").scrub("keep"), "keep");
    }
}
