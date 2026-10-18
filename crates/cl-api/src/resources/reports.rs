//! The three statements P08 reads: balance sheet (opening-balance review,
//! year-end check), trial balance (restore verification) and profit and
//! loss (sample-company golden figures), plus the account quick report the
//! Chart of Accounts opens and the client and vendor balance summaries the
//! centers show (P11).
//!
//! Report JSON carries JavaScript numbers (`Number(Decimal.toFixed(4))`)
//! until U-14, so every amount is a display-only [`ReportAmount`] that keeps
//! the exact JSON text and is never compared or summed. Where equality
//! matters (restore verification, acceptance tests) the CSV form is used:
//! `format=csv` keeps the engine's decimal strings.

use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Timestamp};
use serde::{Deserialize, Deserializer};

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::resources::operations::{
    REPORT_ACCOUNT, REPORT_AP_AGING, REPORT_BALANCE_SHEET, REPORT_CUSTOMER_BALANCE,
    REPORT_PROFIT_LOSS, REPORT_TRIAL_BALANCE,
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

/// A report row's date as the engine sends it. Report queries that read a
/// `date` column straight from PostgreSQL get a JavaScript `Date` at the
/// engine's local midnight, serialized as a UTC instant
/// (`2026-07-03T05:00:00.000Z`); the day is that instant read in the
/// engine's time zone, which is this computer's (the engine runs here).
/// Plain `YYYY-MM-DD` text is taken as is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportDate {
    /// A calendar date.
    Day(CalendarDate),
    /// Local midnight as a UTC instant (convert with the local zone).
    Midnight(Timestamp),
}

impl<'de> Deserialize<'de> for ReportDate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s.len() == 10 {
            return CalendarDate::parse_wire(&s)
                .map(ReportDate::Day)
                .map_err(serde::de::Error::custom);
        }
        Timestamp::parse_wire(&s)
            .map(ReportDate::Midnight)
            .map_err(serde::de::Error::custom)
    }
}

/// One posted journal line of the account (`/reports/account-report`).
/// Debit and credit are the engine's exact decimal strings.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AccountReportLine {
    /// The transaction.
    #[serde(rename = "id")]
    pub transaction_id: TransactionId,
    /// Transaction type (`journal_entry`, `expense`, ...), as sent.
    pub txn_type: String,
    /// Number.
    #[serde(default)]
    pub txn_number: Option<String>,
    /// Date.
    pub txn_date: ReportDate,
    /// Memo.
    #[serde(default)]
    pub memo: Option<String>,
    /// Check number.
    #[serde(default)]
    pub check_number: Option<String>,
    /// Payee (linked contact, else the name on the check).
    #[serde(default)]
    pub payee: Option<String>,
    /// Debit (exact).
    pub debit: Money,
    /// Credit (exact).
    pub credit: Money,
}

/// `GET /api/v1/reports/account-report`: the account's posted lines, newest
/// first. No running balance or total: the screen shows the account's
/// engine-kept balance and never sums these lines (NN7).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountReport {
    /// The account.
    pub account_id: AccountId,
    /// Lines, newest first.
    #[serde(default)]
    pub data: Vec<AccountReportLine>,
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

    /// The quick report of one account: its posted lines between two optional
    /// dates (both inclusive), newest first.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn account_report(
        &self,
        account: AccountId,
        start: Option<CalendarDate>,
        end: Option<CalendarDate>,
    ) -> Result<AccountReport, ApiError> {
        let mut call = Call::at(Method::Get, REPORT_ACCOUNT.route());
        call.query = vec![("account_id".into(), account.to_string())];
        if let Some(d) = start {
            call.query.push(("start_date".into(), d.to_wire()));
        }
        if let Some(d) = end {
            call.query.push(("end_date".into(), d.to_wire()));
        }
        let reply = self.0.send(&REPORT_ACCOUNT, call, None).await?;
        decode(&reply, "account report")
    }

    /// Each active client's open invoice balance
    /// (`/reports/customer-balance-summary`). Clients with nothing open are
    /// left out by the engine; inactive clients are never listed.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn customer_balance_summary(&self) -> Result<CustomerBalanceSummary, ApiError> {
        let call = Call::at(Method::Get, REPORT_CUSTOMER_BALANCE.route());
        let reply = self.0.send(&REPORT_CUSTOMER_BALANCE, call, None).await?;
        decode(&reply, "client balance summary")
    }

    /// What is owed to each vendor on open bills as of `as_of`, by age
    /// (`/reports/ap-aging-summary`). Vendors with nothing open are left
    /// out.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn ap_aging_summary(&self, as_of: CalendarDate) -> Result<ApAgingSummary, ApiError> {
        let mut call = Call::at(Method::Get, REPORT_AP_AGING.route());
        call.query = vec![("as_of_date".into(), as_of.to_wire())];
        let reply = self.0.send(&REPORT_AP_AGING, call, None).await?;
        decode(&reply, "vendor aging summary")
    }
}

