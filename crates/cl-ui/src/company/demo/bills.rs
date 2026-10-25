//! The demo "engine" for Enter Bill (P17 WP4): bills and vendor credits over
//! the demo ledger, their own fields, their files, and the story's vendor,
//! customer and account (Midwest Equipment Rental, Table Rock Marina,
//! 6210 · Equipment rental).
//!
//! It plays an engine with the P17 engine work (`ap-closing-v1`,
//! `ap-edit-v1`, `bill-lines-v1`, `bill-terms-v1`, `bill-received-v1`) for
//! `--demo` only, including its rules: the lines add up to the amount due
//! exactly; a change dated on or before the closing date needs the
//! closing-date password ("demo-close"); a bill with payments keeps its
//! vendor, date and total and can't be voided; an edit is guarded by the
//! record's version. Tests turn single capabilities off to play the pinned
//! engine (see [`DemoBills`]).
//!
//! A bill posts its lines as debits and the total as a credit to Accounts
//! Payable (2000); a vendor credit the other way round. The screen never adds
//! anything up: what it shows after a save comes from here. All data is
//! synthetic.

use std::collections::BTreeMap;
use std::fmt;

use cl_api::{
    ApiError, AttachTo, Attachment, AttachmentId, BillDraft, BillLineDraft, BillRecord,
    BillRecordLine, BillStatus, ConflictReason, DraftKey, EditableFields, LocalValidation,
    RecordVersion, UserText, VendorCreditDraft, Versioned,
};
use cl_core::ids::{AccountId, ContactId, ItemId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity, Secret};
use cl_model::forms::bill::BillCaps;

use super::DemoState;
use super::register::{DemoJournal, DemoLedger, DemoLine, check_closing};

/// The story vendor of the Enter Bill mockup.
pub const STORY_VENDOR: &str = "Midwest Equipment Rental";
/// The story customer of the Enter Bill bill (no jobs in the demo, Q-P13h).
/// The mockup's "Table Rock Marina" is the Clients & Jobs scenario's new
/// client (P11), so the sample's own marina client stands in.
pub const STORY_CUSTOMER: &str = "Long Creek Marina";
/// The story vendor's default expense account (6210 · Equipment rental).
pub const STORY_ACCOUNT: u32 = 6210;
/// The mockup's source document (synthetic; `cargo xtask gen-sample`).
pub const STORY_PDF: &[u8] =
    include_bytes!("../../../../../assets/sample/docs/midwest_inv_55871.pdf");
/// Its file name.
pub const STORY_PDF_NAME: &str = "midwest_inv_55871.pdf";

/// The bills and credits entered in the demo: what the ledger itself does
/// not keep.
#[derive(Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // independent test switches
pub struct DemoBills {
    info: BTreeMap<TransactionId, BillInfo>,
    history: Vec<(TransactionId, &'static str)>,
    files: Vec<DemoFile>,
    /// Write requests that reached the demo engine (creates, edits, voids,
    /// deletes). Tests use it to prove that a refused save sent nothing.
    pub writes: u32,
    /// Behave like an engine without `ap-closing-v1` (tests).
    pub closing_off: bool,
    /// Behave like an engine without `ap-edit-v1` (tests).
    pub edit_off: bool,
    /// Behave like an engine without `bill-lines-v1` (tests).
    pub lines_off: bool,
    /// Behave like an engine without `bill-terms-v1` (tests).
    pub terms_off: bool,
    /// Behave like an engine without `bill-received-v1` (tests).
    pub received_off: bool,
    /// The next save answers "outcome unknown" after writing (tests of the
    /// PR-1 rule).
    pub lose_next_reply: bool,
}

impl fmt::Debug for DemoBills {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoBills")
            .field("bills", &self.info.len())
            .field("writes", &self.writes)
            .finish_non_exhaustive()
    }
}

/// A file in the demo's bill attachment store.
#[derive(Clone)]
struct DemoFile {
    id: AttachmentId,
    /// `draft`, `bill` or `vendor_credit`.
    kind: &'static str,
    /// The draft key or the record id.
    to: String,
    name: String,
    bytes: Vec<u8>,
}

/// One line's own fields (the ledger keeps account, amount and memo).
#[derive(Clone)]
struct LineInfo {
    item: Option<ItemId>,
    quantity: Option<Quantity>,
    unit_price: Option<Money>,
    customer: Option<ContactId>,
    billable: bool,
    tag: Option<TagId>,
}

/// A bill's or credit's own fields.
#[derive(Clone)]
struct BillInfo {
    credit: bool,
    vendor: ContactId,
    reference: Option<String>,
    terms: Option<String>,
    terms_days: Option<u32>,
    due: Option<CalendarDate>,
    received: bool,
    lines: Vec<LineInfo>,
    /// Paid or applied so far (tests mark a bill paid).
    paid: Money,
    /// Bumped by every write (the record's `updatedAt`).
    revision: u32,
}

