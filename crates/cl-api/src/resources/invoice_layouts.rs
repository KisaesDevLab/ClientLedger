//! Invoice print layouts (U-130, `invoice-layouts-v1`, Q-P20ac/ae/af):
//! several named layouts per company, exactly one the default; layouts
//! are archived, never deleted. The desktop makes the printed and emailed
//! PDF itself (`cl-print`), so it reads a layout's columns, its
//! ship-to / P.O. / terms switches and its footer.
//!
//! `Debug` never prints the footer text (owner data).

use std::fmt;

use cl_core::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::invoices::InvoiceLayoutId;
use crate::resources::operations::{
    INVOICE_LAYOUT_DEFAULT, INVOICE_LAYOUT_GET, INVOICE_LAYOUT_PUT, INVOICE_LAYOUTS_CREATE,
    INVOICE_LAYOUTS_LIST, Operation,
};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode, json_body};
use crate::wire::Capabilities;

const BASE: &str = "/api/v1/company/invoice-layouts";
/// The longest layout name.
pub const LAYOUT_NAME_MAX: usize = 100;

/// The line columns a layout prints (the amount always prints).
#[allow(clippy::struct_excessive_bools)] // one switch per column
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutColumns {
    /// Item name.
    pub item: bool,
    /// Description.
    pub description: bool,
    /// Service date.
    pub service_date: bool,
    /// Quantity.
    pub quantity: bool,
    /// Rate.
    pub rate: bool,
}

impl Default for LayoutColumns {
    fn default() -> Self {
        LayoutColumns {
            item: true,
            description: true,
            service_date: true,
            quantity: true,
            rate: true,
        }
    }
}

/// The footers a layout resolves per form.
#[derive(Clone, PartialEq, Eq, Default, Deserialize)]
pub struct ResolvedFooters {
    /// The invoice footer.
    #[serde(default)]
    pub invoice: String,
}

/// One invoice layout.
#[allow(clippy::struct_excessive_bools)] // the engine's switches
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceLayout {
    /// Id.
    pub id: InvoiceLayoutId,
    /// Name (unique in the company).
    pub name: String,
    /// The company default (invoices without their own print with it).
    pub is_default: bool,
    /// Archived: out of the pickers; invoices that use it keep it.
    pub is_archived: bool,
    /// Accent color (`#RRGGBB`).
    #[serde(default)]
    pub accent_color: Option<String>,
    /// Print the ship-to block.
    pub show_ship_to: bool,
    /// Print the P.O. number.
    pub show_po_number: bool,
    /// Print the terms.
    pub show_terms: bool,
    /// Footer text as typed (owner data).
    #[serde(default)]
    pub footer_text: Option<String>,
    /// Line columns.
    #[serde(default)]
    pub columns: LayoutColumns,
    /// The footers as they print.
    #[serde(default)]
    pub resolved_footers: ResolvedFooters,
    /// Invoices that print with it.
    #[serde(default)]
    pub invoice_count: u64,
    /// Last changed (`expectedUpdatedAt` of the next edit).
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

impl fmt::Debug for InvoiceLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvoiceLayout")
            .field("id", &self.id)
            .field("is_default", &self.is_default)
            .field("is_archived", &self.is_archived)
            .field("columns", &self.columns)
            .finish_non_exhaustive()
    }
}

/// A layout to create or edit (every field is sent).
#[allow(clippy::struct_excessive_bools)] // the engine's switches
#[derive(Clone, PartialEq, Eq)]
pub struct LayoutDraft {
    /// Name (1-100 characters).
    pub name: String,
    /// Print the ship-to block.
    pub show_ship_to: bool,
    /// Print the P.O. number.
    pub show_po_number: bool,
    /// Print the terms.
    pub show_terms: bool,
    /// Footer text (blank: none).
    pub footer_text: String,
    /// Line columns.
    pub columns: LayoutColumns,
}

impl fmt::Debug for LayoutDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LayoutDraft")
            .field("columns", &self.columns)
            .finish_non_exhaustive()
    }
}

