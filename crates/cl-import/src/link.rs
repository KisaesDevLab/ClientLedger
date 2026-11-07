//! The account identity hash for auto-linking (Q-194, Q-P25e, review S13).
//!
//! `HMAC-SHA256(key, "v1|" + trim(BANKID or "") + "|" + trim(ACCTID))`, as
//! lowercase hex. The key is a per-company secret in the OS keychain
//! (`bank-link-<linkId>`, Q-P25u), so the hash cannot be reversed or
//! brute-forced from the database alone. Only the hash and the mask leave
//! this computer.

use crate::normalize::AccountId;
use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::Sha256;

/// Bytes in a freshly made link key.
pub const LINK_KEY_BYTES: usize = 32;

/// The text the hash is taken over (exposed for tests; never log it).
fn hash_input(id: &AccountId) -> String {
    format!("v1|{}|{}", id.bank_id().unwrap_or_default(), id.acct_id())
}

/// The identity hash of an account number under the company's link key.
#[must_use]
pub fn account_key_hash(key: &[u8], id: &AccountId) -> String {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        // HMAC accepts keys of any length, so this branch cannot run.
        return String::new();
    };
    mac.update(hash_input(id).as_bytes());
    crate::dedupe::hex(&mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_trims_and_separates_bank_id() {
        let key = [7u8; LINK_KEY_BYTES];
        let a = AccountId::new(Some(" 123456789 "), " 000012345678\t").unwrap();
        let b = AccountId::new(Some("123456789"), "000012345678").unwrap();
        assert_eq!(account_key_hash(&key, &a), account_key_hash(&key, &b));
        assert_eq!(account_key_hash(&key, &a).len(), 64);
        let card = AccountId::new(None, "000012345678").unwrap();
        assert_ne!(account_key_hash(&key, &a), account_key_hash(&key, &card));
        assert_ne!(
            account_key_hash(&key, &a),
            account_key_hash(&[8u8; LINK_KEY_BYTES], &a)
        );
        assert_eq!(hash_input(&card), "v1||000012345678");
    }

    #[test]
    fn known_vector() {
        // HMAC-SHA256(key = 32 x 0x01, "v1|021000021|1234") computed
        // independently (Python hmac) so a refactor cannot drift.
        let id = AccountId::new(Some("021000021"), "1234").unwrap();
        assert_eq!(
            account_key_hash(&[1u8; 32], &id),
            "df37f999eb52cc03915ea7cc4de4fb56254f5d09e44b7d469c91d8bb035558a0"
        );
    }

    #[test]
    fn debug_and_mask_hide_the_number() {
        let id = AccountId::new(Some("021000021"), "9876543210").unwrap();
        let dbg = format!("{id:?}");
        assert!(
            !dbg.contains("9876543210") && !dbg.contains("021000021"),
            "{dbg}"
        );
        assert_eq!(id.mask(), "\u{2022}\u{2022}\u{2022}\u{2022}3210");
        assert_eq!(
            AccountId::new(None, "123456").unwrap().mask(),
            "\u{2022}\u{2022}\u{2022}\u{2022}456"
        );
        assert_eq!(
            AccountId::new(None, "1234").unwrap().mask(),
            "\u{2022}\u{2022}\u{2022}\u{2022}34"
        );
        assert_eq!(
            AccountId::new(None, "7").unwrap().mask(),
            "\u{2022}\u{2022}\u{2022}\u{2022}"
        );
        assert!(AccountId::new(None, "  ").is_none());
    }
}
