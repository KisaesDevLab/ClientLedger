//! The demo "engine" for sales tax (P23 WP-2, U-147/U-148/U-25): agencies,
//! sales tax items and groups, tax codes, the company's switches, the
//! document rule (one sales tax per document, rounded once, the cents
//! spread over the group's members by largest remainder, Q-P23d), the line
//! tax code order (Q-P23k as E1 built it), the stored tax rows each posted
//! document writes, and the accrual liability and owed figures read from
//! those rows. Held to the non-engineOnly cases of
//! `fixtures/sales_tax/golden.json`.
//!
//! `--demo` only; every figure the screens show comes from here. No floats.
//! The seed is synthetic: Monett = State 4.225 % + City 3.875 % (the company
//! default), Springfield 8.875 %, codes Tax, Non and Res, two sample
//! agencies. A new ClientLedger company, so Streamlined Sales Tax is on
//! (Q-SPF6).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use cl_api::{
    ApiError, SalesTaxAgency, SalesTaxBasis, SalesTaxItem, SalesTaxItemDraft, SalesTaxKind,
    SalesTaxMember, TaxCode, TaxCodeDraft,
};
use cl_core::ids::{ContactId, SalesTaxAgencyId, SalesTaxItemId, TaxCodeId, TransactionId};
use cl_core::{CalendarDate, TaxRate};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use super::DemoState;
use super::invoices::round;

/// The state agency's vendor (synthetic).
pub const STATE_AGENCY: &str = "Sample State Revenue Dept.";
/// The city agency's vendor (synthetic).
pub const CITY_AGENCY: &str = "Sample City Revenue Office";

