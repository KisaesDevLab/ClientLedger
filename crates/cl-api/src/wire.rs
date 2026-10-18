//! Shared wire types: engine identity, capabilities, health, pages and
//! report amounts.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use cl_core::Timestamp;
use cl_core::ids::TenantId;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use crate::errors::{ApiError, LocalValidation};

/// Capability strings the engine advertises in `/desktop/info.capabilities`
/// (introduced by U-21; a missing array means none).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Capabilities(BTreeSet<String>);

impl Capabilities {
    /// U-17 record versions and `If-Match`.
    pub const RECORD_VERSION: &'static str = "record-version-v1";
    /// U-18 request idempotency.
    pub const IDEMPOTENCY: &'static str = "idempotency-v1";
    /// U-19 strict money validation.
    pub const MONEY_STRICT: &'static str = "money-strict-v1";
    /// U-21 stable error codes.
    pub const ERROR_CODES: &'static str = "error-codes-v1";
    /// E5a `GET /api/v1/tenants`.
    pub const TENANTS_LIST: &'static str = "tenants-list-v1";
    /// U-15 closing-date password.
    pub const CLOSING_PASSWORD: &'static str = "closing-password-v1";
    /// E5a tenant archives (export job, import, rename).
    pub const TENANT_ARCHIVE: &'static str = "tenant-archive-v1";
    /// E5b safe tenant delete.
    pub const TENANT_DELETE: &'static str = "tenant-delete-v1";
    /// U-23 desktop recovery key.
    pub const RECOVERY_KEY: &'static str = "recovery-key-v1";
    /// E6 per-session secrets (AI keys, SMTP password; U-09, U-13).
    pub const SESSION_SECRETS: &'static str = "session-secrets-v1";
    /// E6 desktop AI: provider allowlist, credential resolution, usage
    /// stamps and the integer usage summary (U-09).
    pub const AI_DESKTOP: &'static str = "ai-desktop-v1";
    /// E6 SMTP password from the session secret (U-13).
    pub const SMTP_SESSION: &'static str = "smtp-session-v1";
    /// U-29 monthly token cap enforced by the engine (`monthlyTokenLimit`).
    pub const AI_TOKEN_CAP: &'static str = "ai-token-cap-v1";
    /// U-31 account merge that moves the journal lines (`/accounts/merge`).
    pub const ACCOUNT_MERGE: &'static str = "account-merge-v1";
    /// U-32 checked sub-accounts (same type, no cycles) and an engine
    /// subtree balance.
    pub const ACCOUNT_TREE: &'static str = "account-tree-v1";
    /// U-33 per-account sensitive flag stored and enforced by the engine.
    pub const ACCOUNT_SENSITIVE: &'static str = "account-sensitive-v1";
    /// U-34 masked bank/card number with an audited Admin reveal.
    pub const ACCOUNT_MASKED_NUMBER: &'static str = "account-masked-number-v1";
    /// U-35 jobs under a client (status, type, start and end dates).
    pub const CONTACT_JOBS: &'static str = "contact-jobs-v1";
    /// U-36 several ship-to addresses (one default) and contact people.
    pub const CONTACT_ADDRESSES: &'static str = "contact-addresses-v1";
    /// U-37 dated notes and to-dos on contacts and jobs.
    pub const CONTACT_NOTES: &'static str = "contact-notes-v1";
    /// U-38 the engine's open balance on each contact read.
    pub const CONTACT_OPEN_BALANCE: &'static str = "contact-open-balance-v1";
    /// U-44 "W-9 on file" recorded by the owner (a paper W-9).
    pub const VENDOR_W9_ON_FILE: &'static str = "vendor-w9-on-file-v1";

    /// True when the engine advertises `name`.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    /// Every advertised capability.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

impl<S: Into<String>> FromIterator<S> for Capabilities {
    fn from_iter<I: IntoIterator<Item = S>>(iter: I) -> Self {
        Capabilities(iter.into_iter().map(Into::into).collect())
    }
}

/// `GET /api/v1/desktop/info` (the fields cl-api uses; others are ignored).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopInfo {
    /// Engine version string.
    #[serde(default)]
    pub version: Option<String>,
    /// Engine source commit.
    #[serde(default)]
    pub engine_commit: Option<String>,
    /// Whether a user exists (`null` while the database is unavailable).
    #[serde(default)]
    pub initialized: Option<bool>,
    /// The hidden owner workspace tenant (Q-P02h); `null` on older engines.
    #[serde(default)]
    pub workspace_tenant_id: Option<TenantId>,
    /// Advertised capabilities.
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// `GET /health` (desktop fields; others are ignored).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// `ok`, `degraded`, …
    #[serde(default)]
    pub status: Option<String>,
    /// Internet reachability; `None` means unknown, not offline (E4).
    #[serde(default)]
    pub online: Option<bool>,
    /// When reachability was last checked.
    #[serde(default)]
    pub last_online_check: Option<Timestamp>,
}

