//! Receive Payment (P21, `clients.receive_payment`, Ctrl+R; mockup
//! `ReceivePayment.html`).
//!
//! - **Every figure is the engine's** (Q-P21s): the form
//!   (`cl_model::forms::payment`) sends what the owner chose to
//!   `POST /payments/preview` (debounced 250 ms) and shows the allocation,
//!   totals, shortfalls, balance before and after and the next step from
//!   the reply. While a newer preview is pending the figures say
//!   "Updating…"; Save waits for the current one and sends its strings and
//!   balance tokens byte for byte.
//! - Auto applies oldest first; ticking (Space) or typing a Payment cell
//!   switches to the owner's rows; Un-apply all (Alt+U).
//! - Callouts: underpayment (leave open, default Q-P21a / write off, with
//!   the limit hint Q-P21h), overpayment (keep as credit, default Q-P21b /
//!   refund after saving, Q-P21m), "Hold as a customer deposit?" (Q-P21e)
//!   and early-payment discounts (Apply copies the engine's figure,
//!   Q-P21c). Discounts & credits (Alt+D) lists credits and deposits on
//!   hand.
//! - Saved payments open with Prev/Next; an edit sends `expectedUpdatedAt`;
//!   a bounced payment is read-only. Record bounced check (Q-82) opens
//!   [`crate::screens::bounced_check`].
//! - The bank-file banner offers to deposit straight to the bank and match
//!   (U-136).
//!
//! `state` carries counts, flags and the demo's figures as the screen shows
//! them (the scenario's assertions); no names or memos.

mod draw;
pub(crate) mod engine;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use cl_api::{Account, ApiError, IdempotencyKey, PaymentPreview, PaymentRecord, RequestGroup};
use cl_core::ids::{AccountId, ContactId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle, Secret, UserInput};
use cl_model::closing::needs_password;
use cl_model::forms::bounced_check::{BounceDefaults, BouncedCheckFlow};
use cl_model::forms::payment::{
    AmountInput, Editing, FiguresView, LoadedPayment, NothingApplied, Over, PaymentDefaults,
    PaymentForm, PaymentProblem,
};
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink};
use crate::data_bus::Subscription;
use crate::forms::closing_prompt::{ClosingPrompt, Verb};
use crate::screens::card_charge::kit::{Books, NameInput};
use crate::screens::company::form::DateInput;
use crate::windows::Screen;
use engine::Job;

/// The command and window kind.
pub const KIND: &str = "clients.receive_payment";

/// The Receive Payment window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(ReceivePaymentScreen::new(Arc::clone(link), today)) as Box<_>)
}

/// Payment methods (the mockup's list).
pub(crate) const METHODS: [&str; 4] = ["check", "cash", "card", "bank_transfer"];

/// What a save does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum After {
    New,
    Close,
    Stay,
}

/// What the closing-date password is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Save(After),
    Void,
    Bounce,
}

/// An open dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Dialog {
    /// Discounts & credits.
    Credits,
    /// Pick the write-off account (no company default).
    WriteOffAccount,
    /// Pick the discount account (no company default).
    DiscountAccount,
    /// Refund the overpayment of the saved payment (Q-P21m).
    Refund(Refund),
    /// The bounced-check flow.
    Bounce,
    /// Void the open payment.
    Void,
}

/// The refund dialog's fields.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Refund {
    pub payment: TransactionId,
    pub client: ContactId,
    pub amount: String,
    pub from: Option<AccountId>,
    pub method: String,
    pub ref_no: String,
    pub date: CalendarDate,
    pub error: Option<&'static str>,
}

impl std::fmt::Debug for Refund {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Refund")
            .field("payment", &self.payment)
            .finish_non_exhaustive()
    }
}

/// Settings Receive Payment uses (engine company settings, Q-P21q).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Defaults {
    pub payment: PaymentDefaults,
    pub bounce: BounceDefaults,
    pub write_off_limit: Option<Money>,
}

/// A bank-file deposit the banner offers (U-136).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FeedMatch {
    pub id: String,
    pub date: CalendarDate,
    pub amount: Money,
    pub bank: Option<AccountId>,
    pub bank_name: String,
}

