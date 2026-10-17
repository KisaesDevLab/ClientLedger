//! Chart of accounts (`/api/v1/accounts`, P05 plan §2.8).
//!
//! `balance` is maintained by the engine and read-only here: no request
//! type carries it and the client never recomputes it (NN7). DELETE
//! deactivates (the engine refuses when the balance is not zero).

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
use crate::resources::versioning::{EditableFields, RecordVersion, Versioned, versioned_from};
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
}

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
        common::create(self.0, &SPEC, json_body(new)?, key, None).await
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

    /// Deactivates an account (`DELETE`; refused while it has a balance).
    ///
    /// # Errors
    /// `Validation`; any [`ApiError`].
    pub async fn deactivate(
        &self,
        id: AccountId,
        key: IdempotencyKey,
    ) -> Result<Versioned<Account>, ApiError> {
        let reply = common::delete(self.0, &SPEC, &id.to_string(), key, None).await?;
        versioned_from(&reply.body, SPEC.item_key)
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
}
