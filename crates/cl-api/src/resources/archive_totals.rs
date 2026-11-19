//! Verification totals of a tenant archive (P32, engine U-283,
//! `archive-verification-totals-v1`) and what a transfer export reports
//! (U-284, `tenant-archive-transfer-v1`).
//!
//! **Provisional contract.** Written from the P32 plan (E2/E3) before the
//! engine PRs merged; re-checked against `target/plans/P32-engine-status.md`
//! at the pin bump. Every amount is a decimal string read exactly
//! ([`Money`] refuses a JSON number); counts are whole numbers. The app
//! shows these figures and never recomputes them (non-negotiable 7).

use cl_core::ids::TenantId;
use cl_core::{CalendarDate, Money};
use serde::Deserialize;
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::TENANT_VERIFICATION_TOTALS;
use crate::resources::tenants::Tenants;
use crate::session::decode;
use crate::wire::Capabilities;

/// The totals definition this version reads.
pub const TOTALS_DEFINITION: &str = "vt1";

/// Net income over a window.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetIncome {
    /// First day.
    pub from: CalendarDate,
    /// Last day.
    pub to: CalendarDate,
    /// Amount.
    pub amount: Money,
}

/// Debits and credits of the trial balance.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TrialBalanceTotals {
    /// Sum of debit balances.
    pub debits: Money,
    /// Sum of credit balances.
    pub credits: Money,
}

/// What the archive holds, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs, reason = "engine count names")]
pub struct TotalsCounts {
    pub accounts: u64,
    pub clients: u64,
    pub vendors: u64,
    pub items: u64,
    /// Every transaction row that travels, in any status.
    pub transactions: u64,
    pub posted_transactions: u64,
    pub attachments: u64,
    pub attachment_bytes: u64,
    pub audit_entries: u64,
}

/// Verification totals (`vt1`): accrual basis, posted entries only,
/// hidden accounts included.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationTotals {
    /// Definition version (`vt1`).
    pub definition: String,
    /// As-of date.
    pub as_of: CalendarDate,
    /// Start of the fiscal year containing `as_of`.
    #[serde(default)]
    pub fiscal_year_start: Option<CalendarDate>,
    /// Total assets.
    pub total_assets: Money,
    /// Total liabilities.
    pub total_liabilities: Money,
    /// Total equity, current earnings included.
    pub total_equity: Money,
    /// Net income of the fiscal year to date.
    pub net_income: NetIncome,
    /// A/R balance.
    pub accounts_receivable: Money,
    /// A/P balance.
    pub accounts_payable: Money,
    /// Trial balance totals.
    pub trial_balance: TrialBalanceTotals,
    /// Counts.
    pub counts: TotalsCounts,
}

/// The import's comparison (`ImportResult.verificationTotals`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TotalsComparison {
    /// Definition version.
    #[serde(default)]
    pub definition: Option<String>,
    /// What the file says (`None` for a format 1 archive).
    #[serde(default)]
    pub expected: Option<VerificationTotals>,
    /// What the import recomputed.
    #[serde(default)]
    pub actual: Option<VerificationTotals>,
    /// They match (strictly, or with only backfill-explained differences).
    #[serde(default)]
    pub matches: bool,
    /// The fields that differ.
    #[serde(default)]
    pub differences: Vec<Value>,
    /// Upgrade backfills that ran during the import.
    #[serde(default)]
    pub backfills: Vec<String>,
}

/// The options an export echoes (`manifestSummary.options`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveOptions {
    /// Written in transfer mode (users as names only).
    #[serde(default)]
    pub transfer: bool,
}

/// What an export reports about the manifest it wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSummary {
    /// The options as written.
    #[serde(default)]
    pub options: Option<ArchiveOptions>,
    /// The totals in the file.
    #[serde(default)]
    pub verification_totals: Option<VerificationTotals>,
    /// What the file leaves out.
    #[serde(default)]
    pub omitted: Vec<String>,
}

impl Tenants<'_> {
    /// `GET /api/v1/tenants/{id}/verification-totals?asOf=`: the totals a
    /// file made now would carry (the Send screen's preview, Q-P32l).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `archive-verification-totals-v1`
    /// (nothing sent); `Decode` for an amount that isn't an exact string;
    /// any [`ApiError`].
    pub async fn verification_totals(
        &self,
        id: TenantId,
        as_of: CalendarDate,
    ) -> Result<VerificationTotals, ApiError> {
        if !self
            .0
            .client()
            .capabilities()
            .has(Capabilities::ARCHIVE_VERIFICATION_TOTALS)
        {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let mut call = Call::new(
            Method::Get,
            TENANT_VERIFICATION_TOTALS.route(),
            TENANT_VERIFICATION_TOTALS
                .route()
                .replace("{id}", &id.to_string()),
        );
        call.query = vec![("asOf".into(), as_of.to_wire())];
        let reply = self.0.send(&TENANT_VERIFICATION_TOTALS, call, None).await?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wrapped {
            verification_totals: VerificationTotals,
        }
        if let Ok(w) = serde_json::from_slice::<Wrapped>(&reply.body) {
            return Ok(w.verification_totals);
        }
        decode(&reply, "verification totals")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const TOTALS: &str = r#"{"definition":"vt1","asOf":"2026-11-19",
        "fiscalYearStart":"2026-01-01","totalAssets":"12500.1250","totalLiabilities":"2000.00",
        "totalEquity":"10500.1250","netIncome":{"from":"2026-01-01","to":"2026-11-19","amount":"4321.0049"},
        "accountsReceivable":"1250.00","accountsPayable":"400.00",
        "trialBalance":{"debits":"20000.00","credits":"20000.00"},
        "counts":{"accounts":42,"clients":7,"vendors":5,"items":3,"transactions":1284,
        "postedTransactions":1270,"attachments":63,"attachmentBytes":1048576,"auditEntries":4200}}"#;

    #[test]
    fn totals_read_exact_strings_and_refuse_numbers() {
        let t: VerificationTotals = serde_json::from_str(TOTALS).expect("totals");
        assert_eq!(t.definition, TOTALS_DEFINITION);
        assert_eq!(t.total_assets.to_wire(), "12500.1250");
        assert_eq!(t.net_income.amount.to_wire(), "4321.0049");
        assert_eq!(t.counts.transactions, 1284);
        let number = TOTALS.replace(
            r#""totalAssets":"12500.1250""#,
            r#""totalAssets":12500.125"#,
        );
        assert!(serde_json::from_str::<VerificationTotals>(&number).is_err());
        let five = TOTALS.replace("12500.1250", "12500.12501");
        assert!(serde_json::from_str::<VerificationTotals>(&five).is_err());
    }

    #[test]
    fn a_comparison_from_a_format_one_archive_has_no_expected_totals() {
        let c: TotalsComparison = serde_json::from_str(&format!(
            r#"{{"definition":"vt1","expected":null,"actual":{TOTALS},"matches":true,"differences":[],"backfills":[]}}"#
        ))
        .expect("comparison");
        assert!(c.expected.is_none() && c.matches);
        let m: ManifestSummary = serde_json::from_str(&format!(
            r#"{{"options":{{"transfer":true}},"verificationTotals":{TOTALS},"omitted":["check_signatures"]}}"#
        ))
        .expect("summary");
        assert!(m.options.is_some_and(|o| o.transfer));
    }
}
