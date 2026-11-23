//! Certificate fingerprints and the pairing code (Q-P33b, review M2).
//!
//! A fingerprint is the SHA-256 of a certificate's DER bytes. It is public:
//! knowing it does not let anyone connect, because the TLS 1.3 handshake
//! signature proves possession of the private key (S6).
//!
//! The pairing code is 20 Crockford base32 characters (100 bits):
//! - the first 12 characters (60 bits) are the start of the host's
//!   fingerprint, so the workstation can pin the host during pairing;
//! - the last 8 characters (40 bits) are a one-time secret that proves the
//!   person at the workstation can read the host's screen.
//!
//! A man in the middle would need a certificate whose fingerprint shares 60
//! bits with the host's (about 2^60 work) and the secret.

use std::fmt;

use sha2::{Digest, Sha256};

/// The SHA-256 of a certificate's DER encoding.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// Fingerprint of a DER certificate.
    #[must_use]
    pub fn of_der(der: &[u8]) -> Fingerprint {
        Fingerprint(Sha256::digest(der).into())
    }

    /// The raw bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 64 lowercase hex characters (the engine's `X-CL-Device` format).
    #[must_use]
    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    /// Parses 64 hex characters (either case).
    #[must_use]
    pub fn from_hex(s: &str) -> Option<Fingerprint> {
        let b = s.as_bytes();
        if b.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, pair) in b.chunks(2).enumerate() {
            let hi = hex_val(pair[0])?;
            let lo = hex_val(pair[1])?;
            out[i] = (hi << 4) | lo;
        }
        Some(Fingerprint(out))
    }

    /// For people: groups of four upper-case hex digits, the first 16
    /// groups (the full value is in "Copy details").
    #[must_use]
    pub fn display(&self) -> String {
        let h = self.hex().to_uppercase();
        h.as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The first 60 bits, as the pairing code carries them.
    #[must_use]
    pub fn pin60(&self) -> u64 {
        let mut first = [0u8; 8];
        first.copy_from_slice(&self.0[..8]);
        u64::from_be_bytes(first) >> 4
    }

    /// The first 12 base32 characters (the unpaired hello's prefix, S11).
    #[must_use]
    pub fn prefix(&self) -> String {
        encode_bits(self.pin60(), 12)
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({}…)", &self.hex()[..12])
    }
}

fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(H[usize::from(b >> 4)]));
        s.push(char::from(H[usize::from(b & 0xf)]));
    }
    s
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Crockford base32 (no I, L, O, U).
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn encode_bits(value: u64, chars: u32) -> String {
    (0..chars)
        .rev()
        .map(|i| {
            // Masked to 5 bits, so the index is always in range.
            let idx = usize::try_from((value >> (i * 5)) & 0x1f).unwrap_or(0);
            char::from(ALPHABET[idx])
        })
        .collect()
}

fn decode_char(c: char) -> Option<u64> {
    let c = match c.to_ascii_uppercase() {
        'O' => '0',
        'I' | 'L' => '1',
        other => other,
    };
    ALPHABET
        .iter()
        .position(|&a| char::from(a) == c)
        .and_then(|p| u64::try_from(p).ok())
}

/// A parsed pairing code.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PairingCode {
    /// The first 60 bits of the host's fingerprint.
    pub pin: u64,
    /// The 40-bit one-time secret.
    pub secret: u64,
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingCode(***)")
    }
}

impl PairingCode {
    /// A fresh code for this host.
    ///
    /// # Errors
    /// The OS random source failed.
    pub fn generate(host: &Fingerprint) -> Result<PairingCode, getrandom::Error> {
        let mut b = [0u8; 8];
        getrandom::fill(&mut b[3..])?;
        Ok(PairingCode {
            pin: host.pin60(),
            secret: u64::from_be_bytes(b) & 0xff_ffff_ffff,
        })
    }

