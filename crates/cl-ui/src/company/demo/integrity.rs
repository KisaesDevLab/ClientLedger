//! The demo's Verify Data engine (P34, U-12): one fixable cached-balance
//! drift on Operating Checking and one report-only unbalanced entry, the
//! summary Home reads, and the fix (which needs a "backup" first, as the
//! real flow does). All figures are synthetic and come back as the
//! engine's exact strings would.

use cl_api::{ApiError, FixResult, IntegrityLevel, IntegrityRun, IntegritySummary};
use serde_json::json;

/// What the demo engine remembers.
#[derive(Clone, Debug, Default)]
pub struct DemoIntegrity {
    /// The drift was fixed (the next check finds only the report-only one).
    pub fixed: bool,
    /// Checks run so far (run ids).
    pub runs: u32,
    /// Summary of the last quick and full runs (JSON as the engine sends).
    last_quick: Option<serde_json::Value>,
    last_full: Option<serde_json::Value>,
    /// The ledger version (moves when the developer command "changes the
    /// books", to show `INTEGRITY_STALE`).
    pub ledger_version: u64,
    /// Fixes made (with the backup archive id they named).
    pub fixes: Vec<String>,
    /// Developer: the next fix answers 409 `INTEGRITY_STALE`.
    pub stale_next: bool,
}

const FINISHED: &str = "2026-11-04T08:12:00.000Z";

impl DemoIntegrity {
    /// `POST /integrity/checks`.
    ///
    /// # Errors
    /// Never (the demo has every right).
    pub fn check(&mut self, level: IntegrityLevel) -> Result<IntegrityRun, ApiError> {
        self.runs += 1;
        let mut findings = Vec::new();
        findings.push(json!({"id":"UNBALANCED_TRANSACTION:t-demo-1","code":"UNBALANCED_TRANSACTION",
            "severity":"error","subject":{"type":"transaction","id":"d0000000-0000-4000-8000-0000000000a1",
            "label":"Journal entry 1042, 10/14/2026"},
            "amounts":{"expectedExact":"0.0000","actualExact":"25.0000","differenceExact":"25.0000"},"fix":null}));
        if !self.fixed {
            findings.push(json!({"id":"ACCOUNT_BALANCE_DRIFT:acct-1010","code":"ACCOUNT_BALANCE_DRIFT",
                "severity":"warning","subject":{"type":"account","id":"d0000000-0000-4000-8000-000000001010",
                "label":"1010 · Operating Checking"},
                "amounts":{"expectedExact":"84210.5500","actualExact":"84185.5500","differenceExact":"25.0000"},
                "fix":"recompute_account_balance"}));
        }
        let lvl = match level {
            IntegrityLevel::Quick => "quick",
            IntegrityLevel::Full => "full",
        };
        let id = format!("demo-run-{}", self.runs);
        let fixable = u64::from(!self.fixed);
        let run = json!({"id":id,"level":lvl,"companyId":"demo","startedAt":FINISHED,"finishedAt":FINISHED,
            "ledgerVersion": self.ledger_version.to_string(),
            "checks":[{"code":"UNBALANCED_TRANSACTION","status":"found","found":1},
                      {"code":"ACCOUNT_BALANCE_DRIFT","status": if self.fixed {"ok"} else {"found"},"found":fixable},
                      {"code":"DRAFT_AFFECTS_BOOKS","status":"ok","found":0}],
            "findings":findings,"truncated":false});
        let summary = json!({"id":id,"finishedAt":FINISHED,
            "counts":{"error":1,"warning":fixable},"fixable":fixable});
        match level {
            IntegrityLevel::Quick => self.last_quick = Some(summary),
            IntegrityLevel::Full => self.last_full = Some(summary),
        }
        serde_json::from_value(run).map_err(|_| ApiError::Internal)
    }

    /// `GET /integrity/summary`.
    ///
    /// # Errors
    /// Never.
    pub fn summary(&self) -> Result<IntegritySummary, ApiError> {
        serde_json::from_value(json!({"lastQuick":self.last_quick,"lastFull":self.last_full}))
            .map_err(|_| ApiError::Internal)
    }

    /// `POST /integrity/fixes` (after the demo "backup").
    ///
    /// # Errors
    /// 409 `INTEGRITY_STALE` once after the developer command.
    pub fn fix(&mut self, run: &IntegrityRun, archive_id: &str) -> Result<FixResult, ApiError> {
        if std::mem::take(&mut self.stale_next) || run.ledger_version.value() != self.ledger_version
        {
            return Err(ApiError::Conflict {
                reason: cl_api::ConflictReason::Unspecified,
                current_version: None,
                code: cl_api::ErrorCode::parse("INTEGRITY_STALE"),
                message: cl_api::UserText::default(),
                details: cl_api::ConflictDetails::default(),
            });
        }
        let fixed = if self.fixed {
            Vec::new()
        } else {
            vec!["ACCOUNT_BALANCE_DRIFT:acct-1010"]
        };
        self.fixed = true;
        self.fixes.push(archive_id.to_string());
        serde_json::from_value(json!({"fixed":fixed,"skipped":[],
            "ledgerVersion": self.ledger_version.to_string()}))
        .map_err(|_| ApiError::Internal)
    }

    /// Developer: put the drift back and forget the runs (scenarios run
    /// light then dark in one app).
    pub fn reset(&mut self) {
        *self = DemoIntegrity::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_fix_and_check_again() {
        let mut d = DemoIntegrity::default();
        assert!(d.summary().expect("s").last_quick.is_none());
        let run = d.check(IntegrityLevel::Full).expect("run");
        assert_eq!(run.findings.len(), 2);
        assert_eq!(run.fixable().count(), 1);
        assert!(d.summary().expect("s").last_full.is_some());
        let r = d.fix(&run, "demo-arch").expect("fixed");
        assert_eq!(r.fixed.len(), 1);
        let again = d.check(IntegrityLevel::Full).expect("run");
        assert_eq!(again.findings.len(), 1);
        d.stale_next = true;
        assert!(cl_api::is_integrity_stale(
            &d.fix(&again, "x").expect_err("stale")
        ));
        d.reset();
        assert!(!d.fixed);
    }
}
