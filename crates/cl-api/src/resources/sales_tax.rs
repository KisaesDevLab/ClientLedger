//! Sales tax setup, owed and exemption certificates (P23, engine U-147,
//! U-148, U-149, U-151, U-25; contract per the P23 plan §2.4-§2.7,
//! verified at the WP-4 pin bump).
//!
//! - **Setup:** agencies (a vendor the tax is paid to), sales tax items (a
//!   rate owed to one agency) and groups (1-10 items charged together,
//!   rounded once per document, Q-P23d), and tax codes (`Tax`, `Non` and
//!   the owner's own). Rates are [`TaxRate`] fractions with at most six
//!   places, `0 ≤ r < 1` on setup (plan §2.0). Every write takes an
//!   `Idempotency-Key` (`idempotency-sales-tax-v1`); edits send
//!   `expectedUpdatedAt` (409 `STALE`). Nothing is deleted: deactivate.
//! - **Owed** (`GET /sales-tax/owed`) is the engine's figure per agency and
//!   item as of a date; the UI shows the strings and adds nothing up.
//! - **Exemption certificates** (U-151) belong to a client; the certificate
//!   number and note are owner data and never reach a log (`Debug` hides
//!   them).
//! - Shapes the plan leaves open are desktop-side assumptions: single
//!   records arrive bare or wrapped under their kind (`{"item": {…}}`), lists
//!   as a bare array or under `data`; deactivate and reactivate are
//!   `POST …/{id}/deactivate|reactivate`.

use std::fmt;

use cl_core::ids::ContactId;
pub use cl_core::ids::{SalesTaxAgencyId, SalesTaxItemId, TaxCodeId, TaxExemptionId};
use cl_core::{CalendarDate, Money, TaxRate, Timestamp};
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Call, EngineReply, Method};
use crate::errors::{ApiError, ConflictReason, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::attachments::AttachmentId;
use crate::resources::customer_payments::{
    de_cents, de_cents_opt, de_cents_zero, error_code, null_false, null_vec, require, too_long,
    with_version,
};
use crate::resources::operations::{
    Operation, REPORT_EXEMPTIONS_EXPIRING, SALES_TAX_AGENCIES, SALES_TAX_AGENCY_CREATE,
    SALES_TAX_AGENCY_DEACTIVATE, SALES_TAX_AGENCY_PUT, SALES_TAX_AGENCY_REACTIVATE,
    SALES_TAX_CODE_CREATE, SALES_TAX_CODE_DEACTIVATE, SALES_TAX_CODE_PUT,
    SALES_TAX_CODE_REACTIVATE, SALES_TAX_CODES, SALES_TAX_EXEMPTION_CREATE,
    SALES_TAX_EXEMPTION_DEACTIVATE, SALES_TAX_EXEMPTION_PUT, SALES_TAX_EXEMPTION_REACTIVATE,
    SALES_TAX_EXEMPTIONS, SALES_TAX_ITEM_CREATE, SALES_TAX_ITEM_DEACTIVATE, SALES_TAX_ITEM_PUT,
    SALES_TAX_ITEM_REACTIVATE, SALES_TAX_ITEMS, SALES_TAX_OWED,
};
use crate::session::{Session, json_body};
use crate::wire::Capabilities;

/// Longest sales tax item or group name (characters, trimmed; a
/// desktop-side limit until the pin bump).
pub const SALES_TAX_NAME_MAX: usize = 100;
/// Longest description of a sales tax item.
pub const SALES_TAX_DESCRIPTION_MAX: usize = 500;
/// Longest tax code (characters, trimmed; U-25).
pub const TAX_CODE_MAX: usize = 8;
/// Longest tax code name.
pub const TAX_CODE_NAME_MAX: usize = 100;
/// Items in a group (plan §2.1).
pub const GROUP_MEMBERS_MAX: usize = 10;
/// Longest exemption certificate number.
pub const EXEMPTION_NUMBER_MAX: usize = 50;
/// Longest exemption note.
pub const EXEMPTION_NOTE_MAX: usize = 1000;

// ---- Refusals ------------------------------------------------------------------

/// The engine's refusals of a sales tax setup write or a document's sales
/// tax (plan §2.4, §2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SalesTaxRefusal {
    /// Someone changed it since it was read (409 `STALE`).
    Stale,
    /// The vendor is already an agency (409 `AGENCY_EXISTS`).
    AgencyExists,
    /// Active items still use the agency (409 `AGENCY_IN_USE`).
    AgencyInUse,
    /// Another item or group has the name (409 `NAME_IN_USE`).
    NameInUse,
    /// The item is in an active group (409 `ITEM_IN_GROUP`).
    ItemInGroup,
    /// The code is on clients or items (409 `CODE_IN_USE`).
    CodeInUse,
    /// `Tax` and `Non` can't be changed (409 `SYSTEM_CODE`).
    SystemCode,
    /// A group member is a group (400 `GROUP_MEMBER_NOT_ITEM`).
    GroupMemberNotItem,
    /// A group needs 1-10 items (400 `GROUP_SIZE`).
    GroupSize,
    /// An item needs an agency (400 `AGENCY_REQUIRED`).
    AgencyRequired,
    /// A line rate together with a sales tax (400
    /// `TAX_RATE_WITH_SALES_TAX`).
    TaxRateWithSalesTax,
    /// Sales tax is off for the company (400 `SALES_TAX_OFF`).
    SalesTaxOff,
    /// The sales tax is inactive (400 `TAX_ITEM_INACTIVE`, new documents).
    TaxItemInactive,
}

