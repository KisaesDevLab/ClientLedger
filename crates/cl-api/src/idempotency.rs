//! Idempotency keys and the safe-retry policy (P05 plan §2.6).
//!
//! The engine ignores `Idempotency-Key` until U-18, so sending it is harmless
//! and also no protection: a mutation is retried only when it provably never
//! reached a handler, or when outcome reconciliation proves the first attempt
//! did not commit.

use std::fmt;

/// Opaque key sent as `Idempotency-Key` on mutations.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(uuid::Uuid);

impl IdempotencyKey {
    /// A fresh random key (one per logical user action).
    #[must_use]
    pub fn new() -> IdempotencyKey {
        IdempotencyKey(uuid::Uuid::new_v4())
    }

    /// Header value (36-character lower-case UUID).
    #[must_use]
    pub fn as_header(&self) -> String {
        self.0.hyphenated().to_string()
    }
}

impl Default for IdempotencyKey {
    fn default() -> Self {
        IdempotencyKey::new()
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IdempotencyKey({})", self.0.hyphenated())
    }
}
