//! Enter Bill (P17, `vendors.enter_bill`, Ctrl+B): one form for a bill and
//! a vendor credit, beside the vendor panel and the source-document card
//! (mockup `docs/design/mockups/EnterBill.html`, plan §1).
//!
//! The header is a `FormState<BillForm>` (cl-model, WP3) whose lines stay
//! empty; the Expenses tab is the shared [`LinesGrid`] and the Items tab the
//! [`items::ItemsGrid`]. [`EnterBillScreen::compose`] puts them together.
//! The one exact gate is `cl_model::forms::bill::check` (the lines must
//! equal the amount due to the cent; nothing is sent otherwise), and again
//! `ap_doc::check_doc` in cl-api. The save maps the checked bill field by
//! field onto the engine draft (`save::to_draft`), with no arithmetic.
//!
//! A Bill / Vendor credit radio switches the posting route and the labels,
//! not the screen. Every save goes through the PR-1 unknown-save rule
//! (Q-PR1a/b): the shared lost-save registry on `CompanyLink` under the
//! name `enter_bill` (the 14th posting form). The pinned engine lacks the
//! P17 engine work (PR-A), so its pieces are gated by capability (plan §2):
//! the Items tab, line customers and Billable (`bill-lines-v1`), the
//! closing-date password (`ap-closing-v1`; a closed-period bill is refused
//! before sending without it), editing credits and Delete (`ap-edit-v1`),
//! discount terms (`bill-terms-v1`) and Bill received (`bill-received-v1`).
//!
//! The vendor panel (WP8) and the document reading (WP7) are their own
//! files; this screen calls their fixed entry points. Every engine call
//! runs on the request queue and is polled each frame; `--demo` answers
//! from `company::demo::bills`.
//!
//! Opening a prefilled bill: another screen pushes
//! `CompanyEvent::OpenWith(vendors.enter_bill, OpenPayload::BillPrefill(..))`
//! (statement match) or `OpenPayload::InboxDraft(..)` (receipt inbox). After
//! every save, void or delete the screen publishes
//! `data_bus::Change::Bills(vendor)` and the ledger change. Strings go in
//! `i18n/bill.rs`.

mod attach;
pub mod doc_read;
// A04 WP-F2: draft mode (a document from Review Documents).
pub(crate) mod doc_read_local;
mod draft;
mod form;
pub mod items;
mod prefill;
mod save;
pub mod statement_match;
pub mod vendor_panel;

#[cfg(test)]
mod tests;

/// The bill's own builder, for the A04 review digest tests (form → body).
#[cfg(test)]
pub(crate) use save::{Draft as BillBody, to_draft as bill_body};

use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, Capabilities, CompanySettings, Contact, ContactFilter,
    IdempotencyKey, Item, ItemFilter, RequestGroup, Tag, TagFilter, Versioned,
};
use cl_core::ids::{AccountId, ContactId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money};
use cl_model::forms::FormState;
use cl_model::forms::bill::{BillCaps, BillForm, BillKind, BillProblem, PaymentTerm, TermsDates};
use cl_model::forms::item_lines::ItemLines;
use cl_model::forms::lines::{FormLines, TotalsStatus};
use cl_model::lists::tags::TagList;
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::forms::closing_prompt::ClosingPrompt;
use crate::forms::lines_grid::{L_BILLABLE, L_CUSTOMER, LinesGrid, LinesSpec};
use crate::i18n::{t, t_with};
use crate::screens::company::form::DateInput;
use crate::screens::register::lists::RegisterLists;
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "vendors.enter_bill";

/// The name of this form in the shared lost-save registry (Q-PR1a).
pub const LOST_SCOPE: &str = "enter_bill";

/// The Expenses lines (every column).
pub const EXPENSES: LinesSpec = LinesSpec::all("enter_bill/expenses", "bill.expenses.label");

/// The Enter Bill window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(EnterBillScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// A fresh bill or credit filled from a vendor statement line ("Create
/// bill" / "Create credit", WP6). The amount is the one the owner typed or
/// pasted from the statement; the form stays an ordinary unsaved form.
#[derive(Clone, PartialEq, Eq)]
pub struct BillPrefill {
    /// Bill or vendor credit.
    pub kind: BillKind,
    /// The vendor.
    pub vendor: ContactId,
    /// The statement line's date.
    pub date: CalendarDate,
    /// The statement line's reference (owner data).
    pub ref_no: String,
    /// The statement line's amount.
    pub amount: Money,
}

/// No refs or amounts.
impl std::fmt::Debug for BillPrefill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BillPrefill")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// Header fields (undo merges typing within one field).
mod field {
    pub const VENDOR: u64 = 1;
    pub const DATE: u64 = 2;
    pub const REFERENCE: u64 = 3;
    pub const TERMS: u64 = 4;
    pub const AMOUNT: u64 = 5;
    pub const DUE: u64 = 6;
    pub const MEMO: u64 = 7;
    /// Custom discount % and days (WP8).
    pub const DISCOUNT: u64 = 8;
    /// The discount date typed by the owner (WP8).
    pub const DISCOUNT_DATE: u64 = 9;
}

/// A header field the keyboard can be put in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    Vendor,
    Date,
    Reference,
    Terms,
    TermsDays,
    /// Custom terms: Discount % (WP8).
    Discount,
    /// Custom terms: Discount days (WP8).
    DiscountDays,
    /// The discount date (`bill-terms-v1`, WP8).
    DiscountDate,
    Amount,
    DueDate,
    Memo,
}

/// Which lines tab is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    #[default]
    Expenses,
    Items,
}

