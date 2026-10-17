//! Customers and vendors (`/api/v1/contacts`, P05 plan §2.8).
//!
//! Contact data is owner data (names, emails, addresses): never logged.
//! The opening balance is set on create only (the engine's update schema
//! has no such field). DELETE deactivates.

use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::resources::common::{
    self, Query, SortDir, Spec, editable_with_active, null_false, null_true, yes,
};
use crate::resources::operations::{
    CONTACT_DELETE, CONTACT_GET, CONTACT_PUT, CONTACTS_CREATE, CONTACTS_LIST,
};
use crate::resources::versioning::{EditableFields, RecordVersion, Versioned, saved_from};
use crate::session::{Session, json_body};
use crate::wire::{Page, PageRequest};

pub(crate) const SPEC: Spec = Spec {
    list: &CONTACTS_LIST,
    create: &CONTACTS_CREATE,
    get: &CONTACT_GET,
    put: &CONTACT_PUT,
    delete: &CONTACT_DELETE,
    base: "/api/v1/contacts",
    item_key: "contact",
    list_key: "data",
};

/// Kind of contact (`contactType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactType {
    /// Customer.
    Customer,
    /// Vendor.
    Vendor,
    /// Both.
    Both,
}

impl ContactType {
    const fn as_str(self) -> &'static str {
        match self {
            ContactType::Customer => "customer",
            ContactType::Vendor => "vendor",
            ContactType::Both => "both",
        }
    }
}

/// The fields the contact form edits. `None` clears a nullable field;
/// the two country codes cannot be cleared, so `None` leaves them as they
/// are (the engine defaults them to `US`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(
    missing_docs,
    reason = "field names are the engine's contact form fields"
)]
pub struct ContactFields {
    pub contact_type: ContactType,
    /// Display name (1-255 characters).
    pub display_name: String,
    #[serde(default)]
    pub company_name: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub billing_line1: Option<String>,
    #[serde(default)]
    pub billing_line2: Option<String>,
    #[serde(default)]
    pub billing_city: Option<String>,
    #[serde(default)]
    pub billing_state: Option<String>,
    #[serde(default)]
    pub billing_zip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_country: Option<String>,
    #[serde(default)]
    pub shipping_line1: Option<String>,
    #[serde(default)]
    pub shipping_line2: Option<String>,
    #[serde(default)]
    pub shipping_city: Option<String>,
    #[serde(default)]
    pub shipping_state: Option<String>,
    #[serde(default)]
    pub shipping_zip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipping_country: Option<String>,
    #[serde(default)]
    pub default_payment_terms: Option<String>,
    #[serde(default)]
    pub default_expense_account_id: Option<AccountId>,
    #[serde(default)]
    pub tax_id: Option<String>,
    #[serde(default)]
    pub vendor_account_number: Option<String>,
    #[serde(default, deserialize_with = "null_false")]
    pub is1099_eligible: bool,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub default_tag_id: Option<TagId>,
}

impl ContactFields {
    /// A contact with only the required fields.
    #[must_use]
    pub fn named(contact_type: ContactType, display_name: impl Into<String>) -> ContactFields {
        ContactFields {
            contact_type,
            display_name: display_name.into(),
            company_name: None,
            first_name: None,
            last_name: None,
            email: None,
            phone: None,
            billing_line1: None,
            billing_line2: None,
            billing_city: None,
            billing_state: None,
            billing_zip: None,
            billing_country: None,
            shipping_line1: None,
            shipping_line2: None,
            shipping_city: None,
            shipping_state: None,
            shipping_zip: None,
            shipping_country: None,
            default_payment_terms: None,
            default_expense_account_id: None,
            tax_id: None,
            vendor_account_number: None,
            is1099_eligible: false,
            notes: None,
            default_tag_id: None,
        }
    }
}

/// Body of a contact create.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewContact {
    /// Form fields.
    #[serde(flatten)]
    pub fields: ContactFields,
    /// Opening balance (exact; sent as a decimal string).
    pub opening_balance: Money,
    /// Date of the opening balance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opening_balance_date: Option<CalendarDate>,
}

/// Body of a contact update: every editable field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactUpdate {
    /// Form fields.
    #[serde(flatten)]
    pub fields: ContactFields,
    /// Active.
    pub is_active: bool,
}

