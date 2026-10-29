//! Pay Bills (P18, `vendors.pay_bills`, Ctrl+Shift+B; mockup
//! `PayBills.html`): pick the bills to pay, adjust the amounts, then pay
//! them all at once.
//!
//! - The list is the engine's `/bills/payable` for the payment date (its
//!   `discountAvailable` is the only discount the screen offers; nothing
//!   multiplies a percent here). Filters (due on or before / all, vendor)
//!   and the order apply on the screen ([`PayBillsForm`]).
//! - Ticking a bill applies its discount (when "Take discounts" is on) and
//!   the vendor's open credits; Set Discount and Set Credits change them.
//! - Method (owner 2026-10-28): Check (from a bank account, "To be printed"
//!   or numbers now from the Q-P13m proposal) or Credit card (from a card
//!   account, with an optional reference).
//! - The ending balance is the engine's balance with the selected cash
//!   taken off (Q-P18c); after paying, the engine's balance is read again.
//! - The pay is one request (one payment per vendor), keyed under
//!   `idempotency-pay-bills-v1`. A closed-period date takes the closing-date
//!   password. A refusal because the bills changed reads the list again
//!   (Q-P18g).
//! - A pay whose reply was lost (outcome unknown) blocks paying until the
//!   list is read again. The bills and credits it sent are remembered: if
//!   any is gone from the list or its amount changed, the pay went through
//!   and the screen clears it; if all are as they were, the same pay goes
//!   again under the same key.
//!
//! `state` carries counts, flags and ids only: no amounts or names.

mod draw;
pub(crate) mod engine;
mod grid;
pub mod view;

use std::sync::Arc;

use std::fmt;

use cl_api::{
    Account, AccountFilter, ApiError, BillToPay, Capabilities, CheckNumbering,
    ClosedPeriodAuthorization, CreatedPayment, CreditToApply, IdempotencyKey, OrphanLabel,
    PayBillsRequest, PayMethod, PayableFilter, PayableList, PostingOptions, RequestGroup,
};
use cl_core::ids::{AccountId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use cl_grid::{Grid, GridCommand, GridSnapshot, NoLookups};
use cl_model::closing::needs_password;
use cl_model::forms::bill_payment::{OpenCredit, PayBillsForm, PayProblem, PayableRow};
use cl_model::forms::check::propose_number;
use cl_model::register::AccountClass;
use cl_model::{CommandId, Scope};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::forms::closing_prompt::ClosingPrompt;
use crate::grid_env::EnvParts;
use crate::screens::company::form::DateInput;
use crate::windows::Screen;

pub(crate) use engine::{DemoPayBills, LocalPayBills, PayCaps, Setup};

/// The command and window kind.
pub const KIND: &str = "vendors.pay_bills";

/// The Pay Bills window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(PayBillsScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// How the bills are paid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Check,
    Card,
}

/// The open dialog under the selected-bill panel.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Dialog {
    /// Set Discount: the amount typed and the discount account.
    Discount {
        bill: TransactionId,
        text: String,
        account: Option<AccountId>,
        /// Opened because a discount had no account (Q-P18a).
        asked: bool,
    },
    /// Set Credits: one amount text per open credit of the vendor.
    Credits {
        bill: TransactionId,
        texts: Vec<(TransactionId, String)>,
    },
}

/// Ids only: the typed amounts stay out of logs.
impl fmt::Debug for Dialog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Dialog::Discount {
                bill,
                account,
                asked,
                ..
            } => f
                .debug_struct("Discount")
                .field("bill", bill)
                .field("account", account)
                .field("asked", asked)
                .finish_non_exhaustive(),
            Dialog::Credits { bill, texts } => f
                .debug_struct("Credits")
                .field("bill", bill)
                .field("credits", &texts.len())
                .finish_non_exhaustive(),
        }
    }
}

/// A pay whose reply was lost: what its bills owed and its credits had
/// left when it was sent, to tell from the list read again whether it went
/// through.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SentPay {
    bills: Vec<(TransactionId, Money)>,
    credits: Vec<(TransactionId, Money)>,
}

impl fmt::Debug for SentPay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SentPay")
            .field("bills", &self.bills.len())
            .field("credits", &self.credits.len())
            .finish()
    }
}