const REFUSALS: &[(&str, SalesTaxRefusal)] = &[
    (crate::resources::ap_doc::STALE, SalesTaxRefusal::Stale),
    ("AGENCY_EXISTS", SalesTaxRefusal::AgencyExists),
    ("AGENCY_IN_USE", SalesTaxRefusal::AgencyInUse),
    ("NAME_IN_USE", SalesTaxRefusal::NameInUse),
    ("ITEM_IN_GROUP", SalesTaxRefusal::ItemInGroup),
    ("CODE_IN_USE", SalesTaxRefusal::CodeInUse),
    ("SYSTEM_CODE", SalesTaxRefusal::SystemCode),
    ("GROUP_MEMBER_NOT_ITEM", SalesTaxRefusal::GroupMemberNotItem),
    ("GROUP_SIZE", SalesTaxRefusal::GroupSize),
    ("AGENCY_REQUIRED", SalesTaxRefusal::AgencyRequired),
    (
        "TAX_RATE_WITH_SALES_TAX",
        SalesTaxRefusal::TaxRateWithSalesTax,
    ),
    ("SALES_TAX_OFF", SalesTaxRefusal::SalesTaxOff),
    ("TAX_ITEM_INACTIVE", SalesTaxRefusal::TaxItemInactive),
];

/// Every engine code [`sales_tax_refusal`] knows.
#[must_use]
pub fn sales_tax_refusal_codes() -> Vec<&'static str> {
    REFUSALS.iter().map(|(c, _)| *c).collect()
}

/// Classifies `e` (`None` for anything else).
#[must_use]
pub fn sales_tax_refusal(e: &ApiError) -> Option<SalesTaxRefusal> {
    if let ApiError::Conflict {
        reason: ConflictReason::Stale,
        ..
    } = e
    {
        return Some(SalesTaxRefusal::Stale);
    }
    let code = error_code(e)?;
    REFUSALS.iter().find(|(c, _)| *c == code).map(|(_, r)| *r)
}

// ---- Shared kinds ----------------------------------------------------------------

/// Which basis sales tax is filed on (Q-P23a: a company setting, accrual
/// by default; not tied to the books' report basis).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SalesTaxBasis {
    /// Tax owed when invoiced (the default).
    #[default]
    Accrual,
    /// Tax owed when collected.
    Cash,
}

impl SalesTaxBasis {
    /// The wire value.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            SalesTaxBasis::Accrual => "accrual",
            SalesTaxBasis::Cash => "cash",
        }
    }
}

/// A sales tax item or a group of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SalesTaxKind {
    /// One rate owed to one agency.
    Item,
    /// 1-10 items charged together (rounded once per document).
    Group,
    /// A kind this version does not know.
    #[serde(other)]
    Other,
}

// ---- Agencies --------------------------------------------------------------------

/// A sales tax agency: a vendor the tax is paid to.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesTaxAgency {
    /// The agency.
    pub id: SalesTaxAgencyId,
    /// Its vendor.
    pub vendor_id: ContactId,
    /// The vendor's name (owner data).
    #[serde(default)]
    pub vendor_name: String,
    /// Active.
    #[serde(default = "yes", deserialize_with = "null_true")]
    pub is_active: bool,
    /// The version to send back as `expectedUpdatedAt`.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

impl fmt::Debug for SalesTaxAgency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesTaxAgency")
            .field("id", &self.id)
            .field("vendor_id", &self.vendor_id)
            .field("is_active", &self.is_active)
            .finish_non_exhaustive()
    }
}

// ---- Items and groups ------------------------------------------------------------

/// One item of a group as the engine lists it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesTaxMember {
    /// The item.
    #[serde(alias = "itemId")]
    pub id: SalesTaxItemId,
    /// Its name (owner data).
    #[serde(default)]
    pub name: Option<String>,
    /// Its rate.
    #[serde(default)]
    pub rate: Option<TaxRate>,
    /// Its agency.
    #[serde(default)]
    pub agency_id: Option<SalesTaxAgencyId>,
}

impl fmt::Debug for SalesTaxMember {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesTaxMember")
            .field("id", &self.id)
            .field("rate", &self.rate)
            .field("agency_id", &self.agency_id)
            .finish_non_exhaustive()
    }
}

/// Members arrive as objects or as bare ids.
fn de_members<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<SalesTaxMember>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum M {
        Id(SalesTaxItemId),
        Full(SalesTaxMember),
    }
    let v: Option<Vec<M>> = Option::deserialize(d)?;
    Ok(v.unwrap_or_default()
        .into_iter()
        .map(|m| match m {
            M::Id(id) => SalesTaxMember {
                id,
                name: None,
                rate: None,
                agency_id: None,
            },
            M::Full(m) => m,
        })
        .collect())
}