/// The Receive Payment window.
pub struct ReceivePaymentScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) bus: Subscription,
    pub(crate) today: CalendarDate,
    pub(crate) started: Instant,
    pub(crate) books: Books,
    pub(crate) form: PaymentForm,
    pub(crate) client_input: NameInput,
    pub(crate) date: DateInput,
    pub(crate) amount_text: String,
    pub(crate) cells: BTreeMap<TransactionId, String>,
    pub(crate) preview: Option<(u64, Job<PaymentPreview>)>,
    pub(crate) save: Option<(After, Job<PaymentRecord>)>,
    pub(crate) save_key: Option<IdempotencyKey>,
    pub(crate) misc: Option<Job<()>>,
    pub(crate) misc_kind: Option<Misc>,
    pub(crate) saved_next: Option<(u32, Money)>,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    pub(crate) error: Option<ApiError>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) problems: Vec<PaymentProblem>,
    pub(crate) saved_count: u32,
    pub(crate) last_saved: Option<PaymentRecord>,
    pub(crate) editing: Option<PaymentRecord>,
    pub(crate) defaults: Defaults,
    pub(crate) defaults_job: Option<Job<Defaults>>,
    pub(crate) saved_list: Vec<TransactionId>,
    pub(crate) list_job: Option<(i8, Job<Vec<TransactionId>>)>,
    pub(crate) open_job: Option<Job<PaymentRecord>>,
    pub(crate) dialog: Option<Dialog>,
    pub(crate) bounce: Option<BouncedCheckFlow>,
    pub(crate) bounce_keys: (IdempotencyKey, IdempotencyKey),
    pub(crate) bounce_job: Option<Job<cl_api::BounceResult>>,
    pub(crate) fee_job: Option<Job<TransactionId>>,
    pub(crate) feed: Vec<FeedMatch>,
    pub(crate) feed_job: Option<Job<Vec<FeedMatch>>>,
    pub(crate) feed_dismissed: Vec<String>,
    pub(crate) write_off_pick: Option<AccountId>,
    /// Each grid row's Apply box id, last frame (Up/Down move between rows).
    pub(crate) grid_boxes: Vec<(TransactionId, eframe::egui::Id)>,
    pub(crate) layout: crate::layout::FrameReport,
    pub(crate) money_style: MoneyStyle,
    pub(crate) dirty: bool,
}

impl std::fmt::Debug for ReceivePaymentScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReceivePaymentScreen")
            .field("form", &self.form)
            .finish_non_exhaustive()
    }
}

/// The i18n key of a save problem.
pub(crate) fn problem_key(p: &PaymentProblem) -> &'static str {
    match p {
        PaymentProblem::NoClient => "payment.problem.no_client",
        PaymentProblem::NoDate => "payment.problem.no_date",
        PaymentProblem::NoDepositTo => "payment.problem.no_deposit_to",
        PaymentProblem::NoAmount => "payment.problem.no_amount",
        PaymentProblem::Updating => "payment.problem.updating",
        PaymentProblem::CantCalculate => "payment.cant_calculate",
        PaymentProblem::AskDeposit => "payment.problem.ask_deposit",
        PaymentProblem::WriteOffAccount => "payment.account.write_off",
        PaymentProblem::DiscountAccount => "payment.account.discount",
        PaymentProblem::Engine(c) if c == "AMOUNT_REQUIRED" => "payment.problem.no_amount",
        PaymentProblem::Engine(_) => "payment.refusal.overapplied",
    }
}

/// The money text the screen shows.
pub(crate) fn shown(m: Money) -> String {
    m.format(MoneyStyle::default())
}