impl SentPay {
    /// The bills and credits of `req` as `form` lists them now.
    fn of(req: &PayBillsRequest, form: &PayBillsForm) -> SentPay {
        let owed = |b: TransactionId| {
            form.all_rows()
                .iter()
                .find(|r| r.bill == b)
                .map_or(Money::ZERO, |r| r.balance_due)
        };
        let left = |c: TransactionId| {
            form.open_credits()
                .iter()
                .find(|x| x.credit == c)
                .map_or(Money::ZERO, |x| x.open)
        };
        let mut credits: Vec<(TransactionId, Money)> = Vec::new();
        for c in &req.credits {
            if !credits.iter().any(|(id, _)| *id == c.credit) {
                credits.push((c.credit, left(c.credit)));
            }
        }
        SentPay {
            bills: req.bills.iter().map(|b| (b.bill, owed(b.bill))).collect(),
            credits,
        }
    }

    /// Whether `list` shows the pay went through: one of its bills is gone
    /// or owes another amount, or one of its credits has another amount
    /// left (or none).
    fn went_through(&self, list: &PayableList) -> bool {
        let bill_moved = self.bills.iter().any(|(id, owed)| {
            list.bills
                .iter()
                .find(|b| b.id == *id)
                .is_none_or(|b| b.balance_due != *owed)
        });
        let credit_moved = self.credits.iter().any(|(id, left)| {
            list.credits
                .iter()
                .find(|c| c.id == *id)
                .is_none_or(|c| c.balance_due != *left)
        });
        bill_moved || credit_moved
    }
}

/// What a pay sends, for its idempotency key (never the password).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PayIdentity {
    request: PayBillsRequest,
    password: bool,
}

/// The Pay Bills window.
pub struct PayBillsScreen {
    pub(crate) link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    pub(crate) local: Option<Box<dyn LocalPayBills>>,
    pub(crate) form: PayBillsForm,
    pub(crate) revision: u64,
    grid: Grid,
    parts: EnvParts,
    snap: GridSnapshot,
    pub(crate) accounts: Vec<Account>,
    pub(crate) closing: Option<CalendarDate>,
    pub(crate) discount_default: Option<AccountId>,
    setup_task: Option<Task<Setup>>,
    pub(crate) loaded: bool,
    list_task: Option<Task<PayableList>>,
    pub(crate) list_loaded: bool,
    pub(crate) list_failed: bool,
    pub(crate) method: Method,
    pub(crate) bank: Option<AccountId>,
    pub(crate) card: Option<AccountId>,
    pub(crate) date: DateInput,
    pub(crate) through: DateInput,
    pub(crate) show_all: bool,
    pub(crate) print_later: bool,
    pub(crate) first_number: String,
    numbers_task: Option<Task<(Option<u32>, Option<u32>)>>,
    pub(crate) reference: String,
    pub(crate) balance: Option<Money>,
    balance_task: Option<Task<Option<Money>>>,
    pay_task: Option<Task<Vec<CreatedPayment>>>,
    key: Option<(PayIdentity, IdempotencyKey)>,
    /// The pay on its way: its bills (they come off the selection once
    /// paid) and what they owed.
    sent: Option<(Vec<TransactionId>, SentPay)>,
    /// The pay whose outcome is unknown, until the list tells.
    pub(crate) pending: Option<SentPay>,
    /// The first check number last proposed (a typed one is kept).
    proposed_number: Option<String>,
    pub(crate) problems: Vec<PayProblem>,
    pub(crate) error: Option<ApiError>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) unknown: bool,
    pub(crate) last_paid: Option<(usize, usize)>,
    pub(crate) paid_count: u32,
    pub(crate) dialog: Option<Dialog>,
    /// The discount account picked in Set Discount for this run.
    pub(crate) discount_account: Option<AccountId>,
    save_default_task: Option<Task<()>>,
    pub(crate) closing_prompt: ClosingPrompt<()>,
    pub(crate) money_style: cl_core::MoneyStyle,
    bus: crate::data_bus::Subscription,
    published: Vec<AccountId>,
    /// The fixed bars' sizes last frame (Q-265).
    pub(crate) layout: crate::layout::FrameReport,
}

fn options(password: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: password.map(ClosedPeriodAuthorization::with_password),
    }
}

