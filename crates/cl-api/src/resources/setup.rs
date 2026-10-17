//! Company setup helpers (P08): chart-of-accounts templates and bulk
//! account import, guided opening balances, and "setup complete".
//!
//! - `GET /api/v1/coa-templates/options`: the engine's own template list
//!   (`[{value,label}]`). `ClientLedger`'s four starter charts (Q-49) are not
//!   engine templates: a company is created with `systemAccountsOnly` and
//!   the chart is imported through `POST /api/v1/accounts/import`, which is
//!   transactional and never overwrites a system account.
//! - `POST /api/v1/export/opening-balances` posts one balanced journal
//!   entry dated `asOfDate`; the sign of each balance follows the account's
//!   normal side and the engine offsets the difference to its
//!   `opening_balances` equity account. Amounts travel as exact strings.

use cl_core::ids::AccountId;
use cl_core::ids::TransactionId;
use cl_core::{CalendarDate, Money};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::accounts::{AccountType, Accounts};
use crate::resources::operations::{
    ACCOUNTS_IMPORT, COA_TEMPLATE_OPTIONS, OPENING_BALANCES, SETUP_COMPLETE,
};
use crate::session::{Session, decode, json_body};

/// One engine COA template.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CoaTemplateOption {
    /// Slug sent as `businessType`.
    pub value: String,
    /// Shown to the owner.
    pub label: String,
}

/// One row of a bulk account import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportRow {
    /// Account number (≤ 20 characters), unique in the company (Q-209).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_number: Option<String>,
    /// Name (1-255).
    pub name: String,
    /// Type.
    pub account_type: AccountType,
    /// Detail type key, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_type: Option<String>,
}

/// Rows the engine did not import.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedAccount {
    /// 1-based row.
    #[serde(default)]
    pub row: Option<u64>,
    /// Account number of the row.
    #[serde(default)]
    pub account_number: Option<String>,
    /// Name of the row.
    #[serde(default)]
    pub name: Option<String>,
    /// Why (engine text).
    #[serde(default)]
    pub reason: Option<String>,
}

/// Answer of [`Accounts::import`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportResult {
    /// Rows created.
    pub imported: u64,
    /// Rows updated.
    #[serde(default)]
    pub updated: u64,
    /// Rows skipped (existing numbers, system accounts).
    #[serde(default)]
    pub skipped: Vec<SkippedAccount>,
}

/// Maximum rows per import request.
pub const IMPORT_MAX_ROWS: usize = 2000;

impl Accounts<'_> {
    /// `POST /api/v1/accounts/import`: create up to 2,000 accounts in one
    /// transaction. Existing numbers are skipped unless `update_existing`.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for an empty or oversized list (nothing sent);
    /// `Validation`; any [`ApiError`].
    pub async fn import(
        &self,
        rows: &[AccountImportRow],
        update_existing: bool,
        key: IdempotencyKey,
    ) -> Result<AccountImportResult, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            accounts: &'a [AccountImportRow],
            update_existing: bool,
        }
        if rows.is_empty() || rows.len() > IMPORT_MAX_ROWS {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call =
            Call::at(Method::Post, ACCOUNTS_IMPORT.route()).json_body(json_body(&Body {
                accounts: rows,
                update_existing,
            })?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&ACCOUNTS_IMPORT, call, None).await?;
        decode(&reply, "account import").map_err(crate::resources::versioning::saved_unreadable)
    }
}

/// How an opening balance names its account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountRef {
    /// By id.
    Id(AccountId),
    /// By account number.
    Number(String),
    /// By exact name (case-insensitive in the engine).
    Name(String),
}

/// One opening balance: positive means the account's normal side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpeningBalance {
    /// The account.
    pub account: AccountRef,
    /// The balance as of the start date.
    pub balance: Money,
}

impl Serialize for OpeningBalance {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("OpeningBalance", 2)?;
        match &self.account {
            AccountRef::Id(id) => st.serialize_field("accountId", id)?,
            AccountRef::Number(n) => st.serialize_field("accountNumber", n)?,
            AccountRef::Name(n) => st.serialize_field("accountName", n)?,
        }
        st.serialize_field("balance", &self.balance.to_wire())?;
        st.end()
    }
}

/// Answer of [`Session::post_opening_balances`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpeningBalancesPosted {
    /// The journal entry.
    pub transaction_id: TransactionId,
    /// Lines written (balances plus the offset line).
    #[serde(default)]
    pub lines_created: u64,
}

