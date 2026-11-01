//! Refund a Credit (P22 D5, `clients.refund`): money paid back to a client
//! from a payment's credit, a credit memo or a held customer deposit.
//!
//! - The client's credits come from the engine (`GET /payments/credits` and
//!   P21's deposits on hand), oldest first, each with what is left of it.
//!   The amount defaults to the picked credit's remaining string; a typed
//!   amount is the owner's (the engine answers 409 `CREDIT_NOT_AVAILABLE`
//!   / `DEPOSIT_NOT_AVAILABLE` when there isn't that much left — nothing
//!   here compares money).
//! - Paid by check (queued to print by default, Q-P22d: File › Print Forms
//!   lists it with the client's address), cash, card, bank transfer or
//!   other (Q-88), from a bank-type account only (Q-P22l).
//! - One idempotency key per save attempt (`RefundForm::attempt`), reused
//!   through the closing-date prompt and after a lost reply.
//! - Receive Payment's "Refund it" and a credit memo's Refund open it
//!   prefilled (`OpenPayload::RefundFrom`, Q-P22m); Client Center's Refund
//!   opens it for the selected client (`OpenPayload::ForClient`).
//! - A saved refund stays open with Void (the credit comes back; a queued
//!   check leaves the print queue).

use std::sync::Arc;

use cl_api::{
    ApiError, IdempotencyKey, NewRefund, RefundCheck, RefundMethod, RefundRecord, RefundRefusal,
    RefundSource, RequestGroup, refund_refusal,
};
use cl_core::ids::{AccountId, ContactId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle, Secret, UserInput};
use cl_model::closing::needs_password;
use cl_model::forms::refund::{
    CheckPlan, RefundContext, RefundForm, RefundMethodChoice, RefundPlan, RefundProblem,
    RefundSourceChoice, RefundSourceKind,
};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink};
use crate::data_bus::Subscription;
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt, Verb};
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::card_charge::kit::{self, Books, NameInput, NameOutcome};
use crate::screens::company::form::{self, DateInput};
use crate::screens::invoice::engine::{self, Job};
use crate::screens::register::lists::RegisterLists;
use crate::theme::Tokens;
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "clients.refund";

/// The Refund a Credit window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(RefundScreen::new(Arc::clone(link), today)) as Box<_>)
}

/// One credit the client can be refunded from.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SourceRow {
    pub id: TransactionId,
    pub kind: RefundSourceKind,
    pub date: CalendarDate,
    pub number: Option<String>,
    pub remaining: Money,
}

impl std::fmt::Debug for SourceRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceRow")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl SourceRow {
    fn choice(&self) -> RefundSourceChoice {
        RefundSourceChoice {
            id: self.id,
            kind: self.kind,
            remaining: self.remaining,
        }
    }
}

/// What the closing-date password is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Save(After),
    Void,
}

/// What a save does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum After {
    New,
    Close,
    Stay,
}

/// The Refund a Credit window.
pub struct RefundScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) bus: Subscription,
    pub(crate) today: CalendarDate,
    pub(crate) books: Books,
    pub(crate) clients: Arc<RegisterLists>,
    pub(crate) form: RefundForm,
    pub(crate) client: Option<ContactId>,
    pub(crate) client_input: NameInput,
    pub(crate) date: DateInput,
    pub(crate) amount_text: String,
    pub(crate) check_no: String,
    pub(crate) memo: String,
    pub(crate) from: Option<AccountId>,
    pub(crate) sources: Vec<SourceRow>,
    pub(crate) sources_for: Option<ContactId>,
    pub(crate) sources_job: Option<Job<Vec<SourceRow>>>,
    pub(crate) save_job: Option<(After, Job<RefundRecord>)>,
    pub(crate) void_job: Option<Job<RefundRecord>>,
    /// The key of the save attempt in flight or undecided.
    pub(crate) key: Option<(u64, IdempotencyKey)>,
    pub(crate) saved: Option<RefundRecord>,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    pub(crate) problems: Vec<RefundProblem>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) error: Option<ApiError>,
    pub(crate) saved_count: u32,
    pub(crate) void_open: bool,
    pub(crate) void_reason: String,
    pub(crate) dirty: bool,
    pub(crate) layout: crate::layout::FrameReport,
    pub(crate) money_style: MoneyStyle,
}