impl PayBillsScreen {
    /// Pay Bills in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> PayBillsScreen {
        let local: Option<Box<dyn LocalPayBills>> = link
            .is_demo()
            .then(|| Box::new(DemoPayBills) as Box<dyn LocalPayBills>);
        PayBillsScreen::with_local(link, tenant, today, local)
    }

    pub(crate) fn with_local(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
        local: Option<Box<dyn LocalPayBills>>,
    ) -> PayBillsScreen {
        let group = link.group().and_then(Result::ok);
        // Bills due within a week (DESIGN.md "Pay Bills"; the mockup's
        // 11/11 for 11/04).
        let through = today.add_days(7).unwrap_or(today);
        let grid = Grid::new("pay_bills", grid::spec())
            .unwrap_or_else(|_| unreachable!("the Pay Bills grid spec is valid (tested)"));
        let mut s = PayBillsScreen {
            bus: link.bus().subscribe(),
            link,
            tenant,
            group,
            today,
            local,
            form: PayBillsForm::new(through),
            revision: 0,
            grid,
            parts: EnvParts::default(),
            snap: GridSnapshot::default(),
            accounts: Vec::new(),
            closing: None,
            discount_default: None,
            setup_task: None,
            loaded: false,
            list_task: None,
            list_loaded: false,
            list_failed: false,
            method: Method::Check,
            bank: None,
            card: None,
            date: DateInput::new(today),
            through: DateInput::new(through),
            show_all: false,
            print_later: true,
            first_number: String::new(),
            numbers_task: None,
            reference: String::new(),
            balance: None,
            balance_task: None,
            pay_task: None,
            key: None,
            sent: None,
            pending: None,
            proposed_number: None,
            problems: Vec::new(),
            error: None,
            notice: None,
            unknown: false,
            last_paid: None,
            paid_count: 0,
            dialog: None,
            discount_account: None,
            save_default_task: None,
            closing_prompt: ClosingPrompt::default(),
            money_style: cl_core::MoneyStyle::default(),
            published: Vec::new(),
            layout: crate::layout::FrameReport::default(),
        };
        s.start_setup();
        s.load_list();
        s.poll();
        s
    }

    pub(crate) fn caps(&self) -> PayCaps {
        if let Some(l) = &self.local {
            return l.caps();
        }
        let c = self.link.capabilities();
        PayCaps {
            discounts: c.has(Capabilities::BILL_DISCOUNTS),
            numbers: c.has(Capabilities::BILL_PAYMENT_CHECK_NUMBER),
            closing: c.has(Capabilities::AP_CLOSING) && c.has(Capabilities::CLOSING_PASSWORD),
        }
    }

    fn start_setup(&mut self) {
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            if let Ok(s) = local.setup(&link) {
                self.setup_task = Some(Task::ready(s));
            } else {
                self.loaded = true;
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.loaded = true;
            return;
        };
        match group.read(&session, |s| async move {
            let accounts = s.accounts().list_all(&AccountFilter::default()).await?;
            let settings = s.company_settings().await.ok();
            Ok(Setup {
                accounts,
                closing: settings.as_ref().and_then(|c| c.lock_date),
                discount_account: settings.and_then(|c| c.bill_discount_account_id),
            })
        }) {
            Ok(p) => self.setup_task = Some(Task::Pending(p)),
            Err(_) => self.loaded = true,
        }
    }

    /// Reads the payable bills for the payment date again.
    pub(crate) fn load_list(&mut self) {
        let filter = PayableFilter {
            vendor: None,
            due_on_or_before: None,
            payment_date: self.date.value,
        };
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            match local.payable(&link, &filter) {
                Ok(l) => self.list_task = Some(Task::ready(l)),
                Err(_) => self.list_failed = true,
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            s.bills().payable_list(&filter).await
        }) {
            self.list_task = Some(Task::Pending(p));
        }
    }

    /// The list is being read (Pay waits for it).
    pub(crate) fn list_task_running(&self) -> bool {
        self.list_task.is_some()
    }

    /// The account the bills are paid from (by the method).
    pub(crate) fn pay_from(&self) -> Option<AccountId> {
        match self.method {
            Method::Check => self.bank,
            Method::Card => self.card,
        }
    }

    pub(crate) fn accounts_of(&self, class: AccountClass) -> Vec<&Account> {
        self.accounts
            .iter()
            .filter(|a| a.is_active && crate::screens::register::class_of(a) == Some(class))
            .collect()
    }

    /// The accounts a discount can post to: income and expense accounts.
    pub(crate) fn discount_accounts(&self) -> Vec<&Account> {
        use cl_api::AccountType as T;
        self.accounts
            .iter()
            .filter(|a| {
                a.is_active
                    && matches!(
                        a.fields.account_type,
                        T::Revenue | T::OtherRevenue | T::Cogs | T::Expense | T::OtherExpense
                    )
            })
            .collect()
    }

    /// The method or the pay-from account changed: a first number typed
    /// for the old account goes, the new account's is proposed.
    pub(crate) fn account_changed(&mut self) {
        self.first_number.clear();
        self.proposed_number = None;
        self.fetch_account_facts();
    }

    /// Reads the pay-from account's balance and, for checks, the numbers.
    pub(crate) fn fetch_account_facts(&mut self) {
        self.balance = None;
        let Some(account) = self.pay_from() else {
            return;
        };
        let check = self.method == Method::Check;
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            self.balance_task = Some(Task::ready(local.balance(&link, account)));
            if check {
                self.numbers_task = Some(Task::ready(local.numbers(&link, account)));
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            Ok(s.accounts().get(account).await?.record.balance)
        }) {
            self.balance_task = Some(Task::Pending(p));
        }
        if check
            && let Ok(p) = group.read(&session, move |s| async move {
                Ok(crate::screens::write_check::check_numbers(&s, account).await)
            })
        {
            self.numbers_task = Some(Task::Pending(p));
        }
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = self.setup_task.as_mut().and_then(Task::try_take) {
            self.setup_task = None;
            self.loaded = true;
            if let Ok(s) = r {
                self.accounts = s.accounts;
                self.closing = s.closing;
                self.discount_default = s.discount_account;
                if self.bank.is_none() {
                    self.bank = self.accounts_of(AccountClass::Bank).first().map(|a| a.id);
                }
                if self.card.is_none() {
                    self.card = self
                        .accounts_of(AccountClass::CreditCard)
                        .first()
                        .map(|a| a.id);
                }
                self.fetch_account_facts();
            }
        }
        if let Some(r) = self.list_task.as_mut().and_then(Task::try_take) {
            self.list_task = None;
            match r {
                Ok(l) => {
                    self.list_loaded = true;
                    self.list_failed = false;
                    self.take_list(&l);
                }
                Err(_) => self.list_failed = true,
            }
        }
        if let Some(r) = self.balance_task.as_mut().and_then(Task::try_take) {
            self.balance_task = None;
            self.balance = r.ok().flatten();
        }
        if let Some(r) = self.numbers_task.as_mut().and_then(Task::try_take) {
            self.numbers_task = None;
            if let Ok((next, highest)) = r {
                // A number the owner typed stays (review, Low); the
                // proposal follows the account while untouched.
                let proposal = propose_number(next, highest).to_string();
                if self.first_number.trim().is_empty()
                    || self.proposed_number.as_deref() == Some(self.first_number.trim())
                {
                    self.first_number.clone_from(&proposal);
                }
                self.proposed_number = Some(proposal);
            }
        }
        if let Some(r) = self.save_default_task.as_mut().and_then(Task::try_take) {
            self.save_default_task = None;
            if r.is_err() {
                self.notice = Some("paybills.discount.default_not_saved");
            }
        }
        if let Some(r) = self.pay_task.as_mut().and_then(Task::try_take) {
            self.pay_task = None;
            match r {
                Ok(p) => self.paid(&p),
                Err(e) => self.pay_failed(e),
            }
        }
    }

    /// The engine's list as the form keeps it. After a pay whose outcome
    /// was unknown, the list tells whether it went through (review fix 1).
    fn take_list(&mut self, l: &PayableList) {
        if let Some(p) = self.pending.take() {
            if p.went_through(l) {
                self.went_through();
            } else {
                self.pending = Some(p);
            }
        }
        let before: Vec<TransactionId> = self.form.all_rows().iter().map(|r| r.bill).collect();
        let rows: Vec<PayableRow> = l
            .bills
            .iter()
            .map(|b| PayableRow {
                bill: b.id,
                vendor: b.contact_id,
                vendor_name: b.contact_name.clone().unwrap_or_default(),
                note: b.memo.clone().filter(|m| !m.trim().is_empty()),
                reference: b.vendor_invoice_number.clone().or_else(|| b.number.clone()),
                due: b.due_date,
                discount_date: b.discount_date,
                terms: b.payment_terms.clone(),
                balance_due: b.balance_due,
                discount_available: b.discount_available.unwrap_or(Money::ZERO),
            })
            .collect();
        let credits: Vec<OpenCredit> = l
            .credits
            .iter()
            .map(|c| OpenCredit {
                credit: c.id,
                vendor: c.contact_id,
                date: c.txn_date,
                label: c
                    .vendor_invoice_number
                    .clone()
                    .or_else(|| c.number.clone())
                    .unwrap_or_default(),
                open: c.balance_due,
            })
            .collect();
        self.form.load(rows, credits);
        self.revision += 1;
        // The rows were read again: the grid follows without "That line
        // is no longer here." (UI review 4). Another set of bills puts the
        // cursor back on the first row.
        let same = self.form.all_rows().iter().map(|r| r.bill).eq(before);
        self.rows_replaced(!same);
    }

    /// The grid follows rows the screen replaced.
    fn rows_replaced(&mut self, reset: bool) {
        let label = String::new();
        let env = self.parts.env(
            &NoLookups,
            &label,
            cl_grid::GridHeight::Rows { min: 1, max: 1 },
        );
        let mut rows = grid::Rows::new(&mut self.form, &mut self.revision);
        if reset {
            self.grid.reset(cl_grid::ModelRef::Edit(&mut rows), &env);
        } else {
            self.grid.rebind(cl_grid::ModelRef::Edit(&mut rows), &env);
        }
        self.snap = self.grid.snapshot(&cl_grid::ModelRef::Edit(&mut rows));
    }

    /// The pay whose outcome was unknown went through: its selection and
    /// key go, and the screen says so.
    fn went_through(&mut self) {
        crate::forms::publish_saved(&self.link, &self.bus, &self.published);
        self.closing_prompt.saved();
        self.form.clear();
        self.key = None;
        self.sent = None;
        self.unknown = false;
        self.dialog = None;
        self.last_paid = None;
        self.paid_count += 1;
        self.notice = Some("paybills.unknown.went_through");
        self.fetch_account_facts();
    }

    /// Saves elsewhere (Q-262): the list and the balance come again.
    fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        let ap = self
            .accounts
            .iter()
            .find(|a| a.system_tag.as_deref() == Some("accounts_payable"))
            .map(|a| a.id);
        if ap.is_some_and(|a| changes.ledger_touches(a)) && self.list_task.is_none() {
            self.load_list();
        }
        if self.pay_from().is_some_and(|a| changes.ledger_touches(a)) {
            self.fetch_account_facts();
        }
        if changes.lists() && self.setup_task.is_none() {
            self.start_setup();
        }
    }

    /// The payment date changed: the engine's discounts are for that date.
    pub(crate) fn date_changed(&mut self) {
        if self.date.value.is_some() {
            self.load_list();
        }
    }

    /// The figure before → after for the pay-from account (Q-P18c): a bank
    /// balance falls by the cash, a card's amount owed rises by it.
    pub(crate) fn preview(&self) -> Option<(Money, Money)> {
        self.form
            .preview(self.balance?, self.method == Method::Card)
    }

    /// The request for the checked plan, or the problems.
    fn request(&mut self) -> Option<PayBillsRequest> {
        let caps = self.caps();
        let plan = match self.form.plan(self.pay_from().is_some(), caps.discounts) {
            Ok(p) => p,
            Err(p) => {
                self.problems = p;
                return None;
            }
        };
        let account = self.pay_from()?;
        let Some(date) = self.date.value else {
            self.notice = Some("paybills.problem.date_missing");
            return None;
        };
        let method = match self.method {
            Method::Card => PayMethod::CreditCard {
                reference: Some(self.reference.trim().to_string()).filter(|r| !r.is_empty()),
            },
            Method::Check if self.print_later => PayMethod::Check(CheckNumbering::PrintLater),
            Method::Check => match self.first_number.trim().parse::<u64>() {
                Ok(first) if first > 0 && caps.numbers => {
                    PayMethod::Check(CheckNumbering::Assign { first })
                }
                Ok(_) | Err(_) if !caps.numbers => {
                    self.notice = Some("paybills.numbers.needs_update");
                    return None;
                }
                _ => {
                    self.notice = Some("paybills.numbers.invalid");
                    return None;
                }
            },
        };
        let any_discount = plan.bills.iter().any(|b| b.discount.is_positive());
        let discount_account = self.discount_account.or(self.discount_default);
        if any_discount && discount_account.is_none() {
            // Q-P18a: ask for the account once; it becomes the default.
            if let Some(b) = plan.bills.iter().find(|b| b.discount.is_positive()) {
                self.open_discount(b.bill, true);
            }
            self.notice = Some("paybills.discount.account_needed");
            return None;
        }
        Some(PayBillsRequest {
            bank_account: account,
            date,
            bills: plan
                .bills
                .iter()
                .map(|b| BillToPay {
                    bill: b.bill,
                    cash: b.cash,
                    discount: b.discount,
                })
                .collect(),
            credits: plan
                .credits
                .iter()
                .map(|c| CreditToApply {
                    credit: c.credit,
                    bill: c.bill,
                    amount: c.amount,
                })
                .collect(),
            discount_account: if any_discount { discount_account } else { None },
            method,
            memo: None,
        })
    }

    /// Checks and pays. Nothing is sent unless the check passes, and
    /// nothing while the list is being read again (its discounts are for
    /// the payment date).
    pub(crate) fn start_pay(&mut self, password: Option<Secret>) -> bool {
        if self.pay_task.is_some() || self.unknown || self.list_task.is_some() {
            return false;
        }
        self.problems.clear();
        self.error = None;
        self.notice = None;
        let Some(req) = self.request() else {
            return false;
        };
        if password.is_none() && needs_password(None, req.date, self.closing) {
            let supported = self.caps().closing;
            if supported {
                self.closing_prompt.ask((), supported, self.closing);
            } else {
                self.closing_prompt.unavailable(supported, self.closing);
            }
            return false;
        }
        let ap = self
            .accounts
            .iter()
            .find(|a| a.system_tag.as_deref() == Some("accounts_payable"))
            .map(|a| a.id);
        self.published = [Some(req.bank_account), req.discount_account, ap]
            .into_iter()
            .flatten()
            .collect();
        let key = self.key_for(&PayIdentity {
            request: req.clone(),
            password: password.is_some(),
        });
        self.sent = Some((
            req.bills.iter().map(|b| b.bill).collect(),
            SentPay::of(&req, &self.form),
        ));
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            let r = local.pay(&link, &req, password.as_ref(), &key);
            match r {
                Ok(p) => {
                    self.paid(&p);
                    return true;
                }
                Err(e) => {
                    self.pay_failed(e);
                    return false;
                }
            }
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.pay_failed(ApiError::NoCompanyOpen);
            return false;
        };
        let label = OrphanLabel {
            what: "orphan.pay_bills",
            key: Some(key.clone()),
        };
        match group.write(&session, label, move |s| async move {
            s.bill_payments().pay(&req, &options(password), key).await
        }) {
            Ok(p) => self.pay_task = Some(Task::Pending(p)),
            Err(_) => self.pay_failed(ApiError::Local(cl_api::LocalValidation::InvalidValue)),
        }
        false
    }

    /// The key for this pay: the stored one when the same pay goes again
    /// after an unknown outcome, a fresh one otherwise.
    fn key_for(&mut self, id: &PayIdentity) -> IdempotencyKey {
        match &self.key {
            Some((d, k)) if d == id => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((id.clone(), k.clone()));
                k
            }
        }
    }

    fn paid(&mut self, payments: &[CreatedPayment]) {
        crate::forms::publish_saved(&self.link, &self.bus, &self.published);
        self.closing_prompt.saved();
        self.key = None;
        self.unknown = false;
        self.pending = None;
        let bills: usize = payments
            .iter()
            .map(|p| usize::try_from(p.bills_paid).unwrap_or(0))
            .sum();
        self.last_paid = Some((payments.len(), bills));
        self.paid_count += 1;
        // Only the bills paid come off; selected bills a filter hid stay.
        match self.sent.take() {
            Some((bills, _)) => self.form.clear_bills(&bills),
            None => self.form.clear(),
        }
        self.revision += 1;
        self.dialog = None;
        self.link.push(CompanyEvent::Toast("paybills.paid.toast"));
        self.load_list();
        self.fetch_account_facts();
    }

    pub(crate) fn pay_failed(&mut self, e: ApiError) {
        if !crate::screens::register::keeps_key(&e) {
            self.key = None;
        }
        let sent = self.sent.take();
        self.pending = if matches!(e, ApiError::OutcomeUnknown { .. }) {
            sent.map(|(_, p)| p)
        } else {
            None
        };
        if self
            .closing_prompt
            .failed(&e, (), self.caps().closing, self.closing)
        {
            return;
        }
        if matches!(e, ApiError::OutcomeUnknown { .. }) {
            // Blocked until the list is read again (PR-1): the bills it paid
            // are gone from it if it went through.
            self.unknown = true;
            self.load_list();
            self.fetch_account_facts();
            return;
        }
        match cl_api::pay_problem(&e) {
            Some(cl_api::PayProblem::BillsChanged) => {
                self.notice = Some("paybills.changed");
                self.load_list();
            }
            Some(cl_api::PayProblem::DiscountAccountRequired) => {
                self.notice = Some("paybills.discount.account_needed");
            }
            Some(cl_api::PayProblem::DiscountAccountInvalid) => {
                self.notice = Some("paybills.discount.account_invalid");
            }
            Some(cl_api::PayProblem::CheckNumberInUse) => {
                self.notice = Some("paybills.numbers.in_use");
                self.fetch_account_facts();
            }
            None => self.error = Some(e),
        }
    }

    /// The owner looked again after an unknown outcome: the list is read
    /// again and paying is allowed (the same key goes with the same pay).
    pub(crate) fn recheck(&mut self) {
        self.unknown = false;
        self.load_list();
        self.fetch_account_facts();
    }

    pub(crate) fn submit_closing(&mut self) {
        let Some(((), secret)) = self.closing_prompt.submit() else {
            return;
        };
        self.start_pay(Some(secret));
        if self.pay_task.is_none() && self.closing_prompt.dialog_open() {
            self.closing_prompt.nothing_sent();
        }
    }

    // ------------------------------------------------------------ dialogs

    /// The bill under the grid's cursor (the selected-bill panel's bill).
    pub(crate) fn cursor_bill(&self) -> Option<TransactionId> {
        let i = self.snap.cursor_row?;
        self.form.visible().get(i).map(|r| r.bill)
    }

    pub(crate) fn open_discount(&mut self, bill: TransactionId, asked: bool) {
        let d = self.form.discount(bill);
        let row = self.form.all_rows().iter().find(|r| r.bill == bill);
        let shown = if d.is_zero() {
            row.map_or(Money::ZERO, |r| r.discount_available)
        } else {
            d
        };
        self.dialog = Some(Dialog::Discount {
            bill,
            text: shown.format(self.money_style),
            account: self.discount_account.or(self.discount_default),
            asked,
        });
    }

    pub(crate) fn open_credits(&mut self, bill: TransactionId) {
        let vendor = self
            .form
            .all_rows()
            .iter()
            .find(|r| r.bill == bill)
            .and_then(|r| r.vendor);
        let on = self.form.credits_on(bill).to_vec();
        let texts = self
            .form
            .credits_for(vendor)
            .iter()
            .map(|c| {
                let used = on
                    .iter()
                    .find(|(id, _)| *id == c.credit)
                    .map_or(Money::ZERO, |(_, m)| *m);
                (c.credit, used.format(self.money_style))
            })
            .collect();
        self.dialog = Some(Dialog::Credits { bill, texts });
    }

    /// Set Discount's OK: the amount on the bill, and the account as this
    /// run's (and, only when the company has none yet, the company's
    /// default, Q-P18a).
    pub(crate) fn apply_discount(&mut self) -> bool {
        let Some(Dialog::Discount {
            bill,
            text,
            account,
            ..
        }) = self.dialog.clone()
        else {
            return false;
        };
        let amount = if text.trim().is_empty() {
            Money::ZERO
        } else {
            match Money::parse_user(&text, cl_core::UserInput::MONEY) {
                Ok(m) if !m.is_negative() => m,
                _ => {
                    self.notice = Some("paybills.discount.invalid");
                    return false;
                }
            }
        };
        if amount.is_positive() && account.is_none() {
            self.notice = Some("paybills.discount.account_needed");
            return false;
        }
        self.form.set_discount(bill, Some(amount));
        self.revision += 1;
        if let Some(a) = account {
            self.discount_account = Some(a);
            if self.discount_default.is_none() {
                self.save_default(a);
            }
        }
        self.dialog = None;
        self.notice = None;
        true
    }

    fn save_default(&mut self, account: AccountId) {
        self.discount_default = Some(account);
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            if local.set_discount_account(&link, account).is_err() {
                self.notice = Some("paybills.discount.default_not_saved");
            }
            return;
        }
        if !self.caps().discounts {
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let label = OrphanLabel {
            what: "orphan.pay_bills_discount_account",
            key: None,
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.set_bill_discount_account(Some(account), IdempotencyKey::new())
                .await
                .map(|_| ())
        }) {
            self.save_default_task = Some(Task::Pending(p));
        }
    }

    /// Set Credits' OK.
    pub(crate) fn apply_credits(&mut self) -> bool {
        let Some(Dialog::Credits { bill, texts }) = self.dialog.clone() else {
            return false;
        };
        let mut credits = Vec::new();
        for (credit, text) in texts {
            if text.trim().is_empty() {
                continue;
            }
            match Money::parse_user(&text, cl_core::UserInput::MONEY) {
                Ok(m) if !m.is_negative() => credits.push((credit, m)),
                _ => {
                    self.notice = Some("paybills.credits.invalid");
                    return false;
                }
            }
        }
        self.form.set_credits(bill, credits);
        self.revision += 1;
        self.dialog = None;
        self.notice = None;
        true
    }

    /// Go to bill: Enter Bill opens the bill.
    pub(crate) fn go_to_bill(&self, bill: TransactionId) {
        self.link.push(CompanyEvent::OpenRecord(
            CommandId::new(crate::screens::bill::KIND),
            bill,
        ));
    }

    fn grid_command(&mut self, cmd: GridCommand) -> bool {
        let label = String::new();
        let env = self.parts.env(
            &NoLookups,
            &label,
            cl_grid::GridHeight::Rows { min: 1, max: 1 },
        );
        let mut rows = grid::Rows::new(&mut self.form, &mut self.revision);
        let handled = self
            .grid
            .command(cmd, cl_grid::ModelRef::Edit(&mut rows), &env)
            .0
            .yes();
        self.snap = self.grid.snapshot(&cl_grid::ModelRef::Edit(&mut rows));
        handled
    }

    fn state_json(&self) -> Value {
        let totals = self.form.totals();
        let mut v = json!({
            "screen": "pay_bills",
            "layout": self.layout.json(),
            "company_open": self.tenant.is_some(),
            "loaded": self.loaded,
            "list_loaded": self.list_loaded,
            "list_failed": self.list_failed,
            "listed": totals.map(|t| t.listed),
            "selected": totals.map(|t| t.selected),
            "take_discounts": self.form.take_discounts(),
            "hidden_selected": totals.map(|t| t.hidden),
            "pending": self.pending.is_some(),
            "show": if self.show_all { "all" } else { "due" },
            "sort": format!("{:?}", self.form.sort).to_lowercase(),
            "vendor_filtered": self.form.vendor.is_some(),
            "method": match self.method { Method::Check => "check", Method::Card => "card" },
            "numbering": if self.print_later { "print_later" } else { "assign" },
            "first_number": self.first_number.parse::<u64>().ok(),
            "pay_from": self.pay_from().map(|a| a.to_string()),
            "balance_shown": self.balance.is_some(),
            "date": self.date.value.map(CalendarDate::to_wire),
            "paying": self.pay_task.is_some(),
            "paid_count": self.paid_count,
            "last_payments": self.last_paid.map(|p| p.0),
            "last_bills": self.last_paid.map(|p| p.1),
            "problems": self.problems.iter().map(|p| p.key().0).collect::<Vec<_>>(),
            "error": self.error.is_some(),
            "notice": self.notice,
            "unknown": self.unknown,
            "dialog": match &self.dialog {
                Some(Dialog::Discount { .. }) => Some("discount"),
                Some(Dialog::Credits { .. }) => Some("credits"),
                None => None,
            },
            "discount_account_set": self.discount_account.or(self.discount_default).is_some(),
            "cursor_row": self.snap.cursor_row,
            "focused": self.snap.focused,
            "published": self.published.iter().map(ToString::to_string).collect::<Vec<_>>(),
        });
        if let (Value::Object(m), Value::Object(c)) =
            (&mut v, self.closing_prompt.state_json(self.caps().closing))
        {
            m.extend(c);
        }
        v
    }
}