impl LayoutDraft {
    /// The draft of an existing layout.
    #[must_use]
    pub fn of(l: &InvoiceLayout) -> LayoutDraft {
        LayoutDraft {
            name: l.name.clone(),
            show_ship_to: l.show_ship_to,
            show_po_number: l.show_po_number,
            show_terms: l.show_terms,
            footer_text: l.footer_text.clone().unwrap_or_default(),
            columns: l.columns,
        }
    }

    fn body(&self) -> Result<Map<String, Value>, ApiError> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > LAYOUT_NAME_MAX {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut m = Map::new();
        m.insert("name".into(), json!(name));
        m.insert("showShipTo".into(), json!(self.show_ship_to));
        m.insert("showPoNumber".into(), json!(self.show_po_number));
        m.insert("showTerms".into(), json!(self.show_terms));
        let footer = self.footer_text.trim();
        m.insert(
            "footerText".into(),
            if footer.is_empty() {
                Value::Null
            } else {
                json!(footer)
            },
        );
        m.insert("columns".into(), json!(self.columns));
        Ok(m)
    }
}

/// Invoice layout operations (`session.invoice_layouts()`).
#[derive(Clone, Copy, Debug)]
pub struct InvoiceLayouts<'a>(&'a Session);

impl Session {
    /// The company's invoice layouts (U-130).
    #[must_use]
    pub fn invoice_layouts(&self) -> InvoiceLayouts<'_> {
        InvoiceLayouts(self)
    }
}

