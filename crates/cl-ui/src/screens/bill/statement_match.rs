//! Match Vendor Statement (P17, `nav.statement_match`, plan §3 WP6, D4,
//! Q-168, Q-P17q): a vendor statement typed or pasted against the books.
//!
//! The owner picks the vendor and the statement period and types or pastes
//! the statement's lines (date, ref. no., amount, optional type; read by
//! `cl_model::statement_match::parse_paste`). **Match** reads the vendor's
//! posted bills, vendor credits and bill payments for the period (plus the
//! 7-day date slack on each side) through the `cl-api` lists, and
//! `cl_model::statement_match::match_lines` pairs them. The results come
//! in four lists:
//!
//! - **Matched**: the line and the books entry it paired with, and how.
//! - **On the statement, not in your books**: each line has **Create bill**
//!   / **Create credit**, which opens Enter Bill as an ordinary unsaved form
//!   filled with the vendor, the line's date, ref. no. and amount (the
//!   amount the owner typed or pasted from the statement; WP0's
//!   `CompanyEvent::OpenWith(vendors.enter_bill, OpenPayload::BillPrefill)`).
//! - **In your books, not on the statement**: books entries dated inside
//!   the period that no line can pair with; a bill or credit opens its
//!   record.
//! - **Needs your choice**: lines with more than one possible books entry,
//!   listed with their candidates. Nothing is picked for the owner.
//!
//! Every amount shown is either the owner's line or the engine's total as
//! sent. Nothing is added up: there is no running balance and no balance
//! comparison (that waits for U-14). On the pinned engine vendor credits
//! carry no ref. no. (review S6, `ap-edit-v1`), so credits pair by amount and
//! date only, and the screen says so.
//!
//! The Enter Bill toolbar overflow opens this window with
//! `CompanyEvent::OpenWindow(nav.statement_match)` (WP4's toolbar).

use std::sync::Arc;

use cl_api::{
    ApiError, BillListQuery, BillPaymentSummary, BillSummary, Capabilities, Contact, ContactFilter,
    ContactType, CreditListQuery, CreditSummary, PageRequest, RequestGroup, TxnStatus,
};
use cl_core::ids::{ContactId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle};
use cl_model::CommandId;
use cl_model::date_entry::DateOrder;
use cl_model::forms::bill::BillKind;
use cl_model::statement_match::{
    self as sm, BooksItem, DEFAULT_DATE_SLACK_DAYS, MatchPass, MatchResult, ParsedPaste, PastedRow,
    StatementKind,
};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::BillPrefill;
use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::data_bus::Subscription;
use crate::i18n::{t, t_with};
use crate::screens::company::form::{self, DateInput};
use crate::theme::Tokens;
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "nav.statement_match";

/// Bills and credits are read in pages of this many rows...
const PAGE: u16 = 50;
/// ...up to this many pages each (1,000 rows; more is "couldn't read
/// everything").
const MAX_PAGES: u32 = 20;
/// Bill payments: rows per page and the most pages read.
const PAYMENT_PAGE: u16 = 100;
const PAYMENT_PAGES: u32 = 10;

mod read;

/// The statement match window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(StatementMatchScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// The vendor's books for the read window, as the engine sent them.
#[derive(Default)]
struct Books {
    bills: Vec<BillSummary>,
    credits: Vec<CreditSummary>,
    payments: Vec<BillPaymentSummary>,
    /// False when the page cap left rows unread.
    complete: bool,
}

/// What one Match was run with.
#[derive(Clone)]
struct Request {
    vendor: ContactId,
    from: CalendarDate,
    to: CalendarDate,
    rows: Vec<PastedRow>,
}

/// One Match's answer.
struct Outcome {
    request: Request,
    books: Vec<BooksItem>,
    result: MatchResult,
    complete: bool,
}

impl Outcome {
    fn item(&self, id: TransactionId) -> Option<&BooksItem> {
        self.books.iter().find(|b| b.id == id)
    }
}

/// The posted books entries with a total above zero, as match items.
/// Without `credit_refs` (the pinned engine, review S6) a credit's ref is
/// left blank, so it pairs by amount and date only.
fn books_items(b: &Books, credit_refs: bool) -> Vec<BooksItem> {
    let positive = |m: Option<Money>| m.filter(|m| m.is_positive());
    let mut out = Vec::new();
    for r in b.bills.iter().filter(|r| r.status == TxnStatus::Posted) {
        if let Some(amount) = positive(r.total) {
            out.push(BooksItem {
                id: r.id,
                kind: StatementKind::Charge,
                date: r.txn_date,
                reference: r.vendor_invoice_number.clone().unwrap_or_default(),
                amount,
            });
        }
    }
    for r in b.credits.iter().filter(|r| r.status == TxnStatus::Posted) {
        if let Some(amount) = positive(r.total) {
            out.push(BooksItem {
                id: r.id,
                kind: StatementKind::Credit,
                date: r.txn_date,
                reference: if credit_refs {
                    r.vendor_invoice_number.clone().unwrap_or_default()
                } else {
                    String::new()
                },
                amount,
            });
        }
    }
    for r in b.payments.iter().filter(|r| r.status == TxnStatus::Posted) {
        if let Some(amount) = positive(r.total) {
            let reference = r
                .reference_number
                .clone()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| r.check_number.map(|n| n.to_string()))
                .unwrap_or_default();
            out.push(BooksItem {
                id: r.id,
                kind: StatementKind::Payment,
                date: r.txn_date,
                reference,
                amount,
            });
        }
    }
    out
}

/// Matches `request`'s lines against `books`. Books-only entries dated
/// outside the period (read only for the date slack) are left out.
fn outcome(request: Request, books: &Books, credit_refs: bool) -> Outcome {
    let items = books_items(books, credit_refs);
    let lines: Vec<_> = request.rows.iter().map(|r| r.entry.clone()).collect();
    let mut result = sm::match_lines(&lines, &items, DEFAULT_DATE_SLACK_DAYS);
    result.missing_on_statement.retain(|id| {
        items
            .iter()
            .find(|b| b.id == *id)
            .is_some_and(|b| b.date >= request.from && b.date <= request.to)
    });
    Outcome {
        request,
        books: items,
        result,
        complete: books.complete,
    }
}