/// What the demo engine can do (the screen's capability switches).
#[must_use]
pub fn caps(state: &DemoState) -> BillCaps {
    let b = &state.bills;
    BillCaps {
        closing: !b.closing_off,
        lines: !b.lines_off,
        terms: !b.terms_off,
        received: !b.received_off,
    }
}

/// The demo engine edits bills with its version guard and vendor credits at
/// all (`ap-edit-v1`).
#[must_use]
pub fn can_edit(state: &DemoState) -> bool {
    !state.bills.edit_off
}

fn closing(state: &DemoState) -> Option<CalendarDate> {
    let open = state.open?;
    state
        .companies
        .iter()
        .find(|c| c.tenant_id == open)
        .and_then(|c| c.closed_through)
}

fn chart(state: &mut DemoState) -> Vec<cl_api::Account> {
    state.chart.get_or_insert_with(super::chart).clone()
}

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

fn conflict(code: &str) -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: cl_api::ErrorCode::parse(code),
        message: UserText::default(),
    }
}

/// The demo chart's Accounts Payable (the engine's system account).
fn payables(chart: &[cl_api::Account]) -> Option<AccountId> {
    chart
        .iter()
        .find(|a| a.system_tag.as_deref() == Some("accounts_payable"))
        .map(|a| a.id)
}

fn contact_name(state: &mut DemoState, id: ContactId) -> Option<String> {
    state
        .contacts_mut()
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.fields.display_name.clone())
}

fn tag_name(state: &mut DemoState, id: Option<TagId>) -> Option<String> {
    let id = id?;
    state
        .tags_mut()
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.fields.name.clone())
}

/// The story contact the sample fixture does not have: the mockup's vendor
/// (default account 6210, 1099, Net 15). Appended after the sample's, so
/// every other demo id stays as it was.
pub fn add_story_contacts(contacts: &mut Vec<cl_api::Contact>) {
    use cl_api::{ContactFields, ContactType};
    let mut n = u32::try_from(contacts.len()).unwrap_or(u32::MAX);
    let mut push = |fields: ContactFields| {
        n = n.saturating_add(1);
        contacts.push(cl_api::Contact {
            id: super::contacts::contact_id(n),
            fields,
            is_active: true,
            opening_balance: Some(Money::ZERO),
            opening_balance_date: None,
            created_at: Some(DemoState::now()),
            updated_at: Some(DemoState::now()),
        });
    };
    let mut v = ContactFields::named(ContactType::Vendor, STORY_VENDOR);
    v.billing_line1 = Some("88 Industrial Pkwy".into());
    v.billing_city = Some("Joplin".into());
    v.billing_state = Some("MO".into());
    v.billing_zip = Some("64801".into());
    v.billing_country = Some("US".into());
    v.default_expense_account_id = Some(super::account_id(STORY_ACCOUNT));
    v.default_payment_terms = Some("net_15".into());
    v.is1099_eligible = true;
    push(v);
}

/// One document as the demo engine checks and posts it.
struct Doc<'a> {
    credit: bool,
    vendor: ContactId,
    date: CalendarDate,
    reference: Option<&'a str>,
    memo: Option<&'a str>,
    amount: Money,
    lines: &'a [BillLineDraft],
    terms: Option<&'a cl_api::BillTerms>,
    due: Option<CalendarDate>,
    received: bool,
    draft: Option<DraftKey>,
}