impl InvoiceLayouts<'_> {
    fn require(self) -> Result<(), ApiError> {
        if self
            .0
            .client()
            .capabilities()
            .has(Capabilities::INVOICE_LAYOUTS)
        {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::Unsupported))
        }
    }

    async fn one(
        &self,
        op: &'static Operation,
        path: String,
        body: Option<Map<String, Value>>,
    ) -> Result<InvoiceLayout, ApiError> {
        #[derive(Deserialize)]
        struct W {
            layout: InvoiceLayout,
        }
        self.require()?;
        let mut call = Call::new(op.method(), op.route(), path);
        if let Some(b) = body {
            call = call.json_body(json_body(&Value::Object(b))?);
        }
        let reply = self.0.send(op, call, None).await?;
        let read = decode::<W>(&reply, "invoice layout").map(|w| w.layout);
        if op.method().is_read() {
            read
        } else {
            read.map_err(saved_unreadable)
        }
    }

    /// The layouts, default first; archived ones only with `archived`.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `invoice-layouts-v1`; any [`ApiError`].
    pub async fn list(&self, archived: bool) -> Result<Vec<InvoiceLayout>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            data: Vec<InvoiceLayout>,
        }
        self.require()?;
        let mut call = Call::at(Method::Get, INVOICE_LAYOUTS_LIST.route());
        if archived {
            call.query = vec![("includeArchived".into(), "true".into())];
        }
        let reply = self.0.send(&INVOICE_LAYOUTS_LIST, call, None).await?;
        decode::<W>(&reply, "invoice layouts").map(|w| w.data)
    }

    /// One layout.
    ///
    /// # Errors
    /// As [`InvoiceLayouts::list`], plus `NotFound`.
    pub async fn get(&self, id: InvoiceLayoutId) -> Result<InvoiceLayout, ApiError> {
        self.one(&INVOICE_LAYOUT_GET, format!("{BASE}/{id}"), None)
            .await
    }

    /// Creates a layout (never the default).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a blank or long name (nothing sent);
    /// `Conflict` (`INVOICE_LAYOUT_NAME_IN_USE`); as [`InvoiceLayouts::list`].
    pub async fn create(&self, draft: &LayoutDraft) -> Result<InvoiceLayout, ApiError> {
        let body = draft.body()?;
        self.one(&INVOICE_LAYOUTS_CREATE, BASE.to_string(), Some(body))
            .await
    }

    /// Edits a layout if it is still as `expected` (409 `STALE` otherwise).
    ///
    /// # Errors
    /// As [`InvoiceLayouts::create`], plus `Conflict { Stale }`.
    pub async fn update(
        &self,
        expected: &InvoiceLayout,
        draft: &LayoutDraft,
    ) -> Result<InvoiceLayout, ApiError> {
        let mut body = draft.body()?;
        if let Some(t) = expected.updated_at {
            body.insert("expectedUpdatedAt".into(), json!(t.to_wire()));
        }
        self.one(
            &INVOICE_LAYOUT_PUT,
            format!("{BASE}/{}", expected.id),
            Some(body),
        )
        .await
    }

    /// Archives (`true`) or unarchives a layout. The default can't be
    /// archived (409 `INVOICE_LAYOUT_IS_DEFAULT`).
    ///
    /// # Errors
    /// As [`InvoiceLayouts::update`].
    pub async fn set_archived(
        &self,
        expected: &InvoiceLayout,
        archived: bool,
    ) -> Result<InvoiceLayout, ApiError> {
        let mut body = Map::new();
        body.insert("isArchived".into(), json!(archived));
        if let Some(t) = expected.updated_at {
            body.insert("expectedUpdatedAt".into(), json!(t.to_wire()));
        }
        self.one(
            &INVOICE_LAYOUT_PUT,
            format!("{BASE}/{}", expected.id),
            Some(body),
        )
        .await
    }

    /// Makes a layout the company default (409 `INVOICE_LAYOUT_ARCHIVED`
    /// for an archived one).
    ///
    /// # Errors
    /// As [`InvoiceLayouts::get`].
    pub async fn set_default(&self, id: InvoiceLayoutId) -> Result<InvoiceLayout, ApiError> {
        self.one(
            &INVOICE_LAYOUT_DEFAULT,
            format!("{BASE}/{id}/default"),
            Some(Map::new()),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{json as reply, requests, session_with_caps};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const L: &str = "e1000000-0000-4000-8000-000000000001";

    fn layout(extra: &str) -> String {
        format!(
            r##"{{"id":"{L}","name":"Standard","isDefault":true,"isArchived":false,"accentColor":"#1f6feb",
            "logoUrl":null,"showShipTo":true,"showPoNumber":true,"showTerms":true,"footerText":"Thanks",
            "footerOverrides":{{}},"columns":{{"item":true,"description":true,"serviceDate":false,"quantity":true,"rate":true}},
            "resolvedFooters":{{"invoice":"Thanks","cash_sale":"","credit_memo":"","statement":""}},
            "invoiceCount":3,"createdAt":"2026-10-31T10:00:00.000Z","updatedAt":"2026-10-31T10:00:00.000Z"{extra}}}"##
        )
    }

    #[tokio::test]
    async fn layouts_list_edit_archive_and_default() {
        let old = MockServer::start().await;
        let s = session_with_caps(&old, &[]).await;
        assert_eq!(
            s.invoice_layouts().list(false).await,
            Err(ApiError::Local(LocalValidation::Unsupported))
        );
        let server = MockServer::start().await;
        let one = format!(r#"{{"layout":{}}}"#, layout(""));
        Mock::given(method("GET"))
            .and(path(BASE))
            .respond_with(reply(200, &format!(r#"{{"data":[{}]}}"#, layout(""))))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(format!("{BASE}/{L}")))
            .respond_with(reply(200, &one))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{BASE}/{L}/default")))
            .respond_with(reply(200, &one))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[Capabilities::INVOICE_LAYOUTS]).await;
        let all = s.invoice_layouts().list(true).await.expect("list");
        assert_eq!(all.len(), 1);
        let l = &all[0];
        assert!(!l.columns.service_date);
        assert_eq!(l.resolved_footers.invoice, "Thanks");
        assert!(!format!("{l:?}").contains("Thanks"));
        let mut d = LayoutDraft::of(l);
        d.columns.rate = false;
        s.invoice_layouts().update(l, &d).await.expect("edit");
        s.invoice_layouts()
            .set_archived(l, true)
            .await
            .expect("archive");
        s.invoice_layouts()
            .set_default(l.id)
            .await
            .expect("default");
        let puts = requests(&server, "PUT").await;
        let b: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(b["columns"]["rate"], false);
        assert_eq!(b["expectedUpdatedAt"], "2026-10-31T10:00:00.000Z");
        let b: Value = serde_json::from_slice(&puts[1].body).expect("json");
        assert_eq!(
            b,
            json!({"isArchived": true, "expectedUpdatedAt": "2026-10-31T10:00:00.000Z"})
        );
        let blank = LayoutDraft {
            name: "  ".into(),
            ..d
        };
        assert_eq!(
            s.invoice_layouts().create(&blank).await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        );
    }
}