    /// `XXXXX-XXXXX-XXXXX-XXXXX`.
    #[must_use]
    pub fn display(&self) -> String {
        let s = self.compact();
        format!("{}-{}-{}-{}", &s[0..5], &s[5..10], &s[10..15], &s[15..20])
    }

    /// 20 characters, no separators.
    #[must_use]
    pub fn compact(&self) -> String {
        let mut s = encode_bits(self.pin, 12);
        s.push_str(&encode_bits(self.secret, 8));
        s
    }

    /// The secret part as sent in the pair request (8 characters).
    #[must_use]
    pub fn secret_text(&self) -> String {
        encode_bits(self.secret, 8)
    }

    /// Parses what a person typed: separators and spaces are ignored, case
    /// does not matter and O/I/L read as 0/1.
    #[must_use]
    pub fn parse(text: &str) -> Option<PairingCode> {
        let chars: Vec<char> = text
            .chars()
            .filter(|c| !matches!(c, '-' | ' ' | '\t'))
            .collect();
        if chars.len() != 20 {
            return None;
        }
        let mut v: u128 = 0;
        for c in chars {
            v = (v << 5) | u128::from(decode_char(c)?);
        }
        let pin = u64::try_from(v >> 40).ok()?;
        let secret = u64::try_from(v & 0xff_ffff_ffff).ok()?;
        Some(PairingCode { pin, secret })
    }

    /// Parses the 8-character secret part.
    #[must_use]
    pub fn parse_secret(text: &str) -> Option<u64> {
        if text.chars().count() != 8 {
            return None;
        }
        text.chars()
            .try_fold(0u64, |acc, c| Some((acc << 5) | decode_char(c)?))
    }

    /// True when `fp` starts with this code's 60-bit pin.
    #[must_use]
    pub fn matches_host(&self, fp: &Fingerprint) -> bool {
        constant_time_eq_u64(self.pin, fp.pin60())
    }
}

/// Compares without an early exit.
#[must_use]
pub fn constant_time_eq_u64(a: u64, b: u64) -> bool {
    std::hint::black_box(a ^ b) == 0
}

/// Compares byte strings without an early exit on the first difference.
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_is_lowercase() {
        let fp = Fingerprint::of_der(b"cert");
        let h = fp.hex();
        assert_eq!(h.len(), 64);
        assert_eq!(h, h.to_lowercase());
        assert_eq!(Fingerprint::from_hex(&h), Some(fp));
        assert_eq!(Fingerprint::from_hex(&h.to_uppercase()), Some(fp));
        assert_eq!(Fingerprint::from_hex(&h[..63]), None);
        assert_eq!(Fingerprint::from_hex(&"g".repeat(64)), None);
    }

    #[test]
    fn code_round_trips_and_pins_the_host() {
        let host = Fingerprint::of_der(b"host-cert");
        let code = PairingCode::generate(&host).expect("random");
        assert!(code.secret < (1 << 40));
        assert_eq!(code.compact().len(), 20);
        let typed = code.display().to_lowercase().replace('0', "o");
        assert_eq!(PairingCode::parse(&typed), Some(code));
        assert!(code.matches_host(&host));
        assert!(!code.matches_host(&Fingerprint::of_der(b"other")));
        assert_eq!(
            PairingCode::parse_secret(&code.secret_text()),
            Some(code.secret)
        );
        assert_eq!(&code.compact()[..12], host.prefix());
    }

    #[test]
    fn code_parse_refuses_bad_input() {
        assert_eq!(PairingCode::parse("ABC"), None);
        assert_eq!(PairingCode::parse(&"U".repeat(20)), None);
        assert_eq!(PairingCode::parse_secret("1234567"), None);
        assert_eq!(PairingCode::parse_secret("1234567U"), None);
    }

    #[test]
    fn debug_hides_the_code() {
        let code = PairingCode::generate(&Fingerprint::of_der(b"x")).expect("random");
        assert_eq!(format!("{code:?}"), "PairingCode(***)");
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