/// The text of the header inputs as typed (the form holds the values).
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub vendor: String,
    pub date: DateInput,
    pub amount: String,
    /// Why the typed amount is not accepted (i18n key).
    pub amount_problem: Option<&'static str>,
    /// The typed vendor is not a name from the list.
    pub vendor_problem: bool,
    pub due: DateInput,
    pub terms_days: String,
    /// Custom terms: Discount % and Discount days as typed (WP8).
    pub discount_percent: String,
    pub discount_days: String,
    /// The discount date as typed (WP8).
    pub discount: DateInput,
    /// The header revision the texts were last taken from.
    pub rev: u64,
}

/// A saved bill or credit being changed.
#[derive(Clone, Debug)]
pub(crate) struct Editing {
    pub id: TransactionId,
    pub kind: BillKind,
    /// The record as read (`PUT` is guarded by it).
    pub record: Versioned<cl_api::BillRecord>,
    pub old_date: CalendarDate,
    /// Payments or credits are applied: vendor, date and total are
    /// read-only, Void is refused.
    pub has_payments: bool,
    /// Shown, never changed here: void, a credit without `ap-edit-v1`, a
    /// closed-period bill without `ap-closing-v1`, or lines this form can't
    /// show exactly.
    pub read_only: bool,
    /// Why it is read-only (i18n key).
    pub read_only_note: &'static str,
}

/// What the closing-date password is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    /// The save (with what follows it).
    Save(cl_model::forms::SaveIntent),
    /// Void with this reason.
    Void(String),
    /// Delete with this reason.
    Delete(String),
}

/// The last save's outcome, for the control channel (no amounts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LastSaved {
    pub kind: BillKind,
    pub id: TransactionId,
}

/// Saving and its outcome.
#[derive(Default)]
pub(crate) struct SaveState {
    pub task: Option<Task<save::Saved>>,
    pub intent: Option<cl_model::forms::SaveIntent>,
    /// The idempotency key of the request being retried.
    pub key: Option<(save::SaveIdentity, IdempotencyKey)>,
    pub problems: Vec<BillProblem>,
    pub error: Option<ApiError>,
    /// The engine's refusal of the save, void or delete (i18n key).
    pub refusal: Option<&'static str>,
    /// The bill changed somewhere else since it was read.
    pub stale: bool,
    pub count: u32,
    pub last: Option<LastSaved>,
    /// `void`, `delete`, `save`, `clear`, `load`.
    pub last_action: Option<&'static str>,
    /// A plain note under the toolbar (i18n key).
    pub notice: Option<&'static str>,
    /// A save whose outcome is unknown (PR-1).
    pub lost: crate::forms::lost_save::LostSave,
    /// The bill saved but its files did not move onto it yet (plan §5):
    /// the record and the draft to try again.
    pub attach_failed: Option<save::AttachRetry>,
    pub attach_retry: Option<Task<()>>,
}

/// Prev/Next over the bills (or credits), newest first.
#[derive(Default)]
pub(crate) struct Nav {
    pub list: Vec<(TransactionId, CalendarDate)>,
    pub task: Option<Task<Vec<(TransactionId, CalendarDate)>>>,
    /// The list is complete (the last page was short).
    pub done: bool,
    /// The kind the list is for.
    pub kind: Option<BillKind>,
    /// Where the form is in the list (`None`: a new bill).
    pub at: Option<usize>,
    /// A step waiting for the next page: true is older (Prev).
    pub want_older: Option<bool>,
}

/// Void or Delete (Edit menu): the reason being typed.
#[derive(Default)]
pub(crate) struct VoidPrompt {
    pub open: bool,
    /// Delete rather than void (`ap-edit-v1`).
    pub delete: bool,
    pub reason: String,
    pub task: Option<Task<()>>,
    /// The key of the last void or delete sent, kept after an unknown
    /// outcome so sending the same one again is a replay (Q-PR1a,
    /// `idempotency-ap-v1` / `idempotency-v1`), never a second write.
    pub key: Option<(VoidIdentity, IdempotencyKey)>,
}

/// What a void or delete key belongs to: the record, void or delete, the
/// reason and whether the closing-date password went with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VoidIdentity {
    pub id: cl_core::ids::TransactionId,
    pub delete: bool,
    pub reason: String,
    pub password: bool,
}

/// History of a saved bill.
#[derive(Default)]
pub(crate) struct HistoryView {
    pub open: bool,
    pub rows: Vec<String>,
    pub task: Option<Task<Vec<String>>>,
    pub error: bool,
}

/// What a terms-dates request is for: date, terms, custom days, custom
/// discount days and percentage, vendor.
pub(crate) type TermsKey = (
    CalendarDate,
    Option<String>,
    Option<u16>,
    Option<u16>,
    Option<cl_model::forms::bill::Percent>,
    Option<ContactId>,
);

