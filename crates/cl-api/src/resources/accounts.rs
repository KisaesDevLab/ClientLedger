//! Chart of accounts (`/api/v1/accounts`, P05 plan §2.8).
//!
//! `balance` is maintained by the engine and read-only here: no request
//! type carries it and the client never recomputes it (NN7). DELETE
//! deactivates (the engine refuses when the balance is not zero).
//!
//! A credit card account may carry the last four digits of its card
//! (`cardLast4`, U-92, `document-drafts-v1`): the desktop picks the card
//! account of a receipt by them (Q-A03q). They are set on their own
//! ([`Accounts::set_card_last4`]) and never logged.

use cl_core::Money;
use cl_core::Timestamp;
use cl_core::ids::AccountId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::resources::common::{
    self, Query, SortDir, Spec, editable_with_active, null_false, null_true, yes,
};
use crate::resources::operations::{
    ACCOUNT_DELETE, ACCOUNT_GET, ACCOUNT_PUT, ACCOUNTS_CREATE, ACCOUNTS_LIST,
};
use crate::resources::versioning::{EditableFields, RecordVersion, Versioned, saved_from};
use crate::session::{Session, json_body};
use crate::wire::{Page, PageRequest};

pub(crate) const SPEC: Spec = Spec {
    list: &ACCOUNTS_LIST,
    create: &ACCOUNTS_CREATE,
    get: &ACCOUNT_GET,
    put: &ACCOUNT_PUT,
    delete: &ACCOUNT_DELETE,
    base: "/api/v1/accounts",
    item_key: "account",
    list_key: "data",
};

/// Account type (`accountType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountType {
    /// Asset.
    Asset,
    /// Liability.
    Liability,
    /// Equity.
    Equity,
    /// Income.
    Revenue,
    /// Cost of goods sold.
    Cogs,
    /// Expense.
    Expense,
    /// Other income.
    OtherRevenue,
    /// Other expense.
    OtherExpense,
}

impl AccountType {
    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            AccountType::Asset => "asset",
            AccountType::Liability => "liability",
            AccountType::Equity => "equity",
            AccountType::Revenue => "revenue",
            AccountType::Cogs => "cogs",
            AccountType::Expense => "expense",
            AccountType::OtherRevenue => "other_revenue",
            AccountType::OtherExpense => "other_expense",
        }
    }
}

/// The fields the account form edits (also the create body). `None`
/// clears a nullable field on update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountFields {
    /// Name (1-255 characters).
    pub name: String,
    /// Account number (up to 20 characters, unique).
    #[serde(default)]
    pub account_number: Option<String>,
    /// Type.
    pub account_type: AccountType,
    /// Detail type.
    #[serde(default)]
    pub detail_type: Option<String>,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
    /// Parent account (sub-accounts).
    #[serde(default)]
    pub parent_id: Option<AccountId>,
}

/// Body of an account update: every editable field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUpdate {
    /// Form fields.
    #[serde(flatten)]
    pub fields: AccountFields,
    /// Active.
    pub is_active: bool,
}

/// An account as the engine returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// Id.
    pub id: AccountId,
    /// Form fields.
    #[serde(flatten)]
    pub fields: AccountFields,
    /// Active.
    #[serde(default = "yes", deserialize_with = "null_true")]
    pub is_active: bool,
    /// Created by the engine (cannot be deleted).
    #[serde(default, deserialize_with = "null_false")]
    pub is_system: bool,
    /// Engine role of a system account.
    #[serde(default)]
    pub system_tag: Option<String>,
    /// Current balance, maintained by the engine (read-only).
    #[serde(default)]
    pub balance: Option<Money>,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// Last changed.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    /// The last four digits of a credit card account's card
    /// (`document-drafts-v1`; `None` without it or when not set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_last4: Option<CardLast4>,
}

/// The last four digits of a payment card (`"1234"`). `Debug` never shows
/// them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CardLast4([u8; 4]);