impl std::fmt::Debug for RefundScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefundScreen")
            .field("form", &self.form)
            .finish_non_exhaustive()
    }
}

/// The engine's refund method.
fn method_of(m: RefundMethodChoice) -> RefundMethod {
    match m {
        RefundMethodChoice::Check => RefundMethod::Check,
        RefundMethodChoice::Cash => RefundMethod::Cash,
        RefundMethodChoice::Card => RefundMethod::CreditCard,
        RefundMethodChoice::BankTransfer => RefundMethod::Ach,
        RefundMethodChoice::Other => RefundMethod::Other,
    }
}

/// The checked refund as the engine's request (field by field).
pub(crate) fn new_refund(p: &RefundPlan) -> NewRefund {
    NewRefund {
        customer_id: p.client,
        date: p.date,
        from_account: p.from_account,
        amount: p.amount,
        source: match p.source.kind {
            RefundSourceKind::PaymentCredit => RefundSource::Payment(p.source.id),
            RefundSourceKind::CreditMemo => RefundSource::CreditMemo(p.source.id),
            RefundSourceKind::HeldDeposit => RefundSource::Deposit(p.source.id),
        },
        method: method_of(p.method),
        check: match &p.check {
            CheckPlan::NotACheck => RefundCheck::NotACheck,
            CheckPlan::PrintLater => RefundCheck::PrintLater,
            CheckPlan::Number(n) => RefundCheck::Number(n.clone()),
        },
        ref_no: None,
        memo: p.memo.clone(),
    }
}

