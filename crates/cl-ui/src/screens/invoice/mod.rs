//! Create Invoice (P20 WP-B, `clients.create_invoice`, Ctrl+I), after the
//! `CreateInvoice` mockup with the Q-P20v deviations.
//!
//! - **Every figure is the engine's** (Q-P20n): the form sends what the
//!   owner typed to `POST /invoices/preview` (debounced 250 ms, tagged by
//!   `cl_model::forms::invoice::PreviewTracker`) and shows the rates, line
//!   amounts, subtotal, tax, total, due date and the client's balance after
//!   this invoice from the reply. While a newer preview is pending they say
//!   "Updating…", never an old figure; a failed preview says "Can't
//!   calculate" and Save still works (posting runs the same engine code).
//! - Header: Client (customers only; a name not in the list offers Set Up,
//!   Q-P20i), Date, Invoice #, Terms, Due (the engine's from the terms
//!   unless typed), P.O. no., Tags. Ship to sits in the right column
//!   (Q-P20m, Q-P20v). Projects wait for U-35 (Q-P20r).
//! - An edit sends the record's `updatedAt` (409 STALE → Reload), asks for
//!   the closing-date password for a closed period, keeps an unknown
//!   outcome in the shared lost-save book; Void takes a reason.
//! - A sent invoice shows when and to whom it went (Q-P20f) and, after an
//!   edit is saved, offers to send the updated copy.
//! - Add Billable Costs (Ctrl+Shift+B, Q-P20c) and Email (Ctrl+Shift+M,
//!   Q-P20aa) live in [`billable`] and [`email`].

mod billable;
mod draw;
pub(crate) mod email;
pub(crate) mod engine;
mod save;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use cl_api::{
    ApiError, Capabilities, ClientSales, IdempotencyKey, InvoicePreview, InvoiceRecord, Item,
    ItemFilter, RequestGroup, Versioned,
};
use cl_core::ids::{ContactId, ItemId, TenantId, TransactionId};
use cl_core::{CalendarDate, MoneyStyle};
use cl_model::forms::invoice::{
    InvoiceCaps, InvoiceContext, InvoiceForm, InvoiceProblem, PreviewTicket, PreviewTracker,
    TotalsView, check_invoice,
};
use cl_model::forms::{FormState, SaveIntent};
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::forms::closing_prompt::ClosingPrompt;
use crate::forms::item_lines_grid::ItemLists;
use crate::i18n::t;
use crate::screens::card_charge::kit::{Books, NameInput};
use crate::screens::company::form::DateInput;
use crate::screens::register::lists::RegisterLists;
use crate::windows::Screen;

use crate::forms::invoice_lines_grid::{self as grid, InvoiceGrid};
use engine::Job;

/// The command and window kind.
pub const KIND: &str = "clients.create_invoice";

/// The Create Invoice window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(InvoiceScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// Header fields (undo merges typing within one field).
pub(crate) mod field {
    pub const CLIENT: u64 = 1;
    pub const DATE: u64 = 2;
    pub const NUMBER: u64 = 3;
    pub const DUE: u64 = 5;
    pub const PO: u64 = 6;
    pub const MEMO: u64 = 7;
    pub const MESSAGE: u64 = 8;
    pub const TERMS_DAYS: u64 = 9;
}

/// The fixed payment terms (U-125), in menu order.
pub(crate) const TERMS: [&str; 7] = [
    "due_on_receipt",
    "net_10",
    "net_15",
    "net_30",
    "net_45",
    "net_60",
    "custom",
];

/// What the closing-date password is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    /// The save (with what follows it).
    Save(SaveIntent),
    /// Void the open invoice with this reason.
    Void(String),
}

impl Pending {
    pub(crate) fn verb(&self) -> crate::forms::closing_prompt::Verb {
        use crate::forms::closing_prompt::Verb;
        match self {
            Pending::Save(_) => Verb::Change,
            Pending::Void(_) => Verb::Void,
        }
    }
}

/// A saved invoice open in the form.
#[derive(Clone, Debug)]
pub(crate) struct Editing {
    pub id: TransactionId,
    pub record: InvoiceRecord,
    pub old_date: CalendarDate,
}

impl Editing {
    pub(crate) fn void(&self) -> bool {
        self.record.is_void()
    }
}