/// A sales tax item or group (`GET /sales-tax/items`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesTaxItem {
    /// The item or group.
    pub id: SalesTaxItemId,
    /// Item or group.
    pub kind: SalesTaxKind,
    /// Name (owner data).
    #[serde(default)]
    pub name: String,
    /// Description (owner data; printed on invoices as the tax label).
    #[serde(default)]
    pub description: Option<String>,
    /// The rate: an item's own, a group's the sum of its members (engine
    /// figure).
    pub rate: TaxRate,
    /// The agency (`None` for groups and legacy items without one).
    #[serde(default)]
    pub agency_id: Option<SalesTaxAgencyId>,
    /// The agency's name.
    #[serde(default)]
    pub agency_name: Option<String>,
    /// A group's items.
    #[serde(default, deserialize_with = "de_members")]
    pub members: Vec<SalesTaxMember>,
    /// A legacy item's rate (made from a line rate, Q-P23f): it needs an
    /// agency before Pay Sales Tax.
    #[serde(default)]
    pub legacy_rate: Option<TaxRate>,
    /// Active.
    #[serde(default = "yes", deserialize_with = "null_true")]
    pub is_active: bool,
    /// Used on a document (its rate change applies to new documents).
    #[serde(default, deserialize_with = "null_false")]
    pub in_use: bool,
    /// The version to send back as `expectedUpdatedAt`.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

impl SalesTaxItem {
    /// True for a legacy item (made from a line rate).
    #[must_use]
    pub fn is_legacy(&self) -> bool {
        self.legacy_rate.is_some()
    }

    /// True when Pay Sales Tax can't take its tax yet: a legacy item
    /// without an agency (plan §5).
    #[must_use]
    pub fn needs_agency(&self) -> bool {
        self.kind == SalesTaxKind::Item && self.agency_id.is_none()
    }
}

impl fmt::Debug for SalesTaxItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesTaxItem")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("rate", &self.rate)
            .field("agency_id", &self.agency_id)
            .field("members", &self.members.len())
            .field("is_active", &self.is_active)
            .field("in_use", &self.in_use)
            .finish_non_exhaustive()
    }
}

/// A new or edited sales tax item or group.
#[derive(Clone, PartialEq, Eq)]
pub struct SalesTaxItemDraft {
    /// Item or group.
    pub kind: SalesTaxKind,
    /// Name (1-100 characters, unique among items and groups).
    pub name: String,
    /// Description.
    pub description: Option<String>,
    /// An item's rate (`0 ≤ r < 1`); `None` for a group.
    pub rate: Option<TaxRate>,
    /// An item's agency; `None` for a group.
    pub agency_id: Option<SalesTaxAgencyId>,
    /// A group's items (1-10, items only).
    pub member_ids: Vec<SalesTaxItemId>,
}

impl fmt::Debug for SalesTaxItemDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesTaxItemDraft")
            .field("kind", &self.kind)
            .field("rate", &self.rate)
            .field("agency_id", &self.agency_id)
            .field("member_ids", &self.member_ids.len())
            .finish_non_exhaustive()
    }
}

impl SalesTaxItemDraft {
    /// Checks the draft before anything is sent.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a blank or long name or description, a
    /// rate outside `0 ≤ r < 1`, an item without rate or agency, a group
    /// with a rate, an agency or a repeated member, or a kind this version
    /// does not know; `Local(TooFewLines)` / `Local(TooManyLines)` for a
    /// group outside 1-10 items.
    pub fn check(&self) -> Result<(), ApiError> {
        let bad = || ApiError::Local(LocalValidation::InvalidValue);
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > SALES_TAX_NAME_MAX {
            return Err(bad());
        }
        if too_long(self.description.as_deref(), SALES_TAX_DESCRIPTION_MAX) {
            return Err(bad());
        }
        match self.kind {
            SalesTaxKind::Item => {
                let rate = self.rate.ok_or_else(bad)?;
                if !rate.is_setup_valid() || self.agency_id.is_none() || !self.member_ids.is_empty()
                {
                    return Err(bad());
                }
            }
            SalesTaxKind::Group => {
                if self.rate.is_some() || self.agency_id.is_some() {
                    return Err(bad());
                }
                if self.member_ids.is_empty() {
                    return Err(ApiError::Local(LocalValidation::TooFewLines));
                }
                if self.member_ids.len() > GROUP_MEMBERS_MAX {
                    return Err(ApiError::Local(LocalValidation::TooManyLines));
                }
                let mut ids = self.member_ids.clone();
                ids.sort();
                ids.dedup();
                if ids.len() != self.member_ids.len() {
                    return Err(bad());
                }
            }
            SalesTaxKind::Other => return Err(bad()),
        }
        Ok(())
    }

    /// The request body (checked first).
    ///
    /// # Errors
    /// As [`SalesTaxItemDraft::check`].
    pub fn body(&self) -> Result<Value, ApiError> {
        self.check()?;
        let mut m = serde_json::Map::new();
        m.insert("kind".into(), json!(self.kind));
        m.insert("name".into(), json!(self.name.trim()));
        if let Some(d) = self.description.as_deref().map(str::trim) {
            m.insert("description".into(), json!(d));
        }
        if let Some(r) = self.rate {
            m.insert("rate".into(), json!(r.to_wire()));
        }
        if let Some(a) = self.agency_id {
            m.insert("agencyId".into(), json!(a));
        }
        if self.kind == SalesTaxKind::Group {
            m.insert("memberIds".into(), json!(self.member_ids));
        }
        Ok(Value::Object(m))
    }
}