/// The Enter Bill window.
pub struct EnterBillScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    pub(crate) accounts: Vec<Account>,
    accounts_task: Option<Task<Vec<Account>>>,
    pub(crate) accounts_loaded: bool,
    pub(crate) contacts: Vec<Contact>,
    contacts_task: Option<Task<Vec<Contact>>>,
    /// Saved layouts and card accounts for "Read document"
    /// (`document-drafts-v1`, A03 step 19a; empty without it).
    pub(crate) read_context: crate::reading::service::ReadContext,
    read_context_task: Option<Task<crate::reading::service::ReadContext>>,
    pub(crate) tags: Vec<Tag>,
    tags_task: Option<Task<Vec<Tag>>>,
    pub(crate) items_list: Vec<Item>,
    items_task: Option<Task<Vec<Item>>>,
    pub(crate) tag_list: TagList,
    pub(crate) lists: Arc<RegisterLists>,
    settings_task: Option<Task<CompanySettings>>,
    pub(crate) closing: Option<CalendarDate>,
    /// The payment terms the engine takes (`GET /payment-terms` with
    /// `bill-terms-v1`, else the fixed list).
    pub(crate) terms: Vec<PaymentTerm>,
    terms_task: Option<Task<Vec<PaymentTerm>>>,
    /// The engine's due and discount dates for the header as it was asked
    /// (`bill-terms-v1`).
    pub(crate) terms_dates_task: Option<Task<TermsDates>>,
    /// What the last terms-dates request was for.
    pub(crate) terms_dates_for: Option<TermsKey>,
    /// The header (its lines stay empty: the grids hold them).
    pub(crate) header: FormState<BillForm>,
    pub(crate) expenses: LinesGrid<RegisterLists>,
    pub(crate) items: items::ItemsGrid,
    pub(crate) tab: Tab,
    pub(crate) inputs: Inputs,
    pub(crate) editing: Option<Editing>,
    pub(crate) load_task: Option<Task<Versioned<cl_api::BillRecord>>>,
    pub(crate) save: SaveState,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    pub(crate) nav: Nav,
    pub(crate) void: VoidPrompt,
    pub(crate) history: HistoryView,
    /// Put the keyboard here next frame.
    pub(crate) focus: Option<Field>,
    /// The header field that had the keyboard last frame.
    pub(crate) focused: Option<Field>,
    /// The vendor name list is open (typing or Ctrl+L).
    pub(crate) vendor_list: bool,
    /// A lines grid had the keyboard last frame.
    in_grid: bool,
    /// A command to run once the frame's fields are drawn (Enter in a
    /// header field is Save & New, Q-190).
    pub(crate) after_frame: Option<CommandId>,
    /// The fixed bars' sizes last frame (Q-265).
    pub(crate) layout: crate::layout::FrameReport,
    /// The window is narrow (200% zoom on the smallest window): the Bill /
    /// Vendor credit choice moves from the toolbar to under the title and
    /// the header card's fields stack.
    pub(crate) compact: bool,
    /// The owner's money display style (from the shell each frame).
    pub(crate) money_style: cl_core::MoneyStyle,
    /// Vendor prefill (Q-191/Q-192).
    pub(crate) prefill: prefill::PrefillState,
    /// Files and the source-document card.
    pub(crate) attach: attach::AttachState,
    /// The vendor panel (WP8).
    pub(crate) panel: vendor_panel::VendorPanel,
    /// Reading the document with AI (WP7).
    pub(crate) read: doc_read::ReadState,
    /// Prev (true) or Next asked on unsaved changes: Save / Discard /
    /// Keep editing.
    pub(crate) nav_prompt: Option<bool>,
    /// Prev/Next to take once the prompt's save is done.
    pub(crate) after_save_nav: Option<bool>,
    /// Saves in other windows (Q-262).
    pub(crate) bus: crate::data_bus::Subscription,
    /// The names, accounts, tags or items changed elsewhere while the form
    /// had unsaved edits: read them again once it is clean.
    pub(crate) lists_stale: bool,
    /// A04 draft mode: the document draft this form enters (Accept is
    /// Save & Close / Save & New, sent to `convert`; `draft.rs`).
    pub(crate) draft: Option<crate::screens::doc_review::review::DraftSession>,
}

/// The tag list `check` needs for the header-tag defaults.
fn tag_list(tags: &[Tag]) -> TagList {
    TagList::new(
        Vec::new(),
        tags.iter().map(crate::tag_names::entry).collect(),
    )
}

/// A blank header dated `date` (no lines: the grids hold them).
pub(crate) fn blank(kind: BillKind, date: CalendarDate) -> BillForm {
    BillForm {
        kind,
        date: Some(date),
        ..BillForm::default()
    }
}

/// `lines_disagree` and the like (control channel; no amounts).
#[must_use]
pub(crate) fn problem_name(p: BillProblem) -> &'static str {
    let key = p.key().0;
    key.strip_prefix("bill.problem.").unwrap_or(key)
}

