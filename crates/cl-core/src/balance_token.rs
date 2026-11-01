//! [`BalanceToken`]: an invoice's stored balance as the engine sent it
//! (`balanceDueExact`), echoed back unchanged as `expectedBalanceDue` (P21
//! plan §2 "Exact amounts", Q-P21s).
//!
//! The token is opaque by construction: it never becomes [`crate::Money`],
//! it is never shown, added or compared with an amount. There is no
//! `Display`, no arithmetic and no conversion to or from `Money`:
//!
//! ```compile_fail
//! let t = cl_core::BalanceToken::from_wire("385.0000");
//! let m: cl_core::Money = t.into();
//! ```
//!
//! ```compile_fail
//! let t = cl_core::BalanceToken::from_wire("385.0000");
//! let s = format!("{t}");
//! ```

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The engine's stored invoice balance, byte for byte (see the module docs).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BalanceToken(String);

impl BalanceToken {
    /// The token as the engine sent it (the demo engine and tests build
    /// it from the stored balance text).
    #[must_use]
    pub fn from_wire(s: &str) -> BalanceToken {
        BalanceToken(s.to_string())
    }

    /// The exact text to echo back as `expectedBalanceDue`.
    #[must_use]
    pub fn as_wire(&self) -> &str {
        &self.0
    }
}

/// Redacted: a balance is an amount (no amounts in logs).
impl fmt::Debug for BalanceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BalanceToken(..)")
    }
}

impl Serialize for BalanceToken {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BalanceToken {
    /// A JSON string only (a number would already have lost its bytes).
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = BalanceToken;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a decimal string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<BalanceToken, E> {
                if v.is_empty() || v.len() > 40 {
                    return Err(E::custom("balance token length"));
                }
                Ok(BalanceToken(v.to_string()))
            }
        }
        d.deserialize_str(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_byte_for_byte() {
        for s in ["100.0049", "385.0000", "0.01", "1260"] {
            let t: BalanceToken = serde_json::from_str(&format!("\"{s}\"")).unwrap();
            assert_eq!(t.as_wire(), s);
            assert_eq!(serde_json::to_string(&t).unwrap(), format!("\"{s}\""));
        }
    }

    #[test]
    fn a_number_is_refused_and_debug_is_redacted() {
        assert!(serde_json::from_str::<BalanceToken>("385.00").is_err());
        assert!(serde_json::from_str::<BalanceToken>("\"\"").is_err());
        let t = BalanceToken::from_wire("385.0000");
        assert_eq!(format!("{t:?}"), "BalanceToken(..)");
    }
}
