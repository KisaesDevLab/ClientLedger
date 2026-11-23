//! The idempotency key a form holds for its save (PR-8 B1, Q-PR1a): the
//! one place every form's `key_for` goes through.
//!
//! Normally the key follows the request: the same key while the very same
//! request is retried, a fresh one once anything it sends changed. After a
//! `keyed` unknown outcome under the held key that is not safe: the first
//! save may have gone through, so a changed form under a fresh key would
//! post it a second time. The key is **pinned** then: every later save of
//! the form goes under it, whatever it sends. The engine (U-18) looks a
//! key up by user and key on every keyed route, before any validation, so
//!
//! - if the first save went through, a changed body is refused with 422
//!   `IDEMPOTENCY_KEY_REUSED` ("your earlier save went through", Find in
//!   register) and the same body is answered with the first result;
//! - if it did not, the edited body posts once under the old key.
//!
//! The engine never stores a refusal under a key, so the pin survives every
//! failure (a refusal, a sign-in that timed out, a lost link): only a save
//! that went through ([`HeldKey::clear`] on the form's success path) or the
//! owner starting the form over (Clear, another record, a new window) ends
//! it. A 422 keeps it too: the first save is in the books, so one more edit
//! must not post the form again under a fresh key; it is refused the same
//! way until the form starts over.

use cl_api::{ApiError, IdempotencyKey};

/// Why the key is held whatever the form sends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pin {
    /// Not pinned: the key follows the request.
    #[default]
    Free,
    /// A save under the key ended with a `keyed` unknown outcome.
    Unknown,
    /// The engine said the first save under the key went through (422
    /// `IDEMPOTENCY_KEY_REUSED`, or 409 of the same code on `/batch/save`).
    Through,
}

/// A form's held idempotency key and the request it was made for.
#[derive(Clone, Debug)]
pub struct HeldKey<I> {
    held: Option<(I, IdempotencyKey)>,
    pin: Pin,
}

impl<I> Default for HeldKey<I> {
    fn default() -> Self {
        HeldKey {
            held: None,
            pin: Pin::Free,
        }
    }
}

/// True when `e` says the key was already used for another request whose
/// save went through.
#[must_use]
pub fn is_key_reused(e: &ApiError) -> bool {
    matches!(e, ApiError::IdempotencyKeyReused { .. }) || cl_api::is_key_reused(e)
}

impl<I: Clone + PartialEq> HeldKey<I> {
    /// The key for the request `id`: the held one while the very same
    /// request goes again or while the key is pinned, a fresh one
    /// otherwise.
    pub fn key_for<Q>(&mut self, id: &Q) -> IdempotencyKey
    where
        I: std::borrow::Borrow<Q>,
        Q: PartialEq + ToOwned<Owned = I> + ?Sized,
    {
        if let Some((d, k)) = &mut self.held
            && (self.pin != Pin::Free || (*d).borrow() == id)
        {
            if (*d).borrow() != id {
                *d = id.to_owned();
            }
            return k.clone();
        }
        let k = IdempotencyKey::new();
        self.held = Some((id.to_owned(), k.clone()));
        k
    }

    /// The answer to a save sent under the held key was the failure `e`.
    /// A `keyed` unknown outcome of this key pins it; a reused key pins it
    /// as gone through; a pinned key stays on every other failure.
    /// Otherwise the key is dropped on a definitive refusal
    /// ([`crate::screens::register::keeps_key`]) so the next try is never
    /// answered with that refusal.
    pub fn failed(&mut self, e: &ApiError) {
        if !self.pin_on(e) && self.pin == Pin::Free && !crate::screens::register::keeps_key(e) {
            self.held = None;
        }
    }

    /// The pinning half of [`Self::failed`], for a form that keeps its key
    /// through every failure anyway: true when `e` pinned the held key (a
    /// `keyed` unknown outcome of it, or a reused key).
    pub fn pin_on(&mut self, e: &ApiError) -> bool {
        let Some((_, held)) = &self.held else {
            return false;
        };
        if let ApiError::OutcomeUnknown {
            keyed: true,
            idempotency_key: Some(lost),
            ..
        } = e
            && lost == held
        {
            if self.pin == Pin::Free {
                self.pin = Pin::Unknown;
            }
            return true;
        }
        if is_key_reused(e) {
            self.pin = Pin::Through;
            return true;
        }
        false
    }

    /// The save went through, or the form starts over: nothing is held.
    pub fn clear(&mut self) {
        self.held = None;
        self.pin = Pin::Free;
    }