impl EnterBillScreen {
    /// A new bill in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> EnterBillScreen {
        let group = link.group().and_then(Result::ok);
        let lists = Arc::new(RegisterLists::default());
        let expenses = LinesGrid::new(&EXPENSES, Arc::clone(&lists), Vec::new());
        let items = items::ItemsGrid::new(Arc::new(items::ItemLists::new(Arc::clone(&lists), &[])));
        let mut s = EnterBillScreen {
            link,
            tenant,
            group,
            today,
            accounts: Vec::new(),
            accounts_task: None,
            accounts_loaded: false,
            contacts: Vec::new(),
            contacts_task: None,
            read_context: crate::reading::service::ReadContext::default(),
            read_context_task: None,
            tags: Vec::new(),
            tags_task: None,
            items_list: Vec::new(),
            items_task: None,
            tag_list: TagList::default(),
            lists,
            settings_task: None,
            closing: None,
            terms: PaymentTerm::fixed(),
            terms_task: None,
            terms_dates_task: None,
            terms_dates_for: None,
            header: FormState::new(blank(BillKind::Bill, today)),
            expenses,
            items,
            tab: Tab::Expenses,
            inputs: Inputs {
                vendor: String::new(),
                date: DateInput::new(today),
                amount: String::new(),
                amount_problem: None,
                vendor_problem: false,
                due: empty_date(),
                terms_days: String::new(),
                discount_percent: String::new(),
                discount_days: String::new(),
                discount: empty_date(),
                rev: u64::MAX,
            },
            editing: None,
            load_task: None,
            save: SaveState::default(),
            closing_prompt: ClosingPrompt::default(),
            nav: Nav::default(),
            void: VoidPrompt::default(),
            history: HistoryView::default(),
            focus: Some(Field::Vendor),
            focused: None,
            vendor_list: false,
            in_grid: false,
            after_frame: None,
            layout: crate::layout::FrameReport::default(),
            compact: false,
            money_style: cl_core::MoneyStyle::default(),
            prefill: prefill::PrefillState::default(),
            attach: attach::AttachState::default(),
            panel: vendor_panel::VendorPanel::default(),
            read: doc_read::ReadState::default(),
            nav_prompt: None,
            after_save_nav: None,
            bus: crate::data_bus::Subscription::detached(),
            lists_stale: false,
            draft: None,
        };
        s.bus = s.link.bus().subscribe();
        s.start_load();
        // The demo answers at once: the form is complete before the first
        // frame.
        s.poll();
        s
    }

    /// The mockup's unsaved draft for `--demo` and tests: Midwest Equipment
    /// Rental, 11/03/2026, INV-55871, 1,330.00 + 150.00 = 1,480.00 on
    /// 6210 for Long Creek Marina (the mockup's Table Rock Marina, see
    /// `demo::bills::STORY_CUSTOMER`), billable, tag Install, with its
    /// invoice as the source document.
    #[must_use]
    pub fn demo_draft(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> EnterBillScreen {
        let mut s = EnterBillScreen::new(link, tenant, today);
        s.seed_mockup();
        s
    }

    fn seed_mockup(&mut self) {
        use crate::company::demo::bills::{
            STORY_CUSTOMER, STORY_PDF, STORY_PDF_NAME, STORY_VENDOR,
        };
        let by_number = |n: &str| {
            self.accounts
                .iter()
                .find(|a| a.fields.account_number.as_deref() == Some(n))
                .map(|a| a.id)
        };
        let contact = |name: &str| {
            self.contacts
                .iter()
                .find(|c| c.fields.display_name == name)
                .map(|c| c.id)
        };
        let money = |s: &str| Money::parse_wire(s).ok();
        let rental = by_number("6210");
        let (Some(vendor), customer) = (contact(STORY_VENDOR), contact(STORY_CUSTOMER)) else {
            return;
        };
        let install = self
            .tags
            .iter()
            .find(|t| t.fields.name == "Install")
            .map(|t| t.id);
        self.pick_vendor(vendor);
        let date = CalendarDate::from_ymd(2026, 11, 3).ok();
        self.header.edit_step(|h| {
            h.date = date;
            h.reference = "INV-55871".into();
            h.terms = Some("net_15".into());
            h.memo = "Mini excavator + delivery \u{b7} Long Creek Marina seawall".into();
            h.set_amount_due(money("1480.00"));
        });
        let keys = &mut self.expenses.form_mut().adapter_mut().keys;
        let line = |amount: &str, memo: &str, key| cl_model::forms::lines::FormLine {
            account: rental,
            amount: money(amount),
            memo: memo.into(),
            customer,
            billable: customer.is_some(),
            tag: install,
            ..cl_model::forms::lines::FormLine::blank(key)
        };
        let seeded = vec![
            line("1330.00", "Mini excavator, 2 days", keys.next_key()),
            line("150.00", "Delivery and pickup", keys.next_key()),
            cl_model::forms::lines::FormLine::blank(keys.next_key()),
        ];
        self.expenses.form_mut().edit_step(|v| v.lines = seeded);
        self.refresh_terms_dates();
        self.upload_bytes(STORY_PDF_NAME, STORY_PDF.to_vec());
        self.sync_inputs();
    }

    /// The whole bill: the header with both grids' lines.
    #[must_use]
    pub(crate) fn compose(&self) -> BillForm {
        let mut f = self.header.current().clone();
        f.expenses = self.expenses.value().clone();
        f.items = self.items.value().clone();
        f
    }

    /// What the engine can take (plan §2).
    pub(crate) fn caps(&self) -> BillCaps {
        if let Some(d) = self.link.demo_state() {
            return crate::company::demo::bills::caps(&d);
        }
        let c = self.link.capabilities();
        BillCaps {
            closing: c.has(Capabilities::AP_CLOSING),
            lines: c.has(Capabilities::BILL_LINES),
            terms: c.has(Capabilities::BILL_TERMS),
            received: c.has(Capabilities::BILL_RECEIVED),
        }
    }

    /// The engine edits credits, deletes, and guards edits inside its row
    /// lock (`ap-edit-v1`).
    pub(crate) fn ap_edit(&self) -> bool {
        match self.link.demo_state() {
            Some(d) => crate::company::demo::bills::can_edit(&d),
            None => self.link.capabilities().has(Capabilities::AP_EDIT),
        }
    }

    /// Bill or vendor credit.
    pub(crate) fn kind(&self) -> BillKind {
        self.header.current().kind
    }

    /// The vendor picked.
    pub(crate) fn vendor(&self) -> Option<ContactId> {
        self.header.current().vendor
    }

    fn start_load(&mut self) {
        if let Some(mut demo) = self.link.demo_state() {
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let contacts = demo.contacts_mut().clone();
            let tags = demo.tags_mut().clone();
            let items = demo.items_mut().clone();
            let closing = demo.open.and_then(|open| {
                demo.companies
                    .iter()
                    .find(|c| c.tenant_id == open)
                    .and_then(|c| c.closed_through)
            });
            drop(demo);
            self.closing = closing;
            self.accounts_task = Some(Task::ready(chart));
            self.contacts_task = Some(Task::ready(contacts));
            self.tags_task = Some(Task::ready(tags));
            self.items_task = Some(Task::ready(items));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.accounts_loaded = true;
            return;
        };
        if let Ok(p) = group.read(&session, |s| async move {
            s.accounts().list_all(&AccountFilter::default()).await
        }) {
            self.accounts_task = Some(Task::Pending(p));
        } else {
            self.accounts_loaded = true;
        }
        if let Ok(p) = group.read(&session, |s| async move {
            s.contacts().list_all(&ContactFilter::default()).await
        }) {
            self.contacts_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move {
            crate::reading::service::load_context(&s).await
        }) {
            self.read_context_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move {
            s.tags().list_all(&TagFilter::default()).await
        }) {
            self.tags_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move {
            s.items().list_all(&ItemFilter::default()).await
        }) {
            self.items_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move { s.company_settings().await }) {
            self.settings_task = Some(Task::Pending(p));
        }
        if self.caps().terms
            && let Ok(p) = group.read(&session, |s| async move {
                let list = s.bills().payment_terms().await?;
                Ok(list.into_iter().filter_map(save::term_of).collect())
            })
        {
            self.terms_task = Some(Task::Pending(p));
        }
    }

    fn apply_lists(&mut self) {
        let mut lists = RegisterLists::default();
        lists.set_accounts(&self.accounts);
        lists.set_contacts(&self.contacts);
        lists.set_tags(&self.tags);
        self.lists = Arc::new(lists);
        self.expenses.set_lists(Arc::clone(&self.lists));
        self.items.set_lists(Arc::new(items::ItemLists::new(
            Arc::clone(&self.lists),
            &self.items_list,
        )));
        self.tag_list = tag_list(&self.tags);
        self.apply_locks();
        self.inputs.rev = u64::MAX;
    }

    /// Customer / Job and Billable are read-only without `bill-lines-v1`,
    /// and never billable on a credit (process review S2).
    pub(crate) fn apply_locks(&mut self) {
        let lines = self.caps().lines;
        let credit = self.kind() == BillKind::Credit;
        let mut locked = Vec::new();
        if !lines {
            locked.extend([L_CUSTOMER, L_BILLABLE]);
        } else if credit {
            locked.push(L_BILLABLE);
        }
        self.expenses.set_locked(locked);
        let item_locked = if credit {
            vec![items::I_BILLABLE]
        } else {
            Vec::new()
        };
        self.items.set_locked(item_locked);
    }

    pub(crate) fn poll(&mut self) {
        let mut lists_changed = false;
        if let Some(r) = self.accounts_task.as_mut().and_then(Task::try_take) {
            self.accounts_task = None;
            self.accounts_loaded = true;
            if let Ok(a) = r {
                self.accounts = a;
                lists_changed = true;
            }
        }
        if let Some(r) = self.contacts_task.as_mut().and_then(Task::try_take) {
            self.contacts_task = None;
            if let Ok(c) = r {
                self.contacts = c;
                lists_changed = true;
            }
        }
        if let Some(r) = self.read_context_task.as_mut().and_then(Task::try_take) {
            self.read_context_task = None;
            // Without it the reading is rules only (S12).
            if let Ok(c) = r {
                self.read_context = c;
            }
        }
        if let Some(r) = self.tags_task.as_mut().and_then(Task::try_take) {
            self.tags_task = None;
            if let Ok(tags) = r {
                self.tags = tags;
                lists_changed = true;
            }
        }
        if let Some(r) = self.items_task.as_mut().and_then(Task::try_take) {
            self.items_task = None;
            if let Ok(items) = r {
                self.items_list = items;
                lists_changed = true;
            }
        }
        if lists_changed {
            self.apply_lists();
        }
        if let Some(r) = self.settings_task.as_mut().and_then(Task::try_take) {
            self.settings_task = None;
            if let Ok(s) = r {
                self.closing = s.lock_date;
            }
        }
        if let Some(r) = self.terms_task.as_mut().and_then(Task::try_take) {
            self.terms_task = None;
            if let Ok(list) = r
                && !list.is_empty()
            {
                self.terms = list;
            }
        }
        self.poll_terms_dates();
        self.poll_save();
        self.poll_prefill();
        self.poll_attach();
    }

    /// Saves in other windows of this app (Q-262). The name, account, tag
    /// and item lists are read again only while the form has no unsaved
    /// edits; the vendor panel hears its vendor's bills.
    pub(crate) fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if changes.lists() || changes.items {
            self.lists_stale = true;
        }
        if self.lists_stale
            && !self.is_dirty()
            && self.accounts_task.is_none()
            && self.contacts_task.is_none()
            && self.tags_task.is_none()
            && self.items_task.is_none()
        {
            self.lists_stale = false;
            self.start_load();
        }
        if !changes.is_empty() {
            self.panel.bills_changed(&changes);
        }
    }

    /// Tells the other windows (and this window's vendor panel) that a
    /// bill or credit of `vendor` was saved, voided or deleted (Q-262).
    pub(crate) fn publish(&mut self, vendor: Option<ContactId>) {
        let bus = self.link.bus();
        bus.publish_from(&self.bus, crate::data_bus::Change::Ledger(None));
        if let Some(v) = vendor {
            bus.publish_from(&self.bus, crate::data_bus::Change::Bills(v));
            // The publishing window never hears its own notice: tell the
            // panel directly.
            let mut own = crate::data_bus::Changes::default();
            own.bill_vendors.insert(v);
            self.panel.bills_changed(&own);
        }
    }

    /// The header texts from the form (after a load, clear, undo or a
    /// change the screen made).
    pub(crate) fn sync_inputs(&mut self) {
        let f = self.header.current();
        let style = self.money_style;
        if let Some(d) = f.date {
            self.inputs.date = DateInput::new(d);
        }
        self.inputs.vendor = f
            .vendor
            .and_then(|p| self.lists.name_text(p))
            .map(str::to_string)
            .unwrap_or_default();
        self.inputs.amount = f.amount_due.map(|m| m.format(style)).unwrap_or_default();
        self.inputs.due = f.due_date.map_or_else(empty_date, DateInput::new);
        self.inputs.terms_days = f.terms_days.map(|d| d.to_string()).unwrap_or_default();
        self.inputs.discount_percent = f
            .discount_percent
            .map(save::percent_text)
            .unwrap_or_default();
        self.inputs.discount_days = f.discount_days.map(|d| d.to_string()).unwrap_or_default();
        self.inputs.discount = f.discount_date.map_or_else(empty_date, DateInput::new);
        self.inputs.amount_problem = None;
        self.inputs.vendor_problem = false;
        self.inputs.rev = self.header.revision();
    }

    /// The form has unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.header.is_dirty() || self.expenses.dirty() || self.items.dirty()
    }

    /// The owner left the lines (as Write Check, Q-P13r): a blank amount due
    /// takes the lines total; an amount from the lines follows them.
    pub(crate) fn left_lines(&mut self) {
        let mut f = self.compose();
        f.leave_lines();
        let (amount, from) = (f.amount_due, f.amount_from_lines);
        let h = self.header.current();
        if h.amount_due != amount || h.amount_from_lines != from {
            self.header.edit_step(|h| {
                h.amount_due = amount;
                h.amount_from_lines = from;
            });
            self.sync_inputs();
        }
    }

    /// The owner typed the amount due.
    pub(crate) fn amount_typed(&mut self, amount: Option<Money>) {
        self.header
            .edit(field::AMOUNT, |h| h.set_amount_due(amount));
        // A single used expense line with no amount follows it (as the
        // vendor default does): nothing else changes on its own.
        self.inputs.amount = amount
            .map(|m| m.format(self.money_style))
            .unwrap_or_default();
        self.inputs.rev = self.header.revision();
    }

    /// Bill or vendor credit (the radio).
    pub(crate) fn set_kind(&mut self, kind: BillKind) {
        if self.kind() == kind || self.editing.is_some() {
            return;
        }
        self.header.edit_step(|h| h.kind = kind);
        self.nav = Nav::default();
        self.apply_locks();
        self.save.problems.clear();
    }

    /// The term in effect: the form's, else the vendor's default.
    pub(crate) fn effective_terms(&self) -> Option<String> {
        let f = self.header.current();
        f.terms.clone().or_else(|| {
            f.vendor.and_then(|v| {
                self.contacts
                    .iter()
                    .find(|c| c.id == v)
                    .and_then(|c| c.fields.default_payment_terms.clone())
            })
        })
    }

    /// The due and discount dates from the terms: the engine's
    /// `terms-dates` with `bill-terms-v1`, else the local calendar preview
    /// (plan §2). A date the owner typed stays (S3).
    pub(crate) fn refresh_terms_dates(&mut self) {
        let f = self.header.current().clone();
        if f.kind == BillKind::Credit {
            return;
        }
        let Some(date) = f.date else { return };
        let code = self.effective_terms();
        let local = code
            .as_deref()
            .and_then(|c| self.terms.iter().find(|t| t.code == c))
            .map(|term| {
                cl_model::forms::bill::preview_dates(date, term, f.terms_days, f.discount_days)
            })
            .unwrap_or_default();
        let live = !self.link.is_demo() && self.caps().terms;
        let key = (
            date,
            code.clone(),
            f.terms_days,
            f.discount_days,
            f.discount_percent,
            f.vendor,
        );
        if live {
            if self.terms_dates_for.as_ref() == Some(&key) {
                return;
            }
            self.terms_dates_for = Some(key);
            if let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) {
                let q = cl_api::TermsDatesQuery {
                    date,
                    terms: code.map(|c| {
                        let custom = c == cl_api::TERMS_CUSTOM;
                        cl_api::BillTerms {
                            days: custom.then_some(f.terms_days.map(u32::from)).flatten(),
                            // The custom discount (WP8): both or neither.
                            discount_percent: custom
                                .then(|| f.discount_percent.zip(f.discount_days))
                                .flatten()
                                .and_then(|(p, _)| {
                                    cl_api::Percent::parse(&save::percent_text(p)).ok()
                                }),
                            discount_days: custom
                                .then(|| f.discount_percent.zip(f.discount_days))
                                .flatten()
                                .map(|(_, d)| u32::from(d)),
                            ..cl_api::BillTerms::code(&c)
                        }
                    }),
                    vendor: f.vendor,
                };
                if let Ok(p) = group.read(&session, move |s| async move {
                    let d = s.bills().terms_dates(&q).await?;
                    Ok(TermsDates {
                        due: d.due_date,
                        discount: d.discount_date,
                    })
                }) {
                    self.terms_dates_task = Some(Task::Pending(p));
                    return;
                }
            }
        }
        self.apply_dates(local);
    }

    fn poll_terms_dates(&mut self) {
        if let Some(r) = self.terms_dates_task.as_mut().and_then(Task::try_take) {
            self.terms_dates_task = None;
            if let Ok(d) = r {
                self.apply_dates(d);
            }
        }
    }

    /// Dates from the terms into the form (not an undo step of their own).
    fn apply_dates(&mut self, dates: TermsDates) {
        let mut f = self.header.current().clone();
        f.apply_terms_dates(dates);
        let h = self.header.current();
        if f.due_date != h.due_date || f.discount_date != h.discount_date {
            let (due, disc) = (f.due_date, f.discount_date);
            if self.header.is_dirty() {
                self.header.edit(field::DUE, |h| {
                    h.due_date = due;
                    h.discount_date = disc;
                });
            } else {
                // Part of a clean form (a load or a new bill).
                self.header.load(f);
            }
            self.inputs.due = due.map_or_else(empty_date, DateInput::new);
            self.inputs.discount = disc.map_or_else(empty_date, DateInput::new);
        }
    }

    /// The tab title: "Enter Bill", "Enter Bill · INV-55871" or the
    /// vendor-credit equivalents.
    fn title_text(&self) -> String {
        let f = self.header.current();
        let base = match f.kind {
            BillKind::Bill => t("cmd.vendors.enter_bill"),
            BillKind::Credit => t("bill.title.credit"),
        };
        match f.reference.trim() {
            "" => base,
            r => t_with("bill.title.ref", &[("title", &base), ("ref", r)]),
        }
    }

    fn busy(&self) -> bool {
        self.accounts_task.is_some()
            || self.contacts_task.is_some()
            || self.tags_task.is_some()
            || self.items_task.is_some()
            || self.terms_dates_task.is_some()
            || self.save.task.is_some()
            || self.save.attach_retry.is_some()
            || self.load_task.is_some()
            || self.nav.task.is_some()
            || self.void.task.is_some()
            || self.history.task.is_some()
            || self.prefill.task.is_some()
            || self.attach.busy()
            || self.panel.busy()
            || self.read.busy()
            || self
                .draft
                .as_ref()
                .is_some_and(crate::screens::doc_review::review::DraftSession::busy)
    }

    /// The control-channel state: flags, counts, ids and dates only (never
    /// an amount, a name, a ref or a memo).
    pub(crate) fn state_json(&self) -> Value {
        let f = self.header.current();
        let form = self.compose();
        let caps = self.caps();
        let totals = form.totals_status().map(|s| match s {
            TotalsStatus::Balanced => "balanced",
            TotalsStatus::Remaining(_) => "remaining",
            TotalsStatus::Over(_) => "over",
        });
        let mut v = json!({
            "screen": "enter_bill",
            "company_open": self.tenant.is_some(),
            "loaded": self.accounts_loaded,
            "kind": match f.kind { BillKind::Bill => "bill", BillKind::Credit => "credit" },
            "mode": if self.editing.is_some() { "edit" } else { "new" },
            "read_only": self.editing.as_ref().is_some_and(|e| e.read_only),
            "read_only_note": self.editing.as_ref().filter(|e| e.read_only).map(|e| e.read_only_note),
            "has_payments": self.editing.as_ref().is_some_and(|e| e.has_payments),
            "locked": self.editing_locked(),
            "date": f.date.map(CalendarDate::to_wire),
            "due_date": f.due_date.map(CalendarDate::to_wire),
            "due_overridden": f.due_overridden,
            "discount_date": f.discount_date.map(CalendarDate::to_wire),
            "terms": self.effective_terms(),
            "has_vendor": f.vendor.is_some(),
            "has_ref": !f.reference.trim().is_empty(),
            "amount_set": f.amount_due.is_some(),
            "amount_from_lines": f.amount_from_lines,
            "bill_received": f.bill_received,
            "expense_lines": form.expenses.used().count(),
            "item_lines": form.items.used().count(),
            "tab": match self.tab { Tab::Expenses => "expenses", Tab::Items => "items" },
            "totals": totals,
        });
        let more = json!({
            "dirty": self.is_dirty(),
            "saving": self.save.task.is_some(),
            "saved_count": self.save.count,
            "save": {
                "refused": self.save.problems.first().map(|p| problem_name(*p)),
                "problems": self.save.problems.len(),
                "error": self.save.error.is_some(),
                "refusal": self.save.refusal,
                "stale": self.save.stale,
                "attach_failed": self.save.attach_failed.is_some(),
            },
            "lost": self.save.lost.state_json(),
            "last_saved": self.save.last.as_ref().map(|l| json!({
                "kind": match l.kind { BillKind::Bill => "bill", BillKind::Credit => "credit" },
                "id": l.id.to_string(),
            })),
            "last_action": self.save.last_action,
            "notice": self.save.notice,
            "caps": {
                "closing": caps.closing,
                "lines": caps.lines,
                "terms": caps.terms,
                "received": caps.received,
                "edit": self.ap_edit(),
            },
            "items_tab": caps.lines,
        });
        let rest = json!({
            "void_open": self.void.open,
            "delete_open": self.void.open && self.void.delete,
            "history_open": self.history.open,
            "nav_at": self.nav.at,
            "nav_prompt": self.nav_prompt.is_some(),
            "vendor_list": self.vendor_list,
            "focused": self.focused.map(|f| format!("{f:?}").to_lowercase()),
            "grid": {
                "expenses_focused": self.expenses.has_focus(),
                "items_focused": self.items.has_focus(),
            },
            "prefill": {
                "applied": self.prefill.applied,
                "source": self.prefill.source,
                "dropped": self.prefill.dropped,
                "pending": self.prefill.task.is_some(),
            },
            "attachments": self.attach.state_json(&form),
            "panel": self.panel.state_json(),
            "read": self.read.state_json(),
            // The demo engine's writes since the last Read document (null on
            // a live engine): reading makes no draft and no bill (A03 S8).
            "writes_since_read": self.link.demo_state().and_then(|d| {
                self.read
                    .local
                    .writes_at_read
                    .map(|w| d.bills.writes.saturating_sub(w))
            }),
            "layout": self.layout.json(),
            "draft": self.draft_state(),
        });
        for part in [more, rest, self.closing_prompt.state_json(caps.closing)] {
            if let (Value::Object(m), Value::Object(c)) = (&mut v, part) {
                m.extend(c);
            }
        }
        v
    }
}