impl ReceivePaymentScreen {
    /// A new, empty payment.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> ReceivePaymentScreen {
        let group = link.group().and_then(Result::ok);
        let bus = link.bus().subscribe();
        let mut s = ReceivePaymentScreen {
            link,
            group,
            bus,
            today,
            started: Instant::now(),
            books: Books::default(),
            form: PaymentForm::new(Some(today), None),
            client_input: NameInput::default(),
            date: DateInput::new(today),
            amount_text: String::new(),
            cells: BTreeMap::new(),
            preview: None,
            save: None,
            save_key: None,
            misc: None,
            misc_kind: None,
            saved_next: None,
            closing_prompt: ClosingPrompt::default(),
            error: None,
            notice: None,
            problems: Vec::new(),
            saved_count: 0,
            last_saved: None,
            editing: None,
            defaults: Defaults::default(),
            defaults_job: None,
            saved_list: Vec::new(),
            list_job: None,
            open_job: None,
            dialog: None,
            bounce: None,
            bounce_keys: (IdempotencyKey::new(), IdempotencyKey::new()),
            bounce_job: None,
            fee_job: None,
            feed: Vec::new(),
            feed_job: None,
            feed_dismissed: Vec::new(),
            write_off_pick: None,
            grid_boxes: Vec::new(),
            layout: crate::layout::FrameReport::default(),
            money_style: MoneyStyle::default(),
            dirty: false,
        };
        if let Some(mut d) = s.link.demo_state() {
            crate::company::demo::receivables::seed(&mut d);
        }
        s.books.start(&s.link, s.group.as_ref());
        s.start_defaults();
        s.start_feed();
        s.poll();
        s
    }

    /// The screen is for `client` (Client Center's Receive Payment).
    pub(crate) fn for_client(&mut self, client: ContactId) {
        self.books.poll();
        self.client_input.text = self.books.contact_name(client).unwrap_or_default();
        self.form.set_client(Some(client));
    }

    fn ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Payments Clearing (Undeposited Funds, the default deposit account,
    /// Q-202).
    pub(crate) fn clearing(&self) -> Option<AccountId> {
        self.books
            .accounts
            .iter()
            .find(|a| a.system_tag.as_deref() == Some("payments_clearing"))
            .map(|a| a.id)
    }

    /// The accounts money can be deposited to: Undeposited Funds first,
    /// then the banks.
    pub(crate) fn deposit_accounts(&self) -> Vec<&Account> {
        let mut v: Vec<&Account> = self
            .books
            .accounts
            .iter()
            .filter(|a| {
                a.is_active
                    && (a.system_tag.as_deref() == Some("payments_clearing")
                        || a.fields.detail_type.as_deref() == Some("bank"))
            })
            .collect();
        v.sort_by_key(|a| a.system_tag.as_deref() != Some("payments_clearing"));
        v
    }

    /// Expense and income accounts (write-off, discount and fee pickers).
    pub(crate) fn accounts_of(&self, types: &[cl_api::AccountType]) -> Vec<&Account> {
        self.books
            .accounts
            .iter()
            .filter(|a| a.is_active && types.contains(&a.fields.account_type))
            .collect()
    }

    pub(crate) fn account_name(&self, id: AccountId) -> String {
        self.books
            .accounts
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.fields.name.clone())
            .unwrap_or_default()
    }

    // ------------------------------------------------------------ reads

    fn start_defaults(&mut self) {
        self.defaults_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| {
                let s = crate::company::demo::receivables::settings(d);
                Ok(Defaults {
                    payment: PaymentDefaults {
                        write_off_account: s.write_off_account,
                        discount_account: s.discount_account,
                    },
                    bounce: BounceDefaults {
                        bank_fee_account: s.nsf_bank_fee_account,
                        fee_income_account: s.nsf_fee_income_account,
                        customer_fee: s.nsf_customer_fee,
                    },
                    write_off_limit: s.write_off_threshold,
                })
            },
            |s| async move {
                let c = s.company_settings().await?;
                Ok(Defaults {
                    payment: PaymentDefaults {
                        write_off_account: c.write_off_account_id,
                        discount_account: c.sales_discount_account_id,
                    },
                    bounce: BounceDefaults {
                        bank_fee_account: c.nsf_bank_fee_account_id,
                        fee_income_account: c.nsf_fee_income_account_id,
                        customer_fee: c.nsf_customer_fee,
                    },
                    write_off_limit: c.write_off_threshold,
                })
            },
        ));
    }

    fn start_feed(&mut self) {
        let has = self.link.is_demo()
            || self
                .link
                .capabilities()
                .has(cl_api::Capabilities::FEED_CUSTOMER_PAYMENTS);
        if !has {
            return;
        }
        self.feed_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| {
                let chart = d
                    .chart
                    .get_or_insert_with(crate::company::demo::chart)
                    .clone();
                // The sample's bank file is Operating Checking's (1010).
                let bank = chart
                    .iter()
                    .find(|a| a.fields.account_number.as_deref() == Some("1010"));
                Ok(crate::company::demo::deposits::feed_deposits()
                    .into_iter()
                    .enumerate()
                    .map(|(i, f)| FeedMatch {
                        id: format!("d6000000-0000-4000-8000-{:012}", i + 1),
                        date: f.date,
                        amount: f.amount,
                        bank: bank.map(|a| a.id),
                        bank_name: bank.map(|a| a.fields.name.clone()).unwrap_or_default(),
                    })
                    .collect())
            },
            |s| async move {
                let chart = s
                    .accounts()
                    .list_all(&cl_api::AccountFilter::default())
                    .await?;
                Ok(s.feed()
                    .deposits_waiting()
                    .await?
                    .into_iter()
                    .map(|f| {
                        let name = f.account_name.clone().unwrap_or_default();
                        FeedMatch {
                            id: f.id.to_string(),
                            date: f.date,
                            amount: f.amount,
                            bank: chart.iter().find(|a| a.fields.name == name).map(|a| a.id),
                            bank_name: name,
                        }
                    })
                    .collect())
            },
        ));
    }

    /// The bank-file deposit that matches the typed amount (an equality
    /// check, the engine re-checks it) within 7 days of the date.
    pub(crate) fn feed_match(&self) -> Option<&FeedMatch> {
        let AmountInput::Typed(amount) = self.form.amount() else {
            return None;
        };
        let date = self.form.date?;
        if self.form.client.is_none() || self.editing.is_some() {
            return None;
        }
        self.feed.iter().find(|f| {
            f.amount == amount
                && (date.days_until(f.date)).abs() <= 7
                && !self.feed_dismissed.contains(&f.id)
        })
    }

    /// Takes every answer that arrived and sends the preview when due.
    pub(crate) fn poll(&mut self) {
        if self.books.poll() && self.form.deposit_to().is_none() {
            let c = self.clearing();
            self.form.set_deposit_to(c);
        }
        if let Some(r) = engine::poll(&mut self.defaults_job)
            && let Ok(d) = r
        {
            self.defaults = d;
        }
        if let Some(Ok(f)) = engine::poll(&mut self.feed_job) {
            self.feed = f;
        }
        if let Some((rev, job)) = self.preview.as_mut()
            && let Some(r) = job.take()
        {
            let rev = *rev;
            self.preview = None;
            match r {
                Ok(p) => {
                    self.form.apply_preview(rev, engine::figures_of(&p));
                }
                Err(_) => self.form.fail_preview(rev),
            }
        }
        if self.preview.is_none()
            && let Some((rev, req)) = self.form.poll_preview(self.ms())
        {
            let api = engine::preview_request(&req);
            let demo_req = api.clone();
            self.preview = Some((
                rev,
                engine::read(
                    &self.link,
                    self.group.as_ref(),
                    move |d| crate::company::demo::receivables::preview(d, &demo_req),
                    move |s| async move { s.customer_payments().preview(&api).await },
                ),
            ));
        }
        self.poll_save();
        self.poll_nav();
        self.poll_misc();
        self.poll_bounce();
    }

    // ------------------------------------------------------------ edits

    pub(crate) fn set_client(&mut self, c: Option<ContactId>) {
        self.form.set_client(c);
        self.cells.clear();
        self.dirty = true;
        self.notice = None;
    }

    pub(crate) fn amount_typed(&mut self) {
        let t = self.amount_text.trim();
        let a = if t.is_empty() {
            AmountInput::Empty
        } else {
            match Money::parse_user(t, UserInput::MONEY) {
                Ok(m) => AmountInput::Typed(m),
                Err(_) => return,
            }
        };
        self.form.set_amount(a);
        self.cells.clear();
        self.dirty = true;
    }

    pub(crate) fn cell_typed(&mut self, invoice: TransactionId) {
        let Some(t) = self.cells.get(&invoice).map(|t| t.trim().to_string()) else {
            return;
        };
        if t.is_empty() {
            self.form.toggle(invoice);
            if self.form.row_choice(invoice) != cl_model::forms::payment::RowChoice::Off {
                self.form.toggle(invoice);
            }
        } else if let Ok(m) = Money::parse_user(&t, UserInput::MONEY) {
            self.form.type_payment(invoice, m);
        }
        self.cells.remove(&invoice);
        self.dirty = true;
    }

    pub(crate) fn toggle(&mut self, invoice: TransactionId) {
        self.form.toggle(invoice);
        self.cells.clear();
        self.dirty = true;
    }

    /// Clear: a new, empty payment.
    pub(crate) fn clear(&mut self) {
        let deposit = self.clearing();
        self.form = PaymentForm::new(Some(self.today), deposit);
        self.client_input = NameInput::default();
        self.date = DateInput::new(self.today);
        self.amount_text.clear();
        self.cells.clear();
        self.editing = None;
        self.save_key = None;
        self.problems.clear();
        self.error = None;
        self.dirty = false;
        self.dialog = None;
        self.bounce = None;
    }

    // ------------------------------------------------------------ saves

    /// Saves; nothing goes until the current preview is back.
    pub(crate) fn start_save(&mut self, after: After, password: Option<Secret>) -> bool {
        if self.save.is_some() || self.is_read_only() {
            return false;
        }
        self.error = None;
        self.notice = None;
        let plan = self.form.save_plan(self.defaults.payment);
        let plan = match plan {
            Ok(p) => p,
            Err(problems) => {
                if problems.contains(&PaymentProblem::WriteOffAccount) {
                    self.dialog = Some(Dialog::WriteOffAccount);
                } else if problems.contains(&PaymentProblem::DiscountAccount) {
                    self.dialog = Some(Dialog::DiscountAccount);
                }
                self.problems = problems;
                return false;
            }
        };
        self.problems.clear();
        let old = self.editing.as_ref().map(|e| e.date);
        if password.is_none() && needs_password(old, plan.date, self.books.closing) {
            self.closing_prompt.set_verb(Verb::Change);
            self.closing_prompt
                .ask(Pending::Save(after), true, self.books.closing);
            return false;
        }
        let new = match engine::new_payment(&plan) {
            Ok(n) => n,
            Err(e) => {
                self.error = Some(e);
                return false;
            }
        };
        let key = self
            .save_key
            .get_or_insert_with(IdempotencyKey::new)
            .clone();
        let label = engine::label(&key);
        #[allow(clippy::single_match_else)]
        let job = match self.editing.as_ref().map(|e| (e.id, e.updated_at)) {
            Some((id, at)) => {
                let demo_new = new.clone();
                let demo_pw = password.clone();
                let opts = cl_api::PaymentWriteOptions {
                    posting: engine::options(password),
                    expected_updated_at: Some(at),
                };
                engine::write(
                    &self.link,
                    self.group.as_ref(),
                    label,
                    move |d| {
                        crate::company::demo::receivables::update(
                            d,
                            id,
                            &demo_new,
                            Some(at),
                            demo_pw.as_ref(),
                        )
                    },
                    move |s| async move { s.customer_payments().update(id, &new, &opts, key).await },
                )
            }
            None => {
                let demo_new = new.clone();
                let demo_pw = password.clone();
                let demo_key = key.to_string();
                engine::write(
                    &self.link,
                    self.group.as_ref(),
                    label,
                    move |d| {
                        crate::company::demo::receivables::receive(
                            d,
                            &demo_new,
                            demo_pw.as_ref(),
                            Some(&demo_key),
                        )
                    },
                    move |s| async move {
                        s.customer_payments()
                            .receive(&new, &engine::options(password), key)
                            .await
                    },
                )
            }
        };
        self.save = Some((after, job));
        self.poll_save();
        true
    }

    fn poll_save(&mut self) {
        let Some((after, job)) = self.save.as_mut() else {
            return;
        };
        let Some(r) = job.take() else {
            return;
        };
        let after = *after;
        self.save = None;
        match r {
            Ok(rec) => self.saved(rec, after),
            Err(e) => self.save_failed(e, after),
        }
    }

    fn saved(&mut self, rec: PaymentRecord, after: After) {
        self.closing_prompt.saved();
        self.save_key = None;
        self.saved_count += 1;
        let accounts = [rec.deposit_to, self.clearing()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        crate::forms::publish_saved(&self.link, &self.bus, &accounts);
        let refund = (self.form.over() == Over::Refund)
            .then(|| match self.form.figures() {
                FiguresView::Ready(f) => f.overpayment,
                _ => None,
            })
            .flatten();
        let next_step = match self.form.figures() {
            FiguresView::Ready(f) => f.pending,
            _ => None,
        };
        self.link.push(CompanyEvent::Toast("payment.saved.toast"));
        self.last_saved = Some(rec.clone());
        self.saved_next = next_step;
        match after {
            After::Close if refund.is_none() => {
                self.dirty = false;
                self.link
                    .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
            }
            After::Stay => self.open_saved(rec.clone()),
            _ => self.clear(),
        }
        if let Some(amount) = refund {
            let rec = self.last_saved.clone().unwrap_or(rec);
            self.open_refund(&rec, amount);
        }
    }

    fn save_failed(&mut self, e: ApiError, after: After) {
        use cl_api::PaymentRefusal as R;
        if !crate::screens::register::keeps_key(&e) {
            self.save_key = None;
        }
        if self
            .closing_prompt
            .failed(&e, Pending::Save(after), true, self.books.closing)
        {
            return;
        }
        self.notice = match cl_api::payment_refusal(&e) {
            Some(R::InvoiceBalanceChanged) => {
                self.form.refresh();
                Some("payment.refusal.balance_changed")
            }
            Some(R::Stale) => Some("payment.refusal.stale"),
            Some(R::Deposited) => Some("payment.refusal.deposited"),
            Some(R::Bounced) => Some("payment.refusal.bounced"),
            Some(R::Refunded) => Some("payment.refusal.refunded"),
            Some(R::DepositApplied) => Some("payment.refusal.deposit_applied"),
            Some(R::CreditNotAvailable | R::DepositNotAvailable) => {
                Some("payment.refusal.credit_not_available")
            }
            Some(R::CreditInUse) => Some("payment.refusal.credit_in_use"),
            Some(R::WriteOffAccountRequired | R::WriteOffAccountInvalid) => {
                Some("payment.refusal.write_off_account")
            }
            Some(R::DiscountAccountRequired | R::DiscountAccountInvalid) => {
                Some("payment.refusal.discount_account")
            }
            Some(R::Overapplied | R::ApplicationsExceedAmount) => {
                Some("payment.refusal.overapplied")
            }
            Some(R::AmountRequired) => Some("payment.problem.no_amount"),
            Some(_) => Some("payment.refusal.other"),
            None => {
                self.error = Some(e);
                None
            }
        };
    }

    pub(crate) fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.closing_prompt.submit() else {
            return;
        };
        let sent = match pending {
            Pending::Save(after) => self.start_save(after, Some(secret)),
            Pending::Void => self.start_void(Some(secret)),
            Pending::Bounce => self.send_bounce(Some(secret)),
        };
        if !sent && self.closing_prompt.dialog_open() {
            self.closing_prompt.nothing_sent();
        }
    }

    // ------------------------------------------------------------ saved payments

    pub(crate) fn is_read_only(&self) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|e| e.is_bounced() || e.is_void())
    }

    /// Prev (-1) or Next (+1): reads the saved list, then opens one.
    pub(crate) fn nav(&mut self, step: i8) {
        if self.list_job.is_some() {
            return;
        }
        self.list_job = Some((
            step,
            engine::read(
                &self.link,
                self.group.as_ref(),
                |d| {
                    Ok(crate::company::demo::receivables::list(d, None)
                        .into_iter()
                        .map(|p| p.id)
                        .collect())
                },
                |s| async move {
                    let page = cl_api::PageRequest::new(200, 0)?;
                    Ok(s.customer_payments()
                        .list(&cl_api::PaymentListQuery::default(), page)
                        .await?
                        .data
                        .into_iter()
                        .map(|p| p.id)
                        .collect())
                },
            ),
        ));
        self.poll_nav();
    }

    fn poll_nav(&mut self) {
        if let Some((step, job)) = self.list_job.as_mut()
            && let Some(r) = job.take()
        {
            let step = *step;
            self.list_job = None;
            if let Ok(ids) = r {
                self.saved_list = ids;
                let at = self
                    .editing
                    .as_ref()
                    .and_then(|e| self.saved_list.iter().position(|i| *i == e.id));
                let next = match (at, step) {
                    (None, s) if s < 0 => self.saved_list.len().checked_sub(1),
                    (None, _) => None,
                    (Some(i), s) if s < 0 => i.checked_sub(1),
                    (Some(i), _) => (i + 1 < self.saved_list.len()).then_some(i + 1),
                };
                match next.and_then(|i| self.saved_list.get(i).copied()) {
                    Some(id) => self.open_id(id),
                    None if step > 0 && at.is_some() => self.clear(),
                    None => self.notice = Some("payment.nav.none"),
                }
            }
        }
        if let Some(job) = self.open_job.as_mut()
            && let Some(r) = job.take()
        {
            self.open_job = None;
            match r {
                Ok(rec) => self.open_saved(rec),
                Err(e) => self.error = Some(e),
            }
        }
    }

    pub(crate) fn open_id(&mut self, id: TransactionId) {
        self.open_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| crate::company::demo::receivables::get(d, id),
            move |s| async move { s.customer_payments().get(id).await },
        ));
        self.poll_nav();
    }

    fn open_saved(&mut self, rec: PaymentRecord) {
        let loaded = LoadedPayment {
            client: rec.customer_id,
            date: rec.date,
            amount: rec.amount,
            // A payment of 0 that only wrote off has no deposit account:
            // the form keeps Undeposited Funds so an edit can send one.
            deposit_to: rec.deposit_to.or_else(|| self.clearing()),
            method: rec.payment_method.clone().unwrap_or_default(),
            ref_no: rec.ref_no.clone().unwrap_or_default(),
            memo: rec.memo.clone().unwrap_or_default(),
            // Its own rows only: a credit of it that another receipt used
            // later (even the same day) is not sent back as cash.
            rows: rec
                .live_applications()
                .filter(|a| a.is_own(rec.id, rec.date))
                .map(|a| {
                    (
                        a.invoice_id,
                        a.amount,
                        a.discount.is_positive().then_some(a.discount),
                        a.write_off.is_positive().then_some(a.write_off),
                    )
                })
                .collect(),
            // D-B1: the credits and deposits it used go back unchanged.
            credits: rec
                .credit_applications
                .iter()
                .map(|c| (c.source_payment_id, c.invoice_id, c.amount))
                .collect(),
            write_off_account: rec.write_off_account_id,
            discount_account: rec.discount_account_id,
            hold: rec.holds_deposit,
        };
        self.form.load_saved(
            Editing {
                id: rec.id,
                updated_at: rec.updated_at,
            },
            loaded,
        );
        self.client_input = NameInput::default();
        self.client_input.text = self.books.contact_name(rec.customer_id).unwrap_or_default();
        self.date = DateInput::new(rec.date);
        self.amount_text = shown(rec.amount);
        self.cells.clear();
        self.editing = Some(rec);
        self.saved_next = None;
        self.dirty = false;
        self.save_key = None;
        self.problems.clear();
    }

    // ------------------------------------------------------------ void, refund

    pub(crate) fn start_void(&mut self, password: Option<Secret>) -> bool {
        let Some(rec) = self.editing.clone() else {
            return false;
        };
        if password.is_none() && needs_password(None, rec.date, self.books.closing) {
            self.closing_prompt.set_verb(Verb::Void);
            self.closing_prompt
                .ask(Pending::Void, true, self.books.closing);
            return false;
        }
        let key = IdempotencyKey::new();
        let label = engine::label(&key);
        let id = rec.id;
        let at = rec.updated_at;
        let pw = password.clone();
        let opts = cl_api::PaymentWriteOptions {
            posting: engine::options(password),
            expected_updated_at: Some(at),
        };
        self.dialog = None;
        self.misc = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label,
            move |d| crate::company::demo::receivables::void(d, id, Some(at), pw.as_ref()),
            move |s| async move { s.customer_payments().void(id, None, &opts, key).await },
        ));
        self.misc_kind = Some(Misc::Void(id));
        self.poll_misc();
        true
    }

    fn open_refund(&mut self, rec: &PaymentRecord, amount: Money) {
        let bank = self
            .books
            .accounts
            .iter()
            .find(|a| a.fields.detail_type.as_deref() == Some("bank"))
            .map(|a| a.id);
        self.dialog = Some(Dialog::Refund(Refund {
            payment: rec.id,
            client: rec.customer_id,
            amount: shown(amount),
            from: bank,
            method: "check".into(),
            ref_no: String::new(),
            date: rec.date,
            error: None,
        }));
    }

    pub(crate) fn send_refund(&mut self) {
        let Some(Dialog::Refund(r)) = self.dialog.clone() else {
            return;
        };
        let (Ok(amount), Some(from)) = (Money::parse_user(&r.amount, UserInput::MONEY), r.from)
        else {
            if let Some(Dialog::Refund(x)) = self.dialog.as_mut() {
                x.error = Some("payment.refund.invalid");
            }
            return;
        };
        let new = cl_api::NewCustomerRefund {
            customer_id: r.client,
            date: r.date,
            from_account: from,
            amount,
            source_payment: r.payment,
            method: Some(r.method.clone()),
            ref_no: Some(r.ref_no.clone()),
            memo: None,
        };
        let key = IdempotencyKey::new();
        let label = engine::label(&key);
        let demo = new.clone();
        self.misc = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label,
            move |d| crate::company::demo::receivables::refund(d, &demo, None).map(|_| ()),
            move |s| async move {
                s.customer_payments()
                    .refund(&new, &engine::options(None), key)
                    .await
                    .map(|_| ())
            },
        ));
        self.misc_kind = Some(Misc::Refund);
        self.poll_misc();
    }

    /// Applies the held deposit `source` to the open invoice (Create
    /// Invoice's card calls the same path).
    pub(crate) fn poll_misc(&mut self) {
        let Some(r) = engine::poll(&mut self.misc) else {
            return;
        };
        let kind = self.misc_kind.take();
        match (r, kind) {
            (Ok(()), Some(Misc::Void(id))) => {
                self.closing_prompt.saved();
                self.notice = Some("payment.voided");
                self.open_id(id);
            }
            (Ok(()), Some(Misc::Refund)) => {
                self.dialog = None;
                self.notice = Some("payment.refund.done");
            }
            (Ok(()), None) => {}
            (Err(e), Some(Misc::Void(_))) => {
                if !self
                    .closing_prompt
                    .failed(&e, Pending::Void, true, self.books.closing)
                {
                    self.notice = Some(match cl_api::payment_refusal(&e) {
                        Some(cl_api::PaymentRefusal::Deposited) => "payment.refusal.deposited",
                        Some(cl_api::PaymentRefusal::Bounced) => "payment.refusal.bounced",
                        Some(cl_api::PaymentRefusal::Refunded) => "payment.refusal.refunded",
                        Some(cl_api::PaymentRefusal::DepositApplied) => {
                            "payment.refusal.deposit_applied"
                        }
                        _ => "payment.refusal.other",
                    });
                }
            }
            (Err(_), Some(Misc::Refund)) => {
                // Q-P21m: the payment stays saved and the credit shows.
                if let Some(Dialog::Refund(x)) = self.dialog.as_mut() {
                    x.error = Some("payment.refund.failed");
                }
            }
            (Err(e), None) => self.error = Some(e),
        }
    }

    // ------------------------------------------------------------ bounce

    pub(crate) fn open_bounce(&mut self) {
        let Some(rec) = self.editing.clone() else {
            self.notice = Some("payment.bounce.open_first");
            return;
        };
        if rec.is_bounced() || rec.is_void() {
            return;
        }
        // A payment of 0 (only written off) took no money: nothing bounces.
        let Some(to) = rec.deposit_to else {
            self.notice = Some("payment.bounce.nothing");
            return;
        };
        let never = rec.deposited_in.is_none() && Some(to) == self.clearing();
        self.bounce = Some(BouncedCheckFlow::new(
            rec.id,
            never,
            self.today,
            self.defaults.bounce,
        ));
        self.bounce_keys = (IdempotencyKey::new(), IdempotencyKey::new());
        self.dialog = Some(Dialog::Bounce);
    }

    /// Sends the flow's next call (the bounce, else the fee invoice).
    pub(crate) fn send_bounce(&mut self, password: Option<Secret>) -> bool {
        crate::screens::bounced_check::send(self, password)
    }

    fn poll_bounce(&mut self) {
        crate::screens::bounced_check::poll(self);
    }
}

