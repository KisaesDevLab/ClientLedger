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
//! - Add Billable Costs (Ctrl+Shift+U, Q-P20c) and Email (Ctrl+Shift+M,
//!   Q-P20aa) live in [`billable`] and [`email`].
//!
//! P22 D4: the same screen is the **Credit Memo** (`clients.credit_memo`)
//! and the **Sales Receipt** (`clients.sales_receipt`, Ctrl+Shift+R): the
//! form's [`SalesDocKind`] gates the fields (no terms, due date, P.O.,
//! billable costs or deposit card; a receipt adds Deposit to, method and
//! reference), the preview and save go to the kind's engine routes, and a
//! credit memo saved while the client has open invoices asks "Apply to an
//! invoice or keep as credit" first ([`credit`], Q-P22b). The saved
//! document sits in [`Editing`] as a [`SavedDoc`]; invoice-only features
//! read [`Editing::invoice`] and go inert for the other kinds.

mod attach;
mod billable;
pub(crate) mod credit;
// P21 D7: the held customer deposit card (Q-P21f, Q-P21l).
pub(crate) mod deposit_card;
mod draw;
pub(crate) mod email;
pub(crate) mod engine;
mod save;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use cl_api::{
    ApiError, Capabilities, ClientSales, CreditMemoRecord, IdempotencyKey, InvoicePreview,
    InvoiceRecord, Item, ItemFilter, RequestGroup, SalesReceiptRecord,
};
use cl_core::ids::{ContactId, ItemId, TenantId, TransactionId};
use cl_core::{CalendarDate, MoneyStyle, Timestamp};
use cl_model::forms::credit_memo::{CreditMemoFigures, CreditMemoForm};
use cl_model::forms::invoice::{
    InvoiceCaps, InvoiceContext, InvoiceForm, InvoiceProblem, InvoiceRequest, PreviewTicket,
    PreviewTracker, SalesDocKind, TotalsView, check_invoice,
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
/// The Credit Memo window (P22 D4).
pub const CREDIT_MEMO_KIND: &str = "clients.credit_memo";
/// The Sales Receipt window (P22 D4, Q-P22e).
pub const SALES_RECEIPT_KIND: &str = "clients.sales_receipt";

/// The window kind of a document kind.
#[must_use]
pub const fn kind_command(kind: SalesDocKind) -> &'static str {
    match kind {
        SalesDocKind::Invoice => KIND,
        SalesDocKind::CreditMemo => CREDIT_MEMO_KIND,
        SalesDocKind::SalesReceipt => SALES_RECEIPT_KIND,
    }
}

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

/// The Credit Memo window for `kind`, if it is one (P22 D4).
#[must_use]
pub fn make_credit_memo(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == CREDIT_MEMO_KIND).then(|| {
        Box::new(InvoiceScreen::new_kind(
            Arc::clone(link),
            open,
            today,
            SalesDocKind::CreditMemo,
        )) as Box<_>
    })
}

/// The Sales Receipt window for `kind`, if it is one (P22 D4).
#[must_use]
pub fn make_sales_receipt(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == SALES_RECEIPT_KIND).then(|| {
        Box::new(InvoiceScreen::new_kind(
            Arc::clone(link),
            open,
            today,
            SalesDocKind::SalesReceipt,
        )) as Box<_>
    })
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
    /// A sales receipt's check or reference number (P22).
    pub const REF: u64 = 10;
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
    /// Apply this held customer deposit to the open invoice (P21 D7).
    ApplyDeposit(TransactionId),
}

impl Pending {
    pub(crate) fn verb(&self) -> crate::forms::closing_prompt::Verb {
        use crate::forms::closing_prompt::Verb;
        match self {
            Pending::Save(_) | Pending::ApplyDeposit(_) => Verb::Change,
            Pending::Void(_) => Verb::Void,
        }
    }
}

/// A saved document as the engine answered it (P22: the three kinds).
// One per open window, read once per save or load: boxing the invoice buys
// nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub(crate) enum SavedDoc {
    Invoice(InvoiceRecord),
    CreditMemo(CreditMemoRecord),
    SalesReceipt(SalesReceiptRecord),
}