// ---- Tax codes -------------------------------------------------------------------

/// A tax code (U-25): `Tax`, `Non` (system) and the owner's own (`Res`,
/// `Out`, …). A line's code decides whether it is taxed.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxCode {
    /// The code's id.
    pub id: TaxCodeId,
    /// The code (at most 8 characters).
    pub code: String,
    /// Its name.
    #[serde(default)]
    pub name: String,
    /// Lines with it are taxed.
    pub taxable: bool,
    /// `Tax` or `Non`: can't be edited or deactivated.
    #[serde(default, deserialize_with = "null_false")]
    pub is_system: bool,
    /// Active.
    #[serde(default = "yes", deserialize_with = "null_true")]
    pub is_active: bool,
    /// The version to send back as `expectedUpdatedAt`.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

/// A new or edited tax code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaxCodeDraft {
    /// The code (1-8 characters, unique).
    pub code: String,
    /// Its name.
    pub name: String,
    /// Lines with it are taxed.
    pub taxable: bool,
}

impl TaxCodeDraft {
    /// The request body (checked first).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a blank or long code or name.
    pub fn body(&self) -> Result<Value, ApiError> {
        let code = self.code.trim();
        let name = self.name.trim();
        if code.is_empty()
            || code.chars().count() > TAX_CODE_MAX
            || name.is_empty()
            || name.chars().count() > TAX_CODE_NAME_MAX
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        Ok(json!({"code": code, "name": name, "taxable": self.taxable}))
    }
}

// ---- Owed ------------------------------------------------------------------------

/// What one sales tax item owes as of a date.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwedItem {
    /// The item (`None`: tax not assigned to an item, legacy).
    #[serde(default)]
    pub tax_item_id: Option<SalesTaxItemId>,
    /// Owed (the engine's figure).
    #[serde(deserialize_with = "de_cents")]
    pub owed: Money,
}

impl fmt::Debug for OwedItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwedItem")
            .field("tax_item_id", &self.tax_item_id)
            .finish_non_exhaustive()
    }
}

/// What one agency is owed.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwedAgency {
    /// The agency (`None`: items without one).
    #[serde(default)]
    pub agency_id: Option<SalesTaxAgencyId>,
    /// Per item.
    #[serde(default, deserialize_with = "null_vec")]
    pub items: Vec<OwedItem>,
    /// The agency's total (the engine's figure).
    #[serde(deserialize_with = "de_cents")]
    pub total: Money,
}

impl fmt::Debug for OwedAgency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwedAgency")
            .field("agency_id", &self.agency_id)
            .field("items", &self.items)
            .finish_non_exhaustive()
    }
}

/// `GET /sales-tax/owed` (U-149).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesTaxOwed {
    /// Per agency.
    #[serde(default, deserialize_with = "null_vec")]
    pub agencies: Vec<OwedAgency>,
    /// Sales Tax Payable not assigned to an item (legacy rows).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub unassigned: Money,
    /// Everything owed (the Home footer, plan WP-3).
    #[serde(deserialize_with = "de_cents")]
    pub total: Money,
}

impl fmt::Debug for SalesTaxOwed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesTaxOwed")
            .field("agencies", &self.agencies)
            .finish_non_exhaustive()
    }
}

// ---- Exemption certificates ------------------------------------------------------

/// A client's exemption certificate (U-151).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxExemption {
    /// The certificate.
    pub id: TaxExemptionId,
    /// Its client.
    pub contact_id: ContactId,
    /// The certificate number (owner data: never logged).
    #[serde(default)]
    pub number: String,
    /// The state that issued it (two letters).
    #[serde(default)]
    pub state: Option<String>,
    /// When it expires (`None`: never).
    #[serde(default)]
    pub expires_on: Option<CalendarDate>,
    /// The scanned certificate (same company and client).
    #[serde(default)]
    pub attachment_id: Option<AttachmentId>,
    /// A note (owner data).
    #[serde(default)]
    pub note: Option<String>,
    /// Active.
    #[serde(default = "yes", deserialize_with = "null_true")]
    pub is_active: bool,
    /// The version to send back as `expectedUpdatedAt`.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

impl fmt::Debug for TaxExemption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaxExemption")
            .field("id", &self.id)
            .field("contact_id", &self.contact_id)
            .field("state", &self.state)
            .field("expires_on", &self.expires_on)
            .field("attachment", &self.attachment_id.is_some())
            .field("is_active", &self.is_active)
            .finish_non_exhaustive()
    }
}

/// A new or edited exemption certificate.
#[derive(Clone, PartialEq, Eq)]
pub struct TaxExemptionDraft {
    /// The client.
    pub contact_id: ContactId,
    /// The certificate number (1-50 characters).
    pub number: String,
    /// The issuing state (two letters, upper case), if known.
    pub state: Option<String>,
    /// When it expires.
    pub expires_on: Option<CalendarDate>,
    /// The scanned certificate.
    pub attachment_id: Option<AttachmentId>,
    /// A note.
    pub note: Option<String>,
}

impl fmt::Debug for TaxExemptionDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaxExemptionDraft")
            .field("contact_id", &self.contact_id)
            .field("state", &self.state)
            .field("expires_on", &self.expires_on)
            .finish_non_exhaustive()
    }
}