impl Session {
    /// `GET /api/v1/coa-templates/options`.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn coa_template_options(&self) -> Result<Vec<CoaTemplateOption>, ApiError> {
        #[derive(Deserialize)]
        struct Wrapper {
            options: Vec<CoaTemplateOption>,
        }
        let reply = self
            .send(
                &COA_TEMPLATE_OPTIONS,
                Call::at(Method::Get, COA_TEMPLATE_OPTIONS.route()),
                None,
            )
            .await?;
        Ok(decode::<Wrapper>(&reply, "coa templates")?.options)
    }

    /// `POST /api/v1/company/setup-complete`.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn setup_complete(&self, key: IdempotencyKey) -> Result<(), ApiError> {
        let mut call = Call::at(Method::Post, SETUP_COMPLETE.route());
        call.idempotency_key = Some(key);
        self.send(&SETUP_COMPLETE, call, None).await.map(drop)
    }

    /// `POST /api/v1/export/opening-balances`: one balanced opening entry
    /// as of `as_of` (Q-45). Zero balances are skipped by the engine.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for an empty list (nothing sent); `Validation`
    /// for an unknown account; `ClosedPeriod`; any [`ApiError`].
    pub async fn post_opening_balances(
        &self,
        balances: &[OpeningBalance],
        as_of: CalendarDate,
        key: IdempotencyKey,
    ) -> Result<OpeningBalancesPosted, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            balances: &'a [OpeningBalance],
            as_of_date: String,
        }
        if balances.is_empty() || balances.iter().all(|b| b.balance.is_zero()) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call =
            Call::at(Method::Post, OPENING_BALANCES.route()).json_body(json_body(&Body {
                balances,
                as_of_date: as_of.to_wire(),
            })?);
        call.idempotency_key = Some(key);
        let reply = self.send(&OPENING_BALANCES, call, None).await?;
        decode(&reply, "opening balances").map_err(crate::resources::versioning::saved_unreadable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    #[tokio::test]
    async fn import_sends_rows_and_reads_skips() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/accounts/import"))
            .respond_with(json(
                201,
                r#"{"imported":2,"updated":0,"skipped":[{"row":3,"accountNumber":"30000","name":"Opening Balances","reason":"system account"}],"accounts":[]}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let rows = vec![
            AccountImportRow {
                account_number: Some("1010".into()),
                name: "Operating Checking".into(),
                account_type: AccountType::Asset,
                detail_type: Some("checking".into()),
            },
            AccountImportRow {
                account_number: None,
                name: "Fuel".into(),
                account_type: AccountType::Expense,
                detail_type: None,
            },
        ];
        let r = s
            .accounts()
            .import(&rows, false, IdempotencyKey::new())
            .await
            .expect("import");
        assert_eq!(r.imported, 2);
        assert_eq!(r.skipped.len(), 1);
        let posts = requests(&server, "POST").await;
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["accounts"][0]["accountNumber"], "1010");
        assert!(body["accounts"][1].get("accountNumber").is_none());
        assert_eq!(body["updateExisting"], false);
        let e = s
            .accounts()
            .import(&[], false, IdempotencyKey::new())
            .await
            .expect_err("empty");
        assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
    }

    #[tokio::test]
    async fn opening_balances_are_exact_strings_with_the_start_date() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/export/opening-balances"))
            .respond_with(json(
                201,
                r#"{"transactionId":"3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","linesCreated":3}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let balances = vec![
            OpeningBalance {
                account: AccountRef::Number("1010".into()),
                balance: Money::parse_wire("84210.55").expect("m"),
            },
            OpeningBalance {
                account: AccountRef::Name("Truck Loan".into()),
                balance: Money::parse_wire("-12000").expect("m"),
            },
        ];
        let r = s
            .post_opening_balances(
                &balances,
                CalendarDate::from_ymd(2026, 1, 1).expect("d"),
                IdempotencyKey::new(),
            )
            .await
            .expect("posted");
        assert_eq!(r.lines_created, 3);
        let posts = requests(&server, "POST").await;
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["asOfDate"], "2026-01-01");
        assert_eq!(body["balances"][0]["balance"], "84210.5500");
        assert_eq!(body["balances"][1]["accountName"], "Truck Loan");
        assert_eq!(body["balances"][1]["balance"], "-12000.0000");
    }

    #[tokio::test]
    async fn templates_and_setup_complete() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/coa-templates/options"))
            .respond_with(json(
                200,
                r#"{"options":[{"value":"default","label":"General"}]}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/company/setup-complete"))
            .respond_with(json(200, r#"{"message":"Setup complete"}"#))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let t = s.coa_template_options().await.expect("templates");
        assert_eq!(t[0].value, "default");
        s.setup_complete(IdempotencyKey::new()).await.expect("done");
    }
}
