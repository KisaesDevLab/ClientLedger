//! AI settings kept on this computer (P09 step 3): the provider choice
//! names, the keychain entry names, the per-company local switches, the
//! client-side monthly token cap (Q-138, Q-P02e: the engine's dollar
//! figures are floats, so the enforced cap is integer tokens), and the
//! check of an Ollama address (local or private hosts only, security M1).
//! Payload redaction before a cloud call is the engine's job (E6 strict
//! sanitizer and error scrubbing, Q-139); nothing here touches a network.

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

    /// True when `model` on this provider runs online: every model of an
    /// online provider, and an Ollama model whose name ends in `cloud`
    /// after `:` or `-` (`gpt-oss:120b-cloud`, `kimi-k2:cloud`), which
    /// Ollama runs on its own servers (A03 review S5, engine #179 M2).
    #[must_use]
    pub fn runs_online(self, model: Option<&str>) -> bool {
        self.is_cloud() || model.is_some_and(is_ollama_cloud_model)
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

/// An Ollama model name matching `(^|[:-])cloud$`, any case.
#[must_use]
pub fn is_ollama_cloud_model(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.strip_suffix("cloud")
        .is_some_and(|rest| rest.is_empty() || rest.ends_with(':') || rest.ends_with('-'))
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

// ------------------------------------------------------------ ollama url

/// Why an Ollama address was refused (security M1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OllamaUrlError {
    /// Not `http://host[:port][/path]` or `https://…`.
    #[error("not an http or https address")]
    Scheme,
    /// A user name or password in the URL.
    #[error("credentials in the address")]
    Credentials,
    /// Not a local or private network host.
    #[error("not a local or private address")]
    NotLocal,
    /// Not a well-formed address.
    #[error("malformed address")]
    Malformed,
}

/// Parses an IPv4 dotted quad.
fn ipv4(host: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut out = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        out[i] = p.parse().ok()?;
    }
    Some(out)
}

/// True for a host this computer may reach without the internet: loopback,
/// `localhost`, RFC 1918 private ranges, CGNAT (100.64/10), IPv6 loopback
/// and unique-local (`fc00::/7`), or a `.local` / single-label name.
#[must_use]
pub fn is_local_host(host: &str) -> bool {
    let h = host
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    if h.is_empty() {
        return false;
    }
    if h == "localhost"
        || h.rsplit_once('.')
            .is_some_and(|(_, tld)| tld == "local" || tld == "localhost")
    {
        return true;
    }
    if let Some([a, b, _, _]) = ipv4(&h) {
        return a == 127
            || a == 10
            || (a == 172 && (16..=31).contains(&b))
            || (a == 192 && b == 168)
            || (a == 100 && (64..=127).contains(&b));
    }
    if h.contains(':') {
        // IPv6: loopback or unique-local only.
        return h == "::1" || h.starts_with("fc") || h.starts_with("fd");
    }
    // A bare machine name (no dot) resolves on the LAN, never on the
    // internet; it needs a letter so an all-digit or `0x…` form cannot
    // normalise to a public IPv4 address.
    !h.contains('.')
        && h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && h.bytes().any(|b| b.is_ascii_alphabetic())
        && !h.starts_with("0x")
}

/// Checks an Ollama address: `http`/`https`, no credentials, and a host
/// that is loopback, a private network, or a local name. The engine makes
/// the same check (`validateOllamaBaseUrl`); this refuses earlier, before
/// the address is saved or sent. Returns the trimmed address.
///
/// # Errors
/// [`OllamaUrlError`].
pub fn validate_ollama_url(raw: &str) -> Result<String, OllamaUrlError> {
    let url = raw.trim();
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or(OllamaUrlError::Scheme)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(OllamaUrlError::Credentials);
    }
    if authority.is_empty()
        || authority
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(OllamaUrlError::Malformed);
    }
    let host = if let Some(end) = authority.strip_prefix('[').and_then(|a| a.find(']')) {
        &authority[1..=end]
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, port)| {
            if port.bytes().all(|b| b.is_ascii_digit()) && !port.is_empty() {
                h
            } else {
                authority
            }
        })
    };
    if host.is_empty() {
        return Err(OllamaUrlError::Malformed);
    }
    if !is_local_host(host) {
        return Err(OllamaUrlError::NotLocal);
    }
    Ok(url.to_string())
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

    /// Review S5: Ollama's cloud models run online.
    #[test]
    fn ollama_cloud_models_run_online() {
        for m in [
            "cloud",
            "kimi-k2:cloud",
            "gpt-oss:120b-cloud",
            "Qwen3-Coder:480B-CLOUD",
        ] {
            assert!(is_ollama_cloud_model(m), "{m}");
            assert!(AiProvider::Ollama.runs_online(Some(m)), "{m}");
        }
        for m in [
            "llama3.1:8b",
            "soundcloud",
            "cloudy:7b",
            "mycloud",
            "cloud:7b",
            "",
        ] {
            assert!(!is_ollama_cloud_model(m), "{m}");
        }
        assert!(!AiProvider::Ollama.runs_online(Some("llama3.1:8b")));
        assert!(!AiProvider::Ollama.runs_online(None));
        assert!(AiProvider::Anthropic.runs_online(None));
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
    fn ollama_addresses_must_be_local_and_clean() {
        for ok in [
            "http://localhost:11434",
            "https://127.0.0.1:11434/",
            "http://10.0.0.5:11434",
            "http://172.20.1.2:11434/v1",
            "http://192.168.1.40:11434",
            "http://100.64.3.3:11434",
            "http://[::1]:11434",
            "http://[fd00::1]:11434",
            "http://ollama-box:11434",
            "http://studio.local:11434",
            "http://localhost",
        ] {
            assert_eq!(validate_ollama_url(ok).as_deref(), Ok(ok), "{ok}");
        }
        let cases = [
            ("ftp://localhost:11434", OllamaUrlError::Scheme),
            ("localhost:11434", OllamaUrlError::Scheme),
            (
                "http://user:pw@localhost:11434",
                OllamaUrlError::Credentials,
            ),
            ("http://api.example.com:11434", OllamaUrlError::NotLocal),
            ("http://8.8.8.8:11434", OllamaUrlError::NotLocal),
            ("http://172.32.0.1:11434", OllamaUrlError::NotLocal),
            ("http://[2001:db8::1]:11434", OllamaUrlError::NotLocal),
            ("http://169.254.169.254/latest", OllamaUrlError::NotLocal),
            ("http://", OllamaUrlError::Malformed),
            ("http://2130706433:11434", OllamaUrlError::NotLocal),
            ("http://0x7f000001:11434", OllamaUrlError::NotLocal),
            ("http://0xC0A80105:11434", OllamaUrlError::NotLocal),
            ("http://local host:11434", OllamaUrlError::Malformed),
        ];
        for (bad, why) in cases {
            assert_eq!(validate_ollama_url(bad), Err(why), "{bad}");
        }
        assert!(!is_local_host(""));
    }
}