impl SavedDoc {
    pub(crate) fn id(&self) -> TransactionId {
        match self {
            SavedDoc::Invoice(r) => r.txn.id,
            SavedDoc::CreditMemo(r) => r.id,
            SavedDoc::SalesReceipt(r) => r.id,
        }
    }

    pub(crate) fn date(&self) -> CalendarDate {
        match self {
            SavedDoc::Invoice(r) => r.txn.txn_date,
            SavedDoc::CreditMemo(r) => r.date,
            SavedDoc::SalesReceipt(r) => r.date,
        }
    }
}

/// A saved invoice, credit memo or sales receipt open in the form.
#[derive(Clone, Debug)]
pub(crate) struct Editing {
    pub id: TransactionId,
    pub doc: SavedDoc,
    pub old_date: CalendarDate,
}

impl Editing {
    pub(crate) fn of(doc: SavedDoc) -> Editing {
        Editing {
            id: doc.id(),
            old_date: doc.date(),
            doc,
        }
    }

    /// The invoice (invoice-only features read this and go inert for a
    /// credit memo or sales receipt).
    pub(crate) fn invoice(&self) -> Option<&InvoiceRecord> {
        match &self.doc {
            SavedDoc::Invoice(r) => Some(r),
            _ => None,
        }
    }

    /// The credit memo.
    pub(crate) fn memo(&self) -> Option<&CreditMemoRecord> {
        match &self.doc {
            SavedDoc::CreditMemo(r) => Some(r),
            _ => None,
        }
    }

    /// The sales receipt.
    pub(crate) fn receipt(&self) -> Option<&SalesReceiptRecord> {
        match &self.doc {
            SavedDoc::SalesReceipt(r) => Some(r),
            _ => None,
        }
    }

    pub(crate) fn void(&self) -> bool {
        match &self.doc {
            SavedDoc::Invoice(r) => r.is_void(),
            SavedDoc::CreditMemo(r) => r.is_void(),
            SavedDoc::SalesReceipt(r) => r.status == cl_api::SalesDocStatus::Void,
        }
    }

    /// The document number.
    pub(crate) fn number(&self) -> Option<String> {
        match &self.doc {
            SavedDoc::Invoice(r) => r.txn.txn_number.clone(),
            SavedDoc::CreditMemo(r) => r.number.clone(),
            SavedDoc::SalesReceipt(r) => r.number.clone(),
        }
    }

    /// The record's stamp (an edit sends it back; 409 STALE when it moved).
    pub(crate) fn updated_at(&self) -> Option<Timestamp> {
        match &self.doc {
            SavedDoc::Invoice(r) => r.txn.updated_at,
            SavedDoc::CreditMemo(r) => Some(r.updated_at),
            SavedDoc::SalesReceipt(r) => Some(r.updated_at),
        }
    }

    /// The client.
    pub(crate) fn client(&self) -> Option<ContactId> {
        match &self.doc {
            SavedDoc::Invoice(r) => r.txn.contact_id,
            SavedDoc::CreditMemo(r) => Some(r.customer_id),
            SavedDoc::SalesReceipt(r) => r.customer_id,
        }
    }
}

