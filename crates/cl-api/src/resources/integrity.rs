//! Verify Data (P34, engine U-12 `integrity-v1`): company-wide integrity
//! checks, their stored runs, and the one kind of fix v1 offers
//! (recomputing an account's cached balance).
//!
//! - `POST /api/v1/integrity/checks` `{level}` runs the checks of one level
//!   synchronously (the engine's statement budget is 120 s) and stores the
//!   run; it answers the run.
//! - `POST /api/v1/integrity/fixes` `{runId, findingIds, expectedLedgerVersion,
//!   backupArchiveId?}` fixes the chosen findings. 409 `INTEGRITY_STALE` when
//!   the books moved since the run ([`is_integrity_stale`]); 404
//!   `INTEGRITY_RUN_UNKNOWN`. Each finding is re-checked under lock, so a
//!   repeat is harmless. In LAN mode it is a maintenance route: call it under
//!   a routine hold.
//! - `GET /api/v1/integrity/summary` (any member) and
//!   `GET /api/v1/integrity/runs/{id}`.
//!
//! Typed ahead of the engine merge (plan P34 §2): every call is refused
//! locally (`Local(Unsupported)`, nothing sent) until the engine advertises
//! `integrity-v1`.
//!
//! **Money:** every amount is the engine's exact decimal string read into
//! [`Money`] (a JSON number is refused). Pass, fail and differences come
//! from the engine; the app never subtracts or compares amounts here
//! (non-negotiable 7). A finding's subject label and amounts are owner data:
//! `Debug` shows only codes and ids.

use cl_core::Money;
use cl_core::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::{
    INTEGRITY_CHECKS, INTEGRITY_FIXES, INTEGRITY_RUN, INTEGRITY_SUMMARY,
};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode, json_body};
use crate::wire::Capabilities;

/// Most findings one run lists (the engine's cap); `checks[].found` stays
/// exact beyond it.
pub const MAX_FINDINGS: usize = 500;

/// How deep a check goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntegrityLevel {
    /// The weekly check: unbalanced entries, cached balances, drafts.
    Quick,
    /// Everything: also line scopes, A/R and A/P control accounts and the
    /// inventory value.
    Full,
}

/// An id the engine hands out and the app sends back in a path or body:
/// `[A-Za-z0-9_:-]{1,128}`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IntegrityId(String);

impl IntegrityId {
    /// Accepts `[A-Za-z0-9_:-]{1,128}`.
    ///
    /// # Errors
    /// `Local(InvalidValue)` otherwise.
    pub fn parse(s: &str) -> Result<IntegrityId, ApiError> {
        let ok = (1..=128).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b':');
        if ok {
            Ok(IntegrityId(s.to_string()))
        } else {
            Err(ApiError::Local(LocalValidation::InvalidValue))
        }
    }

    /// The id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for IntegrityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for IntegrityId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IntegrityId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        IntegrityId::parse(&s).map_err(|_| serde::de::Error::custom("integrity id"))
    }
}

/// The ledger version a run saw (the engine's per-company counter; only
/// equality means anything). Read from a JSON number or a decimal string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RunLedgerVersion(u64);

impl RunLedgerVersion {
    /// The counter, as sent back in `expectedLedgerVersion`.
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for RunLedgerVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            N(u64),
            S(String),
        }
        match Wire::deserialize(d)? {
            Wire::N(n) => Ok(RunLedgerVersion(n)),
            Wire::S(s) => s
                .parse::<u64>()
                .map(RunLedgerVersion)
                .map_err(|_| serde::de::Error::custom("ledger version")),
        }
    }
}

/// Whether one check found anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    /// Nothing found.
    Ok,
    /// `found` findings.
    Found,
    /// Not run (no inventory, or a check this engine cannot run).
    Skipped,
    /// A status this version does not know.
    #[serde(other)]
    Unknown,
}

/// One check of a run.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CheckResult {
    /// The check (`UNBALANCED_TRANSACTION`, `ACCOUNT_BALANCE_DRIFT`, ...;
    /// an open set).
    pub code: String,
    /// Its status.
    pub status: CheckStatus,
    /// How many it found (exact, also past the listing cap).
    #[serde(default)]
    pub found: u64,
}