impl Screen for PayBillsScreen {
    fn title(&self) -> String {
        crate::i18n::t("cmd.vendors.pay_bills")
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui) {
        self.parts = EnvParts::capture(ui.ctx());
        self.money_style = self.parts.frame.money_style;
        self.poll();
        self.hear_changes();
        self.draw(ui);
        let busy = self.setup_task.is_some()
            || self.list_task.is_some()
            || self.balance_task.is_some()
            || self.numbers_task.is_some()
            || self.pay_task.is_some()
            || self.save_default_task.is_some();
        if busy {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        // The grid takes Esc as `form.revert` (plan P10 §2.3).
        Scope::Form
    }

    fn dirty(&self) -> bool {
        self.form.totals().is_some_and(|t| t.selected > 0)
    }

    fn save_unconfirmed(&self) -> bool {
        self.unknown
    }

    fn save(&mut self) -> bool {
        self.start_pay(None)
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            return h;
        }
        match id.as_str() {
            KIND => true,
            // Alt+S pays (the mockup's status bar), Alt+N too.
            "form.save_close" | "form.save_new" => {
                self.start_pay(None);
                true
            }
            "form.clear" => {
                self.form.clear();
                self.revision += 1;
                true
            }
            "form.revert" => {
                if self.dialog.take().is_some() {
                    return true;
                }
                self.grid_command(GridCommand::Revert)
            }
            // Demo only: void the demo's bill payments (the scenario's
            // second theme run starts from the same list).
            "dev.demo_pay_bills_reset" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                crate::company::demo::pay_bills::reset(&mut d);
                drop(d);
                self.discount_default = None;
                self.discount_account = None;
                self.last_paid = None;
                self.form.clear();
                self.form.set_take_discounts(true);
                self.form.vendor = None;
                self.pending = None;
                self.sent = None;
                self.key = None;
                self.unknown = false;
                self.notice = None;
                self.first_number.clear();
                self.proposed_number = None;
                self.show_all = false;
                if let Some(d) = self.through.value {
                    self.form.show = cl_model::forms::bill_payment::ShowBills::DueOnOrBefore(d);
                }
                self.method = Method::Check;
                self.print_later = true;
                self.reference.clear();
                self.revision += 1;
                self.load_list();
                self.fetch_account_facts();
                true
            }
            "form.prev_record" => self.grid_command(GridCommand::PageUp),
            "form.next_record" => self.grid_command(GridCommand::PageDown),
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
mod tests;