/// An empty date field.
pub(crate) fn empty_date() -> DateInput {
    DateInput {
        text: String::new(),
        value: None,
    }
}

impl Screen for EnterBillScreen {
    fn title(&self) -> String {
        self.title_text()
    }

    fn ui(&mut self, ui: &mut Ui) {
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        if style != self.money_style {
            self.money_style = style;
            self.inputs.rev = u64::MAX;
        }
        self.poll();
        self.hear_changes();
        // A04 draft mode: the draft's load, convert and questions.
        self.poll_draft(ui.ctx());
        if self.inputs.rev != self.header.revision() {
            self.sync_inputs();
        }
        // A finished reading fills the form (WP7): read, never accepted.
        self.poll_read();
        self.panel_tick();
        self.compare_before();
        self.draw(ui);
        self.compare_after();
        if let Some(d) = self.draft.as_mut() {
            d.dialogs_ui(ui.ctx(), self.money_style);
        }
        // A read field the owner changed counts as accepted.
        self.track_read();
        // Leaving the lines (Q-P13r b).
        let in_grid = self.expenses.has_focus() || self.items.has_focus();
        if self.in_grid && !in_grid {
            self.left_lines();
        }
        self.in_grid = in_grid;
        self.recheck_problems();
        if let Some(cmd) = self.after_frame.take() {
            self.command(cmd);
        }
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        if self.attach.viewer.has_focus() {
            Scope::Viewer
        } else {
            Scope::Form
        }
    }

