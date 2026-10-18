//! AI settings kept on this computer (P09 step 3): the provider choice
//! names, the keychain entry names, the per-company local switches, the
//! client-side monthly token cap (Q-138, Q-P02e: the engine's dollar
//! figures are floats, so the enforced cap is integer tokens), and the
//! redaction pass applied to anything the app would send to a cloud model
//! (Q-139). No I/O here; nothing in this module touches a network.

use serde::{Deserialize, Serialize};

/// Keychain entry for the Anthropic key.
pub const ANTHROPIC_KEY: &str = "ai-key-anthropic";
/// Keychain entry for the `OpenAI` key.
pub const OPENAI_KEY: &str = "ai-key-openai";

/// Where AI runs (Q-33: Anthropic, `OpenAI`, or a local Ollama).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiProvider {
    /// On this computer, no key, works offline.
    #[default]
    Ollama,
    /// Anthropic with the owner's key (online).
    Anthropic,
    /// `OpenAI` with the owner's key (online).
    OpenAi,
}

impl AiProvider {
    /// Every provider, in the order the panel shows them.
    pub const ALL: [AiProvider; 3] = [
        AiProvider::Ollama,
        AiProvider::Anthropic,
        AiProvider::OpenAi,
    ];

    /// The engine's provider name.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            AiProvider::Ollama => "ollama",
            AiProvider::Anthropic => "anthropic",
            AiProvider::OpenAi => "openai",
        }
    }

    /// Parses the engine's provider name.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<AiProvider> {
        match s {
            "ollama" => Some(AiProvider::Ollama),
            "anthropic" => Some(AiProvider::Anthropic),
            "openai" => Some(AiProvider::OpenAi),
            _ => None,
        }
    }

    /// True for a provider that needs the internet and a key.
    #[must_use]
    pub fn is_cloud(self) -> bool {
        !matches!(self, AiProvider::Ollama)
    }

    /// The keychain entry holding this provider's key (none for Ollama).
    #[must_use]
    pub fn key_name(self) -> Option<&'static str> {
        match self {
            AiProvider::Ollama => None,
            AiProvider::Anthropic => Some(ANTHROPIC_KEY),
            AiProvider::OpenAi => Some(OPENAI_KEY),
        }
    }

    /// i18n key of the name.
    #[must_use]
    pub fn label_key(self) -> &'static str {
        match self {
            AiProvider::Ollama => "prefs.ai.provider_ollama",
            AiProvider::Anthropic => "prefs.ai.provider_anthropic",
            AiProvider::OpenAi => "prefs.ai.provider_openai",
        }
    }
}

/// Default Ollama address.
pub const OLLAMA_DEFAULT_URL: &str = "http://localhost:11434";

/// Default monthly token cap for cloud providers (about a few dollars at
/// 2026 list prices; the owner changes it in Preferences).
pub const DEFAULT_MONTHLY_TOKEN_CAP: u64 = 1_000_000;

/// Tokens counted so far in one calendar month.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenUsage {
    /// `YYYY-MM` (UTC).
    pub month: String,
    /// Input plus output tokens of cloud calls this app made.
    pub tokens: u64,
}

/// Per-company AI choices the engine has no column for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiLocalPrefs {
    /// Offer a Bank Feed rule after repeated accepts (Q-141).
    pub suggest_rules: bool,
    /// Monthly cloud token cap; `None` means no cap.
    pub monthly_token_cap: Option<u64>,
    /// What this app counted this month (from `usage` on AI replies).
    pub usage: TokenUsage,
}

impl Default for AiLocalPrefs {
    fn default() -> Self {
        AiLocalPrefs {
            suggest_rules: false,
            monthly_token_cap: Some(DEFAULT_MONTHLY_TOKEN_CAP),
            usage: TokenUsage::default(),
        }
    }
}

/// Whether a cloud call may go ahead under the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapCheck {
    /// Under the cap (or no cap).
    Allowed,
    /// At or over the cap: no further cloud call this month.
    Blocked {
        /// Tokens counted this month.
        used: u64,
        /// The cap.
        cap: u64,
    },
}

impl AiLocalPrefs {
    /// Tokens counted in `month` (zero for any other month).
    #[must_use]
    pub fn used_in(&self, month: &str) -> u64 {
        if self.usage.month == month {
            self.usage.tokens
        } else {
            0
        }
    }

    /// Whether a cloud call may start in `month` (Q-138). The check is
    /// "already at the cap", so one call may finish past it; the next is
    /// refused.
    #[must_use]
    pub fn check(&self, month: &str) -> CapCheck {
        let used = self.used_in(month);
        match self.monthly_token_cap {
            Some(cap) if used >= cap => CapCheck::Blocked { used, cap },
            _ => CapCheck::Allowed,
        }
    }