impl RefundScreen {
    /// A new, empty refund.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> RefundScreen {
        let group = link.group().and_then(Result::ok);
        let bus = link.bus().subscribe();
        if let Some(mut d) = link.demo_state() {
            crate::company::demo::receivables::seed(&mut d);
            crate::company::demo::credit_memos::seed(&mut d);
        }
        let mut s = RefundScreen {
            link,
            group,
            bus,
            today,
            books: Books::default(),
            clients: Arc::new(RegisterLists::default()),
            form: RefundForm::new(Some(today), None),
            client: None,
            client_input: NameInput::default(),
            date: DateInput::new(today),
            amount_text: String::new(),
            check_no: String::new(),
            memo: String::new(),
            from: None,
            sources: Vec::new(),
            sources_for: None,
            sources_job: None,
            save_job: None,
            void_job: None,
            key: None,
            saved: None,
            closing_prompt: ClosingPrompt::default(),
            problems: Vec::new(),
            notice: None,
            error: None,
            saved_count: 0,
            void_open: false,
            void_reason: String::new(),
            dirty: false,
            layout: crate::layout::FrameReport::default(),
            money_style: MoneyStyle::default(),
        };
        s.books.start(&s.link, s.group.as_ref());
        s.poll();
        s
    }

    /// The bank-type accounts a refund may come from (Q-P22l).
    pub(crate) fn banks(&self) -> Vec<(AccountId, String)> {
        self.books
            .accounts
            .iter()
            .filter(|a| a.is_active && a.fields.detail_type.as_deref() == Some("bank"))
            .map(|a| (a.id, crate::screens::register::account_text(a)))
            .collect()
    }

    fn lists_arrived(&mut self) {
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
        if self.from.is_none() {
            self.from = self.banks().first().map(|(id, _)| *id);
            self.form.set_from_account(self.from);
        }
        if let Some(c) = self.client {
            self.client_input.text = self.books.contact_name(c).unwrap_or_default();
        }
    }

    /// Takes every answer that arrived.
    pub(crate) fn poll(&mut self) {
        if self.books.poll() {
            self.lists_arrived();
        }
        if let Some(r) = engine::poll(&mut self.sources_job) {
            match r {
                Ok(rows) => self.sources = rows,
                Err(e) => self.error = Some(e),
            }
        }
        if let Some((after, job)) = self.save_job.as_mut()
            && let Some(r) = job.take()
        {
            let after = *after;
            self.save_job = None;
            match r {
                Ok(rec) => self.saved(rec, after),
                Err(e) => self.save_failed(&e, after),
            }
        }
        if let Some(r) = engine::poll(&mut self.void_job) {
            match r {
                Ok(rec) => {
                    self.closing_prompt.saved();
                    let accounts: Vec<AccountId> = self.from.into_iter().collect();
                    crate::forms::publish_saved(&self.link, &self.bus, &accounts);
                    self.saved = Some(rec);
                    self.void_open = false;
                    self.notice = Some("refund.voided");
                    self.fetch_sources();
                }
                Err(e) => {
                    if !self
                        .closing_prompt
                        .failed(&e, Pending::Void, true, self.books.closing)
                    {
                        self.error = Some(e);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ edits

    /// The client (a change reads its credits and drops the picked one).
    pub(crate) fn set_client(&mut self, c: Option<ContactId>) {
        self.client = c;
        self.form.set_client(c);
        self.amount_text.clear();
        self.sources.clear();
        self.sources_for = None;
        self.dirty = true;
        self.notice = None;
        self.fetch_sources();
    }

    /// Reads the client's open credits and deposits on hand, oldest first.
    pub(crate) fn fetch_sources(&mut self) {
        let Some(c) = self.client else {
            return;
        };
        self.sources_for = Some(c);
        let deposits = self.link.is_demo()
            || self
                .link
                .capabilities()
                .has(cl_api::Capabilities::CUSTOMER_DEPOSITS);
        self.sources_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| {
                let mut rows = rows_of(
                    crate::company::demo::credit_memos::credits(d, c),
                    crate::company::demo::receivables::deposits_on_hand(d, c),
                );
                rows.sort_by_key(|r| r.date);
                Ok(rows)
            },
            move |s| async move {
                let credits = s.credit_memos().credits(c).await?;
                let held = if deposits {
                    s.customer_payments().deposits_on_hand(c).await?
                } else {
                    Vec::new()
                };
                let mut rows = rows_of(credits, held);
                rows.sort_by_key(|r| r.date);
                Ok(rows)
            },
        ));
        self.poll();
    }

    /// Picks a credit (its remaining becomes the amount).
    pub(crate) fn pick_source(&mut self, row: &SourceRow) {
        self.form.set_source(Some(row.choice()));
        self.amount_text = self
            .form
            .amount()
            .map(|m| m.format(self.money_style))
            .unwrap_or_default();
        self.dirty = true;
    }

    /// The amount as typed (blank: the credit's remaining again).
    pub(crate) fn amount_typed(&mut self) {
        let text = self.amount_text.trim();
        if text.is_empty() {
            self.form.set_amount(None);
        } else if let Ok(m) = Money::parse_user(text, UserInput::MONEY) {
            self.form.set_amount(Some(m));
        } else {
            return;
        }
        self.dirty = true;
    }

    /// Prefills from a source (Receive Payment, a credit memo).
    pub(crate) fn prefill(&mut self, client: ContactId, source: RefundSourceChoice) {
        self.books.poll();
        self.lists_arrived();
        self.client = Some(client);
        self.form = RefundForm::for_source(client, source, self.date.value, self.from);
        self.client_input.text = self.books.contact_name(client).unwrap_or_default();
        self.amount_text = source.remaining.format(self.money_style);
        self.dirty = true;
        self.fetch_sources();
    }

    /// Clear: a new, empty refund.
    pub(crate) fn clear(&mut self) {
        self.form = RefundForm::new(Some(self.today), self.from);
        self.client = None;
        self.client_input = NameInput::default();
        self.date = DateInput::new(self.today);
        self.amount_text.clear();
        self.check_no.clear();
        self.memo.clear();
        self.sources.clear();
        self.sources_for = None;
        self.saved = None;
        self.key = None;
        self.problems.clear();
        self.error = None;
        self.notice = None;
        self.void_open = false;
        self.dirty = false;
    }

    // ------------------------------------------------------------ saves

    /// Checks and sends the refund. False: nothing sent.
    #[allow(clippy::needless_pass_by_value)] // the secret moves into the queued write
    pub(crate) fn start_save(&mut self, after: After, password: Option<Secret>) -> bool {
        if self.save_job.is_some() || self.saved.is_some() {
            return false;
        }
        self.error = None;
        self.notice = None;
        self.form.set_date(self.date.value);
        self.form.set_memo(&self.memo);
        if self.form.method() == RefundMethodChoice::Check && !self.form.print_later() {
            self.form.set_check_no(&self.check_no);
        }
        let ctx = RefundContext {
            closing_date: self.books.closing,
            password_given: password.is_some(),
        };
        let plan = match self.form.save_plan(&ctx) {
            Ok(p) => p,
            Err(ps) => {
                if ps == [RefundProblem::Closing] {
                    self.closing_prompt.set_verb(Verb::Change);
                    self.closing_prompt
                        .ask(Pending::Save(after), true, self.books.closing);
                } else {
                    self.problems = ps
                        .into_iter()
                        .filter(|p| *p != RefundProblem::Closing)
                        .collect();
                }
                return false;
            }
        };
        debug_assert!(!needs_password(None, plan.date, self.books.closing) || password.is_some());
        self.problems.clear();
        let new = new_refund(&plan);
        // One key per attempt: a retry of the same refund replays.
        let key = match &self.key {
            Some((a, k)) if *a == plan.attempt => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((plan.attempt, k.clone()));
                k
            }
        };
        let label = engine::label("orphan.refund_save", Some(key.clone()));
        let demo = new.clone();
        let demo_pw = password.clone();
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            label,
            move |d| {
                let id =
                    crate::company::demo::credit_memos::create_refund(d, &demo, demo_pw.as_ref())?;
                crate::company::demo::credit_memos::get_refund(d, id)
            },
            move |s| async move {
                let opts = cl_api::PostingOptions {
                    closed_period: password.map(cl_api::ClosedPeriodAuthorization::with_password),
                };
                let id = s.customer_refunds().create(&new, &opts, key).await?;
                s.customer_refunds().get(id).await
            },
        );
        self.save_job = Some((after, job));
        self.poll();
        true
    }

    fn saved(&mut self, rec: RefundRecord, after: After) {
        self.closing_prompt.saved();
        self.key = None;
        self.saved_count += 1;
        let accounts: Vec<AccountId> = self.from.into_iter().collect();
        crate::forms::publish_saved(&self.link, &self.bus, &accounts);
        let queued = rec.print_status == Some(cl_api::RefundPrintStatus::Queue);
        self.link.push(CompanyEvent::Toast(if queued {
            "refund.saved.queued"
        } else {
            "refund.saved"
        }));
        match after {
            After::Close => {
                self.dirty = false;
                self.link
                    .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
            }
            After::New => self.clear(),
            After::Stay => {
                self.saved = Some(rec);
                self.dirty = false;
                self.notice = Some(if queued {
                    "refund.saved.queued"
                } else {
                    "refund.saved"
                });
            }
        }
    }

    fn save_failed(&mut self, e: &ApiError, after: After) {
        if !crate::screens::register::keeps_key(e) {
            self.key = None;
        }
        if self
            .closing_prompt
            .failed(e, Pending::Save(after), true, self.books.closing)
        {
            return;
        }
        self.notice = match refund_refusal(e) {
            Some(RefundRefusal::CreditNotAvailable | RefundRefusal::DepositNotAvailable) => {
                // The engine's figure: read the credits again.
                self.fetch_sources();
                Some("refund.refusal.not_available")
            }
            Some(RefundRefusal::DateBeforeSource) => Some("refund.refusal.date_before_source"),
            Some(RefundRefusal::CheckNumberInUse) => Some("refund.refusal.check_number_in_use"),
            Some(RefundRefusal::AccountInvalid) => Some("refund.refusal.account"),
            Some(_) => Some("refund.refusal.other"),
            None => {
                self.error = Some(e.clone());
                None
            }
        };
    }

    /// Void the saved refund: its credit comes back; a queued check leaves
    /// the print queue.
    #[allow(clippy::needless_pass_by_value)] // the secret moves into the queued write
    pub(crate) fn start_void(&mut self, password: Option<Secret>) -> bool {
        let Some(rec) = self.saved.clone() else {
            return false;
        };
        if rec.status == cl_api::SalesDocStatus::Void || self.void_job.is_some() {
            return false;
        }
        if password.is_none() && needs_password(None, rec.date, self.books.closing) {
            self.closing_prompt.set_verb(Verb::Void);
            self.closing_prompt
                .ask(Pending::Void, true, self.books.closing);
            return false;
        }
        let reason = Some(self.void_reason.trim().to_string())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| t("refund.void.default_reason"));
        let key = IdempotencyKey::new();
        let id = rec.id;
        let pw = password.clone();
        self.void_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            engine::label("orphan.refund_void", Some(key.clone())),
            move |d| {
                crate::company::demo::credit_memos::void_refund(d, id, pw.as_ref())?;
                crate::company::demo::credit_memos::get_refund(d, id)
            },
            move |s| async move {
                let opts = cl_api::PostingOptions {
                    closed_period: password.map(cl_api::ClosedPeriodAuthorization::with_password),
                };
                s.transactions().void(id, &reason, &opts, key).await?;
                s.customer_refunds().get(id).await
            },
        ));
        self.poll();
        true
    }

    pub(crate) fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.closing_prompt.submit() else {
            return;
        };
        let sent = match pending {
            Pending::Save(after) => self.start_save(after, Some(secret)),
            Pending::Void => self.start_void(Some(secret)),
        };
        if !sent && self.closing_prompt.dialog_open() {
            self.closing_prompt.nothing_sent();
        }
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let mut frame = FixedFrame::begin(ui, "refund", FrameOpts::default());
        frame.top(|ui| self.top_ui(ui, &tokens));
        frame.body(Body::Scroll, |ui| self.body_ui(ui, &tokens));
        let idle = self.save_job.is_none() && !self.closing_prompt.dialog_open();
        let can_save = idle && self.saved.is_none();
        let mut cmd = None;
        frame.bottom(|ui| cmd = kit::actions_row(ui, can_save, idle));
        self.layout = frame.end();
        if let Some(c) = cmd {
            self.command(CommandId::new(c));
        }
    }

    fn top_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.label(
            RichText::new(t(&format!("cmd.{KIND}")))
                .size(22.0)
                .strong()
                .color(tokens.text_primary),
        );
        match self.closing_prompt.ui(ui, tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        if let Some(k) = self.notice {
            form::hint(ui, tokens, &t(k));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
    }

    #[allow(clippy::too_many_lines)]
    fn body_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(rec) = self.saved.clone() {
            self.saved_ui(ui, tokens, &rec);
            return;
        }
        let style = self.money_style;
        form::card(ui, tokens, |ui| {
            crate::forms::header::wrap_row(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                kit::wrap_item(ui, "refund/client", |ui| {
                    let lists = Arc::clone(&self.clients);
                    let (out, _) = kit::name_field(
                        ui,
                        tokens,
                        &t("refund.client"),
                        &mut self.client_input,
                        &lists,
                        self.client,
                        260.0,
                    );
                    match out {
                        NameOutcome::Picked(c) if Some(c) != self.client => {
                            self.set_client(Some(c));
                        }
                        NameOutcome::Cleared if self.client.is_some() => self.set_client(None),
                        _ => {}
                    }
                });
                kit::wrap_item(ui, "refund/date", |ui| {
                    let today = self.today;
                    let (_, d) = kit::date_field(
                        ui,
                        tokens,
                        &t("refund.date"),
                        &mut self.date,
                        today,
                        120.0,
                    );
                    if let kit::DateChange::Set(d) = d {
                        self.form.set_date(d);
                        self.dirty = true;
                    }
                });
            });
        });
        ui.add_space(8.0);
        ui.label(RichText::new(t("refund.credits")).strong());
        if self.client.is_none() {
            form::hint(ui, tokens, &t("refund.credits.choose_client"));
        } else if self.sources_job.is_some() {
            form::hint(ui, tokens, &t("refund.credits.loading"));
        } else if self.sources.is_empty() && self.form.source().is_none() {
            let name = self
                .client
                .and_then(|c| self.books.contact_name(c))
                .unwrap_or_default();
            form::hint(
                ui,
                tokens,
                &t_with("refund.credits.none", &[("client", &name)]),
            );
        }
        let picked = self.form.source().map(|s| s.id);
        let mut pick: Option<SourceRow> = None;
        for row in &self.sources {
            let kind = t(match row.kind {
                RefundSourceKind::PaymentCredit => "refund.source.payment",
                RefundSourceKind::CreditMemo => "refund.source.credit_memo",
                RefundSourceKind::HeldDeposit => "refund.source.deposit",
            });
            let label = t_with(
                "refund.source.row",
                &[
                    ("kind", &kind),
                    ("number", row.number.as_deref().unwrap_or_default()),
                    ("date", &crate::shell::us_date(row.date)),
                    ("left", &row.remaining.format(style)),
                ],
            );
            if ui.radio(picked == Some(row.id), label).clicked() {
                pick = Some(row.clone());
            }
        }
        if let Some(r) = pick {
            self.pick_source(&r);
        }
        ui.add_space(8.0);
        form::card(ui, tokens, |ui| {
            crate::forms::header::wrap_row(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                kit::wrap_item(ui, "refund/amount", |ui| {
                    let r = kit::text_field(
                        ui,
                        tokens,
                        &t("refund.amount"),
                        &mut self.amount_text,
                        140.0,
                        20,
                    );
                    if r.lost_focus() || r.changed() {
                        self.amount_typed();
                    }
                    if r.lost_focus()
                        && let Some(s) = form::settled_money_text(&self.amount_text, style)
                    {
                        self.amount_text = s;
                    }
                });
                kit::wrap_item(ui, "refund/method", |ui| {
                    ui.vertical(|ui| {
                        let l = kit::above(ui, tokens, &t("refund.method"));
                        let now = self.form.method();
                        let mut pick = None;
                        egui::ComboBox::from_id_salt("refund/method")
                            .selected_text(t(now.key().0))
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for m in RefundMethodChoice::ALL {
                                    if ui.selectable_label(m == now, t(m.key().0)).clicked() {
                                        pick = Some(m);
                                    }
                                }
                            })
                            .response
                            .labelled_by(l.id);
                        if let Some(m) = pick {
                            self.form.set_method(m);
                            self.check_no.clear();
                            self.dirty = true;
                        }
                    });
                });
                kit::wrap_item(ui, "refund/from", |ui| {
                    ui.vertical(|ui| {
                        let l = kit::above(ui, tokens, &t("refund.from"));
                        let banks = self.banks();
                        let shown = banks
                            .iter()
                            .find(|(id, _)| Some(*id) == self.from)
                            .map(|(_, n)| n.clone())
                            .unwrap_or_default();
                        let mut pick = None;
                        egui::ComboBox::from_id_salt("refund/from")
                            .selected_text(shown)
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for (id, name) in &banks {
                                    if ui.selectable_label(Some(*id) == self.from, name).clicked() {
                                        pick = Some(*id);
                                    }
                                }
                            })
                            .response
                            .labelled_by(l.id)
                            .on_hover_text(t("refund.from.tip"));
                        if let Some(p) = pick {
                            self.from = Some(p);
                            self.form.set_from_account(Some(p));
                            self.dirty = true;
                        }
                    });
                });
            });
            if self.form.method() == RefundMethodChoice::Check {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let mut later = self.form.print_later();
                    if ui
                        .checkbox(&mut later, t("refund.print_later"))
                        .on_hover_text(t("refund.print_later.tip"))
                        .changed()
                    {
                        self.form.set_print_later(later);
                        if later {
                            self.check_no.clear();
                        }
                        self.dirty = true;
                    }
                    if !self.form.print_later() {
                        let r = kit::text_field(
                            ui,
                            tokens,
                            &t("refund.check_no"),
                            &mut self.check_no,
                            120.0,
                            cl_model::forms::refund::CHECK_NO_MAX,
                        );
                        if r.changed() {
                            self.form.set_check_no(&self.check_no);
                            self.dirty = true;
                        }
                    }
                });
            }
            ui.add_space(6.0);
            let r = kit::text_field(ui, tokens, &t("refund.memo"), &mut self.memo, 420.0, 4000);
            if r.changed() {
                self.dirty = true;
            }
        });
    }

    fn saved_ui(&mut self, ui: &mut Ui, tokens: &Tokens, rec: &RefundRecord) {
        let style = self.money_style;
        let void = rec.status == cl_api::SalesDocStatus::Void;
        form::card(ui, tokens, |ui| {
            form::heading(
                ui,
                tokens,
                &t(if void {
                    "refund.saved.void"
                } else {
                    "refund.saved.title"
                }),
            );
            let name = self.books.contact_name(rec.customer_id).unwrap_or_default();
            kit::balance_line(ui, tokens, &t("refund.client"), Some(name));
            kit::balance_line(
                ui,
                tokens,
                &t("refund.amount"),
                Some(rec.amount.format(style)),
            );
            kit::balance_line(
                ui,
                tokens,
                &t("refund.date"),
                Some(crate::shell::us_date(rec.date)),
            );
            if let Some(n) = &rec.check_number {
                kit::balance_line(ui, tokens, &t("refund.check_no"), Some(n.clone()));
            }
            if rec.print_status == Some(cl_api::RefundPrintStatus::Queue) && !void {
                form::hint(ui, tokens, &t("refund.saved.queued"));
                if ui.button(t("refund.print_checks")).clicked() {
                    self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                        crate::screens::print_forms::KIND,
                    )));
                }
            }
            if void {
                return;
            }
            ui.add_space(6.0);
            if self.void_open {
                let r = form::field(ui, &t("refund.void.reason"), &mut self.void_reason);
                let mut go = crate::forms::enter::submitted(&r);
                ui.horizontal(|ui| {
                    go |= ui.button(t("refund.void.confirm")).clicked();
                    if ui.button(t("refund.void.cancel")).clicked() {
                        self.void_open = false;
                    }
                });
                if go {
                    self.start_void(None);
                }
            } else if ui.button(t("refund.void")).clicked() {
                self.void_open = true;
            }
        });
    }

    /// The control channel's view (flags, counts and the engine's figures
    /// as shown; no names or memos).
    #[must_use]
    pub fn state_json(&self) -> Value {
        let style = MoneyStyle::default();
        json!({
            "screen": "refund",
            "loaded": self.books.loaded,
            "client": self.client.is_some(),
            "sources": self.sources.iter().map(|r| json!({
                "kind": format!("{:?}", r.kind).to_lowercase(),
                "remaining": r.remaining.format(style),
            })).collect::<Vec<_>>(),
            "source": self.form.source().map(|s| format!("{:?}", s.kind).to_lowercase()),
            "amount": self.form.amount().map(|m| m.format(style)),
            "method": self.form.method().wire(),
            "print_later": self.form.print_later(),
            "from": self.from.is_some(),
            "problems": self.problems.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>(),
            "notice": self.notice,
            "error": self.error.is_some(),
            "saving": self.save_job.is_some(),
            "saved_count": self.saved_count,
            "saved": self.saved.as_ref().map(|r| json!({
                "amount": r.amount.format(style),
                "void": r.status == cl_api::SalesDocStatus::Void,
                "queued": r.print_status == Some(cl_api::RefundPrintStatus::Queue),
                "source": r.source.as_ref().map(|s| format!("{:?}", s.kind).to_lowercase()),
            })),
            "dirty": self.dirty,
            "closing": self.closing_prompt.state_json(true),
            "layout": self.layout.json(),
        })
    }
}

