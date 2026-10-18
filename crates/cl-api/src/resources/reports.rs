//! The three statements P08 reads: balance sheet (opening-balance review,
//! year-end check), trial balance (restore verification) and profit and
//! loss (sample-company golden figures).
//!
//! Report JSON carries JavaScript numbers (`Number(Decimal.toFixed(4))`)
//! until U-14, so every amount is a display-only [`ReportAmount`] that keeps
//! the exact JSON text and is never compared or summed. Where equality
//! matters (restore verification, acceptance tests) the CSV form is used:
//! `format=csv` keeps the engine's decimal strings.

use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use serde::Deserialize;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::resources::operations::{
    REPORT_BALANCE_SHEET, REPORT_PROFIT_LOSS, REPORT_TRIAL_BALANCE,
};
use crate::resources::transactions::ReportBasis;
use crate::session::{Session, decode};
use crate::wire::ReportAmount;

/// One balance-sheet line (`accountId` is null for the virtual retained
/// earnings and net income rows).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceSheetEntry {
    /// The account, or `None` for a virtual row.
    #[serde(default)]
    pub account_id: Option<AccountId>,
    /// Account number.
    #[serde(default)]
    pub account_number: Option<String>,
    /// Name.
    pub name: String,
    /// Balance (display only).
    pub balance: ReportAmount,
}

/// `GET /api/v1/reports/balance-sheet`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceSheet {
    /// As-of date.
    #[serde(default)]
    pub as_of_date: Option<CalendarDate>,
    /// Asset rows.
    #[serde(default)]
    pub assets: Vec<BalanceSheetEntry>,
    /// Total assets.
    pub total_assets: ReportAmount,
    /// Liability rows.
    #[serde(default)]
    pub liabilities: Vec<BalanceSheetEntry>,
    /// Total liabilities.
    pub total_liabilities: ReportAmount,
    /// Equity rows, including the virtual retained earnings and net income.
    #[serde(default)]
    pub equity: Vec<BalanceSheetEntry>,
    /// Total equity.
    pub total_equity: ReportAmount,
    /// Liabilities plus equity.
    pub total_liabilities_and_equity: ReportAmount,
}

/// One trial-balance row.
#[derive(Clone, Debug, Deserialize)]
pub struct TrialBalanceRow {
    /// Account number (the virtual prior-years row is `30120`).
    #[serde(default)]
    pub account_number: Option<String>,
    /// Name.
    pub name: String,
    /// Type.
    #[serde(default)]
    pub account_type: Option<String>,
    /// Net debit (display only).
    pub debit: ReportAmount,
    /// Net credit (display only).
    pub credit: ReportAmount,
}

/// `GET /api/v1/reports/trial-balance` (JSON form).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialBalance {
    /// Rows in account-number order.
    #[serde(default)]
    pub data: Vec<TrialBalanceRow>,
    /// Sum of debits.
    pub total_debits: ReportAmount,
    /// Sum of credits.
    pub total_credits: ReportAmount,
}

/// `GET /api/v1/reports/profit-loss` (the totals P08 needs).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfitLoss {
    /// Total income.
    #[serde(default)]
    pub total_income: Option<ReportAmount>,
    /// Total expenses.
    #[serde(default)]
    pub total_expenses: Option<ReportAmount>,
    /// Net income.
    pub net_income: ReportAmount,
}

fn basis_str(b: ReportBasis) -> &'static str {
    match b {
        ReportBasis::Cash => "cash",
        ReportBasis::Accrual => "accrual",
    }
}

/// Report reads.
pub struct Reports<'a>(pub(crate) &'a Session);

impl Session {
    /// Financial statements of the open company.
    #[must_use]
    pub fn reports(&self) -> Reports<'_> {
        Reports(self)
    }
}

impl Reports<'_> {
    /// Balance sheet as of a date.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn balance_sheet(
        &self,
        as_of: CalendarDate,
        basis: ReportBasis,
    ) -> Result<BalanceSheet, ApiError> {
        let mut call = Call::at(Method::Get, REPORT_BALANCE_SHEET.route());
        call.query = vec![
            ("as_of_date".into(), as_of.to_wire()),
            ("basis".into(), basis_str(basis).into()),
        ];
        let reply = self.0.send(&REPORT_BALANCE_SHEET, call, None).await?;
        decode(&reply, "balance sheet")
    }

    /// Trial balance over a period (JSON; amounts display-only).
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn trial_balance(
        &self,
        start: CalendarDate,
        end: CalendarDate,
        basis: ReportBasis,
    ) -> Result<TrialBalance, ApiError> {
        let mut call = Call::at(Method::Get, REPORT_TRIAL_BALANCE.route());
        call.query = tb_query(start, end, basis, None);
        let reply = self.0.send(&REPORT_TRIAL_BALANCE, call, None).await?;
        decode(&reply, "trial balance")
    }

    /// Trial balance as CSV text: the engine's own formatted strings
    /// (`"84,210.55"`), never JSON floats, for equality checks (restore
    /// verification, acceptance tests). An empty ledger answers 404 "No data
    /// to export", returned as `Ok("")`.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn trial_balance_csv(
        &self,
        start: CalendarDate,
        end: CalendarDate,
        basis: ReportBasis,
    ) -> Result<String, ApiError> {
        let mut call = Call::at(Method::Get, REPORT_TRIAL_BALANCE.route());
        call.query = tb_query(start, end, basis, Some("csv"));
        match self.0.send(&REPORT_TRIAL_BALANCE, call, None).await {
            Ok(reply) => Ok(String::from_utf8_lossy(&reply.body).into_owned()),
            Err(ApiError::NotFound { code: None }) => Ok(String::new()),
            Err(e) => Err(e),
        }
    }

    /// Profit and loss over a period.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn profit_loss(
        &self,
        start: CalendarDate,
        end: CalendarDate,
        basis: ReportBasis,
    ) -> Result<ProfitLoss, ApiError> {
        let mut call = Call::at(Method::Get, REPORT_PROFIT_LOSS.route());
        call.query = tb_query(start, end, basis, None);
        let reply = self.0.send(&REPORT_PROFIT_LOSS, call, None).await?;
        decode(&reply, "profit and loss")
    }
}