/// How bad a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The books are wrong.
    Error,
    /// Something to look at.
    Warning,
}

/// What a finding is about (`subject.type`; an open set).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    /// An account.
    Account,
    /// A transaction.
    Transaction,
    /// A document draft.
    DocumentDraft,
    /// An item.
    Item,
    /// A kind this version does not know (shown without an Open button).
    #[serde(other)]
    Other,
}

/// The record a finding is about.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct Subject {
    /// Its kind.
    #[serde(rename = "type")]
    pub kind: SubjectKind,
    /// Its id (opaque).
    pub id: String,
    /// What the owner calls it (owner data: shown, never logged).
    #[serde(default)]
    pub label: String,
}

impl std::fmt::Debug for Subject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subject")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// The figures behind a finding, exact strings from the engine.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingAmounts {
    /// What it should be.
    pub expected_exact: Money,
    /// What it is.
    pub actual_exact: Money,
    /// The engine's difference (never computed here).
    pub difference_exact: Money,
}

impl std::fmt::Debug for FindingAmounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FindingAmounts(..)")
    }
}

/// The fix a finding offers (`fix`; an open set).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixKind {
    /// Recompute the account's cached balance from its journal lines.
    RecomputeAccountBalance,
}

fn fix_kind<'de, D: Deserializer<'de>>(d: D) -> Result<Option<FixKind>, D::Error> {
    // An unknown fix name reads as "not fixable here", never as a fix this
    // version would offer for the wrong thing.
    Ok(match Option::<String>::deserialize(d)?.as_deref() {
        Some("recompute_account_balance") => Some(FixKind::RecomputeAccountBalance),
        _ => None,
    })
}

/// One problem a check found.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct Finding {
    /// Id for [`Integrity::fix`] (unique within the run).
    pub id: IntegrityId,
    /// The check that found it (open set).
    pub code: String,
    /// How bad.
    pub severity: Severity,
    /// The record.
    pub subject: Subject,
    /// Its figures, when it has any.
    #[serde(default)]
    pub amounts: Option<FindingAmounts>,
    /// The fix offered; `None`: report only.
    #[serde(default, deserialize_with = "fix_kind")]
    pub fix: Option<FixKind>,
}

impl Finding {
    /// The app can fix it ([`Integrity::fix`]).
    #[must_use]
    pub fn fixable(&self) -> bool {
        self.fix.is_some()
    }
}

impl std::fmt::Debug for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Finding")
            .field("id", &self.id)
            .field("code", &self.code)
            .field("severity", &self.severity)
            .field("subject", &self.subject)
            .field("fix", &self.fix)
            .finish_non_exhaustive()
    }
}

/// One stored check run (`IntegrityRun`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityRun {
    /// The run, for [`Integrity::fix`] and [`Integrity::run`].
    pub id: IntegrityId,
    /// Its level.
    pub level: IntegrityLevel,
    /// The company (opaque).
    #[serde(default)]
    pub company_id: Option<String>,
    /// Started.
    #[serde(default)]
    pub started_at: Option<Timestamp>,
    /// Finished.
    #[serde(default)]
    pub finished_at: Option<Timestamp>,
    /// The ledger version the run saw; a fix sends it back.
    pub ledger_version: RunLedgerVersion,
    /// Every check of the level, in the engine's order.
    #[serde(default)]
    pub checks: Vec<CheckResult>,
    /// What they found (at most [`MAX_FINDINGS`]).
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// More was found than listed.
    #[serde(default)]
    pub truncated: bool,
}

impl IntegrityRun {
    /// Findings of one severity.
    pub fn findings_of(&self, severity: Severity) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(move |f| f.severity == severity)
    }

    /// Findings the app can fix.
    pub fn fixable(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.fixable())
    }
}

/// Why a fix skipped a finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FixSkipReason {
    /// Re-checked under lock: already right.
    NoLongerFound,
    /// Not a fixable finding (or not in this run).
    NotFixable,
    /// A reason this version does not know.
    #[serde(other)]
    Other,
}

/// A finding a fix left alone.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SkippedFinding {
    /// The finding.
    pub id: IntegrityId,
    /// Why.
    pub reason: FixSkipReason,
}