/// The read window: the period widened by the date slack.
fn window(from: CalendarDate, to: CalendarDate) -> (CalendarDate, CalendarDate) {
    (
        from.add_days(-DEFAULT_DATE_SLACK_DAYS).unwrap_or(from),
        to.add_days(DEFAULT_DATE_SLACK_DAYS).unwrap_or(to),
    )
}

/// Reads the vendor's posted-and-void bills, credits and bill payments
/// dated `start..=end` (the caller keeps the posted ones).
async fn read_books(
    s: cl_api::Session,
    vendor: ContactId,
    start: CalendarDate,
    end: CalendarDate,
) -> Result<Books, ApiError> {
    let bq = BillListQuery {
        vendor: Some(vendor),
        start: Some(start),
        end: Some(end),
        ..BillListQuery::default()
    };
    let bills = s.bills().list_capped(&bq, PAGE, MAX_PAGES).await?;
    let cq = CreditListQuery {
        vendor: Some(vendor),
        start: Some(start),
        end: Some(end),
        ..CreditListQuery::default()
    };
    let credits = s.vendor_credits().list_capped(&cq, PAGE, MAX_PAGES).await?;
    let mut payments = Vec::new();
    let mut payments_complete = false;
    for page in 0..PAYMENT_PAGES {
        let req = PageRequest::new(PAYMENT_PAGE, u64::from(page) * u64::from(PAYMENT_PAGE))?;
        let rows = s
            .bill_payments()
            .list(Some(vendor), Some(start), Some(end), req)
            .await?;
        let last = rows.len() < usize::from(PAYMENT_PAGE);
        payments.extend(rows);
        if last {
            payments_complete = true;
            break;
        }
    }
    Ok(Books {
        bills: bills.rows,
        credits: credits.rows,
        payments,
        complete: bills.complete && credits.complete && payments_complete,
    })
}

/// The demo books: the synthetic October fixture
/// (`fixtures/statement-match/books.json`) in the engine's shape, for any
/// vendor, dated `start..=end`.
fn demo_books(start: CalendarDate, end: CalendarDate) -> Books {
    const FIXTURE: &str = include_str!("../../../../../fixtures/statement-match/books.json");
    let mut books = Books {
        complete: true,
        ..Books::default()
    };
    let Ok(v) = serde_json::from_str::<Value>(FIXTURE) else {
        return books;
    };
    for i in v["items"].as_array().into_iter().flatten() {
        let date = i["date"]
            .as_str()
            .and_then(|d| CalendarDate::parse_wire(d).ok());
        if date.is_none_or(|d| d < start || d > end) {
            continue;
        }
        let row = |extra: Value| {
            let mut r = json!({
                "id": i["id"],
                "txnDate": i["date"],
                "total": i["amount"],
                "status": "posted",
            });
            if let (Some(r), Some(e)) = (r.as_object_mut(), extra.as_object()) {
                r.extend(e.clone());
            }
            r
        };
        match i["kind"].as_str() {
            Some("bill") => {
                if let Ok(b) = serde_json::from_value(row(json!({"vendorInvoiceNumber": i["ref"]})))
                {
                    books.bills.push(b);
                }
            }
            Some("credit") => {
                if let Ok(c) = serde_json::from_value(row(json!({"vendorInvoiceNumber": i["ref"]})))
                {
                    books.credits.push(c);
                }
            }
            Some("payment") => {
                let check: Option<u64> = i["ref"].as_str().and_then(|r| r.parse().ok());
                if let Ok(p) = serde_json::from_value(row(json!({"checkNumber": check}))) {
                    books.payments.push(p);
                }
            }
            _ => {}
        }
    }
    books
}

/// The demo engine's vendor statement report (the stand-in for
/// `/reports/vendor-statement`): the fixture's bills less its credits and
/// payments, before `start` (opening) and through `end` (closing).
#[must_use]
pub(crate) fn demo_balances(
    start: CalendarDate,
    end: CalendarDate,
) -> Option<cl_api::VendorStatementBalances> {
    let all = demo_books(CalendarDate::from_ymd(1900, 1, 1).ok()?, end);
    let mut opening = Money::ZERO;
    let mut closing = Money::ZERO;
    for b in books_items(&all, true) {
        let signed = match b.kind {
            StatementKind::Charge => b.amount,
            _ => b.amount.checked_neg().ok()?,
        };
        closing = closing.checked_add(signed).ok()?;
        if b.date < start {
            opening = opening.checked_add(signed).ok()?;
        }
    }
    Some(cl_api::VendorStatementBalances { opening, closing })
}

/// The statement match window.
pub struct StatementMatchScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    sub: Subscription,
    money_style: MoneyStyle,
    /// Active vendors, by name.
    vendors: Vec<(ContactId, String)>,
    vendors_task: Option<Task<Vec<Contact>>>,
    vendors_failed: bool,
    vendor: Option<ContactId>,
    from: DateInput,
    to: DateInput,
    /// The statement lines as typed or pasted.
    text: String,
    /// The last reading of `text` (on Match).
    parsed: Option<ParsedPaste>,
    /// Why Match did not run (i18n key).
    problem: Option<&'static str>,
    /// The books being read for this request.
    reading: Option<(Request, Task<Books>)>,
    books_failed: Option<Request>,
    outcome: Option<Outcome>,
    /// "Create bill" / "Create credit" hand-offs sent (control channel).
    handoffs: u32,
    last_handoff: Option<BillKind>,
    /// Records opened from "In your books" (control channel).
    opened: u32,
    /// The active vendors as the engine sent them (the reader matches the
    /// statement's vendor against them).
    contacts: Vec<Contact>,
    /// A06: the statement document and its reading.
    doc: read::StatementDoc,
}

