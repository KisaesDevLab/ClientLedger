//! List import and export (P11 step 6): the engine's per-list CSV export
//! routes (`/accounts/export`, `/contacts/export`, `/items/export`, CSV text
//! with numerics as decimal strings) and the contact import, which takes
//! rows the desktop already read and mapped. The account import lives with
//! setup (`Accounts::import`). Items are created one at a time through
//! `Items::create`: the engine's item import doesn't resolve the income
//! account it is sent by name (U-46).

use serde::{Deserialize, Serialize};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::accounts::Accounts;
use crate::resources::contacts::{ContactType, Contacts};
use crate::resources::items::Items;
use crate::resources::operations::{
    ACCOUNTS_EXPORT, CONTACTS_EXPORT, CONTACTS_IMPORT, ITEMS_EXPORT, Operation,
};
use crate::session::{Session, decode, json_body};

/// Most contact rows one import takes.
pub const CONTACT_IMPORT_MAX_ROWS: usize = 10_000;

/// One contact row for `POST /contacts/import`: the fields the engine's
/// import keeps today.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactImportRow {
    /// Display name (1-255).
    pub display_name: String,
    /// Company.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company_name: Option<String>,
    /// Email.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Phone (≤ 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

/// What a contact import created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct ContactImportResult {
    /// Contacts created.
    pub imported: u64,
}

async fn csv_text(
    session: &Session,
    op: &Operation,
    query: &[(&str, &str)],
) -> Result<String, ApiError> {
    let mut call = Call::at(Method::Get, op.route());
    call.query = query
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let reply = session.send(op, call, None).await?;
    Ok(String::from_utf8_lossy(&reply.body).into_owned())
}

impl Accounts<'_> {
    /// The Chart of Accounts as CSV text (number, name, type, detail type,
    /// balance, active, system).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn export_csv(&self) -> Result<String, ApiError> {
        csv_text(self.0, &ACCOUNTS_EXPORT, &[]).await
    }
}

impl Items<'_> {
    /// Items as CSV text (name, description, unit price, income account id,
    /// taxable, active).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn export_csv(&self) -> Result<String, ApiError> {
        csv_text(self.0, &ITEMS_EXPORT, &[]).await
    }
}

impl Contacts<'_> {
    /// Clients and vendors as CSV text; `role` keeps one type only.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn export_csv(&self, role: Option<ContactType>) -> Result<String, ApiError> {
        match role {
            Some(r) => csv_text(self.0, &CONTACTS_EXPORT, &[("contactType", r.as_str())]).await,
            None => csv_text(self.0, &CONTACTS_EXPORT, &[]).await,
        }
    }

    /// `POST /contacts/import`: creates every row with the type `role`.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for an empty or oversized list (nothing sent);
    /// `Validation`; any [`ApiError`].
    pub async fn import(
        &self,
        rows: &[ContactImportRow],
        role: ContactType,
        key: IdempotencyKey,
    ) -> Result<ContactImportResult, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            contacts: &'a [ContactImportRow],
            contact_type: &'static str,
        }
        if rows.is_empty() || rows.len() > CONTACT_IMPORT_MAX_ROWS {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call =
            Call::at(Method::Post, CONTACTS_IMPORT.route()).json_body(json_body(&Body {
                contacts: rows,
                contact_type: role.as_str(),
            })?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&CONTACTS_IMPORT, call, None).await?;
        decode(&reply, "contact import").map_err(crate::resources::versioning::saved_unreadable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn csv(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body.to_owned(), "text/csv")
    }

    #[tokio::test]
    async fn exports_are_text() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/accounts/export"))
            .respond_with(csv("Account Number,Name\n10100,Cash\n"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/items/export"))
            .respond_with(csv("Name,Unit Price\nMowing,45.0450\n"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/export"))
            .and(query_param("contactType", "vendor"))
            .respond_with(csv("Display Name\nAcme\n"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/export"))
            .respond_with(csv("Display Name\nAcme\nAda\n"))
            .mount(&server)
            .await;
        let s = company_session(&server);
        assert!(
            s.accounts()
                .export_csv()
                .await
                .expect("a")
                .contains("10100")
        );
        assert!(s.items().export_csv().await.expect("i").contains("45.0450"));
        let v = s
            .contacts()
            .export_csv(Some(ContactType::Vendor))
            .await
            .expect("v");
        assert_eq!(v.lines().count(), 2);
        let all = s.contacts().export_csv(None).await.expect("all");
        assert_eq!(all.lines().count(), 3);
    }

    #[tokio::test]
    async fn contact_import_body_and_limits() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/contacts/import"))
            .respond_with(json(201, r#"{"imported":1,"contacts":[]}"#))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let row = ContactImportRow {
            display_name: "Ada Park".into(),
            company_name: None,
            email: Some("ada@example.com".into()),
            phone: None,
        };
        let r = s
            .contacts()
            .import(
                std::slice::from_ref(&row),
                ContactType::Both,
                IdempotencyKey::new(),
            )
            .await
            .expect("import");
        assert_eq!(r.imported, 1);
        let sent = requests(&server, "POST").await;
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).expect("json");
        assert_eq!(body["contactType"], "both");
        assert_eq!(body["contacts"][0]["displayName"], "Ada Park");
        assert!(body["contacts"][0].get("companyName").is_none());
        let none = s
            .contacts()
            .import(&[], ContactType::Customer, IdempotencyKey::new())
            .await
            .expect_err("empty");
        assert_eq!(none, ApiError::Local(LocalValidation::InvalidValue));
        let many = vec![row; CONTACT_IMPORT_MAX_ROWS + 1];
        let e = s
            .contacts()
            .import(&many, ContactType::Customer, IdempotencyKey::new())
            .await
            .expect_err("too many");
        assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
        assert_eq!(
            requests(&server, "POST").await.len(),
            1,
            "refusals send nothing"
        );
    }
}