/// The header texts being typed.
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub date: DateInput,
    pub due: DateInput,
    pub client: NameInput,
    pub terms_days: String,
    pub rev: u64,
}

/// The Create Invoice window.
pub struct InvoiceScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    pub(crate) books: Books,
    pub(crate) items: Vec<Item>,
    pub(crate) items_job: Option<Job<Vec<Item>>>,
    /// Customers only (the Client field).
    pub(crate) clients: Arc<RegisterLists>,
    /// The header (its `lines` stay empty: the grid holds them).
    pub(crate) header: FormState<InvoiceForm>,
    pub(crate) grid: InvoiceGrid,
    pub(crate) inputs: Inputs,
    pub(crate) tracker: PreviewTracker,
    pub(crate) preview_job: Option<(PreviewTicket, Job<InvoicePreview>)>,
    pub(crate) preview_problem: Option<&'static str>,
    pub(crate) clock: Instant,
    pub(crate) editing: Option<Editing>,
    pub(crate) save_job: Option<Job<Versioned<InvoiceRecord>>>,
    pub(crate) intent: Option<SaveIntent>,
    pub(crate) key: Option<(String, IdempotencyKey)>,
    pub(crate) void_job: Option<(Pending, Job<Versioned<InvoiceRecord>>)>,
    pub(crate) load_job: Option<Job<Versioned<InvoiceRecord>>>,
    pub(crate) nav_job: Option<Job<Vec<TransactionId>>>,
    pub(crate) nav: Vec<TransactionId>,
    pub(crate) nav_at: Option<usize>,
    pub(crate) listed: bool,
    pub(crate) want_older: Option<bool>,
    pub(crate) sales_job: Option<(ContactId, Job<ClientSales>)>,
    pub(crate) sales: Option<(ContactId, ClientSales)>,
    pub(crate) billable: billable::Billable,
    pub(crate) email: email::EmailFlow,
    pub(crate) problems: Vec<InvoiceProblem>,
    pub(crate) error: Option<ApiError>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) stale: bool,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    pub(crate) void_open: bool,
    pub(crate) void_reason: String,
    /// After an edit of a sent invoice was saved: offer to send it again.
    pub(crate) offer_send: bool,
    /// "Email a PDF after saving".
    pub(crate) email_after_save: bool,
    /// A name typed in Client that isn't in the list (Set Up offered).
    pub(crate) unknown_client: Option<String>,
    /// The client form was opened for this name: picked once it arrives.
    pub(crate) setting_up: Option<String>,
    pub(crate) saved_count: u32,
    pub(crate) last_action: Option<&'static str>,
    pub(crate) last_saved: Option<TransactionId>,
    pub(crate) focused: Option<u64>,
    pub(crate) after_frame: Option<CommandId>,
    pub(crate) layout: crate::layout::FrameReport,
    pub(crate) money_style: MoneyStyle,
    pub(crate) bus: crate::data_bus::Subscription,
    pub(crate) lists_stale: bool,
    pub(crate) lost: crate::forms::lost_save::LostSave,
}