    /// Counts a finished call's tokens into `month` (a new month starts
    /// the count over).
    pub fn record(&mut self, month: &str, input_tokens: u64, output_tokens: u64) {
        if self.usage.month != month {
            self.usage = TokenUsage {
                month: month.to_string(),
                tokens: 0,
            };
        }
        self.usage.tokens = self
            .usage
            .tokens
            .saturating_add(input_tokens)
            .saturating_add(output_tokens);
    }

    /// Fraction of the cap used, in whole percent (0 when there is no cap).
    #[must_use]
    pub fn percent_used(&self, month: &str) -> u8 {
        match self.monthly_token_cap {
            Some(cap) if cap > 0 => {
                let used = self.used_in(month).min(cap);
                u8::try_from(used.saturating_mul(100) / cap).unwrap_or(100)
            }
            _ => 0,
        }
    }
}

/// `YYYY-MM` for a UTC timestamp's year and month.
#[must_use]
pub fn month_key(year: i32, month: u32) -> String {
    format!("{year:04}-{month:02}")
}

// ---------------------------------------------------------------- redaction

/// What a redacted span was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Redacted {
    /// An email address.
    Email,
    /// A phone number.
    Phone,
    /// An EIN or SSN.
    TaxId,
    /// A long digit run (account, card or routing number).
    Number,
}

impl Redacted {
    fn placeholder(self) -> &'static str {
        match self {
            Redacted::Email => "[email]",
            Redacted::Phone => "[phone]",
            Redacted::TaxId => "[tax id]",
            Redacted::Number => "[number]",
        }
    }
}

/// Output of [`redact`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Redaction {
    /// The text with placeholders.
    pub text: String,
    /// What was replaced, in order (counts only; never the values).
    pub kinds: Vec<Redacted>,
}

fn is_sep(c: char) -> bool {
    matches!(c, ' ' | '-' | '.' | '(' | ')' | ',')
}

/// Classifies a digit-and-separator run by its shape.
fn classify_run(run: &str, digits: usize) -> Option<Redacted> {
    let b = run.as_bytes();
    // EIN `12-3456789`, SSN `123-45-6789`.
    let ein = run.len() == 10 && b[2] == b'-' && run.chars().filter(|c| *c == '-').count() == 1;
    let ssn = run.len() == 11 && b[3] == b'-' && b[6] == b'-';
    if digits == 9 && (ein || ssn) {
        return Some(Redacted::TaxId);
    }
    // A date such as 2026-10-31 (eight digits, two dashes) is not a number.
    if run.len() == 10 && digits == 8 && b[4] == b'-' && b[7] == b'-' {
        return None;
    }
    let dashes_or_spaces = run
        .chars()
        .filter(|c| matches!(c, '-' | ' ' | '(' | ')' | '.'))
        .count();
    if (10..=11).contains(&digits) && dashes_or_spaces > 0 && !run.contains(',') {
        return Some(Redacted::Phone);
    }
    if digits >= 8 {
        return Some(Redacted::Number);
    }
    None
}