/// A preview's answer: the shared figures plus a credit memo's apply
/// figures and a document's preview problem codes (P22).
pub(crate) struct Answer {
    pub figures: InvoicePreview,
    pub credit: Option<CreditMemoFigures>,
    pub problems: Vec<String>,
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
    pub(crate) preview_job: Option<(PreviewTicket, InvoiceRequest, Job<Answer>)>,
    /// Invoice, credit memo or sales receipt (P22; fixed per window).
    pub(crate) kind: SalesDocKind,
    /// A credit memo's apply-or-keep state and apply figures (Q-P22b).
    pub(crate) credit: CreditMemoForm,
    /// The apply panel is open over the form, for this save.
    pub(crate) apply_open: Option<SaveIntent>,
    /// Amounts being typed in the apply panel, by invoice.
    pub(crate) apply_cells: BTreeMap<TransactionId, String>,
    /// The latest preview's problem codes (a sales receipt's
    /// `NEGATIVE_RECEIPT_NEEDS_BANK`).
    pub(crate) doc_problems: Vec<String>,
    /// The number the next invoice gets from the company's counter
    /// (`document-numbers-v1`, Q-P20ad): shown while Invoice # is blank;
    /// never sent (a typed number skips the counter).
    pub(crate) next_number: Option<String>,
    next_job: Option<Job<String>>,
    /// The number typed is in use on another invoice (Q-94): saved with a
    /// warning.
    pub(crate) duplicate_number: bool,
    /// Layouts and item names for the PDF (U-130).
    pub(crate) print_books: email::PrintBooks,
    print_books_job: Option<Job<email::PrintBooks>>,
    /// Print is waiting for its print-later save to open Print Forms.
    pub(crate) print_after_save: bool,
    /// A PDF being made on the queue (Preview, Save PDF), opened when done.
    pub(crate) pdf_job: Option<Job<(String, Vec<u8>)>>,
    pub(crate) preview_problem: Option<&'static str>,
    pub(crate) clock: Instant,
    pub(crate) editing: Option<Editing>,
    /// The save in flight: the invoice and whether its typed number is in
    /// use elsewhere (Q-94).
    pub(crate) save_job: Option<Job<(SavedDoc, bool)>>,
    pub(crate) intent: Option<SaveIntent>,
    pub(crate) key: Option<(String, IdempotencyKey)>,
    pub(crate) void_job: Option<(Pending, Job<SavedDoc>)>,
    pub(crate) load_job: Option<Job<SavedDoc>>,
    pub(crate) nav_job: Option<Job<Vec<TransactionId>>>,
    pub(crate) nav: Vec<TransactionId>,
    pub(crate) nav_at: Option<usize>,
    pub(crate) listed: bool,
    pub(crate) want_older: Option<bool>,
    pub(crate) sales_job: Option<(ContactId, Job<ClientSales>)>,
    pub(crate) sales: Option<(ContactId, ClientSales)>,
    pub(crate) billable: billable::Billable,
    /// Files on the saved invoice (Q-P20v).
    pub(crate) attach: attach::InvoiceAttach,
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
    /// Set Up of an item typed on a line (Q-P20i): the line and the name.
    pub(crate) setting_up_item: Option<(cl_grid::RowKey, String)>,
    pub(crate) saved_count: u32,
    pub(crate) last_action: Option<&'static str>,
    pub(crate) last_saved: Option<TransactionId>,
    pub(crate) focused: Option<u64>,
    pub(crate) after_frame: Option<CommandId>,
    /// A dialog over the form was open last frame.
    pub(crate) dialog_was_open: bool,
    /// The dialog's first field takes the keyboard (once, on open).
    pub(crate) dialog_focus: bool,
    /// The widget that had the keyboard before the dialog opened.
    pub(crate) return_focus: Option<eframe::egui::Id>,
    pub(crate) layout: crate::layout::FrameReport,
    pub(crate) money_style: MoneyStyle,
    pub(crate) bus: crate::data_bus::Subscription,
    pub(crate) lists_stale: bool,
    pub(crate) lost: crate::forms::lost_save::LostSave,
    /// Deposits on hand for the client (P21).
    pub(crate) deposit_card: deposit_card::DepositCard,
}

