//! A string that refuses to print itself and is wiped from memory on drop.

use std::fmt;
use zeroize::Zeroizing;

/// A secret value (launch token, access or refresh token, password).
///
/// - `Debug` prints `Secret(***)` and `Display` prints `***`.
/// - There is no `Serialize`: a secret can only reach the wire through an
///   explicit [`Secret::expose`] at the one place it is needed.
/// - The memory is zeroized on drop (also for clones).
/// - Equality compares every byte (no early exit on the first difference), so
///   comparing a guess against a token does not leak how much matched.
///
/// `cl_pg::Secret` is the older cluster-side equivalent and stays where it is
/// (P05 plan §2.1); adapters copy from `cl_pg::Secret::expose()` straight into
/// [`Secret::new`].
#[derive(Clone, Default)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// Wrap a value. The argument is moved in, so no extra copy is left behind.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Secret(Zeroizing::new(value.into()))
    }

    /// The raw value. Never log it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// True when the secret holds no characters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Replace every occurrence of this secret in `text` with `***`.
    #[must_use]
    pub fn scrub(&self, text: &str) -> String {
        if self.0.is_empty() {
            return text.to_string();
        }
        text.replace(self.0.as_str(), "***")
    }
}

impl PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        constant_time_eq(self.0.as_bytes(), other.0.as_bytes())
    }
}

impl Eq for Secret {}

/// Compares all bytes of equal-length inputs without an early exit. Lengths
/// are not secret (tokens have fixed lengths), so a length mismatch returns
/// at once.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
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

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Secret::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_never_shows_the_value() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s}"), "***");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert_eq!(format!("{s:#?}"), "Secret(***)");
        assert_eq!(format!("{:?}", Some(&s)), "Some(Secret(***))");
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn equality_compares_whole_values() {
        assert_eq!(Secret::new("abc"), Secret::new("abc"));
        assert_ne!(Secret::new("abc"), Secret::new("abd"));
        assert_ne!(Secret::new("abc"), Secret::new("abcd"));
        assert_ne!(Secret::new(""), Secret::new("a"));
        assert_eq!(Secret::default(), Secret::new(""));
        assert!(Secret::default().is_empty());
        assert!(!Secret::new("x").is_empty());
    }

    #[test]
    fn clone_is_independent_and_equal() {
        let a = Secret::from("token".to_string());
        let b = a.clone();
        drop(a);
        assert_eq!(b.expose(), "token");
    }

    #[test]
    fn scrub_removes_the_value() {
        let s = Secret::new("abc123");
        assert_eq!(s.scrub("x abc123 y abc123"), "x *** y ***");
        assert_eq!(Secret::new("").scrub("keep"), "keep");
    }
}