/// Answer of [`Integrity::fix`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixResult {
    /// Fixed.
    #[serde(default)]
    pub fixed: Vec<IntegrityId>,
    /// Left alone.
    #[serde(default)]
    pub skipped: Vec<SkippedFinding>,
    /// The ledger version after the fix (a fix of a cached balance does not
    /// move it).
    pub ledger_version: RunLedgerVersion,
}

/// Errors and warnings of a stored run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct SeverityCounts {
    /// Errors.
    #[serde(default)]
    pub error: u64,
    /// Warnings.
    #[serde(default)]
    pub warning: u64,
}

/// One stored run, summarised (Home's line, the weekly check's clock).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    /// The run.
    pub id: IntegrityId,
    /// When it finished.
    pub finished_at: Timestamp,
    /// What it found.
    #[serde(default)]
    pub counts: SeverityCounts,
    /// How many of its findings can be fixed.
    #[serde(default)]
    pub fixable: u64,
}

/// `GET /api/v1/integrity/summary`: the company's newest run of each level.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegritySummary {
    /// Newest quick run.
    #[serde(default)]
    pub last_quick: Option<RunSummary>,
    /// Newest full run.
    #[serde(default)]
    pub last_full: Option<RunSummary>,
}

/// The engine said the books changed since the run (409
/// `INTEGRITY_STALE`): check again before fixing.
#[must_use]
pub fn is_integrity_stale(e: &ApiError) -> bool {
    matches!(e, ApiError::Conflict { code: Some(c), .. } if c.as_str() == "INTEGRITY_STALE")
}

/// Verify Data for the open company.
pub struct Integrity<'a>(&'a Session);

impl Session {
    /// Verify Data (`integrity-v1`) for the open company.
    #[must_use]
    pub fn integrity(&self) -> Integrity<'_> {
        Integrity(self)
    }
}