impl InvoiceScreen {
    /// A new invoice in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> InvoiceScreen {
        InvoiceScreen::new_kind(link, tenant, today, SalesDocKind::Invoice)
    }

    /// A new document of `kind` (P22: credit memo, sales receipt).
    #[must_use]
    pub fn new_kind(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
        kind: SalesDocKind,
    ) -> InvoiceScreen {
        let group = link.group().and_then(Result::ok);
        if kind != SalesDocKind::Invoice
            && let Some(mut d) = link.demo_state()
        {
            // P21's story first (its ids and figures stay), then P22's.
            crate::company::demo::receivables::seed(&mut d);
            crate::company::demo::credit_memos::seed(&mut d);
        }
        let mut s = InvoiceScreen {
            bus: link.bus().subscribe(),
            link,
            tenant,
            group,
            today,
            kind,
            credit: CreditMemoForm::default(),
            apply_open: None,
            apply_cells: BTreeMap::new(),
            doc_problems: Vec::new(),
            books: Books::default(),
            items: Vec::new(),
            items_job: None,
            clients: Arc::new(RegisterLists::default()),
            header: FormState::new(blank(kind, today)),
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
            pdf_job: None,
            print_after_save: false,
            print_books: email::PrintBooks::default(),
            print_books_job: None,
            next_number: None,
            next_job: None,
            duplicate_number: false,
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
            attach: attach::InvoiceAttach::default(),
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
            setting_up_item: None,
            saved_count: 0,
            last_action: None,
            last_saved: None,
            focused: None,
            after_frame: None,
            dialog_was_open: false,
            dialog_focus: false,
            return_focus: None,
            layout: crate::layout::FrameReport::default(),
            money_style: MoneyStyle::default(),
            lists_stale: false,
            lost: crate::forms::lost_save::LostSave::default(),
            deposit_card: deposit_card::DepositCard::default(),
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
        match self.kind {
            SalesDocKind::Invoice => InvoiceCaps {
                fields: self.has(Capabilities::INVOICE_FIELDS),
                pricing: self.has(Capabilities::INVOICE_PRICING),
                billable: self.has(Capabilities::BILLABLE_COSTS),
                closing: self.has(Capabilities::INVOICE_EDIT),
            },
            // P22 (U-138, U-143): the documents' own capabilities.
            SalesDocKind::CreditMemo => InvoiceCaps {
                fields: self.has(Capabilities::CREDIT_MEMO_EDIT),
                pricing: self.has(Capabilities::CREDIT_MEMO_EDIT),
                billable: false,
                closing: self.has(Capabilities::CREDIT_MEMO_EDIT),
            },
            SalesDocKind::SalesReceipt => InvoiceCaps {
                fields: self.has(Capabilities::SALES_RECEIPT_LINES),
                pricing: self.has(Capabilities::SALES_RECEIPT_LINES),
                billable: false,
                closing: self.has(Capabilities::SALES_RECEIPT_LINES),
            },
        }
    }

    /// The engine takes the closing-date password on this document.
    pub(crate) fn closing_supported(&self) -> bool {
        self.caps().closing
    }

    /// The window's command id.
    pub(crate) fn command_kind(&self) -> &'static str {
        kind_command(self.kind)
    }

    /// The control channel's and layout's screen name.
    pub(crate) fn screen_name(&self) -> &'static str {
        match self.kind {
            SalesDocKind::Invoice => "create_invoice",
            SalesDocKind::CreditMemo => "credit_memo",
            SalesDocKind::SalesReceipt => "sales_receipt",
        }
    }

    /// The engine's transaction type of the document (attachments, Prev/
    /// Next).
    pub(crate) fn txn_type(&self) -> cl_api::TxnType {
        match self.kind {
            SalesDocKind::Invoice => cl_api::TxnType::Invoice,
            SalesDocKind::CreditMemo => cl_api::TxnType::CreditMemo,
            SalesDocKind::SalesReceipt => cl_api::TxnType::CashSale,
        }
    }

    /// The i18n key prefix of the kind's own texts.
    pub(crate) fn text(&self, key: &str) -> String {
        let prefix = match self.kind {
            SalesDocKind::Invoice => "invoice",
            SalesDocKind::CreditMemo => "credit_memo",
            SalesDocKind::SalesReceipt => "sales_receipt",
        };
        t(&format!("{prefix}.{key}"))
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
        let e = self.editing.as_ref()?;
        if e.void() {
            Some("invoice.read_only.void")
        } else if e
            .invoice()
            .is_some_and(|r| r.invoice_status == Some(cl_api::InvoiceStatus::Paid))
        {
            // The engine refuses an edit of a paid invoice (400); say so up
            // front (N6).
            Some("invoice.paid.read_only")
        } else {
            None
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.save_job.is_some() || self.void_job.is_some()
    }

    /// Add Billable Costs or Email is open over the form (modal: the form
    /// behind takes no input).
    pub(crate) fn dialog_open(&self) -> bool {
        self.billable.is_open() || self.email.is_open() || self.apply_open.is_some()
    }

    pub(crate) fn idle(&self) -> bool {
        !self.busy() && !self.closing_prompt.dialog_open()
    }

    /// Reads the accounts, names, tags, closing date and items.
    pub(crate) fn start_lists(&mut self) {
        self.books.start(&self.link, self.group.as_ref());
        self.fetch_own_address();
        self.load_email_prefs();
        self.print_books_job = Some(email::print_books_job(&self.link, self.group.as_ref()));
        if let Some(Ok(b)) = engine::poll(&mut self.print_books_job) {
            self.print_books = b;
        }
        if self.editing.is_none() {
            self.fetch_next_number();
        }
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
            // A subtotal is never taxed.
            .map(|i| {
                (
                    i.id,
                    i.fields.is_taxable && i.fields.item_type != cl_api::ItemType::Subtotal,
                )
            })
            .collect();
        let subtotals = self
            .items
            .iter()
            .filter(|i| i.fields.item_type == cl_api::ItemType::Subtotal)
            .map(|i| i.id)
            .collect();
        self.grid.set_lists(
            Arc::new(ItemLists::new(Arc::clone(&self.books.lists), &active)),
            taxable,
            subtotals,
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
        self.item_set_up_back();
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
                Ok((v, duplicate)) => {
                    self.saved(v);
                    self.duplicate_number = duplicate;
                    if duplicate {
                        self.link.push(crate::company::CompanyEvent::Toast(
                            "invoice.number.duplicate",
                        ));
                    }
                }
                Err(e) => self.save_failed(e),
            }
        }
        if let Some(ev) = self.lost.poll(&mut self.header) {
            self.lost_event(ev);
        }
        if let Some((pending, mut job)) = self.void_job.take() {
            match job.take() {
                None => self.void_job = Some((pending, job)),
                Some(Ok(v)) => self.voided(v),
                Some(Err(e)) => self.void_failed(&e, pending),
            }
        }
        if let Some(r) = engine::poll(&mut self.load_job) {
            match r {
                Ok(v) => self.show(v),
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
        self.poll_attach();
        if let Some(Ok(b)) = engine::poll(&mut self.print_books_job) {
            self.print_books = b;
        }
        if let Some(r) = engine::poll(&mut self.next_job) {
            self.next_number = r.ok();
        }
        self.poll_pdf();
        self.email.poll();
        if let Some(done) = self.email.take_done() {
            self.email_done(done);
        }
        self.poll_preview();
    }

    /// Sends a preview when the form has stood still 250 ms; applies the
    /// reply (a stale one is dropped by the tracker).
    pub(crate) fn poll_preview(&mut self) {
        if let Some((ticket, req, mut job)) = self.preview_job.take() {
            match job.take() {
                None => self.preview_job = Some((ticket, req, job)),
                Some(Ok(answer)) => {
                    self.preview_problem = None;
                    let p = answer.figures;
                    if let Some(c) = answer.credit {
                        let caps = self.caps();
                        self.credit.doc = self.compose();
                        self.credit.apply_preview(&req, c, caps);
                    }
                    let figures = engine::figures_of(&p);
                    let held = !figures.billed_on.is_empty();
                    let applied = self.tracker.apply_preview(ticket, figures);
                    if applied {
                        self.doc_problems = answer.problems;
                    }
                    if applied && held {
                        // Another invoice bills a cost on this one (U-128):
                        // the list is read again; the lines say which.
                        if let Some(c) = self.header.current().client {
                            let link = std::sync::Arc::clone(&self.link);
                            self.billable.fetch(&link, self.group.as_ref(), c);
                        }
                    }
                }
                Some(Err(e)) => {
                    self.preview_problem = Some(match e {
                        ApiError::Forbidden { .. } => "invoice.preview.not_allowed",
                        _ => "invoice.preview.failed",
                    });
                    if self.kind == SalesDocKind::CreditMemo {
                        let caps = self.caps();
                        self.credit.doc = self.compose();
                        self.credit.fail_preview(&req, caps);
                    }
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
            match self.preview_job_for(&req) {
                Ok(job) => self.preview_job = Some((ticket, req, job)),
                Err(_) => self.tracker.fail(ticket),
            }
            // The demo answers at once.
            if self.link.is_demo() {
                self.poll_preview();
            }
        }
        self.grid.show(&self.tracker);
    }

    /// The preview call of the document's kind (the demo answers at once).
    fn preview_job_for(&self, req: &InvoiceRequest) -> Result<Job<Answer>, ApiError> {
        let editing = self.editing.as_ref().map(|e| e.id);
        let plain = |figures| Answer {
            figures,
            credit: None,
            problems: Vec::new(),
        };
        Ok(match self.kind {
            SalesDocKind::Invoice => {
                let new = engine::new_invoice(req)?;
                let live = new.clone();
                engine::read(
                    &self.link,
                    self.group.as_ref(),
                    |d| crate::company::demo::invoices::preview(d, &new, editing).map(plain),
                    move |s| async move {
                        s.invoices()
                            .preview(&live, editing)
                            .await
                            .map(|figures| Answer {
                                figures,
                                credit: None,
                                problems: Vec::new(),
                            })
                    },
                )
            }
            SalesDocKind::CreditMemo => {
                let new = engine::new_credit_memo(req)?;
                let live = new.clone();
                let answer = |p: cl_api::CreditMemoPreview| Answer {
                    credit: Some(engine::credit_figures_of(&p)),
                    problems: p.problems.iter().map(|x| x.code.clone()).collect(),
                    figures: p.figures,
                };
                engine::read(
                    &self.link,
                    self.group.as_ref(),
                    |d| {
                        crate::company::demo::credit_memos::preview_memo(d, &new, editing)
                            .map(answer)
                    },
                    move |s| async move { s.credit_memos().preview(&live, editing).await.map(answer) },
                )
            }
            SalesDocKind::SalesReceipt => {
                let new = engine::new_sales_receipt(req)?;
                let live = new.clone();
                let answer = |p: cl_api::SalesReceiptPreview| Answer {
                    problems: p.problems.iter().map(|x| x.code.clone()).collect(),
                    credit: None,
                    figures: p.figures,
                };
                engine::read(
                    &self.link,
                    self.group.as_ref(),
                    |d| crate::company::demo::credit_memos::preview_receipt(d, &new).map(answer),
                    move |s| async move { s.sales_receipts().preview(&live, editing).await.map(answer) },
                )
            }
        })
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
            // Another client's invoices: ask Apply or Keep again (Q-P22b).
            self.credit.reset_answer();
            self.apply_cells.clear();
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
        self.header.load(blank(self.kind, date));
        self.grid.load(Vec::new());
        self.editing = None;
        self.credit = CreditMemoForm::default();
        self.apply_open = None;
        self.apply_cells.clear();
        self.doc_problems.clear();
        self.nav_at = None;
        self.focused = None;
        self.void_open = false;
        self.offer_send = false;
        self.problems.clear();
        self.stale = false;
        self.sales = None;
        self.billable.forget();
        self.unknown_client = None;
        self.duplicate_number = false;
        self.attach = attach::InvoiceAttach::default();
        self.sync_inputs();
        self.fetch_next_number();
    }

    /// Reads the next invoice number (nothing is taken).
    pub(crate) fn fetch_next_number(&mut self) {
        if !self.link.is_demo() && !self.has(Capabilities::DOCUMENT_NUMBERS) {
            return;
        }
        let doc = match self.kind {
            SalesDocKind::Invoice => cl_api::DocType::Invoice,
            SalesDocKind::CreditMemo => cl_api::DocType::CreditMemo,
            SalesDocKind::SalesReceipt => cl_api::DocType::SalesReceipt,
        };
        let kind = self.kind;
        self.next_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| match kind {
                SalesDocKind::Invoice => Ok(crate::company::demo::invoices::next_number(d)),
                // The demo numbers memos and receipts as it saves them.
                _ => Err(ApiError::NotFound { code: None }),
            },
            move |s| async move { Ok(s.printing().next_number(doc).await?.next) },
        ));
        if let Some(Ok(n)) = engine::poll(&mut self.next_job) {
            self.next_number = Some(n);
        }
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
                    "caption": self.grid.caption(l),
                    "taxable": grid::taxable_of(l, self.grid.taxable()),
                    "amount": amount,
                    "rate_source": source,
                })
            })
            .collect();
        let sent = self
            .editing
            .as_ref()
            .and_then(Editing::invoice)
            .map(|r| r.send_history.len());
        let mut v = json!({
            "screen": self.screen_name(),
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
            "line_count": lines.len(),
            "lines": lines,
            "totals": totals,
            "subtotal": subtotal,
            "tax": tax,
            "total": total,
            "balance_after": after,
            "preview_problem": self.preview_problem,
            "open_balance": self.sales.as_ref().and_then(|(_, s)| s.open_balance).map(|m| m.format(style)),
            "unbilled": if self.read_only().is_some() { serde_json::json!(self.billable.costs.len()) } else { self.billable.unbilled_json(&f) },
            "billable_dialog": self.billable.dialog_json(),
            "email": self.email.state_json(),
            "email_after_save": self.email_after_save,
            "sent_events": sent,
            "offer_send": self.offer_send,
            "invoice_status": self.editing.as_ref().and_then(Editing::invoice).and_then(|r| r.invoice_status).map(|s| format!("{s:?}").to_lowercase()),
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
        v["next_number"] = json!(self.next_number);
        v["deposits_on_hand"] = json!(self.deposit_card.count());
        v["deposits_applied"] = json!(self.deposit_card.applied);
        v["deposit_note"] = json!(self.deposit_card.note());
        v["attachments"] = self.attach.state_json();
        v["duplicate_number"] = json!(self.duplicate_number);
        if self.kind != SalesDocKind::Invoice {
            v["kind"] = json!(self.screen_name());
            v["preview_problems"] = json!(self.doc_problems);
            v["void"] = json!(self.editing.as_ref().is_some_and(Editing::void));
        }
        if self.kind == SalesDocKind::CreditMemo {
            v["credit"] = self.credit_state_json();
        }
        if self.kind == SalesDocKind::SalesReceipt {
            v["deposit_to"] = json!(f.deposit_to.map(|a| a.to_string()));
            v["method"] = json!(f.payment_method);
            v["deposited"] = json!(
                self.editing
                    .as_ref()
                    .and_then(Editing::receipt)
                    .is_some_and(|r| r.deposit_id.is_some())
            );
        }
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
        let waiting = self.setting_up.is_some() || self.setting_up_item.is_some();
        if self.lists_stale && (waiting || !self.is_dirty()) && !self.books.busy() {
            self.lists_stale = false;
            self.start_lists();
        }
        if changes.ledger_any() {
            self.listed = false;
            if let Some(c) = self.header.current().client {
                self.client_changed_quiet(c);
            }
            // Printed in Print Forms, emailed from the Outbox, marked sent
            // elsewhere: the open invoice is read again so its send history
            // and `updatedAt` are current (the next edit isn't STALE).
            if !self.is_dirty() && !self.busy() && self.load_job.is_none() {
                self.reload();
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

/// A blank document of `kind` dated `date` (no lines: the grid holds them).
fn blank(kind: SalesDocKind, date: CalendarDate) -> InvoiceForm {
    InvoiceForm {
        kind,
        date: Some(date),
        // A sales receipt is paid by check unless the owner says otherwise.
        payment_method: (kind == SalesDocKind::SalesReceipt).then(|| "check".to_string()),
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
        let key = match self.kind {
            SalesDocKind::Invoice => "invoice.title.number",
            SalesDocKind::CreditMemo => "credit_memo.title.number",
            SalesDocKind::SalesReceipt => "sales_receipt.title.number",
        };
        match self.editing.as_ref().and_then(Editing::number) {
            Some(n) => crate::i18n::t_with(key, &[("number", &n)]),
            None => t(&format!("cmd.{}", self.command_kind())),
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
        // A dialog over the form has the keys: Esc closes it, never the
        // grid line behind it.
        if !self.dialog_open()
            && let Some(h) = self.grid.command(id)
        {
            return h;
        }
        self.form_command(id)
    }

    fn open_with(&mut self, payload: crate::windows::OpenPayload) -> bool {
        match payload {
            // P22: Client Center's "New credit memo" for the selected client.
            crate::windows::OpenPayload::ForClient(c) if self.editing.is_none() => {
                self.books.poll();
                self.apply_lists();
                self.pick_client(c);
                true
            }
            _ => false,
        }
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
#[path = "credit_tests.rs"]
mod credit_tests;
#[cfg(test)]
mod tests;