impl CardLast4 {
    /// Four ASCII digits, or `None`.
    #[must_use]
    pub fn parse(s: &str) -> Option<CardLast4> {
        let digits: [u8; 4] = s.as_bytes().try_into().ok()?;
        digits
            .iter()
            .all(u8::is_ascii_digit)
            .then_some(CardLast4(digits))
    }

    /// The digits.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Only ever built from four ASCII digits.
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

impl TryFrom<String> for CardLast4 {
    type Error = &'static str;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        CardLast4::parse(&s).ok_or("four digits")
    }
}

impl From<CardLast4> for String {
    fn from(c: CardLast4) -> String {
        c.as_str().to_string()
    }
}

impl std::fmt::Debug for CardLast4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CardLast4(<redacted>)")
    }
}

/// The detail type of a credit card account: the only one that takes
/// card digits.
pub const CREDIT_CARD_DETAIL_TYPE: &str = "credit_card";

impl EditableFields for Account {
    const EDITABLE: &'static [&'static str] = &[
        "name",
        "accountNumber",
        "accountType",
        "detailType",
        "description",
        "parentId",
        "isActive",
    ];

    fn editable(&self) -> Value {
        editable_with_active(&self.fields, self.is_active, Self::EDITABLE)
    }
}

/// Sort keys of the account list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccountSort {
    /// Number.
    Number,
    /// Name.
    Name,
    /// Type.
    Type,
    /// Detail type.
    DetailType,
    /// Balance.
    Balance,
    /// Active/inactive.
    Status,
}

impl AccountSort {
    const fn as_str(self) -> &'static str {
        match self {
            AccountSort::Number => "number",
            AccountSort::Name => "name",
            AccountSort::Type => "type",
            AccountSort::DetailType => "detailType",
            AccountSort::Balance => "balance",
            AccountSort::Status => "status",
        }
    }
}

/// Filters of the account list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountFilter {
    /// Any of these types (empty = all).
    pub types: Vec<AccountType>,
    /// Active or inactive only.
    pub is_active: Option<bool>,
    /// Name or number contains.
    pub search: Option<String>,
    /// Sort order.
    pub sort: Option<(AccountSort, SortDir)>,
}

impl AccountFilter {
    /// Query pairs (camelCase; types comma-joined).
    #[must_use]
    pub fn to_query(&self) -> Vec<(String, String)> {
        self.query().0
    }

    fn query(&self) -> Query {
        let mut q = Query::default();
        if !self.types.is_empty() {
            let csv: Vec<&str> = self.types.iter().map(|t| t.as_str()).collect();
            q.put("accountType", csv.join(","));
        }
        q.opt_bool("isActive", self.is_active);
        q.opt_str("search", self.search.as_deref());
        q.sort(self.sort.map(|(s, d)| (s.as_str(), d)));
        q
    }
}

/// Account operations of a session (`session.accounts()`).
#[derive(Clone, Copy, Debug)]
pub struct Accounts<'a>(pub(crate) &'a Session);

impl Session {
    /// The chart of accounts of the open company.
    #[must_use]
    pub fn accounts(&self) -> Accounts<'_> {
        Accounts(self)
    }
}