impl StatementMatchScreen {
    /// An empty statement for last month (the usual statement period).
    #[must_use]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "every screen's constructor takes the open company; this one doesn't need it"
    )]
    pub fn new(
        link: Arc<CompanyLink>,
        open: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> StatementMatchScreen {
        let start = today.add_months(-1).unwrap_or(today).start_of_month();
        let group = link.group().and_then(Result::ok);
        let sub = link.bus().subscribe();
        let mut s = StatementMatchScreen {
            link,
            group,
            today,
            sub,
            money_style: MoneyStyle::default(),
            vendors: Vec::new(),
            vendors_task: None,
            vendors_failed: false,
            vendor: None,
            from: DateInput::new(start),
            to: DateInput::new(start.end_of_month()),
            text: String::new(),
            parsed: None,
            problem: None,
            reading: None,
            books_failed: None,
            outcome: None,
            handoffs: 0,
            last_handoff: None,
            opened: 0,
            contacts: Vec::new(),
            doc: read::StatementDoc::default(),
        };
        s.doc.tenant = open.map(|(t, _)| t);
        s.load_vendors();
        s.load_statement_layouts();
        s
    }

    fn load_vendors(&mut self) {
        self.vendors_failed = false;
        if let Some(mut demo) = self.link.demo_state() {
            let contacts = demo.contacts_mut().clone();
            drop(demo);
            self.vendors_task = Some(Task::ready(contacts));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.vendors_task = group
            .read(&session, |s| async move {
                s.contacts().list_all(&ContactFilter::default()).await
            })
            .ok()
            .map(Task::Pending);
        if self.vendors_task.is_none() {
            self.vendors_failed = true;
        }
    }

    /// Start over: no vendor, last month, no lines, no answer.
    pub(crate) fn clear(&mut self) {
        let start = self
            .today
            .add_months(-1)
            .unwrap_or(self.today)
            .start_of_month();
        self.vendor = None;
        self.from = DateInput::new(start);
        self.to = DateInput::new(start.end_of_month());
        self.text.clear();
        self.parsed = None;
        self.problem = None;
        self.reading = None;
        self.outcome = None;
        self.books_failed = None;
        self.handoffs = 0;
        self.last_handoff = None;
        self.opened = 0;
        self.doc.reset();
    }

    fn credit_refs(&self) -> bool {
        self.link.capabilities().has(Capabilities::AP_EDIT)
    }

    /// Reads the lines and the books; the problem key when it can't.
    pub(crate) fn run_match(&mut self) {
        self.problem = None;
        self.from.reparse(self.today);
        self.to.reparse(self.today);
        let parsed = sm::parse_paste(&self.text, DateOrder::Mdy, self.today);
        let bad = !parsed.bad.is_empty();
        let rows = parsed.rows.clone();
        self.parsed = Some(parsed);
        let Some(vendor) = self.vendor else {
            self.problem = Some("statement.problem.vendor");
            return;
        };
        let (Some(from), Some(to)) = (self.from.value, self.to.value) else {
            self.problem = Some("statement.problem.dates");
            return;
        };
        if from > to {
            self.problem = Some("statement.problem.dates");
            return;
        }
        if bad {
            self.problem = Some("statement.problem.fix_lines");
            return;
        }
        if rows.is_empty() {
            self.problem = Some("statement.problem.no_lines");
            return;
        }
        if self.doc.awaiting_accept(&self.text) {
            self.problem = Some("statement.problem.accept_first");
            return;
        }
        self.start_read(Request {
            vendor,
            from,
            to,
            rows,
        });
    }

    fn start_read(&mut self, request: Request) {
        self.books_failed = None;
        let (start, end) = window(request.from, request.to);
        if self.link.is_demo() {
            self.reading = Some((request, Task::ready(demo_books(start, end))));
            self.poll();
            return;
        }
        let vendor = request.vendor;
        let task = match (self.link.session(), self.group.as_ref()) {
            (Some(session), Some(group)) => group
                .read(&session, move |s| read_books(s, vendor, start, end))
                .ok()
                .map(Task::Pending),
            _ => None,
        };
        match task {
            Some(task) => self.reading = Some((request, task)),
            None => self.books_failed = Some(request),
        }
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = self.vendors_task.as_mut().and_then(Task::try_take) {
            self.vendors_task = None;
            match r {
                Ok(contacts) => {
                    self.contacts.clone_from(&contacts);
                    let mut v: Vec<(ContactId, String)> = contacts
                        .into_iter()
                        .filter(|c| {
                            c.is_active
                                && matches!(
                                    c.fields.contact_type,
                                    ContactType::Vendor | ContactType::Both
                                )
                        })
                        .map(|c| (c.id, c.fields.display_name))
                        .collect();
                    v.sort_by_key(|(_, name)| name.to_lowercase());
                    self.vendors = v;
                }
                Err(_) => self.vendors_failed = true,
            }
        }
        let done = self.reading.as_mut().and_then(|(_, task)| task.try_take());
        if let Some(r) = done
            && let Some((request, _)) = self.reading.take()
        {
            match r {
                Ok(books) => {
                    let refs = self.credit_refs();
                    self.outcome = Some(outcome(request, &books, refs));
                    self.keep_after_match();
                    self.ask_balance();
                }
                Err(_) => self.books_failed = Some(request),
            }
        }
        self.poll_statement();
        // A bill, credit or payment saved elsewhere: read the books again.
        let changes = self.link.bus().take(&mut self.sub);
        if let Some(o) = self.outcome.as_ref()
            && self.reading.is_none()
            && (changes.bills_touch(o.request.vendor) || changes.ledger_any())
        {
            let request = o.request.clone();
            self.start_read(request);
        }
    }

    /// "Create bill" / "Create credit" for statement line `index`: Enter
    /// Bill opens filled with the vendor and the line's date, ref. no. and
    /// amount, unsaved.
    pub(crate) fn create_from(&mut self, index: usize) {
        let Some(o) = self.outcome.as_ref() else {
            return;
        };
        let Some(row) = o.request.rows.get(index) else {
            return;
        };
        // A payment, or a line that may be one, is never entered here.
        let Some(kind) = row.entry.kind.creates() else {
            return;
        };
        let prefill = BillPrefill {
            kind,
            vendor: o.request.vendor,
            date: row.entry.date,
            ref_no: row.entry.reference.clone(),
            amount: row.entry.amount,
        };
        self.link.push(CompanyEvent::OpenWith(
            CommandId::new(super::KIND),
            Box::new(OpenPayload::BillPrefill(prefill)),
        ));
        self.handoffs += 1;
        self.last_handoff = Some(kind);
    }

    /// Opens a books-only bill or credit in Enter Bill.
    fn open_item(&mut self, id: TransactionId) {
        self.link
            .push(CompanyEvent::OpenRecord(CommandId::new(super::KIND), id));
        self.opened += 1;
    }

    fn kind_text(kind: StatementKind) -> String {
        t(match kind {
            StatementKind::Charge => "statement.kind.charge",
            StatementKind::Credit => "statement.kind.credit",
            StatementKind::Payment => "statement.kind.payment",
            StatementKind::CreditOrPayment => "statement.kind.credit_or_payment",
        })
    }

    /// An amount cell: mono and right-aligned (padded to a fixed number of
    /// characters, so it lines up at any zoom).
    fn money(ui: &mut Ui, tokens: &Tokens, m: Money, style: MoneyStyle) {
        ui.label(
            RichText::new(format!("{:>14}", m.format(style)))
                .text_style(egui::TextStyle::Monospace)
                .color(tokens.text_primary),
        );
    }

    /// The statement line's four cells.
    fn line_cells(ui: &mut Ui, tokens: &Tokens, row: &PastedRow, style: MoneyStyle) {
        ui.label(crate::shell::us_date(row.entry.date));
        ui.label(&row.entry.reference);
        ui.label(Self::kind_text(row.entry.kind));
        Self::money(ui, tokens, row.entry.amount, style);
    }

    fn head(ui: &mut Ui, tokens: &Tokens, keys: &[&str]) {
        for k in keys {
            // An empty key: the action column has no heading.
            let text = if k.is_empty() { String::new() } else { t(k) };
            if *k == "statement.col.amount" {
                // Lined up over the padded amounts.
                ui.label(
                    RichText::new(format!("{text:>14}"))
                        .text_style(egui::TextStyle::Monospace)
                        .color(tokens.text_secondary),
                );
            } else {
                ui.label(RichText::new(text).size(12.0).color(tokens.text_secondary));
            }
        }
        ui.end_row();
    }

    /// A table that scrolls sideways inside its card when the window is
    /// narrow (the page itself never scrolls sideways).
    fn table(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
        egui::ScrollArea::horizontal().id_salt(id).show(ui, |ui| {
            egui::Grid::new(id)
                .striped(true)
                .spacing([16.0, 4.0])
                .show(ui, add);
        });
    }

    fn section_title(ui: &mut Ui, tokens: &Tokens, key: &str, n: usize) {
        form::heading(ui, tokens, &t_with(key, &[("n", &n.to_string())]));
    }

    fn input_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            self.statement_file_ui(ui, tokens);
            ui.add_space(4.0);
            let shown = self
                .vendors
                .iter()
                .find(|(id, _)| Some(*id) == self.vendor)
                .map_or_else(|| t("statement.vendor.none"), |(_, n)| n.clone());
            let mut picked = None;
            ui.horizontal(|ui| {
                let l = ui.add(egui::Label::new(t("statement.vendor")).selectable(false));
                egui::ComboBox::from_id_salt("statement/vendor")
                    .width(260.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (id, name) in &self.vendors {
                            if ui
                                .selectable_label(Some(*id) == self.vendor, name)
                                .clicked()
                            {
                                picked = Some(*id);
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            if let Some(v) = picked {
                self.vendor = Some(v);
                self.problem = None;
            }
            if self.vendors_task.is_some() {
                form::progress_line(ui, tokens, &t("statement.loading_vendors"));
            }
            if self.vendors_failed {
                ui.horizontal_wrapped(|ui| {
                    form::hint(ui, tokens, &t("statement.vendors_failed"));
                    if ui.button(t("statement.try_again")).clicked() {
                        self.load_vendors();
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                form::date_field(ui, &t("statement.from"), &mut self.from, self.today);
                ui.add_space(12.0);
                form::date_field(ui, &t("statement.to"), &mut self.to, self.today);
            });
            ui.add_space(4.0);
            let l = ui.add(egui::Label::new(t("statement.lines")).selectable(false));
            form::hint(ui, tokens, &t("statement.lines.hint"));
            ui.add(
                egui::TextEdit::multiline(&mut self.text)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(7)
                    .desired_width(ui.available_width()),
            )
            .labelled_by(l.id);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let busy = self.reading.is_some() || self.doc.awaiting_accept(&self.text);
                if ui
                    .add_enabled_ui(!busy, |ui| {
                        crate::widgets::primary_button(ui, &t("statement.match"))
                    })
                    .inner
                    .clicked()
                {
                    self.run_match();
                }
                if ui.button(t("statement.clear")).clicked() {
                    self.clear();
                }
            });
        });
    }

    fn problems_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(k) = self.problem {
            ui.label(RichText::new(t(k)).color(tokens.warn_text));
        }
        if let Some(p) = self.parsed.as_ref() {
            if p.header_skipped {
                form::hint(ui, tokens, &t("statement.header_skipped"));
            }
            for b in &p.bad {
                ui.label(
                    RichText::new(t_with(
                        "statement.bad_row",
                        &[
                            ("line", &b.line.to_string()),
                            ("problem", &t(b.problem.key().0)),
                        ],
                    ))
                    .color(tokens.warn_text),
                );
            }
        }
        if self.reading.is_some() {
            form::progress_line(ui, tokens, &t("statement.loading"));
        }
        if self.books_failed.is_some() {
            let mut again = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("statement.books_failed")).color(tokens.warn_text));
                again = ui.button(t("statement.try_again")).clicked();
            });
            if again && let Some(r) = self.books_failed.take() {
                self.start_read(r);
            }
        }
    }

    fn results_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        let credit_refs = self.credit_refs();
        let Some(o) = self.outcome.as_ref() else {
            return;
        };
        let mut create = None;
        let mut open = None;
        form::hint(ui, tokens, &t("statement.note.lines_only"));
        if !credit_refs {
            form::hint(ui, tokens, &t("statement.note.credits_no_ref"));
        }
        if !o.complete {
            ui.label(RichText::new(t("statement.incomplete")).color(tokens.warn_text));
        }
        if o.books.is_empty() {
            form::hint(ui, tokens, &t("statement.no_books"));
        }
        ui.add_space(8.0);

        // Matched.
        form::card(ui, tokens, |ui| {
            Self::section_title(ui, tokens, "statement.matched", o.result.pairs.len());
            if o.result.pairs.is_empty() {
                form::hint(ui, tokens, &t("statement.none"));
                return;
            }
            Self::table(ui, "statement/matched", |ui| {
                Self::head(
                    ui,
                    tokens,
                    &[
                        "statement.col.date",
                        "statement.col.ref",
                        "statement.col.type",
                        "statement.col.amount",
                        "statement.col.books",
                        "statement.col.how",
                    ],
                );
                for p in &o.result.pairs {
                    let Some(row) = o.request.rows.get(p.statement) else {
                        continue;
                    };
                    Self::line_cells(ui, tokens, row, style);
                    let books = o.item(p.books).map_or_else(String::new, Self::books_text);
                    ui.label(books);
                    ui.label(t(match p.pass {
                        MatchPass::Reference => "statement.pass.reference",
                        MatchPass::AmountAndDate => "statement.pass.amount_and_date",
                    }));
                    ui.end_row();
                }
            });
        });
        ui.add_space(10.0);

        // On the statement, not in the books.
        form::card(ui, tokens, |ui| {
            Self::section_title(
                ui,
                tokens,
                "statement.missing_in_books",
                o.result.missing_in_books.len(),
            );
            if o.result.missing_in_books.is_empty() {
                form::hint(ui, tokens, &t("statement.none"));
                return;
            }
            form::hint(ui, tokens, &t("statement.missing_in_books.hint"));
            Self::table(ui, "statement/missing_in_books", |ui| {
                Self::head(
                    ui,
                    tokens,
                    &[
                        "statement.col.date",
                        "statement.col.ref",
                        "statement.col.type",
                        "statement.col.amount",
                        "",
                    ],
                );
                for &i in &o.result.missing_in_books {
                    let Some(row) = o.request.rows.get(i) else {
                        continue;
                    };
                    Self::line_cells(ui, tokens, row, style);
                    if let Some(k) = row.entry.kind.creates() {
                        let k = match k {
                            BillKind::Bill => "statement.create_bill",
                            BillKind::Credit => "statement.create_credit",
                        };
                        if ui.button(t(k)).clicked() {
                            create = Some(i);
                        }
                    } else {
                        let hint = if row.entry.kind == StatementKind::CreditOrPayment {
                            "statement.credit_or_payment_hint"
                        } else {
                            "statement.payment_hint"
                        };
                        ui.label(
                            RichText::new(t(hint))
                                .size(12.0)
                                .color(tokens.text_secondary),
                        );
                    }
                    ui.end_row();
                }
            });
        });
        ui.add_space(10.0);

        // In the books, not on the statement.
        form::card(ui, tokens, |ui| {
            Self::section_title(
                ui,
                tokens,
                "statement.missing_on_statement",
                o.result.missing_on_statement.len(),
            );
            if o.result.missing_on_statement.is_empty() {
                form::hint(ui, tokens, &t("statement.none"));
                return;
            }
            Self::table(ui, "statement/missing_on_statement", |ui| {
                Self::head(
                    ui,
                    tokens,
                    &[
                        "statement.col.date",
                        "statement.col.ref",
                        "statement.col.type",
                        "statement.col.amount",
                        "",
                    ],
                );
                for &id in &o.result.missing_on_statement {
                    let Some(b) = o.item(id) else {
                        continue;
                    };
                    ui.label(crate::shell::us_date(b.date));
                    ui.label(&b.reference);
                    ui.label(Self::kind_text(b.kind));
                    Self::money(ui, tokens, b.amount, style);
                    if b.kind == StatementKind::Payment {
                        ui.label("");
                    } else if ui.button(t("statement.open")).clicked() {
                        open = Some(id);
                    }
                    ui.end_row();
                }
            });
        });

        // Lines with more than one candidate: listed, never picked.
        if !o.result.ambiguous.is_empty() {
            ui.add_space(10.0);
            form::card(ui, tokens, |ui| {
                Self::section_title(ui, tokens, "statement.ambiguous", o.result.ambiguous.len());
                form::hint(ui, tokens, &t("statement.ambiguous.hint"));
                Self::table(ui, "statement/ambiguous", |ui| {
                    Self::head(
                        ui,
                        tokens,
                        &[
                            "statement.col.date",
                            "statement.col.ref",
                            "statement.col.type",
                            "statement.col.amount",
                            "statement.col.candidates",
                        ],
                    );
                    for a in &o.result.ambiguous {
                        let Some(row) = o.request.rows.get(a.statement) else {
                            continue;
                        };
                        Self::line_cells(ui, tokens, row, style);
                        ui.vertical(|ui| {
                            for id in &a.candidates {
                                if let Some(b) = o.item(*id) {
                                    ui.label(Self::books_text(b));
                                }
                            }
                        });
                        ui.end_row();
                    }
                });
            });
        }
        if let Some(i) = create {
            self.create_from(i);
        }
        if let Some(id) = open {
            self.open_item(id);
        }
    }

    /// The heading, the form, the read lines and the results; the
    /// document pane above them when `pane_above`.
    fn body_ui(&mut self, ui: &mut Ui, tokens: &Tokens, pane_above: bool) {
        ui.add_space(8.0);
        form::heading(ui, tokens, &t("statement.heading"));
        form::hint(ui, tokens, &t("statement.intro"));
        ui.add_space(8.0);
        if pane_above {
            self.statement_pane_ui(ui, tokens, None);
            ui.add_space(6.0);
        }
        self.input_ui(ui, tokens);
        ui.add_space(6.0);
        self.read_lines_ui(ui, tokens);
        ui.add_space(6.0);
        self.problems_ui(ui, tokens);
        ui.add_space(6.0);
        self.after_match_ui(ui, tokens);
        self.results_ui(ui, tokens);
        ui.add_space(12.0);
    }

    /// A books entry in a cell: its type, date and ref. no.
    fn books_text(b: &BooksItem) -> String {
        let mut s = format!(
            "{} {}",
            Self::kind_text(b.kind),
            crate::shell::us_date(b.date)
        );
        if !b.reference.trim().is_empty() {
            s.push_str(" \u{b7} ");
            s.push_str(&b.reference);
        }
        s
    }
}