impl TaxExemptionDraft {
    /// The request body (checked first).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a blank or long number, a state that is
    /// not two letters, or a long note.
    pub fn body(&self) -> Result<Value, ApiError> {
        let bad = || ApiError::Local(LocalValidation::InvalidValue);
        let number = self.number.trim();
        if number.is_empty() || number.chars().count() > EXEMPTION_NUMBER_MAX {
            return Err(bad());
        }
        if too_long(self.note.as_deref(), EXEMPTION_NOTE_MAX) {
            return Err(bad());
        }
        let state = match self.state.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(s) if s.len() == 2 && s.bytes().all(|b| b.is_ascii_alphabetic()) => {
                Some(s.to_ascii_uppercase())
            }
            Some(_) => return Err(bad()),
        };
        Ok(json!({
            "contactId": self.contact_id,
            "number": number,
            "state": state,
            "expiresOn": self.expires_on,
            "attachmentId": self.attachment_id,
            "note": self.note.as_deref().map(str::trim).filter(|n| !n.is_empty()),
        }))
    }
}

/// Where a certificate stands on a date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExemptionStatus {
    /// Past its expiry date.
    Expired,
    /// Expiring within the window.
    Expiring,
    /// A status this version does not know.
    #[serde(other)]
    Other,
}

/// One row of `GET /reports/exemptions-expiring`.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpiringExemption {
    /// The certificate.
    #[serde(alias = "exemptionId")]
    pub id: TaxExemptionId,
    /// Its client.
    pub contact_id: ContactId,
    /// The client's name (owner data).
    #[serde(default)]
    pub contact_name: String,
    /// The certificate number (owner data).
    #[serde(default)]
    pub number: String,
    /// The issuing state.
    #[serde(default)]
    pub state: Option<String>,
    /// When it expires.
    pub expires_on: CalendarDate,
    /// Expired or expiring.
    pub status: ExemptionStatus,
}

impl fmt::Debug for ExpiringExemption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExpiringExemption")
            .field("id", &self.id)
            .field("contact_id", &self.contact_id)
            .field("expires_on", &self.expires_on)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

// ---- A document's sales tax ------------------------------------------------------

/// One item of a document's sales tax with its share of the tax (the
/// engine's largest-remainder split, Q-P23d).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocSalesTaxMember {
    /// The item.
    #[serde(alias = "itemId", alias = "taxItemId")]
    pub id: SalesTaxItemId,
    /// Its name (owner data).
    #[serde(default)]
    pub name: String,
    /// Its rate as saved on the document.
    pub rate: TaxRate,
    /// Its share of the document tax.
    #[serde(default, alias = "amount", deserialize_with = "de_cents_opt")]
    pub tax: Option<Money>,
}

impl fmt::Debug for DocSalesTaxMember {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocSalesTaxMember")
            .field("id", &self.id)
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

/// The sales tax of an invoice, sales receipt or credit memo (`salesTax`;
/// one per document, Q-P23d): the item or group, its combined rate as
/// saved, and the members' shares. Shown, never computed here.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocSalesTax {
    /// The item or group.
    pub id: SalesTaxItemId,
    /// Its name (owner data; "Monett").
    #[serde(default)]
    pub name: String,
    /// The combined rate as saved on the document.
    pub rate: TaxRate,
    /// Item or group.
    #[serde(default)]
    pub kind: Option<SalesTaxKind>,
    /// The members (one for an item).
    #[serde(default, deserialize_with = "null_vec")]
    pub members: Vec<DocSalesTaxMember>,
}

impl fmt::Debug for DocSalesTax {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocSalesTax")
            .field("id", &self.id)
            .field("rate", &self.rate)
            .field("members", &self.members)
            .finish_non_exhaustive()
    }
}

/// Where a line's tax code came from (Q-P23k).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaxCodeSource {
    /// Set on the line.
    Request,
    /// The client's non-taxable code.
    Client,
    /// The item's code.
    Item,
    /// The line's taxable flag.
    IsTaxable,
    /// `Non` by default.
    Default,
    /// A source this version does not know.
    #[serde(other)]
    Other,
}

/// A document line's tax code as the engine resolved it (`taxCode`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineTaxCode {
    /// The code's id (`None` on legacy lines).
    #[serde(default)]
    pub id: Option<TaxCodeId>,
    /// The code (`Tax`, `Non`, `Res`).
    #[serde(default)]
    pub code: String,
    /// The line is taxed.
    pub taxable: bool,
    /// Where it came from.
    #[serde(default)]
    pub source: Option<TaxCodeSource>,
}

/// A sales tax problem a document preview reports (never a 4xx).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DocTaxProblem {
    /// The saved rates differ from the current ones: offer "Use current
    /// rates" (Q-P23i).
    RateChanged,
    /// A non-taxable client has no certificate on file (Q-P23n).
    ExemptionMissing,
    /// The client's certificate has expired.
    ExemptionExpired,
    /// The client's certificate expires within 30 days.
    ExemptionExpiring,
}

const DOC_PROBLEMS: &[(&str, DocTaxProblem)] = &[
    ("RATE_CHANGED", DocTaxProblem::RateChanged),
    ("EXEMPTION_MISSING", DocTaxProblem::ExemptionMissing),
    ("EXEMPTION_EXPIRED", DocTaxProblem::ExemptionExpired),
    ("EXEMPTION_EXPIRING", DocTaxProblem::ExemptionExpiring),
];