    /// The held key.
    #[must_use]
    pub fn key(&self) -> Option<&IdempotencyKey> {
        self.held.as_ref().map(|(_, k)| k)
    }

    /// The request the held key was last used for.
    #[must_use]
    pub fn identity(&self) -> Option<&I> {
        self.held.as_ref().map(|(i, _)| i)
    }

    /// True while a key is held.
    #[must_use]
    pub fn is_some(&self) -> bool {
        self.held.is_some()
    }

    /// True while no key is held.
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.held.is_none()
    }

    /// True while the key goes with every save whatever it sends.
    #[must_use]
    pub fn pinned(&self) -> bool {
        self.pin != Pin::Free
    }
}

/// Tests: what cl-api reports when the reply to a keyed save under `key`
/// is lost (the link dropped after the engine took it, Q-P33al).
#[cfg(test)]
pub(crate) fn keyed_unknown(key: &IdempotencyKey) -> ApiError {
    ApiError::OutcomeUnknown {
        request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-0000000000b1")
            .expect("id"),
        idempotency_key: Some(key.clone()),
        keyed: true,
    }
}

/// Tests: a bank posting that differs from another only in its memo (what
/// the owner fixes after a lost reply).
#[cfg(test)]
pub(crate) fn posting_with_memo(memo: &str) -> cl_api::BankPosting {
    cl_api::BankPosting::transfer(cl_api::TransferDraft {
        from: cl_core::ids::AccountId::parse("d0000000-0000-4000-8000-a00000001010").expect("id"),
        to: cl_core::ids::AccountId::parse("d0000000-0000-4000-8000-a00000001020").expect("id"),
        date: cl_core::CalendarDate::from_ymd(2026, 8, 14).expect("date"),
        amount: cl_core::Money::parse_wire("1250.00").expect("money"),
        memo: Some(memo.to_string()),
    })
    .expect("transfer")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid() -> cl_core::ids::RequestId {
        cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000001").expect("id")
    }

    fn unknown(key: &IdempotencyKey, keyed: bool) -> ApiError {
        ApiError::OutcomeUnknown {
            request_id: rid(),
            idempotency_key: Some(key.clone()),
            keyed,
        }
    }

    fn validation() -> ApiError {
        ApiError::Local(cl_api::LocalValidation::InvalidValue)
    }

    /// The key follows the request until a keyed unknown outcome; then it
    /// is pinned whatever is sent, through every failure, until cleared.
    #[test]
    fn a_keyed_unknown_outcome_pins_the_key_whatever_is_sent() {
        let mut h: HeldKey<&str> = HeldKey::default();
        let first = h.key_for(&"memo a");
        assert_eq!(h.key_for(&"memo a"), first, "the same request");
        // A definitive failure: a changed request gets a fresh key.
        h.failed(&validation());
        assert!(h.is_none());
        let first = h.key_for(&"memo a");
        assert_ne!(h.key_for(&"memo b"), first, "not pinned: follows");
        let first = h.key_for(&"memo a");
        h.failed(&unknown(&first, true));
        assert!(h.pinned());
        assert_eq!(h.key_for(&"memo b"), first, "pinned: the held key");
        assert_eq!(h.identity(), Some(&"memo b"));
        // A refusal or a sign-in that timed out never drops the pin.
        h.failed(&validation());
        h.failed(&ApiError::Unauthorized(cl_api::AuthFailure::SessionExpired));
        assert_eq!(h.key_for(&"memo c"), first);
        // The first save went through: still pinned (refused again).
        h.failed(&ApiError::IdempotencyKeyReused { entity: None });
        assert!(h.pinned());
        assert_eq!(h.key_for(&"memo d"), first);
        // Saved, or the form starts over: a fresh key.
        h.clear();
        assert!(!h.pinned() && h.is_none());
        assert_ne!(h.key_for(&"memo d"), first);
    }

    /// Only an unknown outcome of the held key pins it; an unkeyed one
    /// keeps it (the form blocks) without pinning.
    #[test]
    fn only_a_keyed_unknown_of_the_held_key_pins_it() {
        let mut h: HeldKey<u8> = HeldKey::default();
        let k = h.key_for(&1);
        h.failed(&unknown(&IdempotencyKey::new(), true));
        assert!(!h.pinned(), "another key's outcome");
        h.failed(&unknown(&k, false));
        assert!(!h.pinned(), "unkeyed: the block, not the pin");
        assert_eq!(h.key(), Some(&k), "kept for the same request");
        assert_ne!(h.key_for(&2), k);
    }
}
