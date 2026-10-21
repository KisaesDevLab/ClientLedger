//! Loan Payment (P14 lane C, `banking.loan_payment`; Q-62, Q-P14p,
//! Q-P14q), on the Write Check pattern (Q-261).
//!
//! The owner types the payment and the interest from the lender's
//! statement; the principal is `cl_model::forms::loan_payment::principal`
//! (`payment − interest − Σextras`, exact) and is shown read-only. The
//! payment posts as a check or an online payment from the bank: principal
//! to the loan, interest to the interest account, plus any extra lines; the
//! posted lines always add up to the payment. After the save the loan's
//! balance is the engine's, compared with the lender's balance when the
//! owner typed one ("Matches your lender's statement ✓" / "Differs by X").
//! No schedules and no amortization (Q-62). Choosing the lender fills the
//! accounts from the last payment to them, never the amounts (Q-191).

use std::sync::Arc;

use cl_api::{
    ApiError, BankPosting, CardChargeDraft, CheckDraft, LocalValidation, RequestGroup, SplitLine,
    Transaction, TxnType, Versioned,
};
use cl_core::ids::{AccountId, ContactId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle, Secret};
use cl_grid::GridHeight;
use cl_model::amount_entry::AmountRules;
use cl_model::forms::lines::{FormLine, FormLines};
use cl_model::forms::loan_payment::{
    CheckedLoanPayment, LenderMatch, LoanMethod, LoanPaymentForm, LoanPosting, LoanProblem, check,
    lender_match,
};
use cl_model::forms::{FormState, SaveIntent};
use cl_model::register::AccountClass;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Ui};
use serde_json::{Value, json};

use crate::company::demo::register::DemoPosting;
use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::forms::lines_grid::{LinesGrid, LinesSpec};
use crate::i18n::{t, t_with};
use crate::screens::card_charge::kit::{
    self, AmountInput, Balances, Books, MessageClick, NameInput, NameOutcome, Outbound, Pending,
    SaveFlow, Sent, Settled,
};
use crate::screens::card_charge::records::{self, RecordForm, RecordScope, Records};
use crate::screens::company::form::{self, DateInput};
use crate::screens::register::lists::RegisterLists;
use crate::theme::Tokens;
use crate::windows::{PromptAnswer, Screen};

/// The kind of record this form writes: an expense (a check or an online
/// payment) paid from the bank or card.
const LOAN_TYPES: &[TxnType] = &[TxnType::Expense];

/// The command and window kind.
pub const KIND: &str = "banking.loan_payment";

/// The extra lines (fees, escrow): Account · Amount · Memo.
pub const EXTRAS: LinesSpec = LinesSpec {
    customer: false,
    billable: false,
    tags: false,
    tag_chips: false,
    height: GridHeight::Rows { min: 2, max: 5 },
    ..LinesSpec::all("loan_payment/extras", "loan.extras.label")
};

/// The Loan Payment window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(LoanPaymentScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// Header fields (undo merges typing within one field).
mod field {
    pub const LOAN: u64 = 1;
    pub const LENDER: u64 = 2;
    pub const PAY_FROM: u64 = 3;
    pub const NUMBER: u64 = 4;
    pub const DATE: u64 = 5;
    pub const PAYMENT: u64 = 6;
    pub const INTEREST: u64 = 7;
    pub const INTEREST_ACCOUNT: u64 = 8;
    pub const LENDER_BALANCE: u64 = 9;
    pub const MEMO: u64 = 10;
}

/// The texts being typed.
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub date: DateInput,
    pub lender: NameInput,
    pub payment: AmountInput,
    pub interest: AmountInput,
    pub lender_balance: AmountInput,
    pub rev: u64,
}

/// The accounts of the last payment to a lender (never its amounts).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LoanPrefill {
    pub loan: Option<AccountId>,
    pub interest_account: Option<AccountId>,
    pub pay_from: Option<AccountId>,
}

/// After a save: the lender's balance to compare with the engine's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LenderCheck {
    pub loan: AccountId,
    pub lender_owed: Money,
}

/// The Loan Payment window.
pub struct LoanPaymentScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    pub(crate) books: Books,
    pub(crate) balances: Balances,
    /// The header (its `extras` stay empty: the grid holds them).
    pub(crate) header: FormState<LoanPaymentForm>,
    pub(crate) extras: LinesGrid<RegisterLists>,
    pub(crate) inputs: Inputs,
    pub(crate) flow: SaveFlow,
    pub(crate) problems: Vec<LoanProblem>,
    prefill_task: Option<Task<Option<LoanPrefill>>>,
    pub(crate) prefill_applied: bool,
    /// The comparison waiting for the engine's balance after a save.
    pub(crate) lender_check: Option<LenderCheck>,
    pub(crate) lender_result: Option<LenderMatch>,
    pub(crate) bus: crate::data_bus::Subscription,
    lists_stale: bool,
    after_frame: Option<CommandId>,
    pub(crate) money_style: MoneyStyle,
    /// Prev/Next and the saved record being read.
    pub(crate) records: Records,
    /// The field that had the keyboard (Esc reverts it).
    pub(crate) focused: Option<u64>,
}

/// The accounts a saved payment used: the line it was paid from (a bank or
/// card credit), the loan (a liability debit) and the interest (an expense
/// debit). Amounts are never read (Q-191).
pub(crate) fn prefill_of(txn: &Transaction, accounts: &[cl_api::Account]) -> LoanPrefill {
    let mut p = LoanPrefill::default();
    for l in &txn.lines {
        let account = accounts.iter().find(|a| a.id == l.account_id);
        let class = account.and_then(crate::screens::register::class_of);
        let expense = account.is_some_and(|a| {
            matches!(
                a.fields.account_type,
                cl_api::AccountType::Expense | cl_api::AccountType::OtherExpense
            )
        });
        if l.credit.is_positive() {
            if matches!(class, Some(AccountClass::Bank | AccountClass::CreditCard)) {
                p.pay_from = p.pay_from.or(Some(l.account_id));
            }
        } else if l.debit.is_positive() {
            if class == Some(AccountClass::OtherLiability) {
                p.loan = p.loan.or(Some(l.account_id));
            } else if expense {
                p.interest_account = p.interest_account.or(Some(l.account_id));
            }
        }
    }
    p
}