/// One page of a list (`{data, total}`; tags answer `{tags, total, …}` and
/// are adapted into the same shape).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    /// The rows of this page.
    pub data: Vec<T>,
    /// Rows matching the filter in all pages.
    pub total: u64,
}

impl<T: DeserializeOwned> Page<T> {
    /// Reads a page whose rows sit under `key` (`data` for most resources,
    /// `tags` for tags). `total` must be a JSON integer.
    ///
    /// # Errors
    /// `Decode` when the body is not that shape.
    pub fn from_json(body: &[u8], key: &str) -> Result<Page<T>, ApiError> {
        let mut obj: Map<String, Value> =
            serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: "page" })?;
        let total = obj
            .get("total")
            .and_then(Value::as_u64)
            .ok_or(ApiError::Decode { what: "page total" })?;
        let rows = obj
            .remove(key)
            .ok_or(ApiError::Decode { what: "page rows" })?;
        let data =
            serde_json::from_value(rows).map_err(|_| ApiError::Decode { what: "page rows" })?;
        Ok(Page { data, total })
    }
}

/// Paging of a list request: `limit` 1..=500 (the engine's maximum).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    limit: u16,
    offset: u64,
}

impl PageRequest {
    /// The largest page the engine serves for core lists.
    pub const MAX_LIMIT: u16 = 500;

    /// A page of `limit` rows starting at `offset`.
    ///
    /// # Errors
    /// `Local(PageLimit)` when `limit` is 0 or above 500.
    pub fn new(limit: u16, offset: u64) -> Result<PageRequest, ApiError> {
        if limit == 0 || limit > Self::MAX_LIMIT {
            return Err(ApiError::Local(LocalValidation::PageLimit));
        }
        Ok(PageRequest { limit, offset })
    }

    /// The first page of the largest size.
    #[must_use]
    pub fn first_max() -> PageRequest {
        PageRequest {
            limit: Self::MAX_LIMIT,
            offset: 0,
        }
    }

    /// Rows per page.
    #[must_use]
    pub fn limit(&self) -> u16 {
        self.limit
    }

    /// Rows skipped.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// The next page.
    #[must_use]
    pub fn next(&self) -> PageRequest {
        PageRequest {
            limit: self.limit,
            offset: self.offset + u64::from(self.limit),
        }
    }

    /// `limit` and `offset` query pairs (same casing for every resource).
    #[must_use]
    pub fn query(&self) -> Vec<(String, String)> {
        vec![
            ("limit".into(), self.limit.to_string()),
            ("offset".into(), self.offset.to_string()),
        ]
    }
}

/// A number from an engine report, kept as the exact JSON text the engine
/// sent (until U-14 makes report money decimal strings).
///
/// Display only: there is deliberately no `PartialEq`, `Eq`, `Ord`, `Hash`,
/// arithmetic or conversion to `Money`, so a report value can never feed a
/// posting or an equality check. P&L totals are float sums in the engine
/// today, so texts like `0.30000000000000004` arrive; [`ReportAmount::display`]
/// is therefore approximate until U-14.
#[derive(Clone)]
pub struct ReportAmount(Box<RawValue>);

impl ReportAmount {
    /// The number text (a JSON number as sent, or the contents of a JSON
    /// string).
    #[must_use]
    pub fn text(&self) -> &str {
        let raw = self.0.get();
        raw.strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or(raw)
    }

    /// Rounded to `dp` decimals, half away from zero, never `-0`. `None` when
    /// the text is not a number `Decimal` can hold.
    #[must_use]
    pub fn display(&self, dp: u32) -> Option<String> {
        let text = self.text();
        let parsed = if text.contains(['e', 'E']) {
            Decimal::from_scientific(text)
        } else {
            Decimal::from_str(text)
        };
        let mut d = parsed
            .ok()?
            .round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero);
        if d.is_zero() {
            d.set_sign_positive(true);
        }
        let width = usize::try_from(dp).ok()?;
        Some(format!("{d:.width$}"))
    }
}

/// Prints the amount text itself: owner data. Never log a value with
/// `?value` at info level or above.
impl fmt::Debug for ReportAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ReportAmount({})", self.text())
    }
}