    fn dirty(&self) -> bool {
        self.is_dirty()
    }

    fn save_unconfirmed(&self) -> bool {
        self.save.lost.pending()
    }

    fn save(&mut self) -> bool {
        self.start_save(cl_model::forms::SaveIntent::Save, None)
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(handled) = self.closing_prompt.command(id) {
            return handled;
        }
        if let Some(handled) = self.draft_command(id) {
            return handled;
        }
        if self.attach.viewer.has_focus() && self.attach.viewer.command(id) {
            return true;
        }
        if let Some(handled) = self.expenses.command(id) {
            return handled;
        }
        if let Some(handled) = self.items.command(id) {
            return handled;
        }
        self.form_command(id)
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        true
    }

    /// The source card's file (A02): the pop-out viewer follows it.
    fn document(&self) -> Option<crate::windows::doc_viewer::DocRef> {
        self.attach
            .viewer
            .source()
            .cloned()
            .map(crate::windows::doc_viewer::DocRef::new)
    }

    /// A saved bill or credit opened from elsewhere (the register, the
    /// vendor panel, statement match).
    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.read_bill(txn, None);
        self.editing.as_ref().is_some_and(|e| e.id == txn) || self.load_task.is_some()
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        match payload {
            OpenPayload::BillPrefill(p) => {
                self.fill_prefill(&p);
                true
            }
            OpenPayload::InboxDraft(d) => {
                self.take_inbox_draft(d);
                true
            }
            OpenPayload::DocumentDraft(o) => {
                self.open_draft(o);
                true
            }
            OpenPayload::Document(_) => false,
        }
    }

    /// The compare tools for the pop-out viewer (A04 D4, S7).
    fn compare(&self) -> Option<crate::widgets::compare::CompareView> {
        self.draft.as_ref().map(|d| d.compare.view())
    }

    /// The pop-out viewer's compare events (the shell's outbox).
    fn on_viewer_event(&mut self, event: crate::widgets::compare::CompareEvent) {
        self.on_compare_event(event);
    }
}