impl<'a> Doc<'a> {
    fn bill(d: &'a BillDraft) -> Doc<'a> {
        Doc {
            credit: false,
            vendor: d.vendor,
            date: d.date,
            reference: d.vendor_invoice_number.as_deref(),
            memo: d.memo.as_deref(),
            amount: d.amount_due,
            lines: &d.lines,
            terms: d.terms.as_ref(),
            due: d.due_date,
            received: d.received,
            draft: d.draft_attachment,
        }
    }

    fn credit(d: &'a VendorCreditDraft) -> Doc<'a> {
        Doc {
            credit: true,
            vendor: d.vendor,
            date: d.date,
            reference: d.vendor_invoice_number.as_deref(),
            memo: d.memo.as_deref(),
            amount: d.amount,
            lines: &d.lines,
            terms: None,
            due: None,
            received: true,
            draft: d.draft_attachment,
        }
    }
}

/// The engine's own due date: the bill date plus the terms' days (the
/// vendor's terms when none are sent).
fn due_of(state: &mut DemoState, doc: &Doc<'_>) -> Option<CalendarDate> {
    if doc.credit {
        return None;
    }
    if doc.due.is_some() {
        return doc.due;
    }
    let vendor_terms = state
        .contacts_mut()
        .iter()
        .find(|c| c.id == doc.vendor)
        .and_then(|c| c.fields.default_payment_terms.clone());
    let code = doc.terms.map(|t| t.code.clone()).or(vendor_terms)?;
    let days = match code.as_str() {
        "custom" => doc.terms.and_then(|t| t.days),
        other => cl_model::lists::terms::term(other)
            .and_then(|t| t.due_days)
            .map(u32::from),
    }?;
    doc.date.add_days(i64::from(days)).ok()
}

/// Saves `doc` (new, or over `existing`). The rules are the engine's; see
/// the module notes.
fn save(
    state: &mut DemoState,
    doc: &Doc<'_>,
    existing: Option<TransactionId>,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    state.bills.writes += 1;
    let old = existing.and_then(|id| state.ledger.date_of(id));
    let dates: Vec<CalendarDate> = old.into_iter().chain([doc.date]).collect();
    if !state.bills.closing_off {
        check_closing(closing(state), &dates, password)?;
    } else if password.is_some() {
        return Err(invalid());
    } else if dates
        .iter()
        .any(|d| closing(state).is_some_and(|c| *d <= c))
    {
        // The pinned engine has no closing-date rule on bills: the screen
        // refuses first, so this is never reached from it.
        return Err(invalid());
    }
    let total = Money::sum(doc.lines.iter().map(|l| l.amount)).map_err(|_| invalid())?;
    if doc.lines.is_empty()
        || total != doc.amount
        || doc.lines.iter().any(|l| !l.amount.is_positive())
    {
        return Err(invalid());
    }
    for l in doc.lines {
        if let (Some(q), Some(r)) = (l.quantity, l.unit_price)
            && r.mul_round(q, 2).ok() != Some(l.amount)
        {
            return Err(conflict(cl_api::LINE_AMOUNT_MISMATCH));
        }
    }
    if let Some(id) = existing
        && let Some(info) = state.bills.info.get(&id)
        && info.paid.is_positive()
    {
        // A bill with payments keeps its vendor, date and total.
        let before = get(state, id).map(|v| v.record);
        let same = before.is_some_and(|b| {
            b.txn.contact_id == Some(doc.vendor)
                && b.txn.txn_date == doc.date
                && b.txn.total == Some(doc.amount)
        });
        if !same {
            return Err(conflict(cl_api::BILL_HAS_PAYMENTS));
        }
    }
    let chart = chart(state);
    let ap = payables(&chart).ok_or_else(invalid)?;
    let mut lines: Vec<DemoLine> = doc
        .lines
        .iter()
        .map(|l| DemoLine {
            account: l.account,
            debit: if doc.credit { Money::ZERO } else { l.amount },
            credit: if doc.credit { l.amount } else { Money::ZERO },
            memo: l
                .description
                .clone()
                .filter(|d| !d.is_empty())
                .or_else(|| doc.memo.map(str::to_string)),
        })
        .collect();
    lines.push(DemoLine {
        account: ap,
        debit: if doc.credit { doc.amount } else { Money::ZERO },
        credit: if doc.credit { Money::ZERO } else { doc.amount },
        memo: None,
    });
    let number = match existing {
        Some(id) => state
            .ledger
            .transactions()
            .into_iter()
            .find(|t| DemoLedger::id_of(t) == id)
            .map(|t| t.number)
            .unwrap_or_default(),
        None => {
            let n = state.bills.info.len() + 1;
            if doc.credit {
                format!("VC-{n:05}")
            } else {
                format!("BILL-{n:05}")
            }
        }
    };
    let journal = DemoJournal {
        kind: if doc.credit { "vendor_credit" } else { "bill" },
        date: doc.date,
        number,
        contact: contact_name(state, doc.vendor),
        memo: doc.memo.unwrap_or_default().to_string(),
        lines,
    };
    let id = state
        .ledger
        .save_lines(&chart, &journal, existing)
        .ok_or_else(invalid)?;
    let details: Vec<(Option<String>, Option<String>)> = doc
        .lines
        .iter()
        .map(|l| {
            (
                l.description
                    .clone()
                    .filter(|d| !d.is_empty())
                    .or_else(|| doc.memo.map(str::to_string)),
                tag_name(state, l.tag),
            )
        })
        .collect();
    state.ledger.set_line_details(&chart, id, ap, &details);
    let due = due_of(state, doc);
    let kept = state
        .bills
        .info
        .get(&id)
        .map_or((Money::ZERO, 0), |i| (i.paid, i.revision));
    state.bills.info.insert(
        id,
        BillInfo {
            credit: doc.credit,
            vendor: doc.vendor,
            reference: doc
                .reference
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(str::to_string),
            terms: doc.terms.map(|t| t.code.clone()),
            terms_days: doc.terms.and_then(|t| t.days),
            due,
            received: doc.received,
            lines: doc
                .lines
                .iter()
                .map(|l| LineInfo {
                    item: l.item,
                    quantity: l.quantity,
                    unit_price: l.unit_price,
                    customer: l.customer,
                    billable: l.billable,
                    tag: l.tag,
                })
                .collect(),
            paid: kept.0,
            revision: kept.1 + 1,
        },
    );
    // The files uploaded before the first save move onto the record.
    if existing.is_none()
        && let Some(draft) = doc.draft
    {
        relink(state, draft, id, doc.credit);
    }
    state.bills.history.push((
        id,
        if existing.is_some() {
            "update"
        } else {
            "create"
        },
    ));
    if std::mem::take(&mut state.bills.lose_next_reply) {
        return Err(ApiError::OutcomeUnknown {
            request_id: cl_core::ids::RequestId::parse("d0000000-0000-4000-8000-f00000000001")
                .map_err(|_| invalid())?,
            idempotency_key: None,
        });
    }
    Ok(id)
}

fn relink(state: &mut DemoState, draft: DraftKey, id: TransactionId, credit: bool) -> u64 {
    let key = draft.to_string();
    let mut moved = 0;
    for f in state
        .bills
        .files
        .iter_mut()
        .filter(|f| f.kind == "draft" && f.to == key)
    {
        f.kind = if credit { "vendor_credit" } else { "bill" };
        f.to = id.to_string();
        moved += 1;
    }
    moved
}

fn check_caps(state: &DemoState, doc: &Doc<'_>, password: Option<&Secret>) -> Result<(), ApiError> {
    let b = &state.bills;
    let line_fields = doc.lines.iter().any(|l| {
        l.item.is_some()
            || l.quantity.is_some()
            || l.unit_price.is_some()
            || l.customer.is_some()
            || l.billable
            || (doc.credit && l.tag.is_some())
    });
    let unsupported = (b.lines_off && (line_fields || doc.draft.is_some()))
        || (b.received_off && !doc.received)
        || (b.closing_off && password.is_some());
    if unsupported {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    Ok(())
}

/// `POST /bills`.
///
/// # Errors
/// `ClosedPeriod` / `ClosingPasswordRejected`, `Local(InvalidValue)` (lines
/// that don't add up exactly, an account outside the chart),
/// `Local(Unsupported)` for a field a switched-off capability can't take.
pub fn create_bill(
    state: &mut DemoState,
    d: &BillDraft,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    let doc = Doc::bill(d);
    check_caps(state, &doc, password)?;
    save(state, &doc, None, password)
}

/// `POST /vendor-credits`.
///
/// # Errors
/// As [`create_bill`].
pub fn create_credit(
    state: &mut DemoState,
    d: &VendorCreditDraft,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    let doc = Doc::credit(d);
    check_caps(state, &doc, password)?;
    save(state, &doc, None, password)
}

fn stale(
    state: &mut DemoState,
    id: TransactionId,
    expected: &RecordVersion,
) -> Result<(), ApiError> {
    let now = get(state, id).ok_or(ApiError::NotFound { code: None })?;
    if &now.version != expected {
        return Err(ApiError::Conflict {
            reason: ConflictReason::Stale,
            current_version: Some(now.version),
            code: None,
            message: UserText::default(),
        });
    }
    if state.ledger.is_void(id) {
        state.bills.writes += 1;
        return Err(invalid());
    }
    Ok(())
}

/// `PUT /bills/{id}` guarded by the version read.
///
/// # Errors
/// `Conflict { Stale }` (nothing changed); `Conflict` `BILL_HAS_PAYMENTS`
/// for a paid bill's vendor, date or total; as [`create_bill`].
pub fn update_bill(
    state: &mut DemoState,
    id: TransactionId,
    expected: &RecordVersion,
    d: &BillDraft,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    stale(state, id, expected)?;
    let doc = Doc {
        draft: None,
        ..Doc::bill(d)
    };
    check_caps(state, &doc, password)?;
    save(state, &doc, Some(id), password)
}

/// `PUT /vendor-credits/{id}` (`ap-edit-v1`).
///
/// # Errors
/// `Local(Unsupported)` without `ap-edit-v1` (nothing changed); `Conflict`
/// `CREDIT_APPLIED` once applied; as [`update_bill`].
pub fn update_credit(
    state: &mut DemoState,
    id: TransactionId,
    expected: &RecordVersion,
    d: &VendorCreditDraft,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    if state.bills.edit_off {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    stale(state, id, expected)?;
    if state
        .bills
        .info
        .get(&id)
        .is_some_and(|i| i.paid.is_positive())
    {
        state.bills.writes += 1;
        return Err(conflict(cl_api::CREDIT_APPLIED));
    }
    let doc = Doc {
        draft: None,
        ..Doc::credit(d)
    };
    check_caps(state, &doc, password)?;
    save(state, &doc, Some(id), password)
}

/// Voids a bill or credit with nothing applied.
///
/// # Errors
/// `Conflict` `BILL_HAS_PAYMENTS` / `CREDIT_APPLIED`; the closing-date rule;
/// as the ledger's void.
pub fn void(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    state.bills.writes += 1;
    let old = state
        .ledger
        .date_of(id)
        .ok_or(ApiError::NotFound { code: None })?;
    if state.bills.closing_off && password.is_some() {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    check_closing(closing(state), &[old], password)?;
    let paid = state.bills.info.get(&id).map(|i| (i.credit, i.paid));
    if let Some((credit, paid)) = paid
        && paid.is_positive()
    {
        let code = if credit {
            cl_api::CREDIT_APPLIED
        } else {
            cl_api::BILL_HAS_PAYMENTS
        };
        return Err(conflict(code));
    }
    state.ledger.void(id)?;
    if let Some(i) = state.bills.info.get_mut(&id) {
        i.revision += 1;
    }
    state.bills.history.push((id, "void"));
    Ok(())
}

/// Deletes an unpaid bill or an unapplied credit (`ap-edit-v1`).
///
/// # Errors
/// `Local(Unsupported)` without `ap-edit-v1` (nothing sent); as [`void`].
pub fn delete(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    if state.bills.edit_off {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    state.bills.writes += 1;
    let old = state
        .ledger
        .date_of(id)
        .ok_or(ApiError::NotFound { code: None })?;
    check_closing(closing(state), &[old], password)?;
    if let Some(i) = state.bills.info.get(&id)
        && i.paid.is_positive()
    {
        let code = if i.credit {
            cl_api::CREDIT_APPLIED
        } else {
            cl_api::BILL_HAS_PAYMENTS
        };
        return Err(conflict(code));
    }
    state.ledger.void(id)?;
    state.bills.history.push((id, "delete"));
    Ok(())
}

/// Marks `amount` of a demo bill or credit as paid or applied (tests: a bill
/// with payments).
pub fn mark_paid(state: &mut DemoState, id: TransactionId, amount: Money) -> bool {
    match state.bills.info.get_mut(&id) {
        Some(i) => {
            i.paid = amount;
            i.revision += 1;
            true
        }
        None => false,
    }
}

/// A sample bill's payment state, as the demo's vendor balances tell it.
fn sample_paid(
    state: &DemoState,
    vendor: Option<ContactId>,
    id: TransactionId,
    total: Money,
) -> Money {
    let Some(vendor) = vendor else {
        return total;
    };
    let open = super::contacts::open_bills(state, vendor);
    match open.iter().find(|b| b.0 == id) {
        Some(b) => total.checked_sub(b.3).unwrap_or(Money::ZERO),
        None => total,
    }
}

/// One bill or vendor credit as `GET /bills/{id}` (or `/vendor-credits`)
/// answers it, and its version.
#[must_use]
pub fn get(state: &mut DemoState, id: TransactionId) -> Option<Versioned<BillRecord>> {
    let chart = chart(state);
    let contacts = state.contacts_mut().clone();
    let mut txn = state.ledger.get(&chart, &contacts, id)?.record;
    let credit = txn.txn_type == cl_api::TxnType::VendorCredit;
    if !credit && txn.txn_type != cl_api::TxnType::Bill {
        return None;
    }
    let ap = payables(&chart);
    let info = state.bills.info.get(&id).cloned();
    let doc_lines: Vec<&cl_api::TransactionLine> = txn
        .lines
        .iter()
        .filter(|l| Some(l.account_id) != ap)
        .collect();
    let mut lines: Vec<BillRecordLine> = doc_lines
        .iter()
        .map(|l| BillRecordLine {
            account: l.account_id,
            amount: if credit { l.credit } else { l.debit },
            description: l.description.clone(),
            tag: l.tag_id,
            customer: None,
            billable: false,
            item: None,
            quantity: None,
            unit_price: None,
        })
        .collect();
    if let Some(i) = &info {
        for (l, x) in lines.iter_mut().zip(&i.lines) {
            l.item = x.item;
            l.quantity = x.quantity;
            l.unit_price = x.unit_price;
            l.customer = x.customer;
            l.billable = x.billable;
            l.tag = x.tag.or(l.tag);
        }
        // The ledger line carries the customer as the engine stores it.
        let mut n = 0;
        for l in txn.lines.iter_mut().filter(|l| Some(l.account_id) != ap) {
            if let Some(x) = i.lines.get(n) {
                l.contact_id = x.customer;
                l.is_billable = x.billable;
                l.tag_id = x.tag.or(l.tag_id);
            }
            n += 1;
        }
    }
    let total = Money::sum(lines.iter().map(|l| l.amount)).ok();
    txn.total = total;
    let void = txn.status == cl_api::TxnStatus::Void;
    let paid = match &info {
        Some(i) => i.paid,
        None => sample_paid(state, txn.contact_id, id, total.unwrap_or(Money::ZERO)),
    };
    let total_m = total.unwrap_or(Money::ZERO);
    let balance = total_m.checked_sub(paid).unwrap_or(Money::ZERO);
    let status = if void {
        None
    } else if !paid.is_positive() {
        Some(BillStatus::Unpaid)
    } else if balance.is_positive() {
        Some(BillStatus::Partial)
    } else {
        Some(BillStatus::Paid)
    };
    let sample_due = state
        .ledger
        .transactions()
        .into_iter()
        .find(|t| DemoLedger::id_of(t) == id)
        .and_then(|t| t.due);
    if let Some(i) = &info {
        // `updatedAt` moves with every write (the version guard).
        txn.updated_at = cl_core::Timestamp::from_unix_millis(
            DemoState::now().unix_millis() + i64::from(i.revision),
        )
        .ok();
    }
    let record = BillRecord {
        lines,
        vendor_invoice_number: info.as_ref().and_then(|i| i.reference.clone()),
        due_date: info.as_ref().map_or(sample_due, |i| i.due),
        payment_terms: info.as_ref().and_then(|i| i.terms.clone()),
        terms_days: info.as_ref().and_then(|i| i.terms_days),
        balance_due: Some(balance),
        amount_paid: (!credit).then_some(paid),
        credits_applied: None,
        bill_status: status.filter(|_| !credit),
        bill_received: info.as_ref().is_none_or(|i| i.received),
        discount_percent: None,
        discount_days: None,
        discount_date: None,
        txn,
    };
    let version = RecordVersion::Fingerprint(record.fingerprint());
    Some(Versioned { record, version })
}

/// The bills (or credits) newest first: (id, date), as `GET /bills?sortBy=
/// date&sortDir=desc` lists them (void ones included).
#[must_use]
pub fn list(state: &mut DemoState, credit: bool) -> Vec<(TransactionId, CalendarDate)> {
    let kind = if credit { "vendor_credit" } else { "bill" };
    let mut txns: Vec<_> = state
        .ledger
        .transactions()
        .into_iter()
        .filter(|t| t.kind == kind)
        .collect();
    txns.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
    txns.iter()
        .map(|t| (DemoLedger::id_of(t), t.date))
        .collect()
}

/// The vendor's newest posted bill (prefill, Q-191).
#[must_use]
pub fn newest_for_vendor(state: &mut DemoState, vendor: ContactId) -> Option<BillRecord> {
    let ids: Vec<TransactionId> = list(state, false).into_iter().map(|(id, _)| id).collect();
    for id in ids {
        if state.ledger.is_void(id) {
            continue;
        }
        if let Some(v) = get(state, id)
            && v.record.txn.contact_id == Some(vendor)
        {
            return Some(v.record);
        }
    }
    None
}

/// The vendor's bills in brief, as `GET /bills/vendor-summary` sends it:
/// the sample's open bills plus the ones entered in the demo (posted, with
/// something still owed), and the demo's unused credits.
#[must_use]
pub fn vendor_summary(state: &mut DemoState, vendor: ContactId) -> cl_api::VendorSummary {
    let sample = super::checks::vendor_summary(state, vendor);
    let ids: Vec<TransactionId> = state
        .bills
        .info
        .iter()
        .filter(|(_, i)| i.vendor == vendor)
        .map(|(id, _)| *id)
        .collect();
    let mut owed = sample.total_owed;
    let mut count = sample.unpaid_count;
    let mut credits = Money::ZERO;
    let mut credit_count = 0u32;
    for id in ids {
        if state.ledger.is_void(id) {
            continue;
        }
        let Some(v) = get(state, id) else { continue };
        let open = v.record.balance_due.unwrap_or(Money::ZERO);
        if !open.is_positive() {
            continue;
        }
        if v.record.kind() == cl_api::ApKind::VendorCredit {
            credits = credits.checked_add(open).unwrap_or(credits);
            credit_count += 1;
        } else {
            owed = owed.checked_add(open).unwrap_or(owed);
            count += 1;
        }
    }
    serde_json::from_value(serde_json::json!({
        "unpaidCount": count,
        "totalOwed": owed.to_wire(),
        "overdueCount": sample.overdue_count,
        "totalOverdue": sample.total_overdue.to_wire(),
        "availableCreditsCount": credit_count,
        "totalAvailableCredits": credits.to_wire(),
    }))
    .unwrap_or(sample)
}

/// A bill's or credit's history in the demo (what this run did to it).
#[must_use]
pub fn history(state: &DemoState, id: TransactionId) -> Vec<&'static str> {
    state
        .bills
        .history
        .iter()
        .filter(|(t, _)| *t == id)
        .map(|(_, a)| *a)
        .collect()
}

// ------------------------------------------------------------------------
// Files.

fn attachment(f: &DemoFile) -> Option<Attachment> {
    serde_json::from_value(serde_json::json!({
        "id": f.id.to_string(),
        "fileName": f.name,
        "fileSize": f.bytes.len(),
        "mimeType": cl_api::FileKind::sniff(&f.bytes).map(cl_api::FileKind::mime),
        "attachableType": f.kind,
        "attachableId": f.to,
    }))
    .ok()
}

/// `POST /attachments`: the file is checked as the client checks it.
///
/// # Errors
/// `Local(AttachmentTooLarge | AttachmentType)`.
pub fn upload(
    state: &mut DemoState,
    to: AttachTo,
    name: &str,
    bytes: &[u8],
) -> Result<Attachment, ApiError> {
    cl_api::check_file(bytes).map_err(ApiError::Local)?;
    let n = state.bills.files.len() + 1;
    let id =
        AttachmentId::parse(&format!("d0000000-0000-4000-8000-e{n:011}")).map_err(|_| invalid())?;
    let f = DemoFile {
        id,
        kind: to.attachable_type(),
        to: to.attachable_id(),
        name: name.to_string(),
        bytes: bytes.to_vec(),
    };
    let a = attachment(&f).ok_or_else(invalid)?;
    state.bills.files.push(f);
    Ok(a)
}

/// `GET /attachments` for a bill, a credit or a draft.
#[must_use]
pub fn attachments(state: &DemoState, of: AttachTo) -> Vec<Attachment> {
    let (kind, to) = (of.attachable_type(), of.attachable_id());
    state
        .bills
        .files
        .iter()
        .filter(|f| f.kind == kind && f.to == to)
        .filter_map(attachment)
        .collect()
}

/// `GET /attachments/{id}/download`.
///
/// # Errors
/// `NotFound`.
pub fn download(state: &DemoState, id: AttachmentId) -> Result<Vec<u8>, ApiError> {
    state
        .bills
        .files
        .iter()
        .find(|f| f.id == id)
        .map(|f| f.bytes.clone())
        .ok_or(ApiError::NotFound { code: None })
}

/// `POST /bills/{id}/attach-draft`: moves the draft's files onto the bill.
///
/// # Errors
/// `NotFound` for an unknown bill.
pub fn attach_draft(
    state: &mut DemoState,
    id: TransactionId,
    draft: DraftKey,
    credit: bool,
) -> Result<u64, ApiError> {
    if !state.bills.info.contains_key(&id) {
        return Err(ApiError::NotFound { code: None });
    }
    Ok(relink(state, draft, id, credit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn by_number(state: &mut DemoState, n: &str) -> AccountId {
        chart(state)
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some(n))
            .map_or_else(|| panic!("{n}"), |a| a.id)
    }

    fn vendor(state: &mut DemoState) -> ContactId {
        state
            .contacts_mut()
            .iter()
            .find(|c| c.fields.display_name == STORY_VENDOR)
            .map(|c| c.id)
            .expect("story vendor")
    }

    fn draft(state: &mut DemoState) -> BillDraft {
        let rental = by_number(state, "6210");
        BillDraft {
            vendor: vendor(state),
            date: CalendarDate::from_ymd(2026, 11, 3).expect("d"),
            vendor_invoice_number: Some("INV-55871".into()),
            terms: Some(cl_api::BillTerms::code("net_15")),
            due_date: None,
            memo: Some("Mini excavator".into()),
            amount_due: money("1480.00"),
            lines: vec![
                BillLineDraft::expense(rental, money("1330.00")),
                BillLineDraft::expense(rental, money("150.00")),
            ],
            received: true,
            draft_attachment: None,
        }
    }

    #[test]
    fn a_bill_posts_to_payables_and_reads_back_exactly() {
        let mut s = DemoState::new();
        let d = draft(&mut s);
        let id = create_bill(&mut s, &d, None).expect("saved");
        let v = get(&mut s, id).expect("get");
        assert_eq!(v.record.txn.total, Some(money("1480.00")));
        assert_eq!(v.record.lines.len(), 2);
        assert_eq!(v.record.vendor_invoice_number.as_deref(), Some("INV-55871"));
        // 11/03 + Net 15.
        assert_eq!(v.record.due_date, CalendarDate::from_ymd(2026, 11, 18).ok());
        assert_eq!(v.record.bill_status, Some(BillStatus::Unpaid));
        let ap = payables(&chart(&mut s)).expect("ap");
        let credit = Money::sum(
            v.record
                .txn
                .lines
                .iter()
                .filter(|l| l.account_id == ap)
                .map(|l| l.credit),
        )
        .expect("sum");
        assert_eq!(credit, money("1480.00"));
        let summary = vendor_summary(&mut s, d.vendor);
        assert_eq!(summary.total_owed, money("1480.00"));
        // Off by a cent: refused, nothing posted.
        let mut off = d.clone();
        off.amount_due = money("1480.01");
        assert_eq!(create_bill(&mut s, &off, None), Err(invalid()));
        // Paid: no void, and the total stays.
        assert!(mark_paid(&mut s, id, money("100.00")));
        assert!(matches!(
            void(&mut s, id, None),
            Err(ApiError::Conflict { .. })
        ));
        let v = get(&mut s, id).expect("get");
        let mut more = d.clone();
        more.amount_due = money("1580.00");
        more.lines[1].amount = money("250.00");
        assert!(matches!(
            update_bill(&mut s, id, &v.version, &more, None),
            Err(ApiError::Conflict { .. })
        ));
    }

    #[test]
    fn closed_periods_versions_and_switched_off_capabilities() {
        let mut s = DemoState::new();
        let mut d = draft(&mut s);
        d.date = CalendarDate::from_ymd(2026, 9, 15).expect("d");
        assert!(matches!(
            create_bill(&mut s, &d, None),
            Err(ApiError::ClosedPeriod(_))
        ));
        let pw = Secret::new(super::super::register::DEMO_CLOSING_PASSWORD);
        let id = create_bill(&mut s, &d, Some(&pw)).expect("saved");
        let v = get(&mut s, id).expect("get");
        // A version from before a write is stale.
        let old = v.version.clone();
        let mut e = d.clone();
        e.memo = Some("changed".into());
        update_bill(&mut s, id, &old, &e, Some(&pw)).expect("edit");
        assert!(matches!(
            update_bill(&mut s, id, &old, &e, Some(&pw)),
            Err(ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            })
        ));
        s.bills.lines_off = true;
        let mut c = draft(&mut s);
        c.lines[0].customer = Some(c.vendor);
        assert_eq!(
            create_bill(&mut s, &c, None),
            Err(ApiError::Local(LocalValidation::Unsupported))
        );
        s.bills.edit_off = true;
        let cd = VendorCreditDraft {
            vendor: c.vendor,
            date: c.date,
            vendor_invoice_number: None,
            memo: None,
            amount: money("150.00"),
            lines: vec![BillLineDraft::expense(c.lines[0].account, money("150.00"))],
            draft_attachment: None,
        };
        let cid = create_credit(&mut s, &cd, None).expect("credit");
        let v = get(&mut s, cid).expect("get");
        assert_eq!(v.record.kind(), cl_api::ApKind::VendorCredit);
        assert_eq!(
            update_credit(&mut s, cid, &v.version, &cd, None),
            Err(ApiError::Local(LocalValidation::Unsupported))
        );
        assert_eq!(vendor_summary(&mut s, c.vendor).available_credits_count, 1);
        void(&mut s, cid, None).expect("void");
        assert_eq!(history(&s, cid), ["create", "void"]);
    }

    #[test]
    fn draft_files_move_onto_the_bill() {
        let mut s = DemoState::new();
        let key = DraftKey::new();
        let a = upload(&mut s, AttachTo::Draft(key), STORY_PDF_NAME, STORY_PDF).expect("upload");
        assert_eq!(attachments(&s, AttachTo::Draft(key)).len(), 1);
        let mut d = draft(&mut s);
        d.draft_attachment = Some(key);
        let id = create_bill(&mut s, &d, None).expect("saved");
        assert!(attachments(&s, AttachTo::Draft(key)).is_empty());
        assert_eq!(attachments(&s, AttachTo::Bill(id)).len(), 1);
        assert_eq!(download(&s, a.id).expect("bytes"), STORY_PDF);
        assert_eq!(attach_draft(&mut s, id, key, false), Ok(0), "nothing more");
    }
}