impl Integrity<'_> {
    fn supported(&self) -> Result<(), ApiError> {
        if self.0.client().capabilities().has(Capabilities::INTEGRITY) {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::Unsupported))
        }
    }

    /// `POST /api/v1/integrity/checks`: run every check of `level` and store
    /// the run (owner or admin). Never re-sent after it reached the engine:
    /// a timeout is returned as is (check again).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `integrity-v1` (nothing sent);
    /// `Forbidden` (`INTEGRITY_ADMIN_REQUIRED`); `Decode` for an amount that
    /// is not an exact string; any [`ApiError`].
    pub async fn check(&self, level: IntegrityLevel) -> Result<IntegrityRun, ApiError> {
        #[derive(Serialize)]
        struct Body {
            level: IntegrityLevel,
        }
        self.supported()?;
        let call = Call::at(Method::Post, INTEGRITY_CHECKS.route())
            .json_body(json_body(&Body { level })?)
            .timeout(crate::client::TimeoutClass::Archive);
        let reply = self.0.send(&INTEGRITY_CHECKS, call, None).await?;
        decode(&reply, "integrity run")
    }

    /// `POST /api/v1/integrity/fixes`: fix the chosen findings of `run`,
    /// guarded by the run's ledger version. `backup_archive_id` names the
    /// verified backup made just before (it goes into the audit row). The
    /// call must run under a routine maintenance hold in LAN mode.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `integrity-v1`, `Local(InvalidValue)`
    /// for no findings (nothing sent); `Conflict` with
    /// [`is_integrity_stale`] when the books moved; `NotFound`
    /// (`INTEGRITY_RUN_UNKNOWN`); `Forbidden`; any [`ApiError`]. A 2xx whose
    /// answer cannot be read is `SavedUnreadable` (check again).
    pub async fn fix(
        &self,
        run: &IntegrityRun,
        finding_ids: &[IntegrityId],
        backup_archive_id: Option<&str>,
    ) -> Result<FixResult, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            run_id: &'a IntegrityId,
            finding_ids: &'a [IntegrityId],
            expected_ledger_version: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            backup_archive_id: Option<&'a str>,
        }
        self.supported()?;
        if finding_ids.is_empty() || finding_ids.len() > MAX_FINDINGS {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let call = Call::at(Method::Post, INTEGRITY_FIXES.route()).json_body(json_body(&Body {
            run_id: &run.id,
            finding_ids,
            expected_ledger_version: run.ledger_version.value().to_string(),
            backup_archive_id,
        })?);
        let reply = self.0.send(&INTEGRITY_FIXES, call, None).await?;
        decode(&reply, "integrity fix").map_err(saved_unreadable)
    }

    /// `GET /api/v1/integrity/summary` (any member).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `integrity-v1`; any [`ApiError`].
    pub async fn summary(&self) -> Result<IntegritySummary, ApiError> {
        self.supported()?;
        let reply = self
            .0
            .send(
                &INTEGRITY_SUMMARY,
                Call::at(Method::Get, INTEGRITY_SUMMARY.route()),
                None,
            )
            .await?;
        decode(&reply, "integrity summary")
    }

    /// `GET /api/v1/integrity/runs/{id}`: one stored run (owner or admin).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `integrity-v1`; `NotFound`; any
    /// [`ApiError`].
    pub async fn run(&self, id: &IntegrityId) -> Result<IntegrityRun, ApiError> {
        self.supported()?;
        let call = Call::new(
            Method::Get,
            INTEGRITY_RUN.route(),
            INTEGRITY_RUN.route().replace("{id}", id.as_str()),
        );
        let reply = self.0.send(&INTEGRITY_RUN, call, None).await?;
        decode(&reply, "integrity run")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests, session_with_caps};
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    pub(crate) const RUN: &str = r#"{"id":"0b4f6a0e-1d2c-4c4e-9a51-1b2c3d4e5f60","level":"full",
        "companyId":"c1","startedAt":"2026-11-23T10:00:00.000Z","finishedAt":"2026-11-23T10:00:02.000Z",
        "ledgerVersion":"9007199254740993",
        "checks":[{"code":"UNBALANCED_TRANSACTION","status":"ok","found":0},
                  {"code":"ACCOUNT_BALANCE_DRIFT","status":"found","found":1},
                  {"code":"INVENTORY_VALUE_DIFFERENCE","status":"skipped","found":0},
                  {"code":"SOMETHING_NEW","status":"later","found":0}],
        "findings":[{"id":"ACCOUNT_BALANCE_DRIFT:a1","code":"ACCOUNT_BALANCE_DRIFT","severity":"warning",
            "subject":{"type":"account","id":"a1","label":"Checking canary"},
            "amounts":{"expectedExact":"1250.1250","actualExact":"1200.0000","differenceExact":"50.1250"},
            "fix":"recompute_account_balance"},
          {"id":"X:t1","code":"NEW_CHECK","severity":"error",
            "subject":{"type":"gizmo","id":"t1","label":"Memo canary"},"fix":"delete_everything"}],
        "truncated":false}"#;

    fn caps() -> [&'static str; 1] {
        [Capabilities::INTEGRITY]
    }

    #[test]
    fn a_run_reads_exact_amounts_and_open_sets() {
        let run: IntegrityRun = serde_json::from_str(RUN).expect("run");
        assert_eq!(run.ledger_version.value(), 9_007_199_254_740_993);
        assert_eq!(run.level, IntegrityLevel::Full);
        assert_eq!(run.checks[3].status, CheckStatus::Unknown);
        let drift = &run.findings[0];
        assert!(drift.fixable());
        let a = drift.amounts.as_ref().expect("amounts");
        assert_eq!(a.difference_exact.to_wire(), "50.1250");
        // An unknown fix is report-only; an unknown subject kind is Other.
        assert!(!run.findings[1].fixable());
        assert_eq!(run.findings[1].subject.kind, SubjectKind::Other);
        assert_eq!(run.fixable().count(), 1);
        assert_eq!(run.findings_of(Severity::Error).count(), 1);
        // No owner data in Debug.
        let d = format!("{run:?}");
        assert!(!d.contains("canary") && !d.contains("1250"), "{d}");
        // A JSON number amount, or a fifth decimal, is refused.
        let number = RUN.replace(
            r#""expectedExact":"1250.1250""#,
            r#""expectedExact":1250.125"#,
        );
        assert!(serde_json::from_str::<IntegrityRun>(&number).is_err());
        let five = RUN.replace("1250.1250", "1250.12501");
        assert!(serde_json::from_str::<IntegrityRun>(&five).is_err());
        let numeric = RUN.replace(
            r#""ledgerVersion":"9007199254740993""#,
            r#""ledgerVersion":42"#,
        );
        let n: IntegrityRun = serde_json::from_str(&numeric).expect("number version");
        assert_eq!(n.ledger_version.value(), 42);
    }

    #[tokio::test]
    async fn nothing_is_sent_without_the_capability() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        let e = s
            .integrity()
            .check(IntegrityLevel::Quick)
            .await
            .expect_err("unsupported");
        assert!(matches!(e, ApiError::Local(LocalValidation::Unsupported)));
        assert!(s.integrity().summary().await.is_err());
        assert!(
            server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn check_fix_summary_and_run_send_the_contract() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/integrity/checks"))
            .respond_with(json(200, RUN))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/integrity/fixes"))
            .respond_with(json(
                200,
                r#"{"fixed":["ACCOUNT_BALANCE_DRIFT:a1"],"skipped":[{"id":"X:t1","reason":"NOT_FIXABLE"}],"ledgerVersion":"9007199254740993"}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/integrity/summary"))
            .respond_with(json(
                200,
                r#"{"lastQuick":null,"lastFull":{"id":"r1","finishedAt":"2026-11-23T10:00:02.000Z","counts":{"error":1,"warning":2},"fixable":1}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/integrity/runs/r1"))
            .respond_with(json(200, RUN))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &caps()).await;
        let run = s
            .integrity()
            .check(IntegrityLevel::Full)
            .await
            .expect("run");
        let ids: Vec<IntegrityId> = run.fixable().map(|f| f.id.clone()).collect();
        let fixed = s
            .integrity()
            .fix(&run, &ids, Some("arch-1"))
            .await
            .expect("fix");
        assert_eq!(fixed.fixed.len(), 1);
        assert_eq!(fixed.skipped[0].reason, FixSkipReason::NotFixable);
        let summary = s.integrity().summary().await.expect("summary");
        assert!(summary.last_quick.is_none());
        assert_eq!(summary.last_full.expect("full").counts.warning, 2);
        let again = s
            .integrity()
            .run(&IntegrityId::parse("r1").expect("id"))
            .await
            .expect("run");
        assert_eq!(again.id, run.id);

        let posts = requests(&server, "POST").await;
        let check: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(check, serde_json::json!({"level":"full"}));
        assert!(posts[0].headers.get("x-company-id").is_some());
        let fix: Value = serde_json::from_slice(&posts[1].body).expect("json");
        assert_eq!(fix["runId"], run.id.as_str());
        assert_eq!(
            fix["findingIds"],
            serde_json::json!(["ACCOUNT_BALANCE_DRIFT:a1"])
        );
        assert_eq!(fix["expectedLedgerVersion"], "9007199254740993");
        assert_eq!(fix["backupArchiveId"], "arch-1");
    }

    #[tokio::test]
    async fn a_stale_fix_is_recognised_and_an_empty_one_is_not_sent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/integrity/fixes"))
            .respond_with(json(
                409,
                r#"{"error":{"message":"The books changed.","code":"INTEGRITY_STALE"}}"#,
            ))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &caps()).await;
        let run: IntegrityRun = serde_json::from_str(RUN).expect("run");
        let e = s.integrity().fix(&run, &[], None).await.expect_err("empty");
        assert!(matches!(e, ApiError::Local(LocalValidation::InvalidValue)));
        let ids = [run.findings[0].id.clone()];
        let e = s
            .integrity()
            .fix(&run, &ids, None)
            .await
            .expect_err("stale");
        assert!(is_integrity_stale(&e), "{e:?}");
        let posts = requests(&server, "POST").await;
        assert_eq!(posts.len(), 1, "never retried");
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert!(body.get("backupArchiveId").is_none());
    }

    #[test]
    fn ids_are_path_safe() {
        assert!(IntegrityId::parse("ACCOUNT_BALANCE_DRIFT:0b4f6a0e-1d2c").is_ok());
        for bad in ["", "a/b", "..", "a.b", "a b", &"x".repeat(129)] {
            assert!(IntegrityId::parse(bad).is_err(), "{bad}");
        }
    }
}