impl InvoiceScreen {
    /// A new invoice in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> InvoiceScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = InvoiceScreen {
            bus: link.bus().subscribe(),
            link,
            tenant,
            group,
            today,
            books: Books::default(),
            items: Vec::new(),
            items_job: None,
            clients: Arc::new(RegisterLists::default()),
            header: FormState::new(blank(today)),
            grid: InvoiceGrid::new(Arc::new(ItemLists::default())),
            inputs: Inputs {
                date: DateInput::new(today),
                due: empty_date(),
                client: NameInput::default(),
                terms_days: String::new(),
                rev: u64::MAX,
            },
            tracker: PreviewTracker::new(),
            preview_job: None,
            preview_problem: None,
            clock: Instant::now(),
            editing: None,
            save_job: None,
            intent: None,
            key: None,
            void_job: None,
            load_job: None,
            nav_job: None,
            nav: Vec::new(),
            nav_at: None,
            listed: false,
            want_older: None,
            sales_job: None,
            sales: None,
            billable: billable::Billable::default(),
            email: email::EmailFlow::default(),
            problems: Vec::new(),
            error: None,
            notice: None,
            stale: false,
            closing_prompt: ClosingPrompt::default(),
            void_open: false,
            void_reason: String::new(),
            offer_send: false,
            email_after_save: false,
            unknown_client: None,
            setting_up: None,
            saved_count: 0,
            last_action: None,
            last_saved: None,
            focused: None,
            after_frame: None,
            layout: crate::layout::FrameReport::default(),
            money_style: MoneyStyle::default(),
            lists_stale: false,
            lost: crate::forms::lost_save::LostSave::default(),
        };
        s.start_lists();
        s.poll();
        s
    }

    /// The engine (or the demo) has `cap`.
    pub(crate) fn has(&self, cap: &str) -> bool {
        self.link.is_demo() || self.link.capabilities().has(cap)
    }

    /// What the engine can take.
    pub(crate) fn caps(&self) -> InvoiceCaps {
        InvoiceCaps {
            fields: self.has(Capabilities::INVOICE_FIELDS),
            pricing: self.has(Capabilities::INVOICE_PRICING),
            billable: self.has(Capabilities::BILLABLE_COSTS),
            closing: self.has(Capabilities::INVOICE_EDIT),
        }
    }

    /// The engine takes the closing-date password on invoices.
    pub(crate) fn closing_supported(&self) -> bool {
        self.has(Capabilities::INVOICE_EDIT)
    }

    pub(crate) fn context(&self) -> InvoiceContext {
        InvoiceContext {
            closing_date: self.books.closing,
            original_date: self.editing.as_ref().map(|e| e.old_date),
            caps: self.caps(),
        }
    }

    /// The whole form: the header with the grid's lines.
    #[must_use]
    pub(crate) fn compose(&self) -> InvoiceForm {
        let mut f = self.header.current().clone();
        f.lines.clone_from(&self.grid.value().lines);
        f
    }

    /// Unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.header.is_dirty() || self.grid.dirty()
    }

    /// A void invoice can't change.
    pub(crate) fn read_only(&self) -> Option<&'static str> {
        self.editing
            .as_ref()
            .filter(|e| e.void())
            .map(|_| "invoice.read_only.void")
    }

    pub(crate) fn busy(&self) -> bool {
        self.save_job.is_some() || self.void_job.is_some()
    }

    pub(crate) fn idle(&self) -> bool {
        !self.busy() && !self.closing_prompt.dialog_open()
    }

    /// Reads the accounts, names, tags, closing date and items.
    pub(crate) fn start_lists(&mut self) {
        self.books.start(&self.link, self.group.as_ref());
        self.items_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(d.items_mut().clone()),
            |s| async move { s.items().list_all(&ItemFilter::default()).await },
        ));
    }

    /// The pickers from the lists read.
    fn apply_lists(&mut self) {
        let customers: Vec<cl_api::Contact> = self
            .books
            .contacts
            .iter()
            .filter(|c| c.fields.contact_type != cl_api::ContactType::Vendor && c.is_active)
            .cloned()
            .collect();
        let mut clients = RegisterLists::default();
        clients.set_contacts(&customers);
        self.clients = Arc::new(clients);
        let active: Vec<Item> = self.items.iter().filter(|i| i.is_active).cloned().collect();
        let taxable: HashMap<ItemId, bool> = self
            .items
            .iter()
            .map(|i| (i.id, i.fields.is_taxable))
            .collect();
        self.grid.set_lists(
            Arc::new(ItemLists::new(Arc::clone(&self.books.lists), &active)),
            taxable,
        );
        self.inputs.rev = u64::MAX;
        if let Some(name) = self.setting_up.clone()
            && let Some(id) = self
                .books
                .contacts
                .iter()
                .find(|c| c.fields.display_name == name)
                .map(|c| c.id)
        {
            // Set Up came back: the new client fills the field (Q-P20i).
            self.setting_up = None;
            self.unknown_client = None;
            self.pick_client(id);
        }
    }

    pub(crate) fn poll(&mut self) {
        let mut lists = self.books.poll();
        if let Some(r) = engine::poll(&mut self.items_job) {
            self.items = r.unwrap_or_default();
            lists = true;
        }
        if lists {
            self.apply_lists();
        }
        if let Some(r) = engine::poll(&mut self.save_job) {
            match r {
                Ok(v) => self.saved(&v),
                Err(e) => self.save_failed(e),
            }
        }
        if let Some(ev) = self.lost.poll(&mut self.header) {
            self.lost_event(ev);
        }
        if let Some((pending, mut job)) = self.void_job.take() {
            match job.take() {
                None => self.void_job = Some((pending, job)),
                Some(Ok(v)) => self.voided(&v),
                Some(Err(e)) => self.void_failed(&e, pending),
            }
        }
        if let Some(r) = engine::poll(&mut self.load_job) {
            match r {
                Ok(v) => self.show(&v),
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.nav_job) {
            self.nav = r.unwrap_or_default();
            self.listed = true;
            if let Some(older) = self.want_older.take() {
                self.step(older);
            }
        }
        if let Some((client, mut job)) = self.sales_job.take() {
            match job.take() {
                None => self.sales_job = Some((client, job)),
                Some(Ok(s)) => self.sales = Some((client, s)),
                Some(Err(_)) => self.sales = None,
            }
        }
        self.billable.poll();
        self.email.poll();
        if let Some(done) = self.email.take_done() {
            self.email_done(done);
        }
        self.poll_preview();
    }

    /// Sends a preview when the form has stood still 250 ms; applies the
    /// reply (a stale one is dropped by the tracker).
    pub(crate) fn poll_preview(&mut self) {
        if let Some((ticket, mut job)) = self.preview_job.take() {
            match job.take() {
                None => self.preview_job = Some((ticket, job)),
                Some(Ok(p)) => {
                    self.preview_problem = None;
                    self.tracker.apply_preview(ticket, engine::figures_of(&p));
                }
                Some(Err(e)) => {
                    self.preview_problem = Some(match e {
                        ApiError::Forbidden { .. } => "invoice.preview.not_allowed",
                        _ => "invoice.preview.failed",
                    });
                    self.tracker.fail(ticket);
                }
            }
        }
        let caps = self.caps();
        let request = if caps.pricing && self.read_only().is_none() {
            self.compose().preview_request(caps)
        } else {
            None
        };
        let now = u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some((ticket, req)) = self.tracker.poll(request, now) {
            match engine::new_invoice(&req) {
                Ok(new) => {
                    let editing = self.editing.as_ref().map(|e| e.id);
                    let job = engine::read(
                        &self.link,
                        self.group.as_ref(),
                        |d| crate::company::demo::invoices::preview(d, &new),
                        {
                            let new = new.clone();
                            move |s| async move { s.invoices().preview(&new, editing).await }
                        },
                    );
                    self.preview_job = Some((ticket, job));
                }
                Err(_) => self.tracker.fail(ticket),
            }
            // The demo answers at once.
            if self.link.is_demo() {
                self.poll_preview();
            }
        }
        self.grid.show(&self.tracker);
    }

    /// The client's open balance, oldest open invoice, price level and
    /// markup (U-38, U-40, U-128).
    pub(crate) fn fetch_sales(&mut self, client: ContactId) {
        if !self.has(Capabilities::CONTACT_OPEN_BALANCE) {
            return;
        }
        let job = engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::invoices::client_sales(d, client)),
            move |s| async move { s.invoices().client_sales(client).await },
        );
        self.sales_job = Some((client, job));
    }

    /// The client was picked: header defaults, the side panel, the
    /// unbilled costs.
    pub(crate) fn pick_client(&mut self, id: ContactId) {
        self.header.edit(field::CLIENT, |h| h.client = Some(id));
        self.inputs.client.text = self.clients.name_text(id).unwrap_or_default().to_string();
        self.inputs.client.problem = false;
        self.unknown_client = None;
        self.client_changed();
    }

    /// The side panel and the unbilled costs follow the client.
    pub(crate) fn client_changed(&mut self) {
        let client = self.header.current().client;
        if self.sales.as_ref().map(|(c, _)| *c) != client {
            self.sales = None;
        }
        if let Some(c) = client {
            self.fetch_sales(c);
            if self.caps().billable {
                let link = Arc::clone(&self.link);
                self.billable.fetch(&link, self.group.as_ref(), c);
            }
        } else {
            self.billable.forget();
        }
    }

    /// The header texts from the form.
    pub(crate) fn sync_inputs(&mut self) {
        let f = self.header.current();
        if let Some(d) = f.date {
            self.inputs.date = DateInput::new(d);
        }
        self.inputs.due = f.due_date.map_or_else(empty_date, DateInput::new);
        self.inputs.client.text = f
            .client
            .and_then(|c| self.clients.name_text(c).map(str::to_string))
            .unwrap_or_default();
        self.inputs.client.problem = false;
        self.inputs.terms_days = f.terms_days.map(|d| d.to_string()).unwrap_or_default();
        self.inputs.rev = self.header.revision();
    }

    /// A blank invoice; Save & New keeps the date.
    pub(crate) fn new_form(&mut self, keep_date: bool) {
        let date = if keep_date {
            self.header.current().date.unwrap_or(self.today)
        } else {
            self.today
        };
        self.header.load(blank(date));
        self.grid.load(Vec::new());
        self.editing = None;
        self.nav_at = None;
        self.focused = None;
        self.void_open = false;
        self.offer_send = false;
        self.problems.clear();
        self.stale = false;
        self.sales = None;
        self.billable.forget();
        self.unknown_client = None;
        self.sync_inputs();
    }

    /// Clear: throw the unsaved edits away.
    pub(crate) fn clear(&mut self) {
        if self.busy() {
            return;
        }
        self.error = None;
        self.notice = None;
        self.last_action = Some("clear");
        self.new_form(false);
    }

    /// The control-channel state (flags, ids, dates and the engine's
    /// figures as shown; never a name, a memo or an address).
    pub(crate) fn state_json(&self) -> Value {
        let f = self.compose();
        let style = MoneyStyle::default();
        let (totals, subtotal, tax, total, after) = match self.tracker.totals() {
            TotalsView::Nothing => ("none", None, None, None, None),
            TotalsView::Updating => ("updating", None, None, None, None),
            TotalsView::CantCalculate => ("cant_calculate", None, None, None, None),
            TotalsView::Ready(t) => (
                "ready",
                Some(t.subtotal.format(style)),
                Some(t.tax_total.format(style)),
                Some(t.total.format(style)),
                t.balance_after.map(|m| m.format(style)),
            ),
        };
        let lines: Vec<Value> = f
            .lines
            .iter()
            .filter(|l| !l.is_blank())
            .map(|l| {
                let shown = self.grid.shown(l.key);
                let (amount, source) = match shown {
                    grid::Shown::Ready { amount, source, .. } => (
                        Some(amount.format(style)),
                        Some(format!("{source:?}").to_lowercase()),
                    ),
                    grid::Shown::Updating => (Some("updating".into()), None),
                    grid::Shown::Cant => (Some("cant_calculate".into()), None),
                    grid::Shown::Nothing => (None, None),
                };
                json!({
                    "item": l.item.is_some(),
                    "billable": l.billable.is_some(),
                    "taxable": grid::taxable_of(l, self.grid.taxable()),
                    "amount": amount,
                    "rate_source": source,
                })
            })
            .collect();
        let sent = self.editing.as_ref().map(|e| e.record.send_history.len());
        let mut v = json!({
            "screen": "create_invoice",
            "layout": self.layout.json(),
            "company_open": self.tenant.is_some(),
            "loaded": self.books.loaded && self.items_job.is_none(),
            "mode": if self.editing.is_some() { "edit" } else { "new" },
            "read_only": self.read_only(),
            "client": f.client.is_some(),
            "unknown_client": self.unknown_client.is_some(),
            "date": f.date.map(CalendarDate::to_wire),
            "number": f.number,
            "terms": f.terms,
            "due_date": self.tracker.shown_due_date(&f).map(CalendarDate::to_wire),
            "lines": lines,
            "totals": totals,
            "subtotal": subtotal,
            "tax": tax,
            "total": total,
            "balance_after": after,
            "preview_problem": self.preview_problem,
            "open_balance": self.sales.as_ref().and_then(|(_, s)| s.open_balance).map(|m| m.format(style)),
            "unbilled": self.billable.unbilled_json(&f),
            "billable_dialog": self.billable.dialog_json(),
            "email": self.email.state_json(),
            "email_after_save": self.email_after_save,
            "sent_events": sent,
            "offer_send": self.offer_send,
            "invoice_status": self.editing.as_ref().and_then(|e| e.record.invoice_status).map(|s| format!("{s:?}").to_lowercase()),
            "dirty": self.is_dirty(),
            "saving": self.save_job.is_some(),
            "saved_count": self.saved_count,
            "txn_id": self.last_saved.map(|id| id.to_string()),
            "editing_id": self.editing.as_ref().map(|e| e.id.to_string()),
            "save": {
                "refused": self.problems.first().map(|p| p.key().0),
                "problems": self.problems.len(),
                "error": self.error.is_some(),
                "stale": self.stale,
            },
            "last_action": self.last_action,
            "notice": self.notice,
            "void_open": self.void_open,
            "nav_at": self.nav_at,
            "lost": self.lost.state_json(),
            "grid": { "focused": self.grid.has_focus() },
        });
        if let (Value::Object(m), Value::Object(c)) = (
            &mut v,
            self.closing_prompt.state_json(self.closing_supported()),
        ) {
            m.extend(c);
        }
        v
    }

    /// A refused save's problems, checked again against the form as it is
    /// now (a fixed problem goes away at once).
    pub(crate) fn recheck_problems(&mut self) {
        if self.problems.is_empty() {
            return;
        }
        let now = check_invoice(&self.compose(), &self.context())
            .err()
            .unwrap_or_default();
        crate::forms::recheck(&mut self.problems, now);
    }

    /// Saves in other windows: a changed list is read again once the form
    /// is clean; a new client (Set Up) is read at once.
    pub(crate) fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if changes.lists() {
            self.lists_stale = true;
        }
        let waiting = self.setting_up.is_some();
        if self.lists_stale && (waiting || !self.is_dirty()) && !self.books.busy() {
            self.lists_stale = false;
            self.start_lists();
        }
        if changes.ledger_any() {
            self.listed = false;
            if let Some(c) = self.header.current().client {
                self.client_changed_quiet(c);
            }
        }
    }

    fn client_changed_quiet(&mut self, c: ContactId) {
        self.fetch_sales(c);
        if self.caps().billable {
            let link = Arc::clone(&self.link);
            self.billable.fetch(&link, self.group.as_ref(), c);
        }
    }
}

