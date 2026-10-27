//! A draft in its real form (A04 D2, D5). WP-F2 (steps 12 and 14) builds
//! it: opening a draft in Enter Bill, Record Card Charge or Write Check in
//! draft mode, Accept through the form's own save to `convert`, every
//! convert answer, Page Up/Down through the queue, and learning after a
//! convert.
//!
//! Frozen by WP-M: the control channel's review digests
//! (`state.review.form_digest` and `state.review.posted_digest`, plan §4
//! N4). At Accept the form publishes a digest of its canonical values
//! ([`ReviewDigests::form_digest`]) and of the convert body it sent
//! ([`ReviewDigests::posted_digest`]); a scenario checks they are equal
//! (AC1: what the owner reviewed is what posted). Digests only: never an
//! amount, a name or document text. WP-F2 defines the canonical text and
//! hashes it with [`digest`].
//!
//! The digest is an HMAC-SHA-256 under a random key made once per process
//! (security review L6): a plain hash of a few form values could be
//! reversed by trying likely amounts and dates; the keyed one can only be
//! compared with another digest from the same run.

use std::sync::OnceLock;

use eframe::egui;
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde::Serialize;
use sha2::Sha256;

/// The last Accept's digests (`state.review`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewDigests {
    /// [`digest`] of the form's canonical values at Accept.
    pub form_digest: Option<String>,
    /// [`digest`] of the canonical convert body sent.
    pub posted_digest: Option<String>,
}

impl ReviewDigests {
    /// The control channel's JSON.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// This process's digest key: 32 random bytes from the operating system,
/// made on first use and never stored, logged or shown.
fn key() -> &'static [u8; 32] {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut key = [0u8; 32];
        if getrandom::fill(&mut key).is_err() {
            key = fallback_key();
        }
        key
    })
}

/// When the operating system gives no random bytes: the standard library's
/// per-process random hash seeds (weaker, still unknown outside this run).
fn fallback_key() -> [u8; 32] {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut key = [0u8; 32];
    for (i, chunk) in key.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(i);
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    key
}

/// HMAC-SHA-256 of `canonical` under this process's key, lowercase hex:
/// equal inputs give equal digests within one run of the app only.
#[must_use]
pub fn digest(canonical: &[u8]) -> String {
    use std::fmt::Write as _;
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key()) else {
        // A 32-byte key is always accepted (HMAC takes any length).
        return String::new();
    };
    mac.update(canonical);
    mac.finalize()
        .into_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn digests_id() -> egui::Id {
    egui::Id::unique("a04/review/digests")
}

/// Records the last Accept's digests (the draft form, at Accept).
pub fn publish_digests(ctx: &egui::Context, digests: ReviewDigests) {
    ctx.data_mut(|d| d.insert_temp(digests_id(), digests));
}

/// The last Accept's digests; both `None` before any.
#[must_use]
pub fn digests(ctx: &egui::Context) -> ReviewDigests {
    ctx.data(|d| d.get_temp::<ReviewDigests>(digests_id()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security review L6: keyed, stable within the run, not the plain
    /// SHA-256 of the values.
    #[test]
    fn digests_are_keyed_hex_and_stable_within_the_run() {
        let d = digest(b"abc");
        assert_eq!(d.len(), 64);
        assert!(
            d.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(d, digest(b"abc"), "stable within the run");
        assert_ne!(d, digest(b"abd"));
        assert_ne!(
            d, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "not the plain SHA-256"
        );
        // The key isn't all zeros (randomness was available) and the
        // fallback isn't either.
        assert_ne!(key(), &[0u8; 32]);
        assert_ne!(fallback_key(), [0u8; 32]);
    }

    #[test]
    fn digests_are_published() {
        let ctx = egui::Context::default();
        assert_eq!(digests(&ctx), ReviewDigests::default());
        assert_eq!(
            digests(&ctx).to_json()["form_digest"],
            serde_json::Value::Null
        );
        let d = ReviewDigests {
            form_digest: Some(digest(b"x")),
            posted_digest: Some(digest(b"x")),
        };
        publish_digests(&ctx, d.clone());
        assert_eq!(digests(&ctx), d);
        let v = digests(&ctx).to_json();
        assert_eq!(v["form_digest"], v["posted_digest"]);
    }
}
