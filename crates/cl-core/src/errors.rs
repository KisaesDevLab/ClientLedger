//! Shared error vocabulary (CLAUDE.md coding standards, Q-119).
//!
//! An error shown to the owner has three parts:
//! - a plain-language message and a suggested fix, carried here as
//!   [`OwnerMessage`] i18n **keys** (the text lives in `cl-ui::i18n`);
//! - an [`ErrorId`] the owner can quote, also written to the log line;
//! - a scrubbed technical detail for "Copy details": error codes, statuses
//!   and ids only, never amounts, names, memo text or secrets.
//!
//! The per-domain error types live next to their values and are re-exported
//! here: [`MoneyError`], [`MoneyInputError`], [`DateError`], [`IdError`],
//! [`FiscalError`]. None of them stores the text that failed to parse, so
//! their `Display` and `Debug` are safe to log.

use std::fmt;

pub use crate::dates::DateError;
pub use crate::fiscal::FiscalError;
pub use crate::ids::IdError;
pub use crate::money::{MoneyError, MoneyInputError};

/// i18n keys for an owner-facing message and an optional suggested fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OwnerMessage {
    /// Key of the plain-language message (`error.money.too_precise`).
    pub key: &'static str,
    /// Key of the suggested fix, if there is one (`fix.money.too_precise`).
    pub suggestion_key: Option<&'static str>,
}

impl OwnerMessage {
    /// A message with a suggested fix.
    #[must_use]
    pub const fn with_fix(key: &'static str, suggestion_key: &'static str) -> OwnerMessage {
        OwnerMessage {
            key,
            suggestion_key: Some(suggestion_key),
        }
    }

    /// A message without a suggested fix.
    #[must_use]
    pub const fn plain(key: &'static str) -> OwnerMessage {
        OwnerMessage {
            key,
            suggestion_key: None,
        }
    }
}

/// An error the owner can see.
pub trait OwnerFacing {
    /// The message and suggested fix keys.
    fn owner_message(&self) -> OwnerMessage;

    /// Technical detail for "Copy details": codes, statuses and ids only.
    /// Must never contain amounts, names, memo text, emails or secrets.
    fn scrubbed_detail(&self) -> String;
}

/// A short opaque id that ties an owner-visible error to its log line.
///
/// The caller supplies the number (from its own random source or counter;
/// cl-core has none). Shown as `CL-XXXX-XXXX`: the low 40 bits in Crockford
/// base 32, which avoids the easily confused letters I, L, O and U.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ErrorId(u64);

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ID_BITS: u32 = 40;

impl ErrorId {
    /// Wrap a caller-generated number (only the low 40 bits are kept).
    #[must_use]
    pub const fn new(raw: u64) -> ErrorId {
        ErrorId(raw & ((1 << ID_BITS) - 1))
    }

    /// The kept 40-bit value.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    /// Parse what the owner pasted: `CL-XXXX-XXXX`, with or without the
    /// prefix and hyphen, any case, with `O` read as `0` and `I`/`L` as `1`.
    #[must_use]
    pub fn parse(s: &str) -> Option<ErrorId> {
        let t = s.trim();
        let t = t
            .strip_prefix("CL-")
            .or_else(|| t.strip_prefix("cl-"))
            .unwrap_or(t);
        let mut value: u64 = 0;
        let mut count = 0;
        for c in t.chars() {
            if c == '-' {
                continue;
            }
            let c = match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                other => other,
            };
            let digit = CROCKFORD.iter().position(|d| char::from(*d) == c)?;
            value = (value << 5) | u64::try_from(digit).ok()?;
            count += 1;
        }
        (count == 8).then_some(ErrorId(value))
    }
}

impl fmt::Display for ErrorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CL-")?;
        for i in 0..8u32 {
            if i == 4 {
                f.write_str("-")?;
            }
            let shift = 5 * (7 - i);
            let idx = usize::try_from((self.0 >> shift) & 0x1f).unwrap_or(0);
            let c = CROCKFORD.get(idx).copied().unwrap_or(b'0');
            write!(f, "{}", char::from(c))?;
        }
        Ok(())
    }
}