impl EnterBillScreen {
    /// A fresh bill or credit from a statement line (WP6): vendor, date,
    /// ref and the statement amount, typed by the owner there. Prefill and
    /// the vendor default still run; nothing is saved.
    pub(crate) fn fill_prefill(&mut self, p: &BillPrefill) {
        self.set_kind(p.kind);
        self.pick_vendor(p.vendor);
        let (date, reference, amount) = (p.date, p.ref_no.clone(), p.amount);
        self.header.edit_step(|h| {
            h.date = Some(date);
            h.reference = reference;
            h.set_amount_due(Some(amount));
        });
        self.refresh_terms_dates();
        self.sync_inputs();
    }

    /// Takes a whole form changed outside the fields (the document reading,
    /// its Accept or Clear): the header as one undo step, the lines into
    /// their grids.
    pub(crate) fn take_form(&mut self, mut form: BillForm) {
        let expenses = std::mem::take(&mut form.expenses);
        let items = std::mem::take(&mut form.items);
        if *self.header.current() != form {
            self.header.edit_step(|h| *h = form);
        }
        // The screen replaced the lines (a reading filled or cleared them):
        // the grids follow without "That line is no longer here." (A03 UI
        // review 1).
        if &expenses != self.expenses.value() {
            self.expenses.form_mut().edit_step(|v| *v = expenses);
            self.expenses.lines_replaced();
        }
        if &items != self.items.value() {
            self.items.form_mut().edit_step(|v| *v = items);
            self.items.lines_replaced();
        }
        self.sync_inputs();
    }