/// One client's row of the client balance summary.
#[derive(Clone, Debug, Deserialize)]
pub struct CustomerBalanceRow {
    /// The client.
    pub id: ContactId,
    /// Name (display only).
    #[serde(default)]
    pub display_name: Option<String>,
    /// Open invoice balance (display only; the engine's sum).
    pub balance: ReportAmount,
}

/// `GET /api/v1/reports/customer-balance-summary`.
#[derive(Clone, Debug, Deserialize)]
pub struct CustomerBalanceSummary {
    /// Clients with an open balance.
    #[serde(default)]
    pub data: Vec<CustomerBalanceRow>,
}

impl CustomerBalanceSummary {
    /// The open balance of `client`, if the report lists it.
    #[must_use]
    pub fn balance_of(&self, client: ContactId) -> Option<&ReportAmount> {
        self.data
            .iter()
            .find(|r| r.id == client)
            .map(|r| &r.balance)
    }
}

/// One vendor's row of the vendor aging summary. Bills without a vendor
/// come as one row with no `contact_id`.
#[derive(Clone, Debug, Deserialize)]
pub struct ApAgingVendor {
    /// The vendor (`None` for bills without one).
    #[serde(default, deserialize_with = "blank_contact")]
    pub contact_id: Option<ContactId>,
    /// Name (display only).
    #[serde(default)]
    pub vendor_name: Option<String>,
    /// Not yet due.
    pub current: ReportAmount,
    /// 1 to 30 days past due.
    #[serde(rename = "bucket1to30")]
    pub days_1_30: ReportAmount,
    /// 31 to 60 days past due.
    #[serde(rename = "bucket31to60")]
    pub days_31_60: ReportAmount,
    /// 61 to 90 days past due.
    #[serde(rename = "bucket61to90")]
    pub days_61_90: ReportAmount,
    /// More than 90 days past due.
    #[serde(rename = "bucketOver90")]
    pub over_90: ReportAmount,
    /// Everything open (display only; the engine's sum).
    pub total: ReportAmount,
}

/// `GET /api/v1/reports/ap-aging-summary` (the per-vendor part).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApAgingSummary {
    /// As-of date.
    #[serde(default)]
    pub as_of_date: Option<CalendarDate>,
    /// Vendors with open bills.
    #[serde(default)]
    pub vendors: Vec<ApAgingVendor>,
}

impl ApAgingSummary {
    /// What is open for `vendor`, if the report lists it.
    #[must_use]
    pub fn total_of(&self, vendor: ContactId) -> Option<&ReportAmount> {
        self.vendors
            .iter()
            .find(|r| r.contact_id == Some(vendor))
            .map(|r| &r.total)
    }
}