fn tb_query(
    start: CalendarDate,
    end: CalendarDate,
    basis: ReportBasis,
    format: Option<&str>,
) -> Vec<(String, String)> {
    let mut q = vec![
        ("start_date".into(), start.to_wire()),
        ("end_date".into(), end.to_wire()),
        ("basis".into(), basis_str(basis).into()),
    ];
    if let Some(f) = format {
        q.push(("format".into(), f.into()));
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn d(y: i32, m: u32, day: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, day).expect("date")
    }

    #[tokio::test]
    async fn balance_sheet_keeps_number_text_and_virtual_rows() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/reports/balance-sheet"))
            .respond_with(json(
                200,
                r#"{"asOfDate":"2026-12-31","assets":[{"accountId":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountNumber":"1010","name":"Checking","balance":84210.55}],
                    "totalAssets":84210.55,"liabilities":[],"totalLiabilities":0,
                    "equity":[{"accountId":null,"name":"Net Income (Current Year)","accountNumber":null,"balance":125985}],
                    "totalEquity":84210.55,"totalLiabilitiesAndEquity":84210.55}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let bs = s
            .reports()
            .balance_sheet(d(2026, 12, 31), ReportBasis::Accrual)
            .await
            .expect("bs");
        assert_eq!(bs.assets[0].balance.text(), "84210.55");
        assert_eq!(bs.equity[0].account_id, None);
        assert_eq!(bs.equity[0].balance.text(), "125985");
        let gets = requests(&server, "GET").await;
        assert!(query(&gets[0]).contains("as_of_date=2026-12-31"));
        assert!(query(&gets[0]).contains("basis=accrual"));
    }

    #[tokio::test]
    async fn trial_balance_json_and_csv() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/reports/trial-balance"))
            .and(wiremock::matchers::query_param("format", "csv"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "#,Account,Type,Debit,Credit\n1010,Checking,asset,5.0000,0\n",
                "text/csv",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/reports/trial-balance"))
            .respond_with(json(
                200,
                r#"{"title":"Trial Balance","data":[{"account_number":"1010","name":"Checking","account_type":"asset","total_debit":5,"total_credit":0,"debit":5,"credit":0}],"totalDebits":5,"totalCredits":5}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let tb = s
            .reports()
            .trial_balance(d(2026, 1, 1), d(2026, 12, 31), ReportBasis::Accrual)
            .await
            .expect("tb");
        assert_eq!(tb.data[0].debit.text(), "5");
        let csv = s
            .reports()
            .trial_balance_csv(d(2026, 1, 1), d(2026, 12, 31), ReportBasis::Cash)
            .await
            .expect("csv");
        assert!(csv.contains("5.0000"));
        let gets = requests(&server, "GET").await;
        assert!(query(&gets[1]).contains("basis=cash"));
    }

    #[tokio::test]
    async fn empty_ledger_csv_is_empty_text() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/reports/trial-balance"))
            .respond_with(json(404, r#"{"error":{"message":"No data to export"}}"#))
            .mount(&server)
            .await;
        let csv = company_session(&server)
            .reports()
            .trial_balance_csv(d(2026, 1, 1), d(2026, 12, 31), ReportBasis::Accrual)
            .await
            .expect("empty");
        assert_eq!(csv, "");
    }

    #[tokio::test]
    async fn profit_loss_net_income() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/reports/profit-loss"))
            .respond_with(json(
                200,
                r#"{"totalIncome":10,"totalExpenses":4,"netIncome":13865.14}"#,
            ))
            .mount(&server)
            .await;
        let pl = company_session(&server)
            .reports()
            .profit_loss(d(2026, 10, 1), d(2026, 10, 31), ReportBasis::Accrual)
            .await
            .expect("pl");
        assert_eq!(pl.net_income.text(), "13865.14");
    }
}