impl LoanPaymentScreen {
    /// A new loan payment in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> LoanPaymentScreen {
        let group = link.group().and_then(Result::ok);
        let books = Books::default();
        let extras = LinesGrid::new(&EXTRAS, Arc::clone(&books.lists), Vec::new());
        let mut s = LoanPaymentScreen {
            link,
            tenant,
            group,
            today,
            books,
            balances: Balances::default(),
            header: FormState::new(LoanPaymentForm::new(Some(today))),
            extras,
            inputs: Inputs {
                date: DateInput::new(today),
                lender: NameInput::default(),
                payment: AmountInput::default(),
                interest: AmountInput::default(),
                lender_balance: AmountInput::default(),
                rev: u64::MAX,
            },
            flow: SaveFlow::default(),
            problems: Vec::new(),
            prefill_task: None,
            prefill_applied: false,
            lender_check: None,
            lender_result: None,
            bus: crate::data_bus::Subscription::detached(),
            lists_stale: false,
            after_frame: None,
            money_style: MoneyStyle::default(),
            records: Records::default(),
            focused: None,
        };
        s.bus = s.link.bus().subscribe();
        s.books.start(&s.link, s.group.as_ref());
        s.poll();
        s
    }

    /// The whole form: the header with the grid's extra lines.
    #[must_use]
    pub(crate) fn compose(&self) -> LoanPaymentForm {
        let mut f = self.header.current().clone();
        f.extras = self.extras.value().clone();
        f
    }

    /// Loan accounts: liabilities other than cards and payables.
    pub(crate) fn loans(&self) -> Vec<(AccountId, String)> {
        self.books.accounts_of(&[AccountClass::OtherLiability])
    }

    /// Where the payment can come from: banks, and cards for online
    /// payments.
    pub(crate) fn pay_from_accounts(&self) -> Vec<(AccountId, String)> {
        self.books
            .accounts_of(&[AccountClass::Bank, AccountClass::CreditCard])
    }

    /// Expense accounts for the interest.
    pub(crate) fn interest_accounts(&self) -> Vec<(AccountId, String)> {
        self.books
            .accounts
            .iter()
            .filter(|a| {
                a.is_active
                    && matches!(
                        a.fields.account_type,
                        cl_api::AccountType::Expense | cl_api::AccountType::OtherExpense
                    )
            })
            .map(|a| (a.id, crate::screens::register::account_text(a)))
            .collect()
    }

    pub(crate) fn poll(&mut self) {
        if self.books.poll() {
            self.extras.set_lists(Arc::clone(&self.books.lists));
            self.inputs.rev = u64::MAX;
            if self.header.current().pay_from.is_none()
                && let Some((bank, _)) = self
                    .books
                    .accounts_of(&[AccountClass::Bank])
                    .first()
                    .cloned()
            {
                let mut f = self.header.current().clone();
                f.pay_from = Some(bank);
                self.header.load(f);
            }
        }
        self.balances.poll();
        if let Some(r) = self.prefill_task.as_mut().and_then(Task::try_take) {
            self.prefill_task = None;
            if let Ok(Some(p)) = r {
                self.apply_prefill(p);
            }
        }
        let (saved, voided) = self.flow.poll();
        match saved {
            Some(Ok(r)) => self.saved(r),
            Some(Err(e)) => self.save_failed(e),
            None => {}
        }
        match voided {
            Some(Ok(())) => self.voided(),
            Some(Err(e)) => {
                let closing = self.books.closing;
                self.flow.failed(&self.link, e, closing);
            }
            None => {}
        }
        // The engine's balance after the save, against the lender's.
        self.poll_lender();
        self.poll_records();
    }

    fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if changes.lists() {
            self.lists_stale = true;
        }
        if self.lists_stale && !self.is_dirty() && !self.books.busy() {
            self.lists_stale = false;
            self.books.start(&self.link, self.group.as_ref());
        }
        if let Some(loan) = self.header.current().loan
            && changes.ledger_touches(loan)
        {
            self.balances.fetch(&self.link, self.group.as_ref(), loan);
        }
    }

    /// The header texts from the form.
    pub(crate) fn sync_inputs(&mut self) {
        let f = self.header.current();
        let style = self.money_style;
        if let Some(d) = f.date {
            self.inputs.date = DateInput::new(d);
        }
        self.inputs.lender.text = f
            .lender
            .and_then(|p| self.books.lists.name_text(p))
            .unwrap_or_default()
            .to_string();
        self.inputs.lender.problem = false;
        self.inputs.payment.show(f.payment, style);
        self.inputs.interest.show(f.interest, style);
        self.inputs.lender_balance.show(f.lender_balance, style);
        self.inputs.rev = self.header.revision();
    }

    /// The form has unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.header.is_dirty() || self.extras.dirty()
    }

    fn busy(&self) -> bool {
        self.books.busy()
            || self.balances.busy()
            || self.flow.busy()
            || self.prefill_task.is_some()
            || self.records.busy()
    }

    pub(crate) fn set_loan(&mut self, loan: AccountId) {
        self.header.edit(field::LOAN, |h| h.loan = Some(loan));
        self.balances.fetch(&self.link, self.group.as_ref(), loan);
    }

    /// A lender was picked: their accounts from the last payment (Q-191).
    pub(crate) fn pick_lender(&mut self, out: NameOutcome) {
        match out {
            NameOutcome::Picked(id) => {
                let same = self.header.current().lender == Some(id);
                self.header.edit(field::LENDER, |h| h.lender = Some(id));
                self.inputs.lender.text = self
                    .books
                    .lists
                    .name_text(id)
                    .unwrap_or_default()
                    .to_string();
                // Prefill never runs while a saved payment is open.
                if !same && self.flow.editing.is_none() {
                    self.start_prefill(id);
                }
            }
            NameOutcome::Cleared => {
                self.header.edit(field::LENDER, |h| h.lender = None);
            }
            NameOutcome::None => return,
        }
        self.inputs.rev = self.header.revision();
    }

    fn start_prefill(&mut self, lender: ContactId) {
        if let Some(mut demo) = self.link.demo_state() {
            let last = crate::company::demo::checks::contact_transactions(&mut demo, lender, 5)
                .into_iter()
                .find(|t| t.txn_type == cl_api::TxnType::Expense)
                .and_then(|t| crate::company::demo::checks::transaction(&mut demo, t.id));
            drop(demo);
            let p = last.map(|txn| prefill_of(&txn, &self.books.accounts));
            self.prefill_task = Some(Task::ready(p));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let accounts = self.books.accounts.clone();
        if let Ok(p) = group.read(&session, move |s| async move {
            let page = cl_api::PageRequest::new(10, 0)?;
            let rows = s.contacts().transactions(lender, page).await?;
            let Some(row) = rows
                .data
                .into_iter()
                .find(|t| t.txn_type == cl_api::TxnType::Expense)
            else {
                return Ok(None);
            };
            let txn = s.transactions().get(row.id).await?.record;
            Ok(Some(prefill_of(&txn, &accounts)))
        }) {
            self.prefill_task = Some(Task::Pending(p));
        }
    }

    /// Fills only the accounts still blank; inactive ones are dropped.
    fn apply_prefill(&mut self, p: LoanPrefill) {
        let active = |id: Option<AccountId>| {
            id.filter(|a| {
                self.books
                    .accounts
                    .iter()
                    .any(|x| x.id == *a && x.is_active)
            })
        };
        let (loan, interest, from) = (
            active(p.loan),
            active(p.interest_account),
            active(p.pay_from),
        );
        let h = self.header.current();
        let fill_loan = h.loan.is_none() && loan.is_some();
        let fill_from = h.pay_from.is_none() && from.is_some();
        if fill_loan || (h.interest_account.is_none() && interest.is_some()) || fill_from {
            self.header.edit_step(|h| {
                if h.loan.is_none() {
                    h.loan = loan;
                }
                if h.interest_account.is_none() {
                    h.interest_account = interest;
                }
                // The owner's (or the default) Pay From stays.
                if h.pay_from.is_none() {
                    h.pay_from = from;
                }
            });
            self.prefill_applied = true;
        }
        if fill_loan && let Some(l) = loan {
            self.balances.fetch(&self.link, self.group.as_ref(), l);
        }
    }

    fn amount_left(&mut self, which: u64) {
        let style = self.money_style;
        let input = match which {
            field::PAYMENT => &mut self.inputs.payment,
            field::INTEREST => &mut self.inputs.interest,
            _ => &mut self.inputs.lender_balance,
        };
        if let Settled::Value(m) = input.settle(style, AmountRules::MONEY) {
            self.header.edit(which, |h| match which {
                field::PAYMENT => h.payment = m,
                field::INTEREST => h.interest = m,
                _ => h.lender_balance = m,
            });
            self.inputs.rev = self.header.revision();
        }
    }

    fn commit_header(&mut self) -> bool {
        if self.inputs.rev != self.header.revision() {
            return true;
        }
        for (which, input) in [
            (field::PAYMENT, self.header.current().payment),
            (field::INTEREST, self.header.current().interest),
            (field::LENDER_BALANCE, self.header.current().lender_balance),
        ] {
            let shown = input
                .map(|m| m.format(self.money_style))
                .unwrap_or_default();
            let typed = match which {
                field::PAYMENT => &self.inputs.payment.text,
                field::INTEREST => &self.inputs.interest.text,
                _ => &self.inputs.lender_balance.text,
            };
            if typed.trim() != shown.trim() {
                self.amount_left(which);
            }
        }
        let lender_shown = self
            .header
            .current()
            .lender
            .and_then(|p| self.books.lists.name_text(p))
            .unwrap_or_default()
            .to_string();
        if self.inputs.lender.text.trim() != lender_shown.trim() {
            let lists = Arc::clone(&self.books.lists);
            let out = kit::resolve_name(&mut self.inputs.lender, &lists);
            self.pick_lender(out);
        }
        self.inputs.payment.problem.is_none()
            && self.inputs.interest.problem.is_none()
            && self.inputs.lender_balance.problem.is_none()
            && !self.inputs.lender.problem
    }

    /// The checked payment as the engine posting and the demo's copy,
    /// field by field (no arithmetic: the lines come from `check`).
    pub(crate) fn outbound(&self, c: &CheckedLoanPayment) -> Result<Outbound, LocalValidation> {
        let lines: Vec<SplitLine> = c
            .lines
            .iter()
            .map(|l| SplitLine {
                account: l.account,
                amount: l.amount,
                memo: l.memo.clone(),
                contact: None,
                billable: false,
                tag: None,
            })
            .collect();
        let lender_name = c.lender.and_then(|l| self.books.contact_name(l));
        let (posting, kind, number) = match c.posting {
            LoanPosting::Check(number) => (
                BankPosting::check(CheckDraft {
                    bank: c.pay_from,
                    date: c.date,
                    payee: c.lender,
                    payee_name: lender_name.clone().unwrap_or_default(),
                    check_number: number,
                    print_later: false,
                    payee_address: None,
                    memo: c.memo.clone(),
                    printed_memo: c.memo.clone(),
                    tag_ids: Vec::new(),
                    draft_attachment: None,
                    amount: c.payment,
                    lines,
                })?,
                "check",
                number.map(|n| n.to_string()).unwrap_or_default(),
            ),
            LoanPosting::Online => (
                BankPosting::card_charge(CardChargeDraft {
                    pay_from: c.pay_from,
                    date: c.date,
                    number: None,
                    payee: c.lender,
                    memo: c.memo.clone(),
                    lines,
                })?,
                "card_charge",
                String::new(),
            ),
        };
        let demo = DemoPosting {
            kind,
            date: c.date,
            number,
            contact: lender_name,
            memo: c.memo.clone().unwrap_or_default(),
            own: c.pay_from,
            own_debit: false,
            lines: c.lines.iter().map(|l| (l.account, l.amount)).collect(),
        };
        Ok(Outbound { posting, demo })
    }

    /// Checks the form and starts the save; nothing is sent unless the
    /// posted lines equal the payment exactly.
    pub(crate) fn start_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        if !self.flow.idle() && password.is_none() {
            return false;
        }
        if !self.commit_header() || !self.extras.commit() {
            return false;
        }
        self.problems.clear();
        self.flow.reset_messages();
        if !self.is_dirty() {
            self.follow_up(intent);
            return true;
        }
        let form = self.compose();
        let checked = match check(&form, &self.books.tag_list) {
            Ok(c) => c,
            Err(problems) => {
                self.problems = problems;
                return false;
            }
        };
        let out = match self.outbound(&checked) {
            Ok(o) => o,
            Err(v) => {
                self.flow.error = Some(ApiError::Local(v));
                return false;
            }
        };
        let mut touched = vec![checked.pay_from, checked.loan];
        touched.extend(checked.lines.iter().map(|l| l.account));
        self.flow.published = touched;
        self.lender_result = None;
        self.lender_check = form.lender_balance.map(|lender_owed| LenderCheck {
            loan: checked.loan,
            lender_owed,
        });
        let closing = self.books.closing;
        match self.flow.send(
            &self.link,
            self.group.as_ref(),
            out,
            intent,
            password,
            closing,
        ) {
            Sent::Saved(r) => {
                self.saved(r);
                true
            }
            Sent::Queued => {
                let lists = Arc::clone(&self.books.lists);
                let _ = self.extras.form_mut().begin_save(intent, &*lists);
                let _ = self.header.begin_save(intent);
                false
            }
            Sent::Asked | Sent::Failed => {
                self.lender_check = None;
                false
            }
        }
    }

    fn saved(&mut self, r: kit::SavedRecord) {
        let intent = self.flow.saved(r);
        crate::forms::publish_saved(&self.link, &self.bus, &self.flow.published);
        self.problems.clear();
        let lists = Arc::clone(&self.books.lists);
        let header = self.header.current().clone();
        if self.header.is_saving() {
            self.header.save_ok(header);
        } else {
            self.header.load(header);
        }
        let lines = self.extras.value().clone();
        self.extras.form_mut().save_ok(lines.clone(), &*lists);
        self.extras.form_mut().load(lines, &*lists);
        // The engine's balance after the save (never `before − principal`).
        if let Some(loan) = self.header.current().loan {
            self.balances.fetch(&self.link, self.group.as_ref(), loan);
        }
        self.records.forget();
        self.follow_up(intent);
        self.poll_lender();
        self.after_saved();
    }

    fn poll_lender(&mut self) {
        if let Some(c) = self.lender_check
            && !self.balances.busy()
            && let Some(owed) = self
                .balances
                .shown(c.loan, Some(AccountClass::OtherLiability))
        {
            self.lender_result = lender_match(owed, c.lender_owed);
            self.lender_check = None;
        }
    }

    fn save_failed(&mut self, e: ApiError) {
        self.lender_check = None;
        let closing = self.books.closing;
        let failure = cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        };
        self.header.save_failed(failure.clone());
        self.extras.form_mut().save_failed(failure);
        self.flow.failed(&self.link, e, closing);
    }

    fn follow_up(&mut self, intent: SaveIntent) {
        match intent {
            SaveIntent::SaveAndNew => self.new_form(true),
            SaveIntent::SaveAndClose => {
                self.link
                    .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
            }
            SaveIntent::Save => {}
        }
    }

    /// A blank form; Save & New keeps the accounts, the lender and the
    /// method, never the amounts.
    pub(crate) fn new_form(&mut self, keep: bool) {
        let c = self.header.current().clone();
        let mut f = LoanPaymentForm::new(Some(if keep {
            c.date.unwrap_or(self.today)
        } else {
            self.today
        }));
        if keep {
            f.loan = c.loan;
            f.lender = c.lender;
            f.pay_from = c.pay_from;
            f.interest_account = c.interest_account;
            f.method = match c.method {
                LoanMethod::Check(_) => LoanMethod::Check(String::new()),
                LoanMethod::Online => LoanMethod::Online,
            };
        } else {
            f.pay_from = c.pay_from;
        }
        self.header.load(f);
        let lists = Arc::clone(&self.books.lists);
        let fresh =
            FormLines::from_lines(Vec::new(), &mut self.extras.form_mut().adapter_mut().keys);
        self.extras.form_mut().load(fresh, &*lists);
        self.problems.clear();
        self.prefill_applied = false;
        self.inputs.lender = NameInput::default();
        self.flow.new_record();
        self.records.new_record();
        self.sync_inputs();
    }

    pub(crate) fn clear(&mut self) {
        if self.flow.busy() {
            return;
        }
        self.flow.reset_messages();
        self.flow.last_action = Some("clear");
        self.lender_result = None;
        self.new_form(false);
    }

    fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.flow.closing_prompt.submit() else {
            return;
        };
        let sent = match pending {
            Pending::Save(intent) => {
                self.start_save(intent, Some(secret));
                self.flow.task.is_some() || !self.flow.closing_prompt.dialog_open()
            }
            Pending::Void(reason) => {
                self.flow.void_reason = reason;
                self.void_now(Some(secret));
                true
            }
        };
        if !sent {
            self.flow.closing_prompt.nothing_sent();
        }
    }

    pub(crate) fn void_now(&mut self, password: Option<Secret>) {
        let closing = self.books.closing;
        if self
            .flow
            .send_void(&self.link, self.group.as_ref(), closing, password)
        {
            self.voided();
        }
    }

    fn voided(&mut self) {
        let accounts = self.flow.void_accounts();
        let was_open = self.flow.editing.is_some();
        self.flow.voided();
        self.lender_result = None;
        crate::forms::publish_saved(&self.link, &self.bus, &accounts);
        self.records.forget();
        if was_open {
            self.new_form(true);
        }
        if let Some(loan) = self.header.current().loan {
            self.balances.fetch(&self.link, self.group.as_ref(), loan);
        }
    }

    /// The saved payment `v` as this form: one credit on a bank or card
    /// (the payment), a debit on the loan (the principal; none when it was
    /// interest only, then `loan` is the form's loan), the first expense
    /// debit as the interest, and any other debits as extra lines. Anything
    /// else is read-only (`false`). Amounts are the engine's, copied as
    /// they are; the principal shown is again `payment − interest − extras`.
    pub(crate) fn form_of(
        &mut self,
        v: &Versioned<Transaction>,
        loan: Option<AccountId>,
    ) -> (LoanPaymentForm, FormLines, bool) {
        let txn = &v.record;
        let class = |id| self.books.class(id);
        let expense = |id| {
            self.books.accounts.iter().any(|a| {
                a.id == id
                    && matches!(
                        a.fields.account_type,
                        cl_api::AccountType::Expense | cl_api::AccountType::OtherExpense
                    )
            })
        };
        let credits: Vec<usize> = txn
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.credit.is_positive())
            .map(|(i, _)| i)
            .collect();
        let from = match credits.as_slice() {
            [i] => Some(*i),
            _ => None,
        };
        let debits: Vec<usize> = (0..txn.lines.len()).filter(|i| Some(*i) != from).collect();
        let loans: Vec<usize> = debits
            .iter()
            .copied()
            .filter(|i| class(txn.lines[*i].account_id) == Some(AccountClass::OtherLiability))
            .collect();
        let principal_line = match loans.as_slice() {
            [i] => Some(*i),
            _ => None,
        };
        let interest_line = debits
            .iter()
            .copied()
            .find(|i| Some(*i) != principal_line && expense(txn.lines[*i].account_id));
        let loan = principal_line.map(|i| txn.lines[i].account_id).or(loan);
        let fits = txn.txn_type == TxnType::Expense
            && from.is_some_and(|i| {
                let l = &txn.lines[i];
                l.debit.is_zero()
                    && matches!(
                        class(l.account_id),
                        Some(AccountClass::Bank | AccountClass::CreditCard)
                    )
            })
            && loans.len() <= 1
            && loan.is_some()
            && debits.iter().all(|i| {
                let l = &txn.lines[*i];
                l.debit.is_positive() && l.credit.is_zero()
            });
        let mut f = LoanPaymentForm::new(Some(txn.txn_date));
        f.loan = loan;
        f.lender = txn.contact_id;
        f.pay_from = from.map(|i| txn.lines[i].account_id);
        f.method = match txn.check_number {
            Some(n) => LoanMethod::Check(n.to_string()),
            None => LoanMethod::Online,
        };
        f.payment = from.map(|i| txn.lines[i].credit);
        f.interest = Some(interest_line.map_or(Money::ZERO, |i| txn.lines[i].debit));
        f.interest_account = interest_line.map(|i| txn.lines[i].account_id);
        f.memo = txn.memo.clone().unwrap_or_default();
        let keys = &mut self.extras.form_mut().adapter_mut().keys;
        let seeds = debits
            .iter()
            .filter(|i| Some(**i) != principal_line && Some(**i) != interest_line)
            .map(|i| {
                let l = &txn.lines[*i];
                FormLine {
                    account: Some(l.account_id),
                    amount: Some(l.debit),
                    memo: l.description.clone().unwrap_or_default(),
                    ..FormLine::blank(keys.next_key())
                }
            })
            .collect();
        (f, FormLines::from_lines(seeds, keys), fits)
    }

    /// Shows a saved payment (Prev/Next, the register, Reload).
    pub(crate) fn show(&mut self, v: &Versioned<Transaction>) {
        let loan = self.header.current().loan;
        let (form, extras, fits) = self.form_of(v, loan);
        let lists = Arc::clone(&self.books.lists);
        self.header.load(form);
        self.extras.form_mut().load(extras, &*lists);
        self.flow.opened_txn(v, fits);

        self.problems.clear();
        self.prefill_task = None;
        self.prefill_applied = false;
        self.lender_check = None;
        self.lender_result = None;
        self.focused = None;
        self.sync_inputs();
        if let Some(loan) = self.header.current().loan {
            self.balances.fetch(&self.link, self.group.as_ref(), loan);
        }
    }

    fn reload(&mut self) {
        if let Some(id) = self.flow.editing.map(|e| e.id) {
            self.open_saved(id);
        }
    }

    /// Esc: the question, the void card or the lender list closes first;
    /// then the focused field goes back to its saved value (or drops the
    /// text typed in it); with nothing to revert the shell closes the
    /// window (asking first when the form is dirty).
    pub(crate) fn revert_field(&mut self) -> bool {
        if self.records.prompt.is_some() {
            self.answer_nav(PromptAnswer::Keep);
            return true;
        }
        if self.flow.void_open {
            self.flow.void_open = false;
            return true;
        }
        if self.inputs.lender.open {
            self.inputs.lender.open = false;
            return true;
        }
        let Some(fid) = self.focused else {
            return false;
        };
        let saved = self.header.original().clone();
        let now = self.header.current().clone();
        let style = self.money_style;
        let shown = |m: Option<Money>| m.map(|m| m.format(style)).unwrap_or_default();
        let typed = match fid {
            field::PAYMENT => self.inputs.payment.text.trim() != shown(now.payment),
            field::INTEREST => self.inputs.interest.text.trim() != shown(now.interest),
            field::LENDER_BALANCE => {
                self.inputs.lender_balance.text.trim() != shown(now.lender_balance)
            }
            field::LENDER => {
                self.inputs.lender.text.trim()
                    != now
                        .lender
                        .and_then(|p| self.books.lists.name_text(p))
                        .unwrap_or_default()
            }
            field::DATE => now
                .date
                .is_some_and(|d| DateInput::new(d).text != self.inputs.date.text),
            _ => false,
        };
        let differs = match fid {
            field::LOAN => now.loan != saved.loan,
            field::LENDER => now.lender != saved.lender,
            field::PAY_FROM => now.pay_from != saved.pay_from,
            field::NUMBER => now.method != saved.method,
            field::DATE => now.date != saved.date,
            field::PAYMENT => now.payment != saved.payment,
            field::INTEREST => now.interest != saved.interest,
            field::INTEREST_ACCOUNT => now.interest_account != saved.interest_account,
            field::LENDER_BALANCE => now.lender_balance != saved.lender_balance,
            field::MEMO => now.memo != saved.memo,
            _ => false,
        };
        if differs {
            self.header.edit_step(|h| match fid {
                field::LOAN => h.loan = saved.loan,
                field::LENDER => h.lender = saved.lender,
                field::PAY_FROM => h.pay_from = saved.pay_from,
                field::NUMBER => h.method = saved.method.clone(),
                field::DATE => h.date = saved.date,
                field::PAYMENT => h.payment = saved.payment,
                field::INTEREST => h.interest = saved.interest,
                field::INTEREST_ACCOUNT => h.interest_account = saved.interest_account,
                field::LENDER_BALANCE => h.lender_balance = saved.lender_balance,
                _ => h.memo.clone_from(&saved.memo),
            });
        }
        if differs || typed {
            self.sync_inputs();
            return true;
        }
        false
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.books.loaded {
            form::progress_line(ui, &tokens, &t("helper.loading"));
            return;
        }
        let idle = self.flow.idle();
        ui.horizontal(|ui| {
            if let Some(cmd) = kit::nav_buttons(ui, idle) {
                self.after_frame = Some(CommandId::new(cmd));
            }
            // "No payment schedules" is said in the side panel: beside the
            // buttons it ran into the heading at the minimum width.
            kit::heading_right(ui, &tokens, &t("cmd.banking.loan_payment"));
        });
        ui.separator();
        match kit::messages(ui, &tokens, &mut self.flow, self.books.closing) {
            MessageClick::SubmitClosing => self.submit_closing(),
            MessageClick::Void => self.void_now(None),
            MessageClick::Reload => self.reload(),
            MessageClick::None => {}
        }
        if self.records.prompt.is_some()
            && let Some(a) = records::prompt_ui(ui, KIND, &t("cmd.banking.loan_payment"))
        {
            self.answer_nav(a);
        }
        if self.loans().is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(t("loan.no_loan")).color(tokens.text_secondary));
            if ui.button(t("helper.open_chart")).clicked() {
                self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                    crate::screens::lists::accounts::KIND,
                )));
            }
            return;
        }
        kit::two_columns(ui, "loan_payment/scroll", |ui, col| match col {
            kit::Column::Form => self.form_ui(ui, &tokens),
            kit::Column::Panel => self.panel_ui(ui, &tokens),
        });
    }

    #[allow(clippy::too_many_lines)] // One card of fields, top to bottom.
    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let enabled = self.flow.idle();
        let loans = self.loans();
        let banks = self.pay_from_accounts();
        let interest_accounts = self.interest_accounts();
        ui.add_enabled_ui(enabled, |ui| {
            egui::Frame::new()
                .fill(tokens.surface_form)
                .stroke(Stroke::new(1.0, tokens.accent_border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(20, 16))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let width = ui.available_width();
                    let sp = ui.spacing().item_spacing.x;
                    let half = ((width - 16.0 - sp) / 2.0).max(160.0);
                    let third = ((width - 2.0 * (16.0 + sp)) / 3.0).max(120.0);
                    let f = self.header.current().clone();
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let (p, r) = kit::account_combo(
                                ui,
                                tokens,
                                "loan_payment/loan",
                                &t("loan.loan"),
                                &loans,
                                f.loan,
                                half - 8.0,
                            );
                            kit::track(&mut self.focused, &r, field::LOAN);
                            if let Some(id) = p {
                                self.set_loan(id);
                            }
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let lists = Arc::clone(&self.books.lists);
                            let (out, r) = kit::name_field(
                                ui,
                                tokens,
                                &t("loan.lender"),
                                &mut self.inputs.lender,
                                &lists,
                                f.lender,
                                half,
                            );
                            self.pick_lender(out);
                            kit::track(&mut self.focused, &r, field::LENDER);
                            if kit::entered(&r) {
                                self.after_frame = Some(CommandId::new("form.save_new"));
                            }
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let (p, r) = kit::account_combo(
                                ui,
                                tokens,
                                "loan_payment/pay_from",
                                &t("loan.pay_from"),
                                &banks,
                                f.pay_from,
                                half - 8.0,
                            );
                            kit::track(&mut self.focused, &r, field::PAY_FROM);
                            if let Some(id) = p {
                                self.header.edit(field::PAY_FROM, |h| {
                                    h.pay_from = Some(id);
                                });
                            }
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            self.method_ui(ui, tokens, half);
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(third);
                            let today = self.today;
                            let (r, d) = kit::date_field(
                                ui,
                                tokens,
                                &t("helper.date"),
                                &mut self.inputs.date,
                                today,
                                third,
                            );
                            if let kit::DateChange::Set(d) = d {
                                self.header.edit(field::DATE, |h| h.date = d);
                                self.inputs.rev = self.header.revision();
                            }
                            kit::track(&mut self.focused, &r, field::DATE);
                            if kit::entered(&r) {
                                self.after_frame = Some(CommandId::new("form.save_new"));
                            }
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.set_width(third);
                            let r = kit::amount_field(
                                ui,
                                tokens,
                                &t("loan.payment"),
                                &mut self.inputs.payment,
                                third,
                                true,
                            );
                            kit::track(&mut self.focused, &r, field::PAYMENT);
                            if r.lost_focus() {
                                self.amount_left(field::PAYMENT);
                            }
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.set_width(third);
                            let r = kit::amount_field(
                                ui,
                                tokens,
                                &t("loan.interest"),
                                &mut self.inputs.interest,
                                third,
                                true,
                            )
                            .on_hover_text(t("loan.interest.tip"));
                            kit::track(&mut self.focused, &r, field::INTEREST);
                            if r.lost_focus() {
                                self.amount_left(field::INTEREST);
                            }
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(third);
                            let principal = self
                                .compose()
                                .principal()
                                .map(|m| m.format(self.money_style))
                                .unwrap_or_default();
                            kit::figure_field(ui, tokens, &t("loan.principal"), &principal, third);
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.set_width(third * 2.0 + 16.0);
                            let (p, r) = kit::account_combo(
                                ui,
                                tokens,
                                "loan_payment/interest_account",
                                &t("loan.interest_account"),
                                &interest_accounts,
                                f.interest_account,
                                third * 2.0,
                            );
                            kit::track(&mut self.focused, &r, field::INTEREST_ACCOUNT);
                            if let Some(id) = p {
                                self.header.edit(field::INTEREST_ACCOUNT, |h| {
                                    h.interest_account = Some(id);
                                });
                            }
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let mut memo = f.memo.clone();
                            let r = kit::text_field(
                                ui,
                                tokens,
                                &t("helper.memo"),
                                &mut memo,
                                half,
                                cl_api::PRINTED_MEMO_MAX,
                            );
                            if r.changed() {
                                self.header.edit(field::MEMO, |h| h.memo = memo);
                                self.inputs.rev = self.header.revision();
                            }
                            kit::track(&mut self.focused, &r, field::MEMO);
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let r = kit::amount_field(
                                ui,
                                tokens,
                                &t("loan.lender_balance"),
                                &mut self.inputs.lender_balance,
                                half,
                                true,
                            )
                            .on_hover_text(t("loan.lender_balance.tip"));
                            kit::track(&mut self.focused, &r, field::LENDER_BALANCE);
                            if r.lost_focus() {
                                self.amount_left(field::LENDER_BALANCE);
                            }
                        });
                    });
                });
        });
        ui.add_space(12.0);
        ui.label(RichText::new(t("loan.extras.heading")).color(tokens.text_primary));
        ui.add_enabled_ui(enabled, |ui| {
            let _ = self.extras.ui(ui);
        });
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
        kit::status(ui, tokens, &self.flow);
        ui.add_space(12.0);
        if let Some(cmd) = kit::actions_row(ui, enabled, enabled) {
            self.after_frame = Some(CommandId::new(cmd));
        }
    }

    fn method_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        kit::above(ui, tokens, &t("loan.paid_by"));
        let f = self.header.current().clone();
        let card = f
            .pay_from
            .is_some_and(|a| self.books.class(a) == Some(AccountClass::CreditCard));
        let mut set = None;
        ui.horizontal(|ui| {
            let is_check = matches!(f.method, LoanMethod::Check(_));
            if ui
                .add_enabled(
                    !card,
                    egui::RadioButton::new(is_check, t("loan.paid_by.check")),
                )
                .clicked()
            {
                set = Some(LoanMethod::Check(String::new()));
            }
            if ui.radio(!is_check, t("loan.paid_by.online")).clicked() {
                set = Some(LoanMethod::Online);
            }
        });
        if let LoanMethod::Check(n) = &f.method {
            let mut text = n.clone();
            let r = kit::text_field(ui, tokens, &t("loan.check_no"), &mut text, width, 10);
            if r.changed() {
                self.header
                    .edit(field::NUMBER, |h| h.method = LoanMethod::Check(text));
            }
            kit::track(&mut self.focused, &r, field::NUMBER);
        }
        if card && matches!(f.method, LoanMethod::Check(_)) {
            set = Some(LoanMethod::Online);
        }
        if let Some(m) = set
            && std::mem::discriminant(&m) != std::mem::discriminant(&f.method)
        {
            self.header.edit_step(|h| h.method = m);
        }
    }

    fn panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        kit::panel_card(ui, tokens, &t("loan.panel.title"), |ui| {
            let owed = self
                .header
                .current()
                .loan
                .and_then(|l| self.balances.shown(l, Some(AccountClass::OtherLiability)))
                .map(|m| m.format(style));
            kit::balance_line(ui, tokens, &t("loan.panel.balance"), owed);
            match self.lender_result {
                Some(LenderMatch::Matches) => {
                    ui.label(RichText::new(t("loan.lender.matches")).color(tokens.text_primary));
                }
                Some(LenderMatch::Differs(d)) => {
                    ui.label(
                        RichText::new(t_with(
                            "loan.lender.differs",
                            &[("amount", &d.abs().format(style))],
                        ))
                        .color(tokens.warn_text),
                    );
                }
                None => {}
            }
            form::hint(ui, tokens, &t("loan.no_schedules"));
            if self.prefill_applied {
                form::hint(ui, tokens, &t("loan.prefilled"));
            }
        });
    }

    fn form_command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "form.save_new" => {
                self.start_save(SaveIntent::SaveAndNew, None);
                true
            }
            "form.save_close" => {
                self.start_save(SaveIntent::SaveAndClose, None);
                true
            }
            "form.clear" => {
                self.clear();
                true
            }
            "form.prev_record" => {
                self.navigate(true);
                true
            }
            "form.next_record" => {
                self.navigate(false);
                true
            }
            "edit.void_transaction" => self.flow.start_void(),
            "edit.open_list" => {
                self.inputs.lender.open = true;
                true
            }
            "edit.undo" => {
                let done = self.header.undo();
                if done {
                    self.sync_inputs();
                }
                done
            }
            "edit.redo" => {
                let done = self.header.redo();
                if done {
                    self.sync_inputs();
                }
                done
            }
            "form.revert" => self.revert_field(),
            _ => false,
        }
    }

    /// The control-channel state (no amounts or names).
    pub(crate) fn state_json(&self) -> Value {
        let f = self.compose();
        let mut v = json!({
            "screen": "loan_payment",
            "company_open": self.tenant.is_some(),
            "loaded": self.books.loaded,
            "loan_id": f.loan.map(|a| a.to_string()),
            "has_lender": f.lender.is_some(),
            "pay_from_id": f.pay_from.map(|a| a.to_string()),
            "interest_account_set": f.interest_account.is_some(),
            "method": match f.method { LoanMethod::Check(_) => "check", LoanMethod::Online => "online" },
            "payment_set": f.payment.is_some(),
            "interest_set": f.interest.is_some(),
            "principal_negative": f.principal().is_some_and(Money::is_negative),
            "lender_balance_set": f.lender_balance.is_some(),
            "extras": f.extras.used().count(),
            "balance_shown": f.loan.is_some_and(|l| self.balances.known.contains_key(&l)),
            "lender_match": self.lender_result.map(|m| match m {
                LenderMatch::Matches => "matches",
                LenderMatch::Differs(_) => "differs",
            }),
            "prefill": { "applied": self.prefill_applied, "pending": self.prefill_task.is_some() },
            "dirty": self.is_dirty(),
            "published_accounts": self.flow.published.len(),
            "save": {
                "refused": self.problems.first().map(|p| p.name()),
                "problems": self.problems.len(),
            },
            "nav_prompt": self.records.prompt.is_some(),
            "nav_at": self.records.at,
            "focused": self.focused,
        });
        if let (Value::Object(m), Value::Object(c)) = (&mut v, self.flow.state_json(&self.link)) {
            m.extend(c);
        }
        v
    }
}

