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
    /// U-39 item types (non-inventory, other charge, discount, subtotal),
    /// the purchase side of two-sided items and a unit-of-measure label.
    pub const ITEM_TYPES: &'static str = "item-types-v1";
    /// U-40 price levels.
    pub const PRICE_LEVELS: &'static str = "price-levels-v1";
    /// U-41 tag merge that moves line tags and defaults, audited group
    /// changes, one tag per group on every path, a working group reorder.
    pub const TAG_MERGE: &'static str = "tag-merge-v1";
    /// U-42 tag groups required for chosen account types.
    pub const TAG_REQUIRED_GROUPS: &'static str = "tag-required-groups-v1";
    /// U-75 tag numbers (Q-270): `tagNumber` on tags, unique per company
    /// (409 `TAG_NUMBER_EXISTS`), lists sorted number then name, `search`
    /// matches the number, carried by tenant archives.
    pub const TAG_NUMBER: &'static str = "tag-number-v1";
    /// U-43 editable payment terms (with discount terms) and payment
    /// methods.
    pub const TERMS_LISTS: &'static str = "terms-lists-v1";
    /// U-45 several tags on one transaction line (one per group).
    pub const LINE_TAGS: &'static str = "line-tags-v1";
    /// U-46 the item CSV import resolves the income account by name.
    pub const ITEMS_IMPORT: &'static str = "items-import-v1";
    /// P12 R1–R4, R6: exact register money (`?exact=1`), filter-independent
    /// running balances, the cleared and tag filters, the summary's cleared
    /// and ending balances, and audit before-images with lines.
    pub const REGISTER_EXACT: &'static str = "register-exact-v1";
    /// P12 R5: toggle a register line's cleared mark (assumed name).
    pub const REGISTER_CLEARED: &'static str = "register-cleared-v1";
    /// P12 R7: soft delete of a transaction (`DELETE /transactions/{id}`,
    /// engine PR #153).
    pub const TXN_DELETE: &'static str = "txn-delete-v1";

    // ---- P13 WP2: payee panel, attachments, suggestions -------------------
    /// P13 U-51: rule suggestions on a form (`POST
    /// /practice/conditional-rules/evaluate`, engine PR in progress).
    pub const RULES_EVALUATE: &'static str = "rules-evaluate-v1";

    // ---- P13 WP1: write check ---------------------------------------------
    /// P13 U-48: edit a saved check (`PUT /checks/{id}`: number, name on
    /// the check, address, Print later) and the exact line sum on
    /// `POST`/`PUT /checks`.
    pub const CHECK_EDIT: &'static str = "check-edit-v1";
    /// P13 U-49: a customer (`contactId`) and a billable mark (`isBillable`)
    /// on check and expense lines; `isBillable`/`billedStatus` on read.
    pub const LINE_BILLABLE: &'static str = "line-billable-v1";

    // ---- P14: deposits, daily sales, owner helpers (engine PR #159) -------
    /// P14 U-52: deposit lines with `sourceTransactionId` group pending
    /// payments out of Payments Clearing (409 `ALREADY_DEPOSITED`), and
    /// `GET /payments/pending-deposits?exact=1` reports string amounts.
    pub const DEPOSIT_GROUPING: &'static str = "deposit-grouping-v1";
    /// P14 U-53: deposit `cashBack` and negative (fee) line amounts.
    pub const DEPOSIT_CASHBACK: &'static str = "deposit-cashback-v1";
    /// P14 U-54: a deposit line on A/P with `sourceTransactionId` pointing
    /// at a vendor credit (a vendor refund).
    pub const VENDOR_REFUND: &'static str = "vendor-refund-v1";
    /// P14 U-55: `GET /banking/feed/{id}/deposit-groupings`.
    pub const FEED_DEPOSIT_GROUPINGS: &'static str = "feed-deposit-groupings-v1";
    /// P14 U-56: `GET /company/equity-roles` and engine-seeded owner,
    /// partner and shareholder equity accounts.
    pub const EQUITY_ROLES: &'static str = "equity-roles-v1";
    /// P14 U-57: `closingPassword` on daily-sales post and void.
    pub const DAILY_SALES_LOCK: &'static str = "daily-sales-lock-v1";

    // ---- P15: journal entries, templates, catch-up (client-declared) ------
    /// P15 part A: a name (`contactId`) on journal entry lines; an A/R line
    /// needs a customer and an A/P line a vendor
    /// (400 `JE_AR_AP_NAME_REQUIRED`), any number of such lines per entry
    /// since engine 33a973fa (older engines refused a second one with 400
    /// `JE_AR_AP_ONE_PER_ENTRY`, Q-206).
    pub const JE_NAMES: &'static str = "je-names-v1";
    /// P15 part A: `txnNumber` on journal entries and
    /// `GET /journal-entries/next-number`.
    pub const JE_NUMBER: &'static str = "je-number-v1";
    /// P15 part A: `isAdjusting` on journal entries and the report filter
    /// `adjusting=include|exclude|only`.
    pub const JE_ADJUSTING: &'static str = "je-adjusting-v1";
    /// P15 part A: `reverseOn` (after the date) creates a linked reversing
    /// entry; the read-back gives the link.
    pub const JE_REVERSING: &'static str = "je-reversing-v1";
    /// P15 part A: every posting's debits must equal its credits exactly at
    /// 4 places (each line rounded half away from zero, no 0.0001
    /// tolerance), else 400 "Transaction does not balance".
    pub const BALANCE_EXACT: &'static str = "balance-exact-v1";
    /// P15 part A (U-64): recurring copies keep line names, billable marks,
    /// a journal entry's basis and adjusting mark; `PUT /recurring/{id}`
    /// takes `expectedUpdatedAt` (409 `STALE_SCHEDULE`).
    pub const RECURRING_GUARD: &'static str = "recurring-guard-v1";
    /// P15 U-62: `/templates` (stored drafts, groups, `from-transaction`,
    /// `draft`, `enter`, `skip`, `resolve`).
    pub const TEMPLATES: &'static str = "templates-v1";
    /// P15 U-63: `POST /recurring/run-due` and `GET /recurring/attention`.
    pub const RECURRING_RUN_DUE: &'static str = "recurring-run-due-v1";
    /// U-82: `POST /recurring/run-due` takes `templateIds` (only these) and
    /// `excludeTemplateIds` (all but these); a filtered-out template is not
    /// touched and stays due (Q-PR1d).
    pub const RUN_DUE_FILTER: &'static str = "run-due-filter-v1";

    // ---- P15A: Bulk Entry (`/batch/*`; client-declared capabilities) ------
    /// P15A: `POST /batch/save` posts every row in one database
    /// transaction; a failure names `{group, rowNumber, field, message}`
    /// and saves nothing. The engine stores `source: 'batch'` and a
    /// `batchId`.
    pub const BATCH_ATOMIC: &'static str = "batch-atomic-v1";
    /// P15A: `txn_type: check` rows with `checkNumber` and `printLater`.
    pub const BATCH_CHECK: &'static str = "batch-check-v1";
    /// P15A: one call carries `groups[{txn_type, context_account_id,
    /// rows}]` (several types and accounts).
    pub const BATCH_MIXED: &'static str = "batch-mixed-v1";
    /// P15A: string amounts read as exact decimals; `parse-csv` returns
    /// string amounts and handles a BOM and quoted newlines.
    pub const BATCH_EXACT: &'static str = "batch-exact-v1";
    /// P15A: `Idempotency-Key` on `/batch/save`; a replay returns the first
    /// result.
    pub const BATCH_IDEMPOTENT: &'static str = "batch-idempotent-v1";
    /// P15A: a row's `closingPassword` lets it post on or before the
    /// closing date (audited), with the P12 closed-period errors.
    pub const BATCH_CLOSING: &'static str = "batch-closing-v1";

    // ---- P16 WP6a: Home (engine PR #168) -----------------------------------
    /// P16 U-76: `/dashboard/*` money as 4-place decimal strings with
    /// `exact=1`, `asOf` as "today", one overdue-invoice rule, cash-position
    /// account numbers, payables `dueWithinDays`.
    pub const DASHBOARD_EXACT: &'static str = "dashboard-exact-v1";

    // ---- Checkpoint PR-1 and U-47 (engine PRs #169, #170) ------------------
    /// U-80/U-81: `PUT /transactions/:id` refuses a deposit edit that drops
    /// its links (409 `DEPOSIT_LINKS_DROPPED`) and a printed check's face
    /// change (409 `CHECK_PRINTED`); the check print routes honour the
    /// closing date.
    pub const LEDGER_GUARDS: &'static str = "ledger-guards-v1";
    /// U-47: `POST /contacts/merge` and `/tags/merge` honour the closing
    /// date and take `closingPassword` (423 / 403 / 429); the tag merge
    /// also moves line tags and default-tag references.
    pub const MERGE_CLOSING_DATE: &'static str = "merge-closing-date-v1";

    // ---- P17: Enter Bill, vendor credits (engine PR-A U-83..U-87, PR-B U-89;
    // in the pinned engine f3ce33fd) ------------------------------------------
    /// U-83: bill and vendor credit create, edit, void (and bill-capture
    /// enter) honour the closing date and take `closingPassword` (423
    /// `TB_PERIOD_LOCKED`, 403 `CLOSING_PASSWORD_INVALID`), audited.
    pub const AP_CLOSING: &'static str = "ap-closing-v1";
    /// U-84: `PUT /vendor-credits/{id}` (409 `CREDIT_APPLIED`), bill and
    /// credit delete through `DELETE /transactions/{id}` when unpaid and
    /// unapplied (409 `BILL_HAS_PAYMENTS`), `expectedUpdatedAt` on edit, void
    /// and delete (409 `STALE`, the U-17 shape; plan review B3), vendor
    /// credit rows carry `vendorInvoiceNumber` (S6), vendor summary counts
    /// posted bills only.
    pub const AP_EDIT: &'static str = "ap-edit-v1";
    /// U-85: bill and credit lines with `itemId`, `quantity`, `unitPrice`
    /// (amount = qty × price rounded half away from zero, 400
    /// `LINE_AMOUNT_MISMATCH`), `contactId`, `isBillable`, `tagId`, and
    /// `draftAttachmentId` on create.
    pub const BILL_LINES: &'static str = "bill-lines-v1";
    /// U-86: the shared payment terms (with discount terms), `GET
    /// /bills/terms-dates`, `GET /payment-terms`, calendar-exact due dates.
    pub const BILL_TERMS: &'static str = "bill-terms-v1";
    /// U-87: `billReceived` on bills (false = left out of `/bills/payable`).
    pub const BILL_RECEIVED: &'static str = "bill-received-v1";
    /// U-89: no OCR on upload in desktop mode; `fieldConfidence` and line
    /// `confidence` strings on `POST /bills/extract-from-attachment/{id}`,
    /// amounts as decimal strings or null.
    pub const BILL_OCR_V2: &'static str = "bill-ocr-v2";
    /// U-83 to U-87: the engine also replays a repeated `Idempotency-Key` on
    /// the A/P document mutations (bill edit and void, vendor credit create,
    /// edit and void, bill-payment void), on top of `idempotency-v1`.
    pub const IDEMPOTENCY_AP: &'static str = "idempotency-ap-v1";
    /// U-90 (engine PR #175, pin 16aff64b): attachment routes work in the
    /// request's company (`X-Company-Id`): uploads are stamped with it, and
    /// reads, downloads, links and deletes see only that company's files and
    /// tenant-wide ones.
    pub const ATTACHMENTS_COMPANY: &'static str = "attachments-company-v1";

    // ---- A01: intake folders (engine U-93; client-declared until the pin has it)
    /// U-93: `GET`/`PUT /company/intake-settings` (the company's intake
    /// folders, a full replace guarded by `expectedUpdatedAt`, 409 `STALE`),
    /// paths never in audit rows, and `omitIntakeSettings` on the tenant
    /// archive export.
    pub const INTAKE_SETTINGS: &'static str = "intake-settings-v1";

    // ---- A03: document drafts (engine U-92; client-declared until the pin has it)
    /// U-92, U-92b: `/document-drafts` (claim an uploaded draft file, list,
    /// get, duplicates, store a reading, discard, convert into a bill, card
    /// charge, cash expense or check), `/vendor-layout-templates`,
    /// `cardLast4` on accounts, 409 `ATTACHMENT_IN_USE` on deleting a live
    /// draft's file, `omitDocumentDrafts` / `resetDocumentDrafts` on tenant
    /// archives.
    pub const DOCUMENT_DRAFTS: &'static str = "document-drafts-v1";
    /// U-92a: the engine replays a repeated `Idempotency-Key` on exactly
    /// `POST /document-drafts` and `POST /document-drafts/{id}/convert`.
    pub const IDEMPOTENCY_DOCS: &'static str = "idempotency-docs-v1";
    /// U-92d: `POST /document-reading/assist` (a second reading of the page
    /// text, and page images, by the company's AI provider; nothing stored)
    /// and the company task switches `document_text_cloud` and
    /// `document_images_cloud` (off for every company).
    pub const DOCUMENT_ASSIST: &'static str = "document-assist-v1";

    // ---- A04: attach a document draft (engine U-95)
    /// U-95: `POST /document-drafts/{id}/attach` (file a live draft's
    /// document on a posted transaction; keyed with `idempotency-docs-v1`'s
    /// contract under this name), the live-draft file guard on every plain
    /// draft-key relink with 409 `ATTACHMENT_IN_USE` on
    /// `POST /attachments/{id}/link`, and `GET /transactions?amount=` (an
    /// exact total).
    pub const DOCUMENT_DRAFTS_ATTACH: &'static str = "document-drafts-attach-v1";

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