/// The sales tax problem a preview problem `code` names, if it is one.
#[must_use]
pub fn doc_tax_problem(code: &str) -> Option<DocTaxProblem> {
    DOC_PROBLEMS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, p)| *p)
}

/// Every code [`doc_tax_problem`] knows.
#[must_use]
pub fn doc_tax_problem_codes() -> Vec<&'static str> {
    DOC_PROBLEMS.iter().map(|(c, _)| *c).collect()
}

// ---- Decoding helpers ------------------------------------------------------------

const fn yes() -> bool {
    true
}

fn null_true<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(d)?.unwrap_or(true))
}

/// A record bare or under `key`.
pub(crate) fn decode_one<T: DeserializeOwned>(
    body: &[u8],
    key: &str,
    what: &'static str,
) -> Result<T, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what })?;
    let inner = match v.get(key) {
        Some(x) if x.is_object() => x.clone(),
        _ => v,
    };
    serde_json::from_value(inner).map_err(|_| ApiError::Decode { what })
}

/// A list bare, under `data` or under `key`.
pub(crate) fn decode_list<T: DeserializeOwned>(
    body: &[u8],
    key: &str,
    what: &'static str,
) -> Result<Vec<T>, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what })?;
    let inner = if v.is_array() {
        v
    } else if let Some(x) = v.get("data").filter(|x| x.is_array()) {
        x.clone()
    } else if let Some(x) = v.get(key).filter(|x| x.is_array()) {
        x.clone()
    } else {
        return Err(ApiError::Decode { what });
    };
    serde_json::from_value(inner).map_err(|_| ApiError::Decode { what })
}

fn with_id(op: &Operation, id: &dyn fmt::Display) -> String {
    op.route().replace("{id}", &id.to_string())
}

// ---- Operations ------------------------------------------------------------------

/// Sales tax operations of a session (`session.sales_tax()`).
#[derive(Clone, Copy, Debug)]
pub struct SalesTax<'a>(pub(crate) &'a Session);

impl Session {
    /// Sales tax setup, owed and exemptions of the open company (P23).
    #[must_use]
    pub fn sales_tax(&self) -> SalesTax<'_> {
        SalesTax(self)
    }
}