impl<'de> Deserialize<'de> for ReportAmount {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(d)?;
        let text = raw.get();
        let body = text
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or(text);
        let numeric = !body.is_empty()
            && body
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E'));
        if numeric {
            Ok(ReportAmount(raw))
        } else {
            Err(serde::de::Error::invalid_type(
                serde::de::Unexpected::Other("a non-numeric value"),
                &"a JSON number or decimal string",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_info_tolerates_missing_and_extra_fields() {
        let info: DesktopInfo = serde_json::from_str(
            r#"{"mode":"desktop","version":"1.2.3","engineCommit":null,"offline":"auto",
                "initialized":true,"workspaceTenantId":"6f1c1e0e-1d7f-4c55-9e3e-1b2f7f3f9a10",
                "capabilities":["idempotency-v1","error-codes-v1"]}"#,
        )
        .expect("info");
        assert!(info.capabilities.has(Capabilities::IDEMPOTENCY));
        assert!(!info.capabilities.has(Capabilities::RECORD_VERSION));
        assert_eq!(info.capabilities.iter().count(), 2);
        assert!(info.workspace_tenant_id.is_some());
        let old: DesktopInfo = serde_json::from_str(r#"{"workspaceTenantId":null}"#).expect("old");
        assert_eq!(old.capabilities, Capabilities::default());
        assert_eq!(old.workspace_tenant_id, None);
    }

    #[test]
    fn health_online_null_is_unknown() {
        let h: Health = serde_json::from_str(r#"{"status":"ok","online":null}"#).expect("h");
        assert_eq!(h.online, None);
        let h: Health = serde_json::from_str(
            r#"{"status":"ok","online":false,"lastOnlineCheck":"2026-10-15T08:00:00.000Z"}"#,
        )
        .expect("h");
        assert_eq!(h.online, Some(false));
        assert!(h.last_online_check.is_some());
        let caps: Capabilities = ["a", "b"].into_iter().collect();
        assert!(caps.has("a"));
    }

    #[test]
    fn report_amounts_display_exactly_or_approximately() {
        #[derive(Deserialize)]
        struct Row {
            amount: ReportAmount,
            label: String,
        }
        let cases = [
            ("0.30000000000000004", 2, "0.30"),
            ("1e-7", 2, "0.00"),
            ("-0", 2, "0.00"),
            ("1e21", 2, "1000000000000000000000.00"),
            ("\"1234.5650\"", 2, "1234.57"),
            ("-2.345", 2, "-2.35"),
            ("-0.004", 2, "0.00"),
            ("12", 0, "12"),
        ];
        for (json, dp, want) in cases {
            let a: ReportAmount = serde_json::from_str(json).expect(json);
            assert_eq!(a.display(dp).as_deref(), Some(want), "{json}");
        }
        let big: ReportAmount = serde_json::from_str("1e300").expect("parses");
        assert_eq!(big.display(2), None);
        assert_eq!(big.text(), "1e300");
        for bad in ["null", "true", "{}", "[1]", "\"abc\"", "\"\""] {
            assert!(serde_json::from_str::<ReportAmount>(bad).is_err(), "{bad}");
        }
        let row: Row = serde_json::from_str(r#"{"label":"Sales","amount":1234.5}"#).expect("row");
        assert_eq!(row.amount.text(), "1234.5");
        assert_eq!(row.label, "Sales");
        assert_eq!(format!("{:?}", row.amount), "ReportAmount(1234.5)");
    }

    #[test]
    fn pages_and_page_requests() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct R {
            id: u8,
        }
        let p: Page<R> =
            Page::from_json(br#"{"data":[{"id":1}],"total":7}"#, "data").expect("page");
        assert_eq!(
            p,
            Page {
                data: vec![R { id: 1 }],
                total: 7
            }
        );
        let t: Page<R> =
            Page::from_json(br#"{"tags":[],"total":0,"limit":200,"offset":0}"#, "tags")
                .expect("tags");
        assert_eq!(t.total, 0);
        assert_eq!(t.data.len(), 0);
        for bad in [
            &br#"{"data":[],"total":"3"}"#[..],
            br#"{"total":1}"#,
            b"[]",
            br#"{"data":[{"id":"x"}],"total":1}"#,
        ] {
            assert!(Page::<R>::from_json(bad, "data").is_err());
        }
        assert!(PageRequest::new(0, 0).is_err());
        assert!(PageRequest::new(501, 0).is_err());
        let r = PageRequest::new(500, 1000).expect("ok");
        assert_eq!(r.next().offset(), 1500);
        assert_eq!(r.limit(), 500);
        assert_eq!(
            PageRequest::first_max().query(),
            vec![
                ("limit".to_string(), "500".to_string()),
                ("offset".to_string(), "0".to_string())
            ]
        );
    }
}