impl Screen for StatementMatchScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    /// The document pane sits beside the lines when there is room, so the
    /// shell gives this screen the window's rectangle and it scrolls its
    /// own parts (see `ui`).
    fn fixed_layout(&self) -> bool {
        true
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.money_style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        self.poll();
        if self.reading.is_some() || self.vendors_task.is_some() || self.doc.busy() {
            ui.ctx().request_repaint();
        }
        let tokens = Tokens::of(ui.ctx());
        // The statement beside its lines (A06 acceptance): the page stays
        // in view while the lines, their marks and Accept scroll beside
        // it. Narrow, or popped out, it goes back above the form.
        let docked =
            self.doc.viewer.source().is_some() && !crate::windows::doc_viewer::is_popped(ui.ctx());
        if docked && ui.available_width() >= read::SIDE_BY_SIDE {
            self.split_ui(ui, &tokens);
        } else {
            egui::ScrollArea::both()
                .id_salt("statement/body")
                .auto_shrink([false, false])
                .show(ui, |ui| self.body_ui(ui, &tokens, true));
        }
    }

    fn state(&self) -> Value {
        // Line numbers, counts and flags only: no amounts, names or refs.
        let line = |i: usize| {
            self.outcome
                .as_ref()
                .and_then(|o| o.request.rows.get(i))
                .map(|r| r.line)
        };
        let result = self.outcome.as_ref().map(|o| {
            json!({
                "matched": o.result.pairs.iter().map(|p| json!({
                    "line": line(p.statement),
                    "pass": match p.pass {
                        MatchPass::Reference => "reference",
                        MatchPass::AmountAndDate => "amount_and_date",
                    },
                })).collect::<Vec<_>>(),
                "missing_in_books": o.result.missing_in_books.iter().map(|&i| line(i)).collect::<Vec<_>>(),
                "missing_on_statement": o.result.missing_on_statement.len(),
                "ambiguous": o.result.ambiguous.iter().map(|a| line(a.statement)).collect::<Vec<_>>(),
                "books": o.books.len(),
                "complete": o.complete,
            })
        });
        json!({
            "screen": "statement_match",
            "loaded": self.vendors_task.is_none(),
            "vendors": self.vendors.len(),
            "vendor": self.vendor.is_some(),
            "from": self.from.value.map(CalendarDate::to_wire),
            "to": self.to.value.map(CalendarDate::to_wire),
            "rows": self.parsed.as_ref().map(|p| p.rows.len()),
            "bad_lines": self.parsed.as_ref().map(|p| p.bad.iter().map(|b| b.line).collect::<Vec<_>>()),
            "problem": self.problem,
            "reading": self.reading.is_some(),
            "failed": self.books_failed.is_some(),
            "credit_refs": self.credit_refs(),
            "result": result,
            "handoffs": self.handoffs,
            "last_handoff": self.last_handoff.map(|k| match k {
                BillKind::Bill => "bill",
                BillKind::Credit => "credit",
            }),
            "opened": self.opened,
            "statement": self.doc.state_json(&self.text, self.vendor),
            "balance_shown": self.balance_line().is_some(),
        })
    }

    /// Form keys while read lines wait for the owner's Accept, so Alt+A
    /// (`form.accept_read`) accepts them as on Enter Bill; window keys
    /// otherwise, so the box's own edit keys stay the box's.
    fn scope(&self) -> cl_model::Scope {
        if self.doc.awaiting_accept(&self.text) {
            cl_model::Scope::Form
        } else {
            cl_model::Scope::Window
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            "form.accept_read" if self.doc.awaiting_accept(&self.text) => {
                self.accept_read_lines();
                true
            }
            // Esc never closes the window on a reading waiting for Accept.
            "form.revert" => self.doc.read.is_some(),
            _ => false,
        }
    }

    fn document(&self) -> Option<crate::windows::doc_viewer::DocRef> {
        self.doc
            .viewer
            .source()
            .cloned()
            .map(crate::windows::doc_viewer::DocRef::new)
    }

    fn compare(&self) -> Option<crate::widgets::compare::CompareView> {
        self.doc.read.as_ref().map(|_| self.doc.compare.view())
    }

    fn on_viewer_event(&mut self, event: crate::widgets::compare::CompareEvent) {
        self.on_statement_event(event);
    }

    /// A statement draft from Review Documents (U-94).
    fn open_with(&mut self, payload: OpenPayload) -> bool {
        match payload {
            OpenPayload::StatementDraft(id) => {
                self.open_statement_draft(id);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TSV: &str = include_str!("../../../../../fixtures/statement-match/vendor_oct.tsv");

    fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        (dir, link)
    }

    fn today() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).expect("date")
    }

    fn screen(link: &Arc<CompanyLink>) -> StatementMatchScreen {
        let mut s = StatementMatchScreen::new(Arc::clone(link), None, today());
        s.poll();
        s
    }

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn matched(s: &mut StatementMatchScreen) {
        s.vendor = s.vendors.first().map(|v| v.0);
        s.text = TSV.to_string();
        s.run_match();
        s.poll();
    }

    #[test]
    fn opens_on_last_month_with_the_demo_vendors() {
        let (_d, link) = link();
        let cmd = cl_model::commands::find(KIND).expect("command");
        let s = make(cmd.id, &link, None, today()).expect("screen");
        assert_eq!(s.state()["screen"], "statement_match");
        let s = screen(&link);
        assert_eq!(s.from.value, CalendarDate::from_ymd(2026, 10, 1).ok());
        assert_eq!(s.to.value, CalendarDate::from_ymd(2026, 10, 31).ok());
        assert_ne!(s.vendors.len(), 0);
        assert!(
            s.vendors
                .windows(2)
                .all(|w| w[0].1.to_lowercase() <= w[1].1.to_lowercase())
        );
    }

    /// The October fixture: one bill missing on each side. Without
    /// `ap-edit-v1` the credit has no ref in the books, so it pairs by
    /// amount and date (review S6); everything else as `expected.json`.
    #[test]
    fn the_fixture_matches_with_one_missing_on_each_side() {
        let (_d, link) = link();
        let mut s = screen(&link);
        matched(&mut s);
        assert_eq!(s.problem, None);
        let st = s.state();
        let r = &st["result"];
        assert_eq!(st["credit_refs"], false);
        assert_eq!(
            r["matched"],
            json!([
                {"line": 2, "pass": "reference"},
                {"line": 3, "pass": "reference"},
                {"line": 4, "pass": "amount_and_date"},
                {"line": 5, "pass": "amount_and_date"},
                {"line": 7, "pass": "reference"},
            ])
        );
        assert_eq!(r["missing_in_books"], json!([6]));
        assert_eq!(r["missing_on_statement"], 1);
        assert_eq!(r["ambiguous"], json!([]));
        assert_eq!(r["complete"], true);
        let o = s.outcome.as_ref().expect("outcome");
        let only = o.item(o.result.missing_on_statement[0]).expect("item");
        assert_eq!(only.reference, "INV-55866");
        assert_eq!(only.amount, m("640.00"));
    }

    #[test]
    fn with_credit_refs_the_credit_pairs_by_ref() {
        let books = demo_books(
            CalendarDate::from_ymd(2026, 9, 24).expect("d"),
            CalendarDate::from_ymd(2026, 11, 7).expect("d"),
        );
        let parsed = sm::parse_paste(TSV, DateOrder::Mdy, today());
        let request = Request {
            vendor: ContactId::parse("00000000-0000-4000-8000-000000000001").expect("id"),
            from: CalendarDate::from_ymd(2026, 10, 1).expect("d"),
            to: CalendarDate::from_ymd(2026, 10, 31).expect("d"),
            rows: parsed.rows,
        };
        let o = outcome(request, &books, true);
        let credit = o
            .result
            .pairs
            .iter()
            .find(|p| p.statement == 2)
            .expect("pair");
        assert_eq!(credit.pass, MatchPass::Reference);
    }

    #[test]
    fn voids_and_entries_outside_the_period_stay_out() {
        let mut books = demo_books(
            CalendarDate::from_ymd(2026, 9, 24).expect("d"),
            CalendarDate::from_ymd(2026, 11, 7).expect("d"),
        );
        // INV-55866 (books only) voided: nothing is missing on the statement.
        for b in &mut books.bills {
            if b.vendor_invoice_number.as_deref() == Some("INV-55866") {
                b.status = TxnStatus::Void;
            }
        }
        let request = Request {
            vendor: ContactId::parse("00000000-0000-4000-8000-000000000001").expect("id"),
            from: CalendarDate::from_ymd(2026, 10, 1).expect("d"),
            // A period ending 10/28: the 10/29 bill is read for the slack only.
            to: CalendarDate::from_ymd(2026, 10, 28).expect("d"),
            rows: Vec::new(),
        };
        let o = outcome(request.clone(), &books, false);
        assert_eq!(o.books.len(), 5);
        assert_eq!(o.result.missing_on_statement.len(), 5);
        for b in &mut books.bills {
            b.status = TxnStatus::Posted;
        }
        let o = outcome(request, &books, false);
        assert_eq!(o.books.len(), 6, "the 10/29 bill is read");
        assert!(
            o.result
                .missing_on_statement
                .iter()
                .all(|id| o.item(*id).is_some_and(|b| b.date.day() <= 28)),
            "but not listed: it is outside the period"
        );
    }

    #[test]
    fn create_bill_hands_enter_bill_the_statement_line() {
        let (_d, link) = link();
        let mut s = screen(&link);
        matched(&mut s);
        let _ = link.drain();
        let i = s.outcome.as_ref().expect("o").result.missing_in_books[0];
        s.create_from(i);
        let events = link.drain();
        let [CompanyEvent::OpenWith(kind, payload)] = events.as_slice() else {
            panic!("one OpenWith: {events:?}");
        };
        assert_eq!(kind.as_str(), crate::screens::bill::KIND);
        let OpenPayload::BillPrefill(p) = payload.as_ref() else {
            panic!("a bill prefill");
        };
        assert_eq!(p.kind, BillKind::Bill);
        assert_eq!(Some(p.vendor), s.vendor);
        assert_eq!(p.date, CalendarDate::from_ymd(2026, 10, 20).expect("d"));
        assert_eq!(p.ref_no, "INV-55840");
        assert_eq!(p.amount, m("265.50"), "the statement's amount, exactly");
        assert_eq!(s.state()["last_handoff"], "bill");
        assert_eq!(s.state()["handoffs"], 1);
    }

    #[test]
    fn a_missing_credit_opens_a_vendor_credit_and_a_payment_creates_nothing() {
        let (_d, link) = link();
        let mut s = screen(&link);
        s.vendor = s.vendors.first().map(|v| v.0);
        s.text = "10/03/2026,CM-1,-20.00,credit\n10/04/2026,PMT 9,-33.00,payment\n\
                  10/05/2026,PMT-48129,-44.00\n"
            .into();
        s.run_match();
        s.poll();
        let missing = s
            .outcome
            .as_ref()
            .expect("o")
            .result
            .missing_in_books
            .clone();
        assert_eq!(missing.len(), 3);
        let _ = link.drain();
        s.create_from(missing[1]);
        assert!(link.drain().is_empty(), "payments are not created here");
        s.create_from(missing[2]);
        assert!(
            link.drain().is_empty(),
            "nor a negative line without its kind: it may be a payment"
        );
        s.create_from(missing[0]);
        let events = link.drain();
        let Some(CompanyEvent::OpenWith(_, payload)) = events.first() else {
            panic!("OpenWith");
        };
        let OpenPayload::BillPrefill(p) = payload.as_ref() else {
            panic!("prefill");
        };
        assert_eq!(p.kind, BillKind::Credit);
        assert_eq!(p.amount, m("20.00"));
    }

    #[test]
    fn match_refuses_until_vendor_dates_and_lines_are_right() {
        let (_d, link) = link();
        let mut s = screen(&link);
        s.text = TSV.into();
        s.run_match();
        assert_eq!(s.problem, Some("statement.problem.vendor"));
        s.vendor = s.vendors.first().map(|v| v.0);
        s.to.text = "09/01/2026".into();
        s.run_match();
        assert_eq!(s.problem, Some("statement.problem.dates"));
        s.to.text = "10/31/2026".into();
        s.text = "10/02/2026\tINV-1\tabc\n10/03/2026\tINV-2\t5.00\n".into();
        s.run_match();
        assert_eq!(s.problem, Some("statement.problem.fix_lines"));
        assert_eq!(s.state()["bad_lines"], json!([1]));
        assert!(s.outcome.is_none());
        s.text = "\n\n".into();
        s.run_match();
        assert_eq!(s.problem, Some("statement.problem.no_lines"));
    }

    #[test]
    fn opening_a_books_only_bill_asks_for_its_record() {
        let (_d, link) = link();
        let mut s = screen(&link);
        matched(&mut s);
        let _ = link.drain();
        let id = s.outcome.as_ref().expect("o").result.missing_on_statement[0];
        s.open_item(id);
        assert!(matches!(
            link.drain().as_slice(),
            [CompanyEvent::OpenRecord(k, t)] if k.as_str() == crate::screens::bill::KIND && *t == id
        ));
    }

    #[test]
    fn a_bill_saved_elsewhere_reads_the_books_again() {
        let (_d, link) = link();
        let mut s = screen(&link);
        matched(&mut s);
        let vendor = s.vendor.expect("vendor");
        // Nothing changed: the answer stays as it is.
        s.outcome.as_mut().expect("o").result.pairs.clear();
        s.poll();
        assert_eq!(s.outcome.as_ref().expect("o").result.pairs.len(), 0);
        // A save for this vendor: read and matched again.
        link.bus().publish(crate::data_bus::Change::Bills(vendor));
        s.poll();
        assert_eq!(s.outcome.as_ref().expect("o").result.pairs.len(), 5);
    }

    #[test]
    fn state_carries_no_amounts_or_refs() {
        let (_d, link) = link();
        let mut s = screen(&link);
        matched(&mut s);
        let shown = s.state().to_string();
        for secret in ["1,330", "1330", "265.5", "INV-", "CM-", "Gravel"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
    }

    /// Draws in light and dark at both scenario sizes with labelled inputs.
    #[test]
    fn draws_with_labelled_inputs() {
        let (_d, link) = link();
        for theme in [
            crate::theme::ThemeChoice::Light,
            crate::theme::ThemeChoice::Dark,
        ] {
            let mut s = screen(&link);
            matched(&mut s);
            for size in [(1440.0, 940.0), (960.0, 640.0)] {
                let ctx = egui::Context::default();
                crate::theme::apply(&ctx, theme);
                crate::fonts::install(&ctx);
                ctx.enable_accesskit();
                let mut nodes = Vec::new();
                for _ in 0..3 {
                    let input = egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(size.0, size.1),
                        )),
                        ..egui::RawInput::default()
                    };
                    let mut out = ctx.run_ui(input, |ui| {
                        // As the shell draws a fixed-layout screen.
                        egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
                    });
                    out.textures_delta.clear();
                    nodes.extend(
                        out.platform_output
                            .accesskit_update
                            .map(|u| u.nodes)
                            .unwrap_or_default(),
                    );
                }
                for label in ["Vendor", "From", "To", "Statement lines"] {
                    let id = nodes
                        .iter()
                        .find(|(_, n)| {
                            n.role() == egui::accesskit::Role::Label && n.value() == Some(label)
                        })
                        .map_or_else(|| panic!("label {label}"), |(id, _)| *id);
                    assert!(
                        nodes.iter().any(|(_, n)| n.labelled_by().contains(&id)),
                        "{label} labels an input"
                    );
                }
                for want in [
                    "Matched (5)",
                    "On the statement, not in your books (1)",
                    "In your books, not on the statement (1)",
                ] {
                    assert!(nodes.iter().any(|(_, n)| n.value() == Some(want)), "{want}");
                }
                assert!(
                    nodes
                        .iter()
                        .any(|(_, n)| n.role() == egui::accesskit::Role::Button
                            && n.label() == Some("Create bill")),
                    "Create bill"
                );
                let unnamed = crate::screens::unnamed_widgets(&nodes);
                assert!(unnamed.is_empty(), "{size:?}: unnamed widgets: {unnamed:?}");
            }
        }
    }
}