impl SalesTax<'_> {
    async fn read(
        &self,
        op: &'static Operation,
        query: Vec<(String, String)>,
    ) -> Result<EngineReply, ApiError> {
        let mut call = Call::at(Method::Get, op.route());
        call.query = query;
        self.0.send(op, call, None).await
    }

    async fn write(
        &self,
        op: &'static Operation,
        path: String,
        body: &Value,
        key: IdempotencyKey,
    ) -> Result<EngineReply, ApiError> {
        let mut call = Call::new(op.method(), op.route(), path).json_body(json_body(body)?);
        call.idempotency_key = Some(key);
        self.0.send(op, call, None).await
    }

    fn inactive_query(include_inactive: bool) -> Vec<(String, String)> {
        if include_inactive {
            vec![("includeInactive".into(), "true".into())]
        } else {
            Vec::new()
        }
    }

    /// The agencies.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `sales-tax-setup-v1`; `Decode`; any
    /// [`ApiError`].
    pub async fn agencies(&self, include_inactive: bool) -> Result<Vec<SalesTaxAgency>, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let reply = self
            .read(&SALES_TAX_AGENCIES, Self::inactive_query(include_inactive))
            .await?;
        decode_list(&reply.body, "agencies", "sales tax agencies")
    }

    /// Makes `vendor` an agency.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `AGENCY_EXISTS`; any [`ApiError`].
    pub async fn create_agency(
        &self,
        vendor: ContactId,
        key: IdempotencyKey,
    ) -> Result<SalesTaxAgency, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let reply = self
            .write(
                &SALES_TAX_AGENCY_CREATE,
                SALES_TAX_AGENCY_CREATE.route().to_string(),
                &json!({"vendorId": vendor}),
                key,
            )
            .await?;
        decode_one(&reply.body, "agency", "sales tax agency")
    }

    /// Moves an agency to another vendor if it is still at `expected`.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `STALE`, `AGENCY_EXISTS`; any [`ApiError`].
    pub async fn update_agency(
        &self,
        id: SalesTaxAgencyId,
        vendor: ContactId,
        expected: Option<Timestamp>,
        key: IdempotencyKey,
    ) -> Result<SalesTaxAgency, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let body = with_version(json!({"vendorId": vendor}), expected);
        let reply = self
            .write(
                &SALES_TAX_AGENCY_PUT,
                with_id(&SALES_TAX_AGENCY_PUT, &id),
                &body,
                key,
            )
            .await?;
        decode_one(&reply.body, "agency", "sales tax agency")
    }

    /// Deactivates (`active: false`) or reactivates an agency.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `AGENCY_IN_USE`; any [`ApiError`].
    pub async fn set_agency_active(
        &self,
        id: SalesTaxAgencyId,
        active: bool,
        key: IdempotencyKey,
    ) -> Result<SalesTaxAgency, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let op = if active {
            &SALES_TAX_AGENCY_REACTIVATE
        } else {
            &SALES_TAX_AGENCY_DEACTIVATE
        };
        let reply = self.write(op, with_id(op, &id), &json!({}), key).await?;
        decode_one(&reply.body, "agency", "sales tax agency")
    }

    /// The sales tax items and groups.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Decode`; any [`ApiError`].
    pub async fn items(&self, include_inactive: bool) -> Result<Vec<SalesTaxItem>, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let reply = self
            .read(&SALES_TAX_ITEMS, Self::inactive_query(include_inactive))
            .await?;
        decode_list(&reply.body, "items", "sales tax items")
    }

    /// Creates an item or group.
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)` ([`SalesTaxItemDraft::check`]); from
    /// the engine see [`sales_tax_refusal`]; any [`ApiError`].
    pub async fn create_item(
        &self,
        d: &SalesTaxItemDraft,
        key: IdempotencyKey,
    ) -> Result<SalesTaxItem, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let body = d.body()?;
        let reply = self
            .write(
                &SALES_TAX_ITEM_CREATE,
                SALES_TAX_ITEM_CREATE.route().to_string(),
                &body,
                key,
            )
            .await?;
        decode_one(&reply.body, "item", "sales tax item")
    }

    /// Edits an item or group if it is still at `expected`. A rate or
    /// agency change applies to new documents only (Q-P23i, Q-P23v).
    ///
    /// # Errors
    /// As [`SalesTax::create_item`], plus `STALE`.
    pub async fn update_item(
        &self,
        id: SalesTaxItemId,
        d: &SalesTaxItemDraft,
        expected: Option<Timestamp>,
        key: IdempotencyKey,
    ) -> Result<SalesTaxItem, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let body = with_version(d.body()?, expected);
        let reply = self
            .write(
                &SALES_TAX_ITEM_PUT,
                with_id(&SALES_TAX_ITEM_PUT, &id),
                &body,
                key,
            )
            .await?;
        decode_one(&reply.body, "item", "sales tax item")
    }

    /// Deactivates or reactivates an item or group.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `ITEM_IN_GROUP`; any [`ApiError`].
    pub async fn set_item_active(
        &self,
        id: SalesTaxItemId,
        active: bool,
        key: IdempotencyKey,
    ) -> Result<SalesTaxItem, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let op = if active {
            &SALES_TAX_ITEM_REACTIVATE
        } else {
            &SALES_TAX_ITEM_DEACTIVATE
        };
        let reply = self.write(op, with_id(op, &id), &json!({}), key).await?;
        decode_one(&reply.body, "item", "sales tax item")
    }

    /// The tax codes.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Decode`; any [`ApiError`].
    pub async fn codes(&self, include_inactive: bool) -> Result<Vec<TaxCode>, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let reply = self
            .read(&SALES_TAX_CODES, Self::inactive_query(include_inactive))
            .await?;
        decode_list(&reply.body, "codes", "tax codes")
    }

    /// Creates a tax code.
    ///
    /// # Errors
    /// `Local(InvalidValue)` ([`TaxCodeDraft::body`]); `CODE_IN_USE` for a
    /// taken code; any [`ApiError`].
    pub async fn create_code(
        &self,
        d: &TaxCodeDraft,
        key: IdempotencyKey,
    ) -> Result<TaxCode, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let body = d.body()?;
        let reply = self
            .write(
                &SALES_TAX_CODE_CREATE,
                SALES_TAX_CODE_CREATE.route().to_string(),
                &body,
                key,
            )
            .await?;
        decode_one(&reply.body, "code", "tax code")
    }

    /// Edits a tax code if it is still at `expected`.
    ///
    /// # Errors
    /// As [`SalesTax::create_code`], plus `STALE`, `SYSTEM_CODE`.
    pub async fn update_code(
        &self,
        id: TaxCodeId,
        d: &TaxCodeDraft,
        expected: Option<Timestamp>,
        key: IdempotencyKey,
    ) -> Result<TaxCode, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let body = with_version(d.body()?, expected);
        let reply = self
            .write(
                &SALES_TAX_CODE_PUT,
                with_id(&SALES_TAX_CODE_PUT, &id),
                &body,
                key,
            )
            .await?;
        decode_one(&reply.body, "code", "tax code")
    }

    /// Deactivates a tax code.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `CODE_IN_USE`, `SYSTEM_CODE`; any
    /// [`ApiError`].
    pub async fn deactivate_code(
        &self,
        id: TaxCodeId,
        key: IdempotencyKey,
    ) -> Result<TaxCode, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let reply = self
            .write(
                &SALES_TAX_CODE_DEACTIVATE,
                with_id(&SALES_TAX_CODE_DEACTIVATE, &id),
                &json!({}),
                key,
            )
            .await?;
        decode_one(&reply.body, "code", "tax code")
    }

    /// Reactivates a tax code.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound`; any [`ApiError`].
    pub async fn reactivate_code(
        &self,
        id: TaxCodeId,
        key: IdempotencyKey,
    ) -> Result<TaxCode, ApiError> {
        require(self.0, Capabilities::SALES_TAX_SETUP)?;
        let reply = self
            .write(
                &SALES_TAX_CODE_REACTIVATE,
                with_id(&SALES_TAX_CODE_REACTIVATE, &id),
                &json!({}),
                key,
            )
            .await?;
        decode_one(&reply.body, "code", "tax code")
    }

    /// What is owed as of `as_of` (`basis`: `None` for the company's;
    /// `agency`: `None` for every agency).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `sales-tax-liability-v2`; `Decode`;
    /// any [`ApiError`].
    pub async fn owed(
        &self,
        as_of: CalendarDate,
        basis: Option<SalesTaxBasis>,
        agency: Option<SalesTaxAgencyId>,
    ) -> Result<SalesTaxOwed, ApiError> {
        require(self.0, Capabilities::SALES_TAX_LIABILITY)?;
        let mut q = vec![("asOf".to_string(), as_of.to_string())];
        if let Some(b) = basis {
            q.push(("basis".into(), b.as_wire().into()));
        }
        if let Some(a) = agency {
            q.push(("agencyId".into(), a.to_string()));
        }
        let reply = self.read(&SALES_TAX_OWED, q).await?;
        decode_one(&reply.body, "owed", "sales tax owed")
    }

    /// A client's certificates (`contact: None` for every client).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `tax-exemptions-v1`; `Decode`; any
    /// [`ApiError`].
    pub async fn exemptions(
        &self,
        contact: Option<ContactId>,
    ) -> Result<Vec<TaxExemption>, ApiError> {
        require(self.0, Capabilities::TAX_EXEMPTIONS)?;
        let q = contact
            .map(|c| vec![("contactId".to_string(), c.to_string())])
            .unwrap_or_default();
        let reply = self.read(&SALES_TAX_EXEMPTIONS, q).await?;
        decode_list(&reply.body, "exemptions", "exemption certificates")
    }

    /// Records a certificate.
    ///
    /// # Errors
    /// `Local(InvalidValue)` ([`TaxExemptionDraft::body`]); any
    /// [`ApiError`].
    pub async fn create_exemption(
        &self,
        d: &TaxExemptionDraft,
        key: IdempotencyKey,
    ) -> Result<TaxExemption, ApiError> {
        require(self.0, Capabilities::TAX_EXEMPTIONS)?;
        let body = d.body()?;
        let reply = self
            .write(
                &SALES_TAX_EXEMPTION_CREATE,
                SALES_TAX_EXEMPTION_CREATE.route().to_string(),
                &body,
                key,
            )
            .await?;
        decode_one(&reply.body, "exemption", "exemption certificate")
    }

    /// Edits a certificate if it is still at `expected`.
    ///
    /// # Errors
    /// As [`SalesTax::create_exemption`], plus `STALE`.
    pub async fn update_exemption(
        &self,
        id: TaxExemptionId,
        d: &TaxExemptionDraft,
        expected: Option<Timestamp>,
        key: IdempotencyKey,
    ) -> Result<TaxExemption, ApiError> {
        require(self.0, Capabilities::TAX_EXEMPTIONS)?;
        let body = with_version(d.body()?, expected);
        let reply = self
            .write(
                &SALES_TAX_EXEMPTION_PUT,
                with_id(&SALES_TAX_EXEMPTION_PUT, &id),
                &body,
                key,
            )
            .await?;
        decode_one(&reply.body, "exemption", "exemption certificate")
    }

    /// Deactivates a certificate.
    ///
    /// # Errors
    /// `Local(Unsupported)`; any [`ApiError`].
    pub async fn deactivate_exemption(
        &self,
        id: TaxExemptionId,
        key: IdempotencyKey,
    ) -> Result<TaxExemption, ApiError> {
        require(self.0, Capabilities::TAX_EXEMPTIONS)?;
        let reply = self
            .write(
                &SALES_TAX_EXEMPTION_DEACTIVATE,
                with_id(&SALES_TAX_EXEMPTION_DEACTIVATE, &id),
                &json!({}),
                key,
            )
            .await?;
        decode_one(&reply.body, "exemption", "exemption certificate")
    }

    /// Reactivates an exemption certificate.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound`; any [`ApiError`].
    pub async fn reactivate_exemption(
        &self,
        id: TaxExemptionId,
        key: IdempotencyKey,
    ) -> Result<TaxExemption, ApiError> {
        require(self.0, Capabilities::TAX_EXEMPTIONS)?;
        let reply = self
            .write(
                &SALES_TAX_EXEMPTION_REACTIVATE,
                with_id(&SALES_TAX_EXEMPTION_REACTIVATE, &id),
                &json!({}),
                key,
            )
            .await?;
        decode_one(&reply.body, "exemption", "exemption certificate")
    }

    /// Certificates expired as of `as_of` or expiring within `within_days`
    /// (the engine's default is 60).
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Decode`; any [`ApiError`].
    pub async fn exemptions_expiring(
        &self,
        as_of: CalendarDate,
        within_days: Option<u16>,
    ) -> Result<Vec<ExpiringExemption>, ApiError> {
        require(self.0, Capabilities::TAX_EXEMPTIONS)?;
        let mut q = vec![("asOf".to_string(), as_of.to_string())];
        if let Some(w) = within_days {
            q.push(("within".into(), w.to_string()));
        }
        let reply = self.read(&REPORT_EXEMPTIONS_EXPIRING, q).await?;
        decode_list(&reply.body, "rows", "expiring exemption certificates")
    }
}

#[cfg(test)]
#[path = "sales_tax_tests.rs"]
pub(crate) mod tests;