fn blank_contact<'de, D: Deserializer<'de>>(d: D) -> Result<Option<ContactId>, D::Error> {
    match Option::<String>::deserialize(d)? {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => ContactId::parse(&s)
            .map(Some)
            .map_err(serde::de::Error::custom),
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

    const ACCT: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

    #[tokio::test]
    async fn account_report_lines_are_exact_and_dates_read_both_ways() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/reports/account-report"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"title":"Check Register","accountId":"{ACCT}","data":[
                    {{"id":"3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02","txn_type":"journal_entry","txn_number":null,
                      "txn_date":"2026-07-03T05:00:00.000Z","memo":"m","check_number":null,"payee":null,
                      "debit":"999999999999999.9999","credit":"0.0000"}},
                    {{"id":"3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03","txn_type":"expense","txn_number":"1001",
                      "txn_date":"2026-07-01","memo":null,"check_number":"1001","payee":"Ozark Feed",
                      "debit":"0","credit":"12.3400"}}]}}"#
                ),
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let acct = AccountId::parse(ACCT).expect("id");
        let r = s
            .reports()
            .account_report(acct, Some(d(2026, 7, 1)), Some(d(2026, 7, 31)))
            .await
            .expect("report");
        assert_eq!(r.account_id, acct);
        assert_eq!(r.data.len(), 2);
        assert_eq!(r.data[0].debit.to_wire(), "999999999999999.9999");
        assert_eq!(r.data[1].credit.to_wire(), "12.3400");
        assert!(matches!(r.data[0].txn_date, ReportDate::Midnight(_)));
        assert_eq!(r.data[1].txn_date, ReportDate::Day(d(2026, 7, 1)));
        assert_eq!(r.data[1].payee.as_deref(), Some("Ozark Feed"));
        let gets = requests(&server, "GET").await;
        assert_eq!(
            query(&gets[0]),
            format!("account_id={ACCT}&start_date=2026-07-01&end_date=2026-07-31")
        );
        s.reports()
            .account_report(acct, None, None)
            .await
            .expect("no dates");
        assert_eq!(
            query(&requests(&server, "GET").await[1]),
            format!("account_id={ACCT}")
        );
    }

    #[tokio::test]
    async fn balance_summaries_keep_the_engine_text() {
        let server = MockServer::start().await;
        let customer_id = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
        let vendor = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
        Mock::given(method("GET"))
            .and(path("/api/v1/reports/customer-balance-summary"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"title":"Customer Balance Summary","data":[
                    {{"id":"{customer_id}","display_name":"Riverbend HOA","balance":"1234.5600"}}]}}"#
                ),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/reports/ap-aging-summary"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"title":"AP Aging Summary","asOfDate":"2026-11-04","vendors":[
                    {{"contact_id":"{vendor}","vendor_name":"Galena Gravel","current":0.1,
                      "bucket1to30":0.2,"bucket31to60":0,"bucket61to90":0,"bucketOver90":0,
                      "total":0.30000000000000004}},
                    {{"contact_id":"","vendor_name":"Unknown","current":5,"bucket1to30":0,
                      "bucket31to60":0,"bucket61to90":0,"bucketOver90":0,"total":5}}],
                    "totals":{{}},"details":[]}}"#
                ),
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let c = s.reports().customer_balance_summary().await.expect("ar");
        let id = ContactId::parse(customer_id).expect("id");
        assert_eq!(c.balance_of(id).map(ReportAmount::text), Some("1234.5600"));
        let v = s
            .reports()
            .ap_aging_summary(d(2026, 11, 4))
            .await
            .expect("ap");
        let vid = ContactId::parse(vendor).expect("id");
        assert_eq!(
            v.total_of(vid).and_then(|t| t.display(2)).as_deref(),
            Some("0.30")
        );
        assert_eq!(v.vendors[1].contact_id, None, "bills without a vendor");
        assert_eq!(
            query(&requests(&server, "GET").await[1]),
            "as_of_date=2026-11-04"
        );
    }

    #[tokio::test]
    async fn account_report_refuses_float_amounts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"accountId":"{ACCT}","data":[{{"id":"3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02",
                    "txn_type":"journal_entry","txn_date":"2026-07-01","debit":12.34,"credit":"0"}}]}}"#
                ),
            ))
            .mount(&server)
            .await;
        let e = company_session(&server)
            .reports()
            .account_report(AccountId::parse(ACCT).expect("id"), None, None)
            .await
            .expect_err("a JSON number is not money");
        assert!(matches!(e, ApiError::Decode { .. }), "{e:?}");
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