/// What the misc write was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Misc {
    Void(TransactionId),
    Refund,
}

impl Screen for ReceivePaymentScreen {
    fn title(&self) -> String {
        crate::i18n::t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        self.draw(ui);
        if self.preview.is_some() || self.form.in_flight() || self.save.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(60));
        } else if !matches!(
            self.form.figures(),
            FiguresView::Ready(_) | FiguresView::Nothing
        ) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            if h && id.as_str() == "form.save_new" {
                self.submit_closing();
            }
            return h;
        }
        match id.as_str() {
            "form.save_new" => {
                self.start_save(After::New, None);
            }
            "form.save_close" => {
                self.start_save(After::Close, None);
            }
            "payment.unapply_all" => {
                self.form.unapply_all();
                self.cells.clear();
                self.dirty = true;
            }
            "payment.discounts_credits" => self.dialog = Some(Dialog::Credits),
            "payment.bounced_check" => self.open_bounce(),
            "form.prev_record" => self.nav(-1),
            "form.next_record" => self.nav(1),
            "edit.void_transaction" => {
                if self.editing.is_some() {
                    self.dialog = Some(Dialog::Void);
                }
            }
            "form.revert" => {
                if self.dialog.is_some() {
                    self.dialog = None;
                } else if !self.dirty {
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

    fn save(&mut self) -> bool {
        self.start_save(After::Stay, None);
        self.poll_save();
        !self.dirty
    }

    fn open_with(&mut self, payload: crate::windows::OpenPayload) -> bool {
        match payload {
            crate::windows::OpenPayload::ReceiveFrom(c) => {
                self.for_client(c);
                true
            }
            _ => false,
        }
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.open_id(txn);
        true
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        true
    }
}

impl ReceivePaymentScreen {
    /// The control channel's view: flags, counts and the shown figures
    /// (the demo's synthetic data).
    #[must_use]
    pub fn state_json(&self) -> Value {
        let figures = match self.form.figures() {
            FiguresView::Nothing => "none",
            FiguresView::Updating => "updating",
            FiguresView::CantCalculate => "failed",
            FiguresView::Ready(_) => "ready",
        };
        let mut v = json!({
            "screen": "receive_payment",
            "loaded": self.books.loaded,
            "mode": if self.editing.is_some() { "edit" } else { "new" },
            "client": self.form.client.is_some(),
            "figures": figures,
            "explicit": self.form.mode() == cl_model::forms::payment::Mode::Explicit,
            "saved_count": self.saved_count,
            "dialog": self.dialog.as_ref().map(|d| match d {
                Dialog::Credits => "credits",
                Dialog::WriteOffAccount => "write_off_account",
                Dialog::DiscountAccount => "discount_account",
                Dialog::Refund(_) => "refund",
                Dialog::Bounce => "bounce",
                Dialog::Void => "void",
            }),
            "notice": self.notice,
            "problems": self.problems.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>(),
            "read_only": self.is_read_only(),
            "bounced": self.editing.as_ref().is_some_and(PaymentRecord::is_bounced),
            "deposited": self.editing.as_ref().is_some_and(|e| e.deposited_in.is_some()),
            "hold": self.form.nothing_applied() == Some(NothingApplied::Hold),
            "write_off": matches!(self.form.under(), cl_model::forms::payment::Under::WriteOff { .. }),
            "banner": self.feed_match().is_some(),
            "feed_linked": self.form.feed_item().is_some(),
            "closing": self.closing_prompt.state_json(true),
        });
        if let FiguresView::Ready(f) = self.form.figures() {
            v["amount"] = json!(shown(f.amount));
            v["rows"] = json!(
                f.rows
                    .iter()
                    .map(|r| json!({"number": r.number, "due": shown(r.balance), "payment": shown(r.payment), "discount": shown(r.discount)}))
                    .collect::<Vec<_>>()
            );
            v["amount_due"] = json!(shown(f.amount_due));
            v["applied"] = json!(shown(f.applied));
            v["discounts_and_credits"] = json!(shown(f.discounts_and_credits));
            v["unapplied"] = json!(shown(f.unapplied));
            v["balance_before"] = json!(shown(f.balance_before));
            v["balance_after"] = json!(shown(f.balance_after));
            v["shortfalls"] = json!(f.shortfalls.len());
            v["within_limit"] = json!(f.shortfalls.iter().any(|s| s.within_limit));
            v["overpayment"] = json!(f.overpayment.map(shown));
            v["ask_deposit"] = json!(f.hold_offered);
            v["credits"] = json!(f.credits.len());
            v["discount_suggestions"] = json!(f.rows.iter().filter(|r| r.suggest_discount).count());
            v["average_days"] = json!(f.average_days);
        }
        if let Some((n, total)) = self.saved_next {
            v["next_step"] = json!({"count": n, "total": shown(total)});
        }
        if let Some(b) = &self.bounce {
            v["bounce"] = json!({
                "step": format!("{:?}", b.step).to_lowercase(),
                "done": b.bounce.is_some(),
                "fee_invoice": b.fee_invoice.is_some(),
                "fee_failed": b.fee_failed,
                "reopened": b.bounce.as_ref().map(|o| o.reopened.iter().map(|(_, m)| shown(*m)).collect::<Vec<_>>()),
            });
        }
        v
    }
}