impl Screen for LoanPaymentScreen {
    fn title(&self) -> String {
        t("cmd.banking.loan_payment")
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
        // A refused save's problems follow the owner's fixes.
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
        Scope::Form
    }

    fn dirty(&self) -> bool {
        self.is_dirty()
    }

    fn save(&mut self) -> bool {
        self.start_save(SaveIntent::Save, None)
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(handled) = self.flow.closing_prompt.command(id) {
            return handled;
        }
        if let Some(handled) = self.extras.command(id) {
            return handled;
        }
        self.form_command(id)
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.open_saved(txn)
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

impl RecordForm for LoanPaymentScreen {
    fn parts(&mut self) -> (&mut Records, &Arc<CompanyLink>, Option<&RequestGroup>) {
        (&mut self.records, &self.link, self.group.as_ref())
    }

    /// The payments on the loan shown.
    fn record_scope(&self) -> Option<RecordScope> {
        self.header.current().loan.map(|account| RecordScope {
            account,
            types: LOAN_TYPES,
        })
    }

    fn form_dirty(&self) -> bool {
        self.is_dirty()
    }

    fn form_busy(&self) -> bool {
        !self.flow.idle()
    }

    fn show_record(&mut self, v: &Versioned<Transaction>) {
        self.show(v);
    }

    fn blank_after_nav(&mut self) {
        self.flow.reset_messages();
        self.new_form(true);
    }

    fn discard_edits(&mut self) {
        let lists = Arc::clone(&self.books.lists);
        self.header.revert_form();
        self.extras.form_mut().revert_form(&*lists);
        self.problems.clear();
        self.sync_inputs();
    }

    fn save_for_nav(&mut self) -> bool {
        self.start_save(SaveIntent::Save, None)
    }

    fn save_queued(&self) -> bool {
        self.flow.task.is_some()
    }

    fn record_notice(&mut self, key: &'static str) {
        self.flow.notice = Some(key);
    }

    fn record_failed(&mut self, e: ApiError) {
        self.flow.error = Some(e);
    }
}

impl LoanPaymentScreen {
    /// A refused save's problems, checked again against the form as it is
    /// now (a fixed problem goes away at once, P13).
    pub(crate) fn recheck_problems(&mut self) {
        if self.problems.is_empty() {
            return;
        }
        let now = check(&self.compose(), &self.books.tag_list)
            .err()
            .unwrap_or_default();
        crate::forms::recheck(&mut self.problems, now);
    }
}

#[cfg(test)]
mod tests;