/// A contact as the engine returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    /// Id.
    pub id: ContactId,
    /// Form fields.
    #[serde(flatten)]
    pub fields: ContactFields,
    /// Active.
    #[serde(default = "yes", deserialize_with = "null_true")]
    pub is_active: bool,
    /// Opening balance (create-only).
    #[serde(default)]
    pub opening_balance: Option<Money>,
    /// Date of the opening balance.
    #[serde(default)]
    pub opening_balance_date: Option<CalendarDate>,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// Last changed.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

impl EditableFields for Contact {
    const EDITABLE: &'static [&'static str] = &[
        "contactType",
        "displayName",
        "companyName",
        "firstName",
        "lastName",
        "email",
        "phone",
        "billingLine1",
        "billingLine2",
        "billingCity",
        "billingState",
        "billingZip",
        "billingCountry",
        "shippingLine1",
        "shippingLine2",
        "shippingCity",
        "shippingState",
        "shippingZip",
        "shippingCountry",
        "defaultPaymentTerms",
        "defaultExpenseAccountId",
        "taxId",
        "vendorAccountNumber",
        "is1099Eligible",
        "notes",
        "defaultTagId",
        "isActive",
    ];

    fn editable(&self) -> Value {
        editable_with_active(&self.fields, self.is_active, Self::EDITABLE)
    }
}

/// Sort keys of the contact list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContactSort {
    /// Display name.
    Name,
    /// Type.
    Type,
    /// Email.
    Email,
    /// Phone.
    Phone,
    /// Active/inactive.
    Status,
}

impl ContactSort {
    const fn as_str(self) -> &'static str {
        match self {
            ContactSort::Name => "name",
            ContactSort::Type => "type",
            ContactSort::Email => "email",
            ContactSort::Phone => "phone",
            ContactSort::Status => "status",
        }
    }
}

/// Filters of the contact list. `Customer` also returns `Both`, and so
/// does `Vendor` (engine rule).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContactFilter {
    /// Kind.
    pub contact_type: Option<ContactType>,
    /// Active or inactive only.
    pub is_active: Option<bool>,
    /// Name, company or email contains.
    pub search: Option<String>,
    /// Sort order.
    pub sort: Option<(ContactSort, SortDir)>,
}

impl ContactFilter {
    /// Query pairs (camelCase).
    #[must_use]
    pub fn to_query(&self) -> Vec<(String, String)> {
        self.query().0
    }

    pub(crate) fn query(&self) -> Query {
        let mut q = Query::default();
        q.opt_str("contactType", self.contact_type.map(ContactType::as_str));
        q.opt_bool("isActive", self.is_active);
        q.opt_str("search", self.search.as_deref());
        q.sort(self.sort.map(|(s, d)| (s.as_str(), d)));
        q
    }
}

/// Contact operations of a session (`session.contacts()`).
#[derive(Clone, Copy, Debug)]
pub struct Contacts<'a>(pub(crate) &'a Session);

impl Session {
    /// Customers and vendors of the open company.
    #[must_use]
    pub fn contacts(&self) -> Contacts<'_> {
        Contacts(self)
    }
}