/// A blank invoice dated `date` (no lines: the grid holds them).
fn blank(date: CalendarDate) -> InvoiceForm {
    InvoiceForm {
        date: Some(date),
        ..InvoiceForm::default()
    }
}

/// An empty date field (Due).
fn empty_date() -> DateInput {
    DateInput {
        text: String::new(),
        value: None,
    }
}

impl Screen for InvoiceScreen {
    fn title(&self) -> String {
        match self
            .editing
            .as_ref()
            .and_then(|e| e.record.txn.txn_number.clone())
        {
            Some(n) => crate::i18n::t_with("invoice.title.number", &[("number", &n)]),
            None => t("cmd.clients.create_invoice"),
        }
    }

    fn ui(&mut self, ui: &mut Ui) {
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        if style != self.money_style {
            self.money_style = style;
            self.inputs.rev = u64::MAX;
        }
        self.poll();
        self.hear_changes();
        if self.inputs.rev != self.header.revision() {
            self.sync_inputs();
        }
        self.draw(ui);
        self.recheck_problems();
        if let Some(cmd) = self.after_frame.take() {
            self.command(cmd);
        }
        let waiting = self.books.busy()
            || self.busy()
            || self.items_job.is_some()
            || self.load_job.is_some()
            || self.nav_job.is_some()
            || self.sales_job.is_some()
            || self.preview_job.is_some()
            || self.billable.busy()
            || self.email.busy();
        if waiting {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        } else if matches!(self.tracker.totals(), TotalsView::Updating) {
            // The debounce: come back when the 250 ms are up.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(
                    cl_model::forms::invoice::PREVIEW_DEBOUNCE_MS,
                ));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn dirty(&self) -> bool {
        self.is_dirty()
    }

    fn save_unconfirmed(&self) -> bool {
        self.lost.pending()
    }

    fn save(&mut self) -> bool {
        self.start_save(SaveIntent::Save, None)
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            return h;
        }
        if let Some(h) = self.grid.command(id) {
            return h;
        }
        self.form_command(id)
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.load(txn);
        self.editing.as_ref().is_some_and(|e| e.id == txn) || self.load_job.is_some()
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