impl fmt::Debug for ErrorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ErrorId({self})")
    }
}

/// `scrubbed_detail` for error enums that carry no user data: the type and
/// variant (with only numeric limits such as `max`).
fn detail<E: fmt::Debug>(kind: &str, e: &E) -> String {
    format!("{kind}::{e:?}")
}

impl OwnerFacing for MoneyError {
    fn owner_message(&self) -> OwnerMessage {
        match self {
            MoneyError::Format => OwnerMessage::with_fix("error.money.format", "fix.money.format"),
            MoneyError::TooPrecise => {
                OwnerMessage::with_fix("error.money.too_precise", "fix.money.too_precise")
            }
            MoneyError::OutOfRange | MoneyError::Overflow => {
                OwnerMessage::with_fix("error.money.too_large", "fix.money.too_large")
            }
            MoneyError::BadWeights => {
                OwnerMessage::with_fix("error.money.bad_weights", "fix.money.bad_weights")
            }
            MoneyError::NotWholeUnit => {
                OwnerMessage::with_fix("error.money.not_whole_cents", "fix.money.not_whole_cents")
            }
            MoneyError::BadScale => OwnerMessage::plain("error.internal"),
        }
    }

    fn scrubbed_detail(&self) -> String {
        detail("MoneyError", self)
    }
}

impl OwnerFacing for MoneyInputError {
    fn owner_message(&self) -> OwnerMessage {
        match self {
            MoneyInputError::Empty => {
                OwnerMessage::with_fix("error.amount.empty", "fix.amount.empty")
            }
            MoneyInputError::Invalid => {
                OwnerMessage::with_fix("error.amount.invalid", "fix.amount.invalid")
            }
            MoneyInputError::AmbiguousSeparator => {
                OwnerMessage::with_fix("error.amount.ambiguous", "fix.amount.ambiguous")
            }
            MoneyInputError::TooManyDecimals { .. } => OwnerMessage::with_fix(
                "error.amount.too_many_decimals",
                "fix.amount.too_many_decimals",
            ),
            MoneyInputError::TooLarge => {
                OwnerMessage::with_fix("error.amount.too_large", "fix.amount.too_large")
            }
            MoneyInputError::SignConflict => {
                OwnerMessage::with_fix("error.amount.sign_conflict", "fix.amount.sign_conflict")
            }
        }
    }

    fn scrubbed_detail(&self) -> String {
        detail("MoneyInputError", self)
    }
}

impl OwnerFacing for DateError {
    fn owner_message(&self) -> OwnerMessage {
        match self {
            DateError::Format | DateError::TimestampFormat => {
                OwnerMessage::with_fix("error.date.format", "fix.date.format")
            }
            DateError::NoSuchDay => {
                OwnerMessage::with_fix("error.date.no_such_day", "fix.date.no_such_day")
            }
            DateError::OutOfRange => {
                OwnerMessage::with_fix("error.date.out_of_range", "fix.date.out_of_range")
            }
        }
    }

    fn scrubbed_detail(&self) -> String {
        detail("DateError", self)
    }
}

impl OwnerFacing for IdError {
    /// An id never comes from the owner's typing, so a bad one is a program
    /// or engine fault.
    fn owner_message(&self) -> OwnerMessage {
        OwnerMessage::with_fix("error.internal", "fix.internal.report")
    }

    fn scrubbed_detail(&self) -> String {
        detail("IdError", self)
    }
}

impl OwnerFacing for FiscalError {
    fn owner_message(&self) -> OwnerMessage {
        match self {
            FiscalError::BadMonth => {
                OwnerMessage::with_fix("error.fiscal.bad_month", "fix.fiscal.bad_month")
            }
            FiscalError::OutOfRange => {
                OwnerMessage::with_fix("error.date.out_of_range", "fix.date.out_of_range")
            }
        }
    }