/// A demo sales tax item id.
#[must_use]
pub fn item_id(n: u32) -> SalesTaxItemId {
    SalesTaxItemId::parse(&format!("d7000000-0000-4000-8000-{n:012}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

/// A demo agency id.
#[must_use]
pub fn agency_id(n: u32) -> SalesTaxAgencyId {
    SalesTaxAgencyId::parse(&format!("d7100000-0000-4000-8000-{n:012}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

/// A demo tax code id.
#[must_use]
pub fn code_id(n: u32) -> TaxCodeId {
    TaxCodeId::parse(&format!("d7200000-0000-4000-8000-{n:012}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

/// State (4.225 %).
pub const STATE: u32 = 1;
/// City (3.875 %).
pub const CITY: u32 = 2;
/// Springfield (8.875 %).
pub const SPRINGFIELD: u32 = 3;
/// The Monett group (State + City), the company default.
pub const MONETT: u32 = 4;
/// The `Tax` code.
pub const TAX: u32 = 1;
/// The `Non` code.
pub const NON: u32 = 2;
/// The `Res` (resale) code.
pub const RES: u32 = 3;

/// A sales tax item or group as the demo keeps it.
#[derive(Clone, PartialEq, Eq)]
struct Stored {
    id: SalesTaxItemId,
    group: bool,
    name: String,
    description: Option<String>,
    /// An item's own rate (fraction, ≤ 6 places).
    rate: Decimal,
    agency: Option<SalesTaxAgencyId>,
    members: Vec<SalesTaxItemId>,
    active: bool,
    version: u32,
}

/// What a stored tax row is for (only sales in WP-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A sales document's tax.
    Sale,
}

/// The kind of document a row belongs to (the cash rules differ).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    /// An invoice: collected on cash as it is paid (running total).
    Invoice,
    /// A credit memo (not on cash: it settles invoices, never cash).
    Memo,
    /// A sales receipt: cash on its date.
    Receipt,
    /// A daily sales summary: cash on its date.
    Daily,
}

/// One stored tax row (`transaction_tax_lines`): written once per post,
/// rewritten on an edit, dropped on a void.
#[derive(Clone, PartialEq, Eq)]
pub struct TaxRow {
    /// What it is for.
    pub kind: RowKind,
    /// The document's kind.
    pub doc: DocKind,
    /// The document date.
    pub event_date: CalendarDate,
    /// The document's sales tax (item or group).
    pub sales_tax: Option<SalesTaxItemId>,
    /// The item (a group's member); `None`: unassigned.
    pub tax_item: Option<SalesTaxItemId>,
    /// The agency at posting time (Q-P23v).
    pub agency: Option<SalesTaxAgencyId>,
    /// A non-taxable row's code (tax 0.00).
    pub code: Option<TaxCodeId>,
    /// The rate.
    pub rate: Decimal,
    /// Taxable (or non-taxable) sales, signed.
    pub base: Decimal,
    /// The tax, signed by liability effect.
    pub tax: Decimal,
}

impl fmt::Debug for TaxRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaxRow")
            .field("kind", &self.kind)
            .field("tax_item", &self.tax_item)
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}

/// The demo company's sales tax setup and rows.
#[derive(Clone)]
pub struct DemoSalesTax {
    /// Sales tax on (`salesTaxEnabled`).
    pub enabled: bool,
    /// The filing basis (Q-P23a).
    pub basis: SalesTaxBasis,
    /// Streamlined Sales Tax rules (on: a new ClientLedger company).
    pub ssuta: bool,
    /// The company's default sales tax.
    pub default_id: Option<SalesTaxItemId>,
    agencies: Vec<(SalesTaxAgencyId, ContactId, bool, u32)>,
    items: Vec<Stored>,
    codes: Vec<TaxCode>,
    /// Each posted document's rows.
    pub rows: BTreeMap<TransactionId, Vec<TaxRow>>,
    /// Each posted document's sales tax as saved (its record's salesTax).
    pub doc_tax: BTreeMap<TransactionId, cl_api::DocSalesTax>,
    /// Setup and settings writes (tests).
    pub writes: u32,
    /// Payments, recoveries and certificates (WP-3).
    pub book: super::sales_tax_owed::DemoTaxBook,
    seeded: bool,
}

impl fmt::Debug for DemoSalesTax {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoSalesTax")
            .field("enabled", &self.enabled)
            .field("items", &self.items.len())
            .field("rows", &self.rows.len())
            .finish_non_exhaustive()
    }
}

/// A 2-place amount string (`0.00`).
fn s2(d: Decimal) -> String {
    format!("{:.2}", round(d, 2))
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap_or(Decimal::ZERO)
}

fn code(n: u32, c: &str, name: &str, taxable: bool, system: bool) -> TaxCode {
    TaxCode {
        id: code_id(n),
        code: c.into(),
        name: name.into(),
        taxable,
        is_system: system,
        is_active: true,
        updated_at: Some(DemoState::now()),
    }
}

impl Default for DemoSalesTax {
    fn default() -> Self {
        let item = |n: u32, name: &str, rate: &str, agency: u32| Stored {
            id: item_id(n),
            group: false,
            name: name.into(),
            description: None,
            rate: dec(rate),
            agency: Some(agency_id(agency)),
            members: Vec::new(),
            active: true,
            version: 1,
        };
        DemoSalesTax {
            enabled: true,
            basis: SalesTaxBasis::Accrual,
            ssuta: true,
            default_id: Some(item_id(MONETT)),
            agencies: Vec::new(),
            items: vec![
                item(STATE, "State", "0.042250", 1),
                item(CITY, "City", "0.038750", 2),
                item(SPRINGFIELD, "Springfield", "0.088750", 1),
                Stored {
                    id: item_id(MONETT),
                    group: true,
                    name: "Monett".into(),
                    description: Some("Sales tax".into()),
                    rate: Decimal::ZERO,
                    agency: None,
                    members: vec![item_id(STATE), item_id(CITY)],
                    active: true,
                    version: 1,
                },
            ],
            codes: vec![
                code(TAX, "Tax", "Taxable", true, true),
                code(NON, "Non", "Non-taxable", false, true),
                code(RES, "Res", "Resale", false, false),
            ],
            rows: BTreeMap::new(),
            doc_tax: BTreeMap::new(),
            writes: 0,
            book: super::sales_tax_owed::DemoTaxBook::default(),
            seeded: false,
        }
    }
}

/// The agencies' vendors, found (or added) among the demo contacts.
fn ensure(state: &mut DemoState) {
    if state.sales_tax.seeded {
        return;
    }
    let mut ids = Vec::new();
    for name in [STATE_AGENCY, CITY_AGENCY] {
        let contacts = state.contacts_mut();
        let found = contacts
            .iter()
            .find(|c| c.fields.display_name == name)
            .map(|c| c.id);
        let id = if let Some(id) = found {
            id
        } else {
            {
                let n = u32::try_from(contacts.len() + 1).unwrap_or(u32::MAX);
                let mut f = cl_api::ContactFields::named(cl_api::ContactType::Vendor, name);
                f.billing_line1 = Some("PO Box 3366".into());
                f.billing_city = Some("Jefferson City".into());
                f.billing_state = Some("MO".into());
                f.billing_zip = Some("65105".into());
                f.billing_country = Some("US".into());
                let id = super::contacts::contact_id(n);
                contacts.push(cl_api::Contact {
                    id,
                    fields: f,
                    is_active: true,
                    opening_balance: Some(cl_core::Money::ZERO),
                    opening_balance_date: None,
                    created_at: Some(DemoState::now()),
                    updated_at: Some(DemoState::now()),
                });
                id
            }
        };
        ids.push(id);
    }
    let st = &mut state.sales_tax;
    st.agencies = ids
        .into_iter()
        .enumerate()
        .map(|(i, v)| (agency_id(u32::try_from(i + 1).unwrap_or(1)), v, true, 1))
        .collect();
    st.seeded = true;
}

fn refused(status: u16, c: &str) -> ApiError {
    cl_api::engine_refusal(
        status,
        &json!({"error": {"message": "refused", "code": c, "details": {}}}),
    )
}

fn version(n: u32) -> cl_core::Timestamp {
    let s = format!("2026-11-04T08:{:02}:{:02}.000Z", 10 + n / 60 % 50, n % 60);
    cl_core::Timestamp::parse_wire(&s).unwrap_or_else(|_| DemoState::now())
}

fn rate6(d: Decimal) -> TaxRate {
    TaxRate::parse_wire(&round(d, 6).to_string()).unwrap_or_else(|_| {
        TaxRate::from_millionths(0).unwrap_or_else(|_| unreachable!("zero is a rate"))
    })
}

impl DemoSalesTax {
    fn find(&self, id: SalesTaxItemId) -> Option<&Stored> {
        self.items.iter().find(|i| i.id == id)
    }

    /// An item's rate, a group's the sum of its members'.
    fn rate_of(&self, s: &Stored) -> Decimal {
        if s.group {
            s.members
                .iter()
                .filter_map(|m| self.find(*m))
                .map(|m| m.rate)
                .sum()
        } else {
            s.rate
        }
    }

    fn agency_name(&self, state_contacts: &[cl_api::Contact], id: SalesTaxAgencyId) -> String {
        self.agencies
            .iter()
            .find(|a| a.0 == id)
            .and_then(|a| state_contacts.iter().find(|c| c.id == a.1))
            .map(|c| c.fields.display_name.clone())
            .unwrap_or_default()
    }

    fn in_use(&self, id: SalesTaxItemId) -> bool {
        self.rows
            .values()
            .flatten()
            .any(|r| r.sales_tax == Some(id) || r.tax_item == Some(id))
    }

    /// A tax code.
    #[must_use]
    pub fn code(&self, id: TaxCodeId) -> Option<&TaxCode> {
        self.codes.iter().find(|c| c.id == id)
    }

    /// A tax item's or group's name.
    #[must_use]
    pub fn item_name(&self, id: SalesTaxItemId) -> Option<&str> {
        self.find(id).map(|i| i.name.as_str())
    }

    /// An item's own rate (a group's: its members').
    #[must_use]
    pub fn item_rate(&self, id: SalesTaxItemId) -> Option<Decimal> {
        self.find(id).map(|i| self.rate_of(i))
    }

    /// The tax items (never groups) paid to `agency`, active or not.
    #[must_use]
    pub fn agency_items(&self, agency: SalesTaxAgencyId) -> Vec<SalesTaxItemId> {
        self.items
            .iter()
            .filter(|i| !i.group && i.agency == Some(agency))
            .map(|i| i.id)
            .collect()
    }

    /// The agency's vendor.
    #[must_use]
    pub fn agency_vendor(&self, agency: SalesTaxAgencyId) -> Option<ContactId> {
        self.agencies.iter().find(|a| a.0 == agency).map(|a| a.1)
    }

    /// Every agency, in setup order.
    #[must_use]
    pub fn agency_ids(&self) -> Vec<SalesTaxAgencyId> {
        self.agencies.iter().map(|a| a.0).collect()
    }
}

/// The developer command dev.demo_sales_tax_reset (the scenario runs
/// light and dark in one app): the setup and switches go back to the seed;
/// posted documents keep their rows.
pub fn reset(state: &mut DemoState) {
    let rows = std::mem::take(&mut state.sales_tax.rows);
    let doc_tax = std::mem::take(&mut state.sales_tax.doc_tax);
    let book = std::mem::take(&mut state.sales_tax.book);
    state.sales_tax = DemoSalesTax {
        rows,
        doc_tax,
        book,
        ..DemoSalesTax::default()
    };
    ensure(state);
}

// ---- Setup -------------------------------------------------------------------------

/// `GET /sales-tax/agencies`.
pub fn agencies(state: &mut DemoState, include_inactive: bool) -> Vec<SalesTaxAgency> {
    ensure(state);
    let contacts = state.contacts_mut().clone();
    let st = &state.sales_tax;
    st.agencies
        .iter()
        .filter(|a| include_inactive || a.2)
        .map(|(id, vendor, active, v)| SalesTaxAgency {
            id: *id,
            vendor_id: *vendor,
            vendor_name: st.agency_name(&contacts, *id),
            is_active: *active,
            updated_at: Some(version(*v)),
        })
        .collect()
}

/// `POST /sales-tax/agencies`.
///
/// # Errors
/// 409 `AGENCY_EXISTS`; 400 for a contact that is not a vendor.
pub fn create_agency(state: &mut DemoState, vendor: ContactId) -> Result<SalesTaxAgency, ApiError> {
    ensure(state);
    let is_vendor = state.contacts_mut().iter().any(|c| {
        c.id == vendor
            && matches!(
                c.fields.contact_type,
                cl_api::ContactType::Vendor | cl_api::ContactType::Both
            )
    });
    if !is_vendor {
        return Err(refused(400, "VALIDATION_ERROR"));
    }
    let st = &mut state.sales_tax;
    if st.agencies.iter().any(|a| a.1 == vendor) {
        return Err(refused(409, "AGENCY_EXISTS"));
    }
    let id = agency_id(u32::try_from(st.agencies.len() + 1).unwrap_or(u32::MAX));
    st.agencies.push((id, vendor, true, 1));
    st.writes += 1;
    agencies(state, true)
        .into_iter()
        .find(|a| a.id == id)
        .ok_or(ApiError::Internal)
}

/// Deactivates or reactivates an agency.
///
/// # Errors
/// 404; 409 `AGENCY_IN_USE` while an active tax is paid to it.
pub fn set_agency_active(
    state: &mut DemoState,
    id: SalesTaxAgencyId,
    active: bool,
) -> Result<SalesTaxAgency, ApiError> {
    ensure(state);
    let st = &mut state.sales_tax;
    if !active && st.items.iter().any(|i| i.active && i.agency == Some(id)) {
        return Err(refused(409, "AGENCY_IN_USE"));
    }
    let a = st
        .agencies
        .iter_mut()
        .find(|a| a.0 == id)
        .ok_or(ApiError::NotFound { code: None })?;
    a.2 = active;
    a.3 += 1;
    st.writes += 1;
    agencies(state, true)
        .into_iter()
        .find(|a| a.id == id)
        .ok_or(ApiError::Internal)
}

/// `GET /sales-tax/items`.
pub fn items(state: &mut DemoState, include_inactive: bool) -> Vec<SalesTaxItem> {
    ensure(state);
    let contacts = state.contacts_mut().clone();
    let st = &state.sales_tax;
    st.items
        .iter()
        .filter(|i| include_inactive || i.active)
        .map(|i| SalesTaxItem {
            id: i.id,
            kind: if i.group {
                SalesTaxKind::Group
            } else {
                SalesTaxKind::Item
            },
            name: i.name.clone(),
            description: i.description.clone(),
            rate: rate6(st.rate_of(i)),
            agency_id: i.agency,
            agency_name: i.agency.map(|a| st.agency_name(&contacts, a)),
            members: i
                .members
                .iter()
                .filter_map(|m| st.find(*m))
                .map(|m| SalesTaxMember {
                    id: m.id,
                    name: Some(m.name.clone()),
                    rate: Some(rate6(m.rate)),
                    agency_id: m.agency,
                })
                .collect(),
            legacy_rate: None,
            is_active: i.active,
            in_use: st.in_use(i.id),
            updated_at: Some(version(i.version)),
        })
        .collect()
}

/// `POST /sales-tax/items` (`id: None`) or `PUT /sales-tax/items/:id`. A
/// change applies to new documents only: posted rows keep their rates and
/// agencies (Q-P23i, Q-P23v).
///
/// # Errors
/// `Local(…)` from the draft check; 400 `GROUP_MEMBER_NOT_ITEM`,
/// `AGENCY_REQUIRED`; 409 `NAME_IN_USE`, `STALE`; 404.
pub fn save_item(
    state: &mut DemoState,
    id: Option<SalesTaxItemId>,
    d: &SalesTaxItemDraft,
    expected: Option<cl_core::Timestamp>,
) -> Result<SalesTaxItem, ApiError> {
    ensure(state);
    d.check()?;
    let st = &mut state.sales_tax;
    let name = d.name.trim().to_string();
    if st
        .items
        .iter()
        .any(|i| Some(i.id) != id && i.name.to_lowercase() == name.to_lowercase())
    {
        return Err(refused(409, "NAME_IN_USE"));
    }
    let group = d.kind == SalesTaxKind::Group;
    if group
        && d.member_ids
            .iter()
            .any(|m| st.find(*m).is_none_or(|x| x.group || Some(x.id) == id))
    {
        return Err(refused(400, "GROUP_MEMBER_NOT_ITEM"));
    }
    if let Some(a) = d.agency_id
        && !st.agencies.iter().any(|x| x.0 == a)
    {
        return Err(refused(400, "AGENCY_REQUIRED"));
    }
    let new_id = match id {
        Some(id) => {
            let cur = st.find(id).ok_or(ApiError::NotFound { code: None })?;
            if expected.is_some_and(|e| e != version(cur.version)) {
                return Err(refused(409, "STALE"));
            }
            id
        }
        None => item_id(u32::try_from(st.items.len() + 1).unwrap_or(u32::MAX)),
    };
    let stored = Stored {
        id: new_id,
        group,
        name,
        description: d
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        rate: d.rate.map_or(Decimal::ZERO, TaxRate::as_decimal),
        agency: d.agency_id,
        members: d.member_ids.clone(),
        active: true,
        version: 1,
    };
    match st.items.iter_mut().find(|i| i.id == new_id) {
        Some(i) => {
            *i = Stored {
                active: i.active,
                version: i.version + 1,
                ..stored
            };
        }
        None => st.items.push(stored),
    }
    st.writes += 1;
    items(state, true)
        .into_iter()
        .find(|i| i.id == new_id)
        .ok_or(ApiError::Internal)
}

/// Deactivates or reactivates a tax item or group (never deleted).
///
/// # Errors
/// 404; 409 `ITEM_IN_GROUP` while an active group holds the item.
pub fn set_item_active(
    state: &mut DemoState,
    id: SalesTaxItemId,
    active: bool,
) -> Result<SalesTaxItem, ApiError> {
    ensure(state);
    let st = &mut state.sales_tax;
    if !active
        && st
            .items
            .iter()
            .any(|g| g.group && g.active && g.members.contains(&id))
    {
        return Err(refused(409, "ITEM_IN_GROUP"));
    }
    let i = st
        .items
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or(ApiError::NotFound { code: None })?;
    i.active = active;
    i.version += 1;
    if !active && st.default_id == Some(id) {
        st.default_id = None;
    }
    st.writes += 1;
    items(state, true)
        .into_iter()
        .find(|i| i.id == id)
        .ok_or(ApiError::Internal)
}

/// `GET /sales-tax/codes`.
pub fn codes(state: &mut DemoState, include_inactive: bool) -> Vec<TaxCode> {
    ensure(state);
    state
        .sales_tax
        .codes
        .iter()
        .filter(|c| include_inactive || c.is_active)
        .cloned()
        .collect()
}

/// `POST /sales-tax/codes` (`id: None`) or `PUT /sales-tax/codes/:id`.
///
/// # Errors
/// `Local(InvalidValue)`; 409 `CODE_IN_USE`, `SYSTEM_CODE`; 404.
pub fn save_code(
    state: &mut DemoState,
    id: Option<TaxCodeId>,
    d: &TaxCodeDraft,
) -> Result<TaxCode, ApiError> {
    ensure(state);
    d.body()?;
    let st = &mut state.sales_tax;
    let c = d.code.trim().to_string();
    if st
        .codes
        .iter()
        .any(|x| Some(x.id) != id && x.code.eq_ignore_ascii_case(&c))
    {
        return Err(refused(409, "CODE_IN_USE"));
    }
    let new_id = match id {
        Some(id) => {
            let cur = st.code(id).ok_or(ApiError::NotFound { code: None })?;
            if cur.is_system {
                return Err(refused(409, "SYSTEM_CODE"));
            }
            id
        }
        None => code_id(u32::try_from(st.codes.len() + 1).unwrap_or(u32::MAX)),
    };
    let made = TaxCode {
        id: new_id,
        code: c,
        name: d.name.trim().to_string(),
        taxable: d.taxable,
        is_system: false,
        is_active: true,
        updated_at: Some(DemoState::now()),
    };
    match st.codes.iter_mut().find(|x| x.id == new_id) {
        Some(x) => *x = made.clone(),
        None => st.codes.push(made.clone()),
    }
    st.writes += 1;
    Ok(made)
}

/// Deactivates a tax code.
///
/// # Errors
/// 404; 409 `SYSTEM_CODE`.
pub fn deactivate_code(state: &mut DemoState, id: TaxCodeId) -> Result<TaxCode, ApiError> {
    ensure(state);
    let st = &mut state.sales_tax;
    let c = st
        .codes
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or(ApiError::NotFound { code: None })?;
    if c.is_system {
        return Err(refused(409, "SYSTEM_CODE"));
    }
    c.is_active = false;
    st.writes += 1;
    Ok(c.clone())
}

/// `PUT /company/settings` with the sales tax fields.
///
/// # Errors
/// 400 for an unknown or inactive default.
pub fn save_settings(
    state: &mut DemoState,
    u: &cl_api::SalesTaxSettingsUpdate,
) -> Result<(), ApiError> {
    ensure(state);
    let st = &mut state.sales_tax;
    if let Some(Some(d)) = u.default_sales_tax_id
        && st.find(d).is_none_or(|i| !i.active)
    {
        return Err(refused(400, "VALIDATION_ERROR"));
    }
    if let Some(b) = u.enabled {
        st.enabled = b;
    }
    if let Some(b) = u.basis {
        st.basis = b;
    }
    if let Some(d) = u.default_sales_tax_id {
        st.default_id = d;
    }
    if let Some(b) = u.ssuta {
        st.ssuta = b;
    }
    st.writes += 1;
    Ok(())
}

/// The company settings' sales tax fields as the engine sends them.
#[must_use]
pub fn settings_json(state: &DemoState) -> Value {
    let st = &state.sales_tax;
    json!({
        "salesTaxEnabled": st.enabled,
        "salesTaxBasis": st.basis.as_wire(),
        "defaultSalesTaxId": st.default_id,
        "salesTaxSsuta": st.ssuta,
    })
}

// ---- Documents ---------------------------------------------------------------------

/// The sales tax a document charges, resolved.
#[derive(Clone, PartialEq, Eq)]
pub struct Resolved {
    id: SalesTaxItemId,
    name: String,
    group: bool,
    /// The combined rate.
    pub rate: Decimal,
    /// The members: id, name, rate, agency.
    members: Vec<(SalesTaxItemId, String, Decimal, Option<SalesTaxAgencyId>)>,
}

impl fmt::Debug for Resolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resolved")
            .field("id", &self.id)
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

/// The document's sales tax: the request's, else the client's default,
/// else the company's (Q-P23d). `None` when sales tax is off or nothing is
/// set.
///
/// # Errors
/// 400 `SALES_TAX_OFF` for a request while tax is off; 400
/// `TAX_ITEM_INACTIVE` for an inactive or unknown one.
pub fn resolve(
    state: &mut DemoState,
    request: Option<SalesTaxItemId>,
    client: ContactId,
) -> Result<Option<Resolved>, ApiError> {
    ensure(state);
    if !state.sales_tax.enabled {
        return if request.is_some() {
            Err(refused(400, "SALES_TAX_OFF"))
        } else {
            Ok(None)
        };
    }
    let client_default = state
        .contacts_mut()
        .iter()
        .find(|c| c.id == client)
        .and_then(|c| c.fields.default_sales_tax_id);
    let st = &state.sales_tax;
    let Some(id) = request.or(client_default).or(st.default_id) else {
        return Ok(None);
    };
    let s = st
        .find(id)
        .filter(|s| s.active || request.is_none())
        .ok_or_else(|| refused(400, "TAX_ITEM_INACTIVE"))?;
    let members = if s.group {
        s.members
            .iter()
            .filter_map(|m| st.find(*m))
            .map(|m| (m.id, m.name.clone(), m.rate, m.agency))
            .collect()
    } else {
        vec![(s.id, s.name.clone(), s.rate, s.agency)]
    };
    Ok(Some(Resolved {
        id: s.id,
        name: s.name.clone(),
        group: s.group,
        rate: st.rate_of(s),
        members,
    }))
}

impl Resolved {
    /// Each member's share of `tax`: `allocateCents(base × member rate,
    /// tax)`; a floored base taxes 0.00 (no allocation).
    ///
    /// # Errors
    /// `Internal` only if the allocation leaves its range.
    pub fn shares(&self, base: Decimal, tax: Decimal) -> Result<Vec<Decimal>, ApiError> {
        if tax.is_zero() {
            return Ok(vec![Decimal::ZERO; self.members.len()]);
        }
        let shares: Vec<Decimal> = self.members.iter().map(|m| base * m.2).collect();
        super::invoices::allocate(&shares, tax)
    }

    /// The `salesTax` object of a preview or record (members' `tax` signed
    /// by `sign`: −1 on a credit memo).
    ///
    /// # Errors
    /// As [`Resolved::shares`].
    pub fn json(&self, base: Decimal, tax: Decimal, sign: Decimal) -> Result<Value, ApiError> {
        let shares = self.shares(base, tax)?;
        Ok(json!({
            "id": self.id,
            "name": self.name,
            "kind": if self.group { "group" } else { "item" },
            "rate": round(self.rate, 6).to_string(),
            "members": self.members.iter().zip(shares).map(|(m, t)| json!({
                "id": m.0, "name": m.1, "rate": round(m.2, 6).to_string(),
                "tax": s2(t * sign),
            })).collect::<Vec<_>>(),
        }))
    }
}

/// A line's tax code as resolved (Q-P23k as E1 built it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineCode {
    /// The code.
    pub id: TaxCodeId,
    /// Its text.
    pub code: String,
    /// Taxed.
    pub taxable: bool,
    /// `request`, `client`, `item` or `is_taxable`.
    pub source: &'static str,
}

impl LineCode {
    /// The preview's `taxCode` (with `source`).
    #[must_use]
    pub fn preview_json(&self) -> Value {
        json!({"id": self.id, "code": self.code, "taxable": self.taxable, "source": self.source})
    }

    /// A record's `taxCode` (no `source`).
    #[must_use]
    pub fn record_json(&self) -> Value {
        json!({"id": self.id, "code": self.code, "taxable": self.taxable})
    }
}

/// The line's code: the request's, else the client's non-taxable code,
/// else the item's code, else the taxable flag (the request's, else the
/// item's) as `Tax` or `Non`. A request's taxable flag never beats the
/// item's code.
pub fn line_code(
    state: &mut DemoState,
    request: Option<TaxCodeId>,
    client: ContactId,
    item: Option<(Option<TaxCodeId>, bool)>,
    flag: Option<bool>,
) -> LineCode {
    ensure(state);
    let client_code = state
        .contacts_mut()
        .iter()
        .find(|c| c.id == client)
        .and_then(|c| c.fields.tax_code_id);
    let st = &state.sales_tax;
    let found = |id: Option<TaxCodeId>| id.and_then(|id| st.code(id));
    let pick = request
        .and_then(|id| found(Some(id)))
        .map(|c| (c, "request"))
        .or_else(|| {
            found(client_code)
                .filter(|c| !c.taxable)
                .map(|c| (c, "client"))
        })
        .or_else(|| found(item.and_then(|i| i.0)).map(|c| (c, "item")));
    if let Some((c, source)) = pick {
        return LineCode {
            id: c.id,
            code: c.code.clone(),
            taxable: c.taxable,
            source,
        };
    }
    let taxable = flag.unwrap_or_else(|| item.is_some_and(|i| i.1));
    let n = if taxable { TAX } else { NON };
    LineCode {
        id: code_id(n),
        code: if taxable { "Tax" } else { "Non" }.into(),
        taxable,
        source: "is_taxable",
    }
}

/// What a posted document writes: its sales tax with the members' shares
/// (sale rows), and its non-taxable lines by code (rows with tax 0.00 under
/// every member). `sign` −1 for a credit memo (its bases; the members'
/// `tax` arrive signed, as the memo preview negates them).
pub fn post_rows(
    state: &mut DemoState,
    txn: TransactionId,
    date: CalendarDate,
    preview: &cl_api::InvoicePreview,
    sign: Decimal,
) {
    let Some(tax) = &preview.sales_tax else {
        state.sales_tax.rows.remove(&txn);
        state.sales_tax.doc_tax.remove(&txn);
        return;
    };
    state.sales_tax.doc_tax.insert(txn, tax.clone());
    let base = preview
        .taxable_base
        .map_or(Decimal::ZERO, cl_core::Money::as_decimal);
    let mut rows = Vec::new();
    let doc = if sign.is_sign_negative() {
        DocKind::Memo
    } else if state.invoices.stored.contains_key(&txn) {
        DocKind::Invoice
    } else {
        DocKind::Receipt
    };
    let st = &state.sales_tax;
    for m in &tax.members {
        let agency = st.find(m.id).and_then(|i| i.agency);
        rows.push(TaxRow {
            kind: RowKind::Sale,
            doc,
            event_date: date,
            sales_tax: Some(tax.id),
            tax_item: Some(m.id),
            agency,
            code: None,
            rate: m.rate.as_decimal(),
            base: base * sign,
            // A memo's members already carry their sign.
            tax: m.tax.map_or(Decimal::ZERO, cl_core::Money::as_decimal),
        });
        for l in &preview.lines {
            if let Some(c) = l.tax_code.as_ref().filter(|c| !c.taxable)
                && let Some(id) = c.id
            {
                rows.push(TaxRow {
                    kind: RowKind::Sale,
                    doc,
                    event_date: date,
                    sales_tax: Some(tax.id),
                    tax_item: Some(m.id),
                    agency,
                    code: Some(id),
                    rate: m.rate.as_decimal(),
                    base: l.amount.as_decimal() * sign,
                    tax: Decimal::ZERO,
                });
            }
        }
    }
    state.sales_tax.rows.insert(txn, rows);
}

/// Drops a voided document's rows.
pub fn void_rows(state: &mut DemoState, txn: TransactionId) {
    state.sales_tax.rows.remove(&txn);
}

/// A posted document's sales tax as saved.
#[must_use]
pub fn doc_tax(state: &DemoState, txn: TransactionId) -> Option<cl_api::DocSalesTax> {
    state.sales_tax.doc_tax.get(&txn).cloned()
}

/// Daily sales: the typed tax on a linked sales tax item (Q-P23g) as a
/// sale row; an unlinked line posted to Sales Tax Payable as an
/// unassigned row.
pub fn post_daily(
    state: &mut DemoState,
    txn: TransactionId,
    date: CalendarDate,
    item: Option<SalesTaxItemId>,
    base: Decimal,
    tax: Decimal,
) {
    ensure(state);
    let agency = item
        .and_then(|i| state.sales_tax.find(i))
        .and_then(|i| i.agency);
    let rate = item
        .and_then(|i| state.sales_tax.find(i))
        .map_or(Decimal::ZERO, |i| i.rate);
    state.sales_tax.rows.insert(
        txn,
        vec![TaxRow {
            kind: RowKind::Sale,
            doc: DocKind::Daily,
            event_date: date,
            sales_tax: item,
            tax_item: item,
            agency,
            code: None,
            rate,
            base,
            tax,
        }],
    );
}

/// What the typed daily-sales tax should be on `sales` at the item's rate
/// (`expectedTax`, a warning only).
#[must_use]
pub fn expected_daily_tax(state: &DemoState, item: SalesTaxItemId, sales: Decimal) -> Decimal {
    let st = &state.sales_tax;
    st.find(item)
        .map_or(Decimal::ZERO, |i| round(sales * st.rate_of(i), 2))
}

// ---- Liability (accrual) and owed --------------------------------------------------

/// One item's accrual columns for a period (from the stored rows).
#[derive(Clone, PartialEq, Eq)]
pub struct ItemColumns {
    /// The agency.
    pub agency: Option<SalesTaxAgencyId>,
    /// The item.
    pub item: SalesTaxItemId,
    /// Taxable sales.
    pub taxable_sales: Decimal,
    /// Non-taxable sales by code text.
    pub non_taxable: BTreeMap<String, Decimal>,
    /// Tax collected (sale rows by event date).
    pub collected: Decimal,
    /// Owed at the end (cumulative; nothing paid in WP-2).
    pub owed_at_end: Decimal,
}

impl fmt::Debug for ItemColumns {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ItemColumns")
            .field("item", &self.item)
            .finish_non_exhaustive()
    }
}

/// The accrual liability by agency and item for `start..=end`, plus the
/// unassigned tax in range: Σ sale rows by event date (Q-P23b: stored
/// cents summed, never recomputed).
#[must_use]
pub fn liability_accrual(
    state: &DemoState,
    start: CalendarDate,
    end: CalendarDate,
) -> (Vec<ItemColumns>, Decimal) {
    let st = &state.sales_tax;
    let mut out: Vec<ItemColumns> = Vec::new();
    let mut unassigned = Decimal::ZERO;
    for r in st.rows.values().flatten() {
        let Some(item) = r.tax_item else {
            if r.event_date >= start && r.event_date <= end {
                unassigned += r.tax;
            }
            continue;
        };
        let i = if let Some(i) = out.iter().position(|c| c.item == item) {
            i
        } else {
            {
                out.push(ItemColumns {
                    agency: r.agency,
                    item,
                    taxable_sales: Decimal::ZERO,
                    non_taxable: BTreeMap::new(),
                    collected: Decimal::ZERO,
                    owed_at_end: Decimal::ZERO,
                });
                out.len() - 1
            }
        };
        let c = &mut out[i];
        if r.event_date <= end {
            c.owed_at_end += r.tax;
        }
        if r.event_date < start || r.event_date > end {
            continue;
        }
        c.collected += r.tax;
        match r.code.and_then(|id| st.code(id)) {
            Some(code) => *c.non_taxable.entry(code.code.clone()).or_default() += r.base,
            None => c.taxable_sales += r.base,
        }
    }
    out.sort_by_key(|c| {
        (
            c.agency.map(|a| a.to_string()),
            st.find(c.item).map(|i| i.name.clone()),
        )
    });
    (out, unassigned)
}

/// What is owed as of `as_of` on the company's basis: by agency and
/// item, the unassigned tax and the total (the engine's JSON).
#[must_use]
pub fn owed(state: &mut DemoState, as_of: CalendarDate) -> Value {
    super::sales_tax_owed::owed_json(state, as_of, None, None)
}

#[cfg(test)]
#[path = "sales_tax_tests.rs"]
pub(crate) mod tests;