/// `deserialize_with` for a money field that must be a decimal string
/// (`dashboard-exact-v1`, P16 D9): a JSON number, `null` or any other type
/// is a decode error, so a float amount can never be read. The text is
/// parsed exactly by [`cl_core::Money::parse_wire`].
///
/// # Errors
/// The deserializer's error for a non-string or a string outside the
/// engine money grammar.
pub fn money_str_strict<'de, D: Deserializer<'de>>(d: D) -> Result<cl_core::Money, D::Error> {
    struct StrOnly;
    impl serde::de::Visitor<'_> for StrOnly {
        type Value = cl_core::Money;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a decimal money string")
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<cl_core::Money, E> {
            cl_core::Money::parse_wire(v).map_err(E::custom)
        }
    }
    d.deserialize_str(StrOnly)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_money_reads_strings_only() {
        #[derive(Deserialize)]
        struct Row {
            #[serde(deserialize_with = "money_str_strict")]
            amount: cl_core::Money,
        }
        let read = |json: &str| serde_json::from_str::<Row>(json).map(|r| r.amount.to_wire());
        assert_eq!(
            read(r#"{"amount":"12345.6700"}"#).ok().as_deref(),
            Some("12345.6700")
        );
        assert_eq!(
            read(r#"{"amount":"-0.0100"}"#).ok().as_deref(),
            Some("-0.0100")
        );
        assert_eq!(read(r#"{"amount":"0"}"#).ok().as_deref(), Some("0.0000"));
        for bad in [
            r#"{"amount":12345.67}"#,
            r#"{"amount":12}"#,
            r#"{"amount":null}"#,
            r#"{"amount":true}"#,
            r#"{"amount":"1e3"}"#,
            r#"{"amount":"12.34567"}"#,
            r#"{"amount":" 1.00"}"#,
            r#"{"amount":""}"#,
            "{}",
        ] {
            assert!(read(bad).is_err(), "{bad}");
        }
    }

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