    fn scrubbed_detail(&self) -> String {
        detail("FiscalError", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dates::CalendarDate;
    use crate::ids::AccountId;
    use crate::money::{Money, UserInput};

    #[test]
    fn error_id_formats_and_parses() {
        let id = ErrorId::new(0x12_3456_789A);
        let s = id.to_string();
        assert_eq!(s.len(), 12);
        assert!(s.starts_with("CL-") && s.as_bytes()[7] == b'-');
        assert_eq!(ErrorId::parse(&s), Some(id));
        assert_eq!(ErrorId::parse(&s.to_lowercase()), Some(id));
        assert_eq!(
            ErrorId::parse(&s.replace('-', "").replacen("CL", "", 1)),
            Some(id)
        );
        assert_eq!(ErrorId::new(0).to_string(), "CL-0000-0000");
        assert_eq!(ErrorId::new(u64::MAX).to_string(), "CL-ZZZZ-ZZZZ");
        assert_eq!(ErrorId::new(u64::MAX).value(), (1 << 40) - 1);
        assert_eq!(ErrorId::parse("CL-OOOO-OOOL"), Some(ErrorId::new(1)));
        assert_eq!(ErrorId::parse("CL-0000-000U"), None);
        assert_eq!(ErrorId::parse("CL-0000-000"), None);
        assert_eq!(ErrorId::parse(""), None);
        assert_eq!(format!("{:?}", ErrorId::new(1)), "ErrorId(CL-0000-0001)");
    }

    #[test]
    fn every_variant_has_keys() {
        let all: Vec<Box<dyn OwnerFacing>> = vec![
            Box::new(MoneyError::Format),
            Box::new(MoneyError::TooPrecise),
            Box::new(MoneyError::OutOfRange),
            Box::new(MoneyError::Overflow),
            Box::new(MoneyError::BadWeights),
            Box::new(MoneyError::NotWholeUnit),
            Box::new(MoneyError::BadScale),
            Box::new(MoneyInputError::Empty),
            Box::new(MoneyInputError::Invalid),
            Box::new(MoneyInputError::AmbiguousSeparator),
            Box::new(MoneyInputError::TooManyDecimals { max: 2 }),
            Box::new(MoneyInputError::TooLarge),
            Box::new(MoneyInputError::SignConflict),
            Box::new(DateError::Format),
            Box::new(DateError::NoSuchDay),
            Box::new(DateError::OutOfRange),
            Box::new(DateError::TimestampFormat),
            Box::new(IdError::Format),
            Box::new(IdError::Digit),
            Box::new(FiscalError::BadMonth),
            Box::new(FiscalError::OutOfRange),
        ];
        for e in &all {
            let m = e.owner_message();
            assert!(m.key.starts_with("error."), "{}", m.key);
            if let Some(fix) = m.suggestion_key {
                assert!(fix.starts_with("fix."), "{fix}");
            }
            let d = e.scrubbed_detail();
            assert!(d.contains("Error::"), "{d}");
        }
        assert_eq!(
            MoneyInputError::TooManyDecimals { max: 2 }.scrubbed_detail(),
            "MoneyInputError::TooManyDecimals { max: 2 }"
        );
        assert_eq!(OwnerMessage::plain("error.x").suggestion_key, None);
    }

    #[test]
    fn errors_never_carry_the_input() {
        // Marker values that must not leak into any message or detail.
        let marker = "987654";
        let errs: Vec<Box<dyn OwnerFacing>> = vec![
            Box::new(Money::parse_wire("987654.123456").unwrap_err()),
            Box::new(Money::parse_user("987,654.555", UserInput::CENTS).unwrap_err()),
            Box::new(CalendarDate::parse_wire("9876-54-32").unwrap_err()),
            Box::new(AccountId::parse("987654").unwrap_err()),
        ];
        for e in &errs {
            assert!(!e.scrubbed_detail().contains(marker));
            assert!(!e.owner_message().key.contains(marker));
        }
        let display = [
            Money::parse_wire("987654.123456").unwrap_err().to_string(),
            Money::parse_user("987,654.555", UserInput::CENTS)
                .unwrap_err()
                .to_string(),
            CalendarDate::parse_wire("9876-54-32")
                .unwrap_err()
                .to_string(),
        ];
        for s in display {
            assert!(!s.contains(marker), "{s}");
        }
    }
}