impl Contacts<'_> {
    /// One page of contacts.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn list(
        &self,
        filter: &ContactFilter,
        page: PageRequest,
    ) -> Result<Page<Contact>, ApiError> {
        common::list_page(self.0, &SPEC, filter.query(), page).await
    }

    /// Every matching contact (pages of 500, at most 100,000 rows).
    ///
    /// # Errors
    /// `Decode` beyond the cap; any [`ApiError`].
    pub async fn list_all(&self, filter: &ContactFilter) -> Result<Vec<Contact>, ApiError> {
        common::list_all(self.0, &SPEC, &filter.query()).await
    }

    /// One contact with its version.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn get(&self, id: ContactId) -> Result<Versioned<Contact>, ApiError> {
        common::get(self.0, &SPEC, &id.to_string()).await
    }

    /// Creates a contact.
    ///
    /// # Errors
    /// `Validation`; any [`ApiError`].
    pub async fn create(
        &self,
        new: &NewContact,
        key: IdempotencyKey,
    ) -> Result<Versioned<Contact>, ApiError> {
        common::create_named(
            self.0,
            &SPEC,
            "displayName",
            new.fields.display_name.as_str(),
            &editable_with_active(&new.fields, true, Contact::EDITABLE),
            json_body(new)?,
            key,
        )
        .await
    }

    /// Saves a contact if it is still at `expected` (§2.5).
    ///
    /// # Errors
    /// `Conflict { Stale }` (nothing sent); any [`ApiError`].
    pub async fn update(
        &self,
        id: ContactId,
        expected: &RecordVersion,
        update: &ContactUpdate,
        key: IdempotencyKey,
    ) -> Result<Versioned<Contact>, ApiError> {
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

    /// Deactivates a contact.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn deactivate(
        &self,
        id: ContactId,
        key: IdempotencyKey,
    ) -> Result<Versioned<Contact>, ApiError> {
        let reply = common::deactivate(self.0, &SPEC, &id.to_string(), key).await?;
        saved_from(&reply.body, SPEC.item_key)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    pub(crate) const CONTACT: &str = r#"{"id":"3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
        "contactType":"vendor","displayName":"Canary Supply","companyName":null,
        "email":"canary@example.test","billingCountry":"US","shippingCountry":"US",
        "openingBalance":"0.0001","openingBalanceDate":"2026-01-01","is1099Eligible":null,
        "isActive":true,"defaultExpenseAccountName":"Supplies",
        "createdAt":"2026-10-01T10:00:00.000Z","updatedAt":"2026-10-01T10:00:00.000Z"}"#;

    fn id() -> ContactId {
        ContactId::parse("3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id")
    }

    #[test]
    fn filter_query_and_model() {
        let f = ContactFilter {
            contact_type: Some(ContactType::Vendor),
            is_active: Some(true),
            search: Some(String::new()),
            sort: Some((ContactSort::Email, SortDir::Asc)),
        };
        let pairs: Vec<String> = f
            .to_query()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        assert_eq!(
            pairs,
            [
                "contactType=vendor",
                "isActive=true",
                "sortBy=email",
                "sortDir=asc"
            ]
        );
        let c: Contact = serde_json::from_str(CONTACT).expect("contact");
        assert!(!c.fields.is1099_eligible);
        assert_eq!(
            c.opening_balance.map(Money::to_wire).as_deref(),
            Some("0.0001")
        );
        let mut renamed = c.clone();
        renamed.fields.display_name = "Other".into();
        assert_ne!(c.fingerprint(), renamed.fingerprint());
        let mut joined = serde_json::from_str::<Value>(CONTACT).expect("v");
        joined["defaultExpenseAccountName"] = "Renamed".into();
        let joined: Contact = serde_json::from_value(joined).expect("c");
        assert_eq!(
            c.fingerprint(),
            joined.fingerprint(),
            "joined names excluded"
        );
    }

    #[tokio::test]
    async fn create_sends_the_opening_balance_as_a_string_and_lists_page() {
        let server = MockServer::start().await;
        let one = format!(r#"{{"contact":{CONTACT}}}"#);
        Mock::given(method("POST"))
            .and(path("/api/v1/contacts"))
            .respond_with(json(201, &one))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts"))
            .respond_with(json(200, &format!(r#"{{"data":[{CONTACT}],"total":3}}"#)))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(json(200, &one))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let new = NewContact {
            fields: ContactFields::named(ContactType::Vendor, "Canary Supply"),
            opening_balance: Money::parse_wire("0.0001").expect("m"),
            opening_balance_date: Some(CalendarDate::from_ymd(2026, 1, 1).expect("d")),
        };
        let c = s
            .contacts()
            .create(&new, IdempotencyKey::new())
            .await
            .expect("create");
        assert_eq!(c.record.id, id());
        let post: Value =
            serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
        assert_eq!(post["openingBalance"], "0.0001");
        assert_eq!(post["openingBalanceDate"], "2026-01-01");
        assert!(post.get("billingCountry").is_none());

        let page = s
            .contacts()
            .list(
                &ContactFilter::default(),
                PageRequest::new(1, 0).expect("p"),
            )
            .await
            .expect("list");
        assert_eq!(page.total, 3);
        assert_eq!(
            query(&requests(&server, "GET").await[0]),
            "limit=1&offset=0"
        );
        s.contacts()
            .deactivate(id(), IdempotencyKey::new())
            .await
            .expect("off");
    }
}
