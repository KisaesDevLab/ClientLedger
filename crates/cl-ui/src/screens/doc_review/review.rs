//! A draft in its real form (A04 D2, D5). WP-F2 (steps 12 and 14) builds
//! it: opening a draft in Enter Bill, Record Card Charge or Write Check in
//! draft mode, Accept through the form's own save to `convert`, every
//! convert answer, Page Up/Down through the queue, and learning after a
//! convert.
//!
//! Frozen by WP-M: the control channel's review digests
//! (`state.review.form_digest` and `state.review.posted_digest`, plan §4
//! N4). At Accept the form publishes the SHA-256 of its canonical values
//! ([`ReviewDigests::form_digest`]) and of the convert body it sent
//! ([`ReviewDigests::posted_digest`]); a scenario checks they are equal
//! (AC1: what the owner reviewed is what posted). Digests only: never an
//! amount, a name or document text. WP-F2 defines the canonical text and
//! hashes it with [`digest`].

use eframe::egui;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

/// The last Accept's digests (`state.review`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewDigests {
    /// SHA-256 (lowercase hex) of the form's canonical values at Accept.
    pub form_digest: Option<String>,
    /// SHA-256 (lowercase hex) of the canonical convert body sent.
    pub posted_digest: Option<String>,
}

impl ReviewDigests {
    /// The control channel's JSON.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// SHA-256 of `canonical`, lowercase hex.
#[must_use]
pub fn digest(canonical: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(canonical)
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

    #[test]
    fn digests_are_published_and_hashed_as_sha256_hex() {
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
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