impl Accounts<'_> {
    /// One page of accounts.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn list(
        &self,
        filter: &AccountFilter,
        page: PageRequest,
    ) -> Result<Page<Account>, ApiError> {
        common::list_page(self.0, &SPEC, filter.query(), page).await
    }

    /// Every matching account (pages of 500, at most 100,000 rows).
    ///
    /// # Errors
    /// `Decode` beyond the cap; any [`ApiError`].
    pub async fn list_all(&self, filter: &AccountFilter) -> Result<Vec<Account>, ApiError> {
        common::list_all(self.0, &SPEC, &filter.query()).await
    }

    /// One account with its version.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn get(&self, id: AccountId) -> Result<Versioned<Account>, ApiError> {
        common::get(self.0, &SPEC, &id.to_string()).await
    }

    /// Creates an account.
    ///
    /// # Errors
    /// `Conflict { AlreadyExists }` for a used number; `Validation`; any
    /// [`ApiError`].
    pub async fn create(
        &self,
        new: &AccountFields,
        key: IdempotencyKey,
    ) -> Result<Versioned<Account>, ApiError> {
        common::create_named(
            self.0,
            &SPEC,
            "name",
            new.name.as_str(),
            &editable_with_active(new, true, Account::EDITABLE),
            json_body(new)?,
            key,
        )
        .await
    }

    /// Saves an account if it is still at `expected` (§2.5).
    ///
    /// # Errors
    /// `Conflict { Stale }` (nothing sent); any [`ApiError`].
    pub async fn update(
        &self,
        id: AccountId,
        expected: &RecordVersion,
        update: &AccountUpdate,
        key: IdempotencyKey,
    ) -> Result<Versioned<Account>, ApiError> {
        common::update(
            self.0,
            &SPEC,
            &id.to_string(),
            expected,
            json_body(update)?,
            key,
        )
        .await
    }

    /// Sets (or with `None` clears) the card digits of a credit card
    /// account, if it is still as `current` was read (§2.5). Every other
    /// field is sent as read, so nothing else changes.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `document-drafts-v1` and
    /// `Local(InvalidValue)` for an account whose detail type is not
    /// `credit_card` (nothing sent); `Conflict { Stale }`; `Validation`;
    /// any [`ApiError`].
    pub async fn set_card_last4(
        &self,
        current: &Versioned<Account>,
        digits: Option<CardLast4>,
        key: IdempotencyKey,
    ) -> Result<Versioned<Account>, ApiError> {
        if !self
            .0
            .client()
            .capabilities()
            .has(crate::wire::Capabilities::DOCUMENT_DRAFTS)
        {
            return Err(ApiError::Local(crate::errors::LocalValidation::Unsupported));
        }
        let a = &current.record;
        if digits.is_some() && a.fields.detail_type.as_deref() != Some(CREDIT_CARD_DETAIL_TYPE) {
            return Err(ApiError::Local(
                crate::errors::LocalValidation::InvalidValue,
            ));
        }
        let mut body = serde_json::to_value(AccountUpdate {
            fields: a.fields.clone(),
            is_active: a.is_active,
        })
        .map_err(|_| ApiError::Decode {
            what: "request body",
        })?;
        if let Value::Object(m) = &mut body {
            m.insert(
                "cardLast4".into(),
                digits.map_or(Value::Null, |d| Value::String(d.as_str().to_string())),
            );
        }
        common::update(
            self.0,
            &SPEC,
            &a.id.to_string(),
            &current.version,
            json_body(&body)?,
            key,
        )
        .await
    }

    /// Deactivates an account (`DELETE`; refused while it has a balance).
    ///
    /// # Errors
    /// `Validation`; any [`ApiError`].
    pub async fn deactivate(
        &self,
        id: AccountId,
        key: IdempotencyKey,
    ) -> Result<Versioned<Account>, ApiError> {
        let reply = common::deactivate(self.0, &SPEC, &id.to_string(), key).await?;
        saved_from(&reply.body, SPEC.item_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    pub(crate) const ACCOUNT: &str = r#"{"id":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
        "tenantId":"1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","companyId":null,
        "accountNumber":"1000","name":"Checking","accountType":"asset","detailType":"bank",
        "description":null,"isActive":true,"isSystem":false,"systemTag":null,"parentId":null,
        "balance":"999999999999999.9999","createdAt":"2026-10-01T10:00:00.000Z",
        "updatedAt":"2026-10-01T10:00:00.000Z"}"#;

    fn id() -> AccountId {
        AccountId::parse("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id")
    }

    #[test]
    fn filter_query_uses_engine_casing() {
        let f = AccountFilter {
            types: vec![AccountType::Asset, AccountType::OtherExpense],
            is_active: Some(false),
            search: Some("a b".into()),
            sort: Some((AccountSort::DetailType, SortDir::Desc)),
        };
        let pairs: Vec<String> = f
            .to_query()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        assert_eq!(
            pairs,
            [
                "accountType=asset,other_expense",
                "isActive=false",
                "search=a b",
                "sortBy=detailType",
                "sortDir=desc"
            ]
        );
        assert_eq!(AccountFilter::default().to_query(), Vec::new());
    }

    #[test]
    fn balance_is_exact_and_not_part_of_the_fingerprint() {
        let a: Account = serde_json::from_str(ACCOUNT).expect("account");
        assert_eq!(
            a.balance.map(Money::to_wire).as_deref(),
            Some("999999999999999.9999")
        );
        let mut moved = a.clone();
        moved.balance = Some(Money::ZERO);
        moved.updated_at = None;
        assert_eq!(a.fingerprint(), moved.fingerprint());
        let mut renamed = a.clone();
        renamed.fields.name = "Savings".into();
        assert_ne!(a.fingerprint(), renamed.fingerprint());
        let mut inactive = a.clone();
        inactive.is_active = false;
        assert_ne!(a.fingerprint(), inactive.fingerprint());
        let float = ACCOUNT.replace(r#""999999999999999.9999""#, "12.5");
        assert!(
            serde_json::from_str::<Account>(&float).is_err(),
            "JSON number refused"
        );
        let nulls = ACCOUNT.replace(r#""isActive":true"#, r#""isActive":null"#);
        assert!(
            serde_json::from_str::<Account>(&nulls)
                .expect("null")
                .is_active
        );
    }

    #[tokio::test]
    async fn list_get_create_update_deactivate() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/accounts"))
            .respond_with(json(200, &format!(r#"{{"data":[{ACCOUNT}],"total":1}}"#)))
            .mount(&server)
            .await;
        let one = format!(r#"{{"account":{ACCOUNT}}}"#);
        for m in ["POST", "PUT", "DELETE"] {
            Mock::given(method(m))
                .respond_with(json(if m == "POST" { 201 } else { 200 }, &one))
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/accounts/{}", id())))
            .respond_with(json(200, &one))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let accts = s.accounts();
        let page = accts
            .list(
                &AccountFilter {
                    types: vec![AccountType::Expense],
                    ..AccountFilter::default()
                },
                PageRequest::new(25, 50).expect("page"),
            )
            .await
            .expect("list");
        assert_eq!(page.total, 1);
        assert_eq!(page.data[0].fields.account_type, AccountType::Asset);
        let all = accts
            .list_all(&AccountFilter::default())
            .await
            .expect("all");
        assert_eq!(all.len(), 1);
        let gets = requests(&server, "GET").await;
        assert_eq!(query(&gets[0]), "accountType=expense&limit=25&offset=50");
        assert_eq!(query(&gets[1]), "limit=500&offset=0");

        let v = accts.get(id()).await.expect("get");
        let created = accts
            .create(&v.record.fields, IdempotencyKey::new())
            .await
            .expect("create");
        assert_eq!(created.record.id, id());
        let upd = AccountUpdate {
            fields: v.record.fields.clone(),
            is_active: true,
        };
        accts
            .update(id(), &v.version, &upd, IdempotencyKey::new())
            .await
            .expect("update");
        accts
            .deactivate(id(), IdempotencyKey::new())
            .await
            .expect("deactivate");

        let post: Value =
            serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
        assert_eq!(post["accountType"], "asset");
        assert!(post.get("balance").is_none(), "balance is never sent");
        let put: Value =
            serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
        assert_eq!(put["isActive"], true);
        assert_eq!(put["description"], Value::Null);
        assert!(put.get("balance").is_none());
        assert_eq!(requests(&server, "DELETE").await.len(), 1);
    }

    #[tokio::test]
    async fn engine_errors_are_typed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(
                409,
                r#"{"error":{"message":"Account number 1000 already exists","code":"ACCOUNT_NUMBER_EXISTS"}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(json(404, r#"{"error":{"message":"Account not found"}}"#))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let a: Account = serde_json::from_str(ACCOUNT).expect("a");
        let e = s
            .accounts()
            .create(&a.fields, IdempotencyKey::new())
            .await
            .expect_err("dup");
        assert!(matches!(
            e,
            ApiError::Conflict {
                reason: crate::ConflictReason::AlreadyExists,
                ..
            }
        ));
        assert_eq!(requests(&server, "POST").await.len(), 1, "never retried");
        let e = s.accounts().get(id()).await.expect_err("missing");
        assert!(matches!(e, ApiError::NotFound { .. }));
    }

    #[test]
    fn card_digits_parse_strictly_and_never_show() {
        assert_eq!(
            CardLast4::parse("4821").map(|c| c.as_str().to_string()),
            Some("4821".into())
        );
        for bad in ["482", "48211", "48a1", "", "４８２１"] {
            assert_eq!(CardLast4::parse(bad), None, "{bad}");
        }
        let card = ACCOUNT.replace(
            r#""detailType":"bank""#,
            r#""detailType":"credit_card","cardLast4":"4821""#,
        );
        let a: Account = serde_json::from_str(&card).expect("account");
        assert_eq!(a.card_last4, CardLast4::parse("4821"));
        assert!(!format!("{a:?}").contains("4821"));
        let plain: Account = serde_json::from_str(ACCOUNT).expect("account");
        assert_eq!(plain.card_last4, None, "older engines send none");
        let bad = ACCOUNT.replace(r#""detailType":"bank""#, r#""cardLast4":"12""#);
        assert!(serde_json::from_str::<Account>(&bad).is_err());
    }

    #[tokio::test]
    async fn card_digits_are_set_with_every_other_field_as_read() {
        let server = MockServer::start().await;
        let card = ACCOUNT.replace(r#""detailType":"bank""#, r#""detailType":"credit_card""#);
        let one = format!(r#"{{"account":{card}}}"#);
        for m in ["GET", "PUT"] {
            Mock::given(method(m))
                .and(path(format!("/api/v1/accounts/{}", id())))
                .respond_with(json(200, &one))
                .mount(&server)
                .await;
        }
        let s = crate::resources::testkit::session_with_caps(
            &server,
            &[crate::wire::Capabilities::DOCUMENT_DRAFTS],
        )
        .await;
        let v = s.accounts().get(id()).await.expect("get");
        s.accounts()
            .set_card_last4(&v, CardLast4::parse("4821"), IdempotencyKey::new())
            .await
            .expect("set");
        let put: Value =
            serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
        assert_eq!(put["cardLast4"], "4821");
        assert_eq!(put["name"], "Checking");
        assert_eq!(put["detailType"], "credit_card");
        assert!(put.get("balance").is_none());
        s.accounts()
            .set_card_last4(&v, None, IdempotencyKey::new())
            .await
            .expect("clear");
        let put: Value =
            serde_json::from_slice(&requests(&server, "PUT").await[1].body).expect("json");
        assert_eq!(put["cardLast4"], Value::Null);

        // Not a card account, or no capability: nothing sent.
        let mut bank = v.clone();
        bank.record.fields.detail_type = Some("bank".into());
        assert_eq!(
            s.accounts()
                .set_card_last4(&bank, CardLast4::parse("4821"), IdempotencyKey::new())
                .await,
            Err(ApiError::Local(
                crate::errors::LocalValidation::InvalidValue
            ))
        );
        let off = company_session(&server);
        assert_eq!(
            off.accounts()
                .set_card_last4(&v, CardLast4::parse("4821"), IdempotencyKey::new())
                .await,
            Err(ApiError::Local(crate::errors::LocalValidation::Unsupported))
        );
        assert_eq!(requests(&server, "PUT").await.len(), 2);
    }
}