/// Removes emails, phone numbers, tax IDs and long digit runs from `text`
/// before it could be sent to a cloud model (Q-139). Amounts are never in
/// these payloads to begin with (non-negotiable 4); short numbers such as
/// check numbers and dates stay.
#[must_use]
pub fn redact(text: &str) -> Redaction {
    let mut out = String::with_capacity(text.len());
    let mut kinds = Vec::new();
    // Pass 1: emails, token by token.
    let mut emailed = String::with_capacity(text.len());
    let mut token = String::new();
    let flush = |token: &mut String, emailed: &mut String, kinds: &mut Vec<Redacted>| {
        if token.is_empty() {
            return;
        }
        let trimmed = token.trim_end_matches(['.', ',', ';', ':', ')']);
        let tail = &token[trimmed.len()..];
        if let Some(at) = trimmed.find('@')
            && at > 0
            && trimmed[at + 1..].contains('.')
            && !trimmed[at + 1..].starts_with('.')
        {
            emailed.push_str(Redacted::Email.placeholder());
            emailed.push_str(tail);
            kinds.push(Redacted::Email);
        } else {
            emailed.push_str(token);
        }
        token.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() {
            flush(&mut token, &mut emailed, &mut kinds);
            emailed.push(c);
        } else {
            token.push(c);
        }
    }
    flush(&mut token, &mut emailed, &mut kinds);

    // Pass 2: digit runs with their separators.
    let chars: Vec<char> = emailed.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() || (c == '(' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let start = i;
            let mut end = i;
            let mut digits = 0;
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_digit() || is_sep(chars[j])) {
                if chars[j].is_ascii_digit() {
                    digits += 1;
                    end = j + 1;
                } else if chars[j] == ')' {
                    end = j + 1;
                }
                j += 1;
            }
            let run: String = chars[start..end].iter().collect();
            match classify_run(&run, digits) {
                Some(kind) => {
                    out.push_str(kind.placeholder());
                    kinds.push(kind);
                }
                None => out.push_str(&run),
            }
            i = end;
        } else {
            out.push(c);
            i += 1;
        }
    }
    Redaction { text: out, kinds }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_have_wire_names_keys_and_labels() {
        for p in AiProvider::ALL {
            assert_eq!(AiProvider::from_wire(p.wire()), Some(p));
            assert_eq!(p.is_cloud(), p.key_name().is_some());
            assert!(p.label_key().starts_with("prefs.ai.provider_"));
        }
        assert_eq!(AiProvider::from_wire("gemini"), None);
        assert_eq!(AiProvider::Anthropic.key_name(), Some(ANTHROPIC_KEY));
        assert_eq!(AiProvider::default(), AiProvider::Ollama);
    }

    #[test]
    fn the_cap_blocks_the_next_call_once_reached() {
        let mut p = AiLocalPrefs {
            monthly_token_cap: Some(1000),
            ..AiLocalPrefs::default()
        };
        let m = month_key(2026, 10);
        assert_eq!(m, "2026-10");
        assert_eq!(p.check(&m), CapCheck::Allowed);
        p.record(&m, 400, 199);
        assert_eq!(p.used_in(&m), 599);
        assert_eq!(p.percent_used(&m), 59);
        assert_eq!(p.check(&m), CapCheck::Allowed);
        p.record(&m, 300, 101);
        assert_eq!(p.used_in(&m), 1000);
        assert_eq!(
            p.check(&m),
            CapCheck::Blocked {
                used: 1000,
                cap: 1000
            }
        );
        assert_eq!(p.percent_used(&m), 100);
        // A new month starts over; another month's count is zero.
        assert_eq!(p.used_in("2026-11"), 0);
        assert_eq!(p.check("2026-11"), CapCheck::Allowed);
        p.record("2026-11", 1, 1);
        assert_eq!(p.used_in("2026-11"), 2);
        assert_eq!(p.used_in(&m), 0);
        // No cap: never blocked.
        p.monthly_token_cap = None;
        p.record("2026-11", u64::MAX, 5);
        assert_eq!(p.check("2026-11"), CapCheck::Allowed);
        assert_eq!(p.percent_used("2026-11"), 0);
        assert_eq!(
            AiLocalPrefs::default().monthly_token_cap,
            Some(DEFAULT_MONTHLY_TOKEN_CAP)
        );
    }

    #[test]
    fn redaction_removes_emails_phones_tax_ids_and_account_numbers() {
        let cases: [(&str, &str, &[Redacted]); 9] = [
            (
                "Call Ozark Supply at (417) 555-0133 or 417-555-0134.",
                "Call Ozark Supply at [phone] or [phone].",
                &[Redacted::Phone, Redacted::Phone],
            ),
            (
                "Send the W-9 to books@ozarkridge.example, thanks.",
                "Send the W-9 to [email], thanks.",
                &[Redacted::Email],
            ),
            (
                "EIN 12-3456789 and SSN 123-45-6789 on file",
                "EIN [tax id] and SSN [tax id] on file",
                &[Redacted::TaxId, Redacted::TaxId],
            ),
            (
                "Acct 4532 0151 1283 0366 routing 081000210",
                "Acct [number] routing [number]",
                &[Redacted::Number, Redacted::Number],
            ),
            (
                "Check 1042 dated 2026-10-31 for invoice INV-2041",
                "Check 1042 dated 2026-10-31 for invoice INV-2041",
                &[],
            ),
            (
                "Fuel 10/31/2026 ref 55512",
                "Fuel 10/31/2026 ref 55512",
                &[],
            ),
            (
                "Card ending 0366, memo 123456789012",
                "Card ending 0366, memo [number]",
                &[Redacted::Number],
            ),
            ("", "", &[]),
            (
                "Mixed: kurt@example.com 555-0100 12345678",
                // Adjacent digit groups merge into one run: over-redaction
                // is the safe side.
                "Mixed: [email] [number]",
                &[Redacted::Email, Redacted::Number],
            ),
        ];
        for (input, want, kinds) in cases {
            let r = redact(input);
            assert_eq!(r.text, want, "{input}");
            assert_eq!(r.kinds, kinds, "{input}");
        }
    }

    #[test]
    fn redaction_fixtures_leave_no_identifiers_behind() {
        let fixtures = include_str!("../tests/fixtures/redaction.txt");
        let mut lines = 0;
        for line in fixtures
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            lines += 1;
            let r = redact(line).text;
            assert!(!r.contains('@'), "email left in {r:?}");
            let longest_digit_run = r
                .split(|c: char| !c.is_ascii_digit())
                .map(str::len)
                .max()
                .unwrap_or(0);
            assert!(longest_digit_run <= 7, "long digit run left in {r:?}");
            assert!(!r.contains("555-01"), "phone left in {r:?}");
            assert!(!r.contains("-45-"), "ssn left in {r:?}");
        }
        assert!(lines >= 12, "fixture lines: {lines}");
    }
}