    /// The vendor panel's reads, every frame (drawn or not); then the save
    /// that waited for the duplicate check, or that the owner chose to
    /// send anyway (WP8).
    fn panel_tick(&mut self) {
        let now = std::time::Instant::now();
        self.with_panel(|panel, input| panel.tick(input, now));
        if let Some(intent) = self.panel.take_resume() {
            self.start_save(intent, None);
        }
    }

    /// Runs `f` with the vendor panel and what it needs from the form.
    pub(crate) fn with_panel<R>(
        &mut self,
        f: impl FnOnce(&mut vendor_panel::VendorPanel, &vendor_panel::PanelInput<'_>) -> R,
    ) -> R {
        let mut panel = std::mem::take(&mut self.panel);
        let h = self.header.current();
        let input = vendor_panel::PanelInput {
            link: &self.link,
            kind: h.kind,
            vendor: h.vendor,
            ref_no: &h.reference,
            date: h.date,
            editing: self.editing.as_ref().map(|e| e.id),
            contact: h
                .vendor
                .and_then(|v| self.contacts.iter().find(|c| c.id == v)),
            accounts: &self.accounts,
            money_style: self.money_style,
        };
        let r = f(&mut panel, &input);
        self.panel = panel;
        r
    }

    /// The default account of the vendor picked (Q-192), when active.
    pub(crate) fn vendor_default_account(&self) -> Option<AccountId> {
        let v = self.vendor()?;
        self.contacts
            .iter()
            .find(|c| c.id == v)
            .and_then(|c| c.fields.default_expense_account_id)
            .filter(|a| self.accounts.iter().any(|x| x.id == *a && x.is_active))
    }

    /// A fresh set of grid lines (a new bill, a load).
    pub(crate) fn load_lines(&mut self, expenses: FormLines, items: ItemLines) {
        let lists = Arc::clone(&self.lists);
        self.expenses.form_mut().load(expenses, &*lists);
        self.items.load(items);
    }

    /// Blank lines with fresh keys.
    pub(crate) fn blank_lines(&mut self) -> (FormLines, ItemLines) {
        let expenses =
            FormLines::from_lines(Vec::new(), &mut self.expenses.form_mut().adapter_mut().keys);
        let items = ItemLines::from_lines(Vec::new(), self.items.keys());
        (expenses, items)
    }
}