/// The engine's credits and held deposits as source rows.
fn rows_of(credits: Vec<cl_api::OpenCredit>, held: Vec<cl_api::DepositOnHand>) -> Vec<SourceRow> {
    credits
        .into_iter()
        .filter_map(|c| {
            let kind = match c.kind {
                cl_api::CreditKind::Payment => RefundSourceKind::PaymentCredit,
                cl_api::CreditKind::CreditMemo => RefundSourceKind::CreditMemo,
                cl_api::CreditKind::Deposit => RefundSourceKind::HeldDeposit,
                cl_api::CreditKind::Other => return None,
            };
            Some(SourceRow {
                id: c.source_id,
                kind,
                date: c.date,
                number: c.number,
                remaining: c.remaining,
            })
        })
        .chain(held.into_iter().map(|d| SourceRow {
            id: d.payment_id,
            kind: RefundSourceKind::HeldDeposit,
            date: d.date,
            number: d.ref_no,
            remaining: d.remaining,
        }))
        .filter(|r| r.remaining.is_positive())
        .collect()
}

impl Screen for RefundScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        self.money_style = style;
        self.poll();
        self.draw(ui);
        if self.save_job.is_some() || self.void_job.is_some() || self.sources_job.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(60));
        }
    }

    fn dirty(&self) -> bool {
        self.dirty && self.saved.is_none()
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn save(&mut self) -> bool {
        self.start_save(After::Stay, None);
        !self.dirty()
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            if h && id.as_str() == "form.save_new" {
                self.submit_closing();
            }
            return h;
        }
        match id.as_str() {
            KIND => {}
            "form.save_new" => {
                if self.saved.is_some() {
                    self.clear();
                } else {
                    self.start_save(After::New, None);
                }
            }
            "form.save_close" => {
                if self.saved.is_some() {
                    self.link
                        .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
                } else {
                    self.start_save(After::Close, None);
                }
            }
            "form.clear" => self.clear(),
            "edit.void_transaction" => {
                if self.saved.is_some() {
                    self.void_open = true;
                }
            }
            "form.revert" => {
                if self.void_open {
                    self.void_open = false;
                } else if !self.dirty() {
                    self.link
                        .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
                } else {
                    self.link.push(CompanyEvent::PromptDirty);
                }
            }
            _ => return false,
        }
        true
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        match payload {
            OpenPayload::RefundFrom(client, source) => {
                self.clear();
                self.prefill(client, source);
                true
            }
            OpenPayload::ForClient(c) => {
                self.clear();
                self.books.poll();
                self.lists_arrived();
                self.client_input.text = self.books.contact_name(c).unwrap_or_default();
                self.set_client(Some(c));
                true
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[path = "refund_tests.rs"]
mod tests;
