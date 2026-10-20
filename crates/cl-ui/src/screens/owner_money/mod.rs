//! Pay Yourself or Put Money In (P14 lane C, `banking.owner_money`; Q-61,
//! Q-P14l, Q-P14m), on the Write Check pattern (Q-261).
//!
//! Paying yourself posts a check, or an online payment, from the bank with
//! one line to the draw account; putting money in posts a deposit with one
//! line from the contribution account. "Recorded in" starts on the engine's
//! equity role account for the entity type (`equity-roles-v1`), else on the
//! owner's earlier choice for this company; it can always be changed. One
//! neutral sentence per entity type and "Ask your accountant if you're not
//! sure which to use." — no tax advice (Q-61). The amount is the owner's
//! typed `Money`, sent as is.

pub(crate) mod remember;

use std::sync::Arc;

use cl_api::{
    ApiError, BankPosting, CardChargeDraft, CheckDraft, DepositDraft, LocalValidation,
    RequestGroup, SplitLine, Transaction, TxnType, Versioned,
};
use cl_core::ids::{AccountId, TenantId, TransactionId};
use cl_core::{CalendarDate, MoneyStyle, Secret};
use cl_model::amount_entry::AmountRules;
use cl_model::forms::owner_money::{
    CheckedOwnerMoney, Direction, EquitySource, OwnerMethod, OwnerMoneyForm, OwnerMoneyProblem,
    OwnerPosting, check, pick_equity,
};
use cl_model::forms::{FormState, SaveIntent};
use cl_model::register::AccountClass;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Ui};
use serde_json::{Value, json};

use crate::company::demo::register::DemoPosting;
use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_with};
use crate::screens::card_charge::kit::{
    self, AmountInput, Balances, Books, Editing, MessageClick, NameInput, NameOutcome, Outbound,
    Pending, SaveFlow, Sent, Settled,
};
use crate::screens::card_charge::records::{self, RecordForm, RecordScope, Records};
use crate::screens::company::form::{self, DateInput};
use crate::theme::Tokens;
use crate::windows::{PromptAnswer, Screen};
use remember::Slot;

/// The command and window kind.
pub const KIND: &str = "banking.owner_money";

/// The window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(OwnerMoneyScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

mod field {
    pub const WHO: u64 = 1;
    pub const BANK: u64 = 2;
    pub const NUMBER: u64 = 3;
    pub const DATE: u64 = 4;
    pub const AMOUNT: u64 = 5;
    pub const MEMO: u64 = 6;
    pub const EQUITY: u64 = 7;
}

/// The texts being typed.
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub date: DateInput,
    pub who: NameInput,
    pub amount: AmountInput,
    pub rev: u64,
}

/// The company key the remembered choices are kept under.
pub(crate) fn company_key(tenant: Option<&(TenantId, String)>) -> String {
    tenant.map_or_else(|| "demo".to_string(), |(t, _)| t.to_string())
}

/// The remembered slot for a direction.
fn slot_of(d: Direction) -> Slot {
    match d {
        Direction::PayYourself => Slot::Out,
        Direction::PutMoneyIn => Slot::In,
    }
}

/// The Pay Yourself or Put Money In window.
pub struct OwnerMoneyScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    pub(crate) books: Books,
    pub(crate) balances: Balances,
    pub(crate) header: FormState<OwnerMoneyForm>,
    pub(crate) inputs: Inputs,
    pub(crate) flow: SaveFlow,
    pub(crate) problems: Vec<OwnerMoneyProblem>,
    /// Where "Recorded in" came from (`None`: picked by the owner, or
    /// nothing yet).
    pub(crate) equity_source: Option<EquitySource>,
    pub(crate) bus: crate::data_bus::Subscription,
    lists_stale: bool,
    after_frame: Option<CommandId>,
    pub(crate) money_style: MoneyStyle,
    /// Prev/Next and the saved record being read.
    pub(crate) records: Records,
    /// The field that had the keyboard (Esc reverts it).
    pub(crate) focused: Option<u64>,
}

/// The kinds of record this form shows: draws (a check or payment from the
/// bank) and contributions (deposits).
const OWNER_TYPES: &[TxnType] = &[TxnType::Expense, TxnType::Deposit];

impl OwnerMoneyScreen {
    /// A new form in the open company (Pay yourself).
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> OwnerMoneyScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = OwnerMoneyScreen {
            link,
            tenant,
            group,
            today,
            books: Books::default(),
            balances: Balances::default(),
            header: FormState::new(OwnerMoneyForm::new(
                Direction::PayYourself,
                None,
                Some(today),
            )),
            inputs: Inputs {
                date: DateInput::new(today),
                who: NameInput::default(),
                amount: AmountInput::default(),
                rev: u64::MAX,
            },
            flow: SaveFlow::default(),
            problems: Vec::new(),
            equity_source: None,
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

    /// Active equity accounts.
    pub(crate) fn equity_accounts(&self) -> Vec<(AccountId, String)> {
        self.books.accounts_of(&[AccountClass::Equity])
    }

    fn remembered(&self, d: Direction) -> Option<AccountId> {
        remember::get(
            remember::dir_of(&self.link).as_deref(),
            &company_key(self.tenant.as_ref()),
            slot_of(d),
        )
    }

    /// The equity account the form starts on for `d` (engine role, else
    /// remembered).
    fn default_equity(&self, d: Direction) -> Option<(AccountId, EquitySource)> {
        let usable: Vec<AccountId> = self.equity_accounts().into_iter().map(|(a, _)| a).collect();
        pick_equity(
            &self.books.roles,
            self.books.entity,
            d,
            self.remembered(d),
            &usable,
        )
    }

    pub(crate) fn poll(&mut self) {
        if self.books.poll() {
            self.inputs.rev = u64::MAX;
            let mut f = self.header.current().clone();
            if f.bank.is_none() {
                f.bank = self
                    .books
                    .accounts_of(&[AccountClass::Bank])
                    .first()
                    .map(|(a, _)| *a);
            }
            if f.equity.is_none()
                && let Some((a, src)) = self.default_equity(f.direction)
            {
                f.equity = Some(a);
                self.equity_source = Some(src);
            }
            if !self.header.is_dirty() {
                self.header.load(f);
            }
            if let Some(b) = self.header.current().bank {
                self.balances.fetch(&self.link, self.group.as_ref(), b);
            }
        }
        self.balances.poll();
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
        if let Some(b) = self.header.current().bank
            && changes.ledger_touches(b)
        {
            self.balances.fetch(&self.link, self.group.as_ref(), b);
        }
    }

    pub(crate) fn sync_inputs(&mut self) {
        let f = self.header.current();
        if let Some(d) = f.date {
            self.inputs.date = DateInput::new(d);
        }
        self.inputs.who.text = f
            .who
            .and_then(|p| self.books.lists.name_text(p))
            .map_or_else(|| f.who_name.clone(), str::to_string);
        self.inputs.who.problem = false;
        self.inputs.amount.show(f.amount, self.money_style);
        self.inputs.rev = self.header.revision();
    }

    /// The form has unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.header.is_dirty()
    }

    /// Pay yourself or put money in: "Recorded in" follows unless the
    /// owner picked it.
    pub(crate) fn set_direction(&mut self, d: Direction) {
        if self.header.current().direction == d {
            return;
        }
        let follow = self.equity_source.is_some() || self.header.current().equity.is_none();
        let next = if follow { self.default_equity(d) } else { None };
        self.header.edit_step(|h| {
            h.direction = d;
            if follow {
                h.equity = next.map(|(a, _)| a);
            }
        });
        if follow {
            self.equity_source = next.map(|(_, s)| s);
        }
    }

    /// The owner chose the equity account: remembered for this company.
    pub(crate) fn set_equity(&mut self, account: AccountId) {
        let d = self.header.current().direction;
        self.header
            .edit(field::EQUITY, |h| h.equity = Some(account));
        self.equity_source = None;
        remember::set(
            remember::dir_of(&self.link).as_deref(),
            &company_key(self.tenant.as_ref()),
            slot_of(d),
            account,
        );
    }

    fn apply_who(&mut self, out: NameOutcome) {
        match out {
            NameOutcome::Picked(id) => {
                let name = self
                    .books
                    .lists
                    .name_text(id)
                    .unwrap_or_default()
                    .to_string();
                self.inputs.who.text.clone_from(&name);
                self.header.edit(field::WHO, |h| {
                    h.who = Some(id);
                    h.who_name = name;
                });
            }
            NameOutcome::Cleared => {
                self.header.edit(field::WHO, |h| {
                    h.who = None;
                    h.who_name.clear();
                });
            }
            NameOutcome::None => return,
        }
        self.inputs.rev = self.header.revision();
    }

    fn amount_left(&mut self) {
        if let Settled::Value(m) = self
            .inputs
            .amount
            .settle(self.money_style, AmountRules::MONEY)
        {
            self.header.edit(field::AMOUNT, |h| h.amount = m);
            self.inputs.rev = self.header.revision();
        }
    }

    fn commit_header(&mut self) -> bool {
        if self.inputs.rev != self.header.revision() {
            return true;
        }
        let shown = self
            .header
            .current()
            .amount
            .map(|m| m.format(self.money_style))
            .unwrap_or_default();
        if self.inputs.amount.text.trim() != shown.trim() {
            self.amount_left();
        }
        let who = self.header.current();
        let who_shown = who
            .who
            .and_then(|p| self.books.lists.name_text(p))
            .map_or_else(|| who.who_name.clone(), str::to_string);
        if self.inputs.who.text.trim() != who_shown.trim() {
            let lists = Arc::clone(&self.books.lists);
            let out = kit::resolve_name(&mut self.inputs.who, &lists);
            self.apply_who(out);
        }
        self.inputs.amount.problem.is_none() && !self.inputs.who.problem
    }

    /// The checked form as the engine posting and the demo's copy, field
    /// by field: one line of exactly the typed amount.
    pub(crate) fn outbound(&self, c: &CheckedOwnerMoney) -> Result<Outbound, LocalValidation> {
        let line = SplitLine {
            account: c.equity,
            amount: c.amount,
            memo: None,
            contact: None,
            billable: false,
            tag: None,
        };
        let name = c
            .who
            .and_then(|w| self.books.contact_name(w))
            .or_else(|| (!c.who_name.is_empty()).then(|| c.who_name.clone()));
        let (posting, kind, number, own_debit) = match c.posting {
            OwnerPosting::Check { number } => (
                BankPosting::check(CheckDraft {
                    bank: c.bank,
                    date: c.date,
                    payee: c.who,
                    payee_name: name.clone().unwrap_or_default(),
                    check_number: number,
                    print_later: false,
                    payee_address: None,
                    memo: c.memo.clone(),
                    printed_memo: c.memo.clone(),
                    tag_ids: Vec::new(),
                    draft_attachment: None,
                    amount: c.amount,
                    lines: vec![line],
                })?,
                "check",
                number.map(|n| n.to_string()).unwrap_or_default(),
                false,
            ),
            OwnerPosting::Payment => (
                BankPosting::card_charge(CardChargeDraft {
                    pay_from: c.bank,
                    date: c.date,
                    number: None,
                    payee: c.who,
                    memo: c.memo.clone(),
                    lines: vec![line],
                })?,
                "card_charge",
                String::new(),
                false,
            ),
            OwnerPosting::Deposit => (
                BankPosting::deposit(DepositDraft::plain(
                    c.bank,
                    c.date,
                    c.memo.clone(),
                    vec![SplitLine {
                        contact: c.who,
                        ..line
                    }],
                ))?,
                "deposit",
                String::new(),
                true,
            ),
        };
        let demo = DemoPosting {
            kind,
            date: c.date,
            number,
            contact: name,
            memo: c.memo.clone().unwrap_or_default(),
            own: c.bank,
            own_debit,
            lines: vec![(c.equity, c.amount)],
        };
        Ok(Outbound { posting, demo })
    }

    pub(crate) fn start_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        if !self.flow.idle() && password.is_none() {
            return false;
        }
        if !self.commit_header() {
            return false;
        }
        self.problems.clear();
        self.flow.reset_messages();
        if !self.is_dirty() {
            self.follow_up(intent);
            return true;
        }
        let checked = match check(self.header.current()) {
            Ok(c) => c,
            Err(p) => {
                self.problems = p;
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
        self.flow.published = vec![checked.bank, checked.equity];
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
                let _ = self.header.begin_save(intent);
                false
            }
            Sent::Asked | Sent::Failed => false,
        }
    }

    fn saved(&mut self, r: kit::SavedRecord) {
        let intent = self.flow.saved(r);
        crate::forms::publish_saved(&self.link, &self.bus, &self.flow.published);
        self.problems.clear();
        let header = self.header.current().clone();
        if self.header.is_saving() {
            self.header.save_ok(header);
        } else {
            self.header.load(header);
        }
        if let Some(b) = self.header.current().bank {
            self.balances.fetch(&self.link, self.group.as_ref(), b);
        }
        self.records.forget();
        self.follow_up(intent);
        self.after_saved();
    }

    fn save_failed(&mut self, e: ApiError) {
        let closing = self.books.closing;
        self.header.save_failed(cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        });
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

    /// A blank form; keeps the direction, the bank, who and the account.
    pub(crate) fn new_form(&mut self, keep_date: bool) {
        let c = self.header.current().clone();
        let mut f = OwnerMoneyForm::new(
            c.direction,
            c.bank,
            Some(if keep_date {
                c.date.unwrap_or(self.today)
            } else {
                self.today
            }),
        );
        f.who = c.who;
        f.who_name = c.who_name;
        f.equity = c.equity;
        f.method = match c.method {
            OwnerMethod::Check(_) => OwnerMethod::Check(String::new()),
            OwnerMethod::Electronic => OwnerMethod::Electronic,
        };
        self.header.load(f);
        self.problems.clear();
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
        let mut accounts = self.flow.published.clone();
        let f = self.header.current();
        accounts.extend(f.bank.into_iter().chain(f.equity));
        let was_open = self.flow.editing.is_some();
        self.flow.voided();
        crate::forms::publish_saved(&self.link, &self.bus, &accounts);
        self.records.forget();
        if was_open {
            self.new_form(true);
        }
        if let Some(b) = self.header.current().bank {
            self.balances.fetch(&self.link, self.group.as_ref(), b);
        }
    }

    /// The saved record `v` as this form: a draw is one credit on a bank
    /// and one debit on an equity account (a check when it has a number); a
    /// contribution is one debit on a bank and one credit on an equity
    /// account, both of the same amount. Anything else is read-only
    /// (`false`). The amount is the engine's, copied as it is.
    pub(crate) fn form_of(&self, v: &Versioned<Transaction>) -> (OwnerMoneyForm, bool) {
        let txn = &v.record;
        let class = |id| self.books.class(id);
        let direction = if txn.txn_type == TxnType::Deposit {
            Direction::PutMoneyIn
        } else {
            Direction::PayYourself
        };
        let debit: Vec<&cl_api::TransactionLine> =
            txn.lines.iter().filter(|l| l.debit.is_positive()).collect();
        let credit: Vec<&cl_api::TransactionLine> = txn
            .lines
            .iter()
            .filter(|l| l.credit.is_positive())
            .collect();
        let (bank, equity) = match direction {
            Direction::PayYourself => (credit.first(), debit.first()),
            Direction::PutMoneyIn => (debit.first(), credit.first()),
        };
        let amount = equity.map(|l| match direction {
            Direction::PayYourself => l.debit,
            Direction::PutMoneyIn => l.credit,
        });
        let fits = debit.len() == 1
            && credit.len() == 1
            && txn.lines.len() == 2
            && matches!(txn.txn_type, TxnType::Expense | TxnType::Deposit)
            && debit[0].debit == credit[0].credit
            && bank.is_some_and(|l| class(l.account_id) == Some(AccountClass::Bank))
            && equity.is_some_and(|l| class(l.account_id) == Some(AccountClass::Equity));
        let mut f = OwnerMoneyForm::new(direction, bank.map(|l| l.account_id), Some(txn.txn_date));
        f.equity = equity.map(|l| l.account_id);
        f.amount = amount;
        f.memo = txn.memo.clone().unwrap_or_default();
        f.who = txn.contact_id.or_else(|| equity.and_then(|l| l.contact_id));
        f.who_name = txn.contact_name.clone().unwrap_or_default();
        f.method = match txn.check_number {
            Some(n) => OwnerMethod::Check(n.to_string()),
            None => OwnerMethod::Electronic,
        };
        (f, fits)
    }

    /// Shows a saved draw or contribution (Prev/Next, the register,
    /// Reload).
    pub(crate) fn show(&mut self, v: &Versioned<Transaction>) {
        let (form, fits) = self.form_of(v);
        self.header.load(form);
        self.flow.opened(Editing::of(v, fits));
        self.equity_source = None;
        self.problems.clear();
        self.focused = None;
        self.sync_inputs();
        if let Some(b) = self.header.current().bank {
            self.balances.fetch(&self.link, self.group.as_ref(), b);
        }
    }

    fn reload(&mut self) {
        if let Some(id) = self.flow.editing.map(|e| e.id) {
            self.open_saved(id);
        }
    }

    /// Esc: the question, the void card or the names list closes first;
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
        if self.inputs.who.open {
            self.inputs.who.open = false;
            return true;
        }
        let Some(fid) = self.focused else {
            return false;
        };
        let saved = self.header.original().clone();
        let now = self.header.current().clone();
        let typed = match fid {
            field::AMOUNT => {
                self.inputs.amount.text.trim()
                    != now
                        .amount
                        .map(|m| m.format(self.money_style))
                        .unwrap_or_default()
            }
            field::WHO => {
                let shown = now
                    .who
                    .and_then(|p| self.books.lists.name_text(p))
                    .map_or_else(|| now.who_name.clone(), str::to_string);
                self.inputs.who.text.trim() != shown.trim()
            }
            field::DATE => now
                .date
                .is_some_and(|d| DateInput::new(d).text != self.inputs.date.text),
            _ => false,
        };
        let differs = match fid {
            field::WHO => now.who != saved.who || now.who_name != saved.who_name,
            field::BANK => now.bank != saved.bank,
            field::NUMBER => now.method != saved.method,
            field::DATE => now.date != saved.date,
            field::AMOUNT => now.amount != saved.amount,
            field::MEMO => now.memo != saved.memo,
            field::EQUITY => now.equity != saved.equity,
            _ => false,
        };
        if differs {
            self.header.edit_step(|h| match fid {
                field::WHO => {
                    h.who = saved.who;
                    h.who_name.clone_from(&saved.who_name);
                }
                field::BANK => h.bank = saved.bank,
                field::NUMBER => h.method = saved.method.clone(),
                field::DATE => h.date = saved.date,
                field::AMOUNT => h.amount = saved.amount,
                field::MEMO => h.memo.clone_from(&saved.memo),
                _ => h.equity = saved.equity,
            });
        }
        if differs || typed {
            self.sync_inputs();
            return true;
        }
        false
    }

    /// The neutral sentence for the entity type (Q-61).
    pub(crate) fn note_key(&self) -> &'static str {
        self.books
            .entity
            .map_or("owner_money.note.unknown", |e| e.note().0)
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.books.loaded {
            form::progress_line(ui, &tokens, &t("helper.loading"));
            return;
        }
        let idle = self.flow.idle();
        let d = self.header.current().direction;
        let mut picked = None;
        ui.horizontal(|ui| {
            if let Some(cmd) = kit::nav_buttons(ui, idle) {
                self.after_frame = Some(CommandId::new(cmd));
            }
            ui.separator();
            ui.add_enabled_ui(idle, |ui| {
                if ui
                    .selectable_label(d == Direction::PayYourself, t("owner_money.pay_yourself"))
                    .clicked()
                {
                    picked = Some(Direction::PayYourself);
                }
                if ui
                    .selectable_label(d == Direction::PutMoneyIn, t("owner_money.put_in"))
                    .clicked()
                {
                    picked = Some(Direction::PutMoneyIn);
                }
            });
            kit::heading_right(ui, &tokens, &t("cmd.banking.owner_money"));
        });
        if let Some(p) = picked {
            self.set_direction(p);
        }
        ui.separator();
        match kit::messages(ui, &tokens, &mut self.flow, self.books.closing) {
            MessageClick::SubmitClosing => self.submit_closing(),
            MessageClick::Void => self.void_now(None),
            MessageClick::Reload => self.reload(),
            MessageClick::None => {}
        }
        if self.records.prompt.is_some()
            && let Some(a) = records::prompt_ui(ui, KIND, &t("cmd.banking.owner_money"))
        {
            self.answer_nav(a);
        }
        kit::two_columns(ui, "owner_money/scroll", |ui, col| match col {
            kit::Column::Form => self.form_ui(ui, &tokens),
            kit::Column::Panel => self.panel_ui(ui, &tokens),
        });
    }

    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let enabled = self.flow.idle();
        let banks = self.books.accounts_of(&[AccountClass::Bank]);
        let equity = self.equity_accounts();
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
                    let f = self.header.current().clone();
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let lists = Arc::clone(&self.books.lists);
                            let (out, r) = kit::name_field(
                                ui,
                                tokens,
                                &t("owner_money.who"),
                                &mut self.inputs.who,
                                &lists,
                                f.who,
                                half,
                            );
                            self.apply_who(out);
                            kit::track(&mut self.focused, &r, field::WHO);
                            if kit::entered(&r) {
                                self.after_frame = Some(CommandId::new("form.save_new"));
                            }
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let (p, r) = kit::account_combo(
                                ui,
                                tokens,
                                "owner_money/bank",
                                &t("owner_money.bank"),
                                &banks,
                                f.bank,
                                half - 8.0,
                            );
                            kit::track(&mut self.focused, &r, field::BANK);
                            if let Some(id) = p {
                                self.header.edit(field::BANK, |h| h.bank = Some(id));
                                self.balances.fetch(&self.link, self.group.as_ref(), id);
                            }
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            if f.direction == Direction::PayYourself {
                                self.method_ui(ui, tokens, half);
                            }
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let today = self.today;
                            let (r, d) = kit::date_field(
                                ui,
                                tokens,
                                &t("helper.date"),
                                &mut self.inputs.date,
                                today,
                                half,
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
                    });
                    ui.add_space(8.0);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let r = kit::amount_field(
                                ui,
                                tokens,
                                &t("helper.amount"),
                                &mut self.inputs.amount,
                                half,
                                true,
                            );
                            kit::track(&mut self.focused, &r, field::AMOUNT);
                            if r.lost_focus() {
                                self.amount_left();
                                if kit::entered(&r) && self.inputs.amount.problem.is_none() {
                                    self.after_frame = Some(CommandId::new("form.save_new"));
                                }
                            }
                        });
                        ui.add_space(16.0);
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
                            }
                            kit::track(&mut self.focused, &r, field::MEMO);
                        });
                    });
                    ui.add_space(8.0);
                    let (p, r) = kit::account_combo(
                        ui,
                        tokens,
                        "owner_money/equity",
                        &t("owner_money.recorded_in"),
                        &equity,
                        f.equity,
                        half - 8.0,
                    );
                    kit::track(&mut self.focused, &r, field::EQUITY);
                    if let Some(id) = p {
                        self.set_equity(id);
                    }
                    if let Some(src) = self.equity_source {
                        let key = match src {
                            EquitySource::Engine => "owner_money.source.engine",
                            EquitySource::Remembered => "owner_money.source.remembered",
                        };
                        form::hint(ui, tokens, &t(key));
                    }
                });
        });
        ui.add_space(6.0);
        let recorded = self
            .header
            .current()
            .equity
            .map(|a| self.books.account_name(a))
            .unwrap_or_default();
        if !recorded.is_empty() {
            ui.label(
                RichText::new(t_with(
                    "owner_money.recorded_line",
                    &[("account", &recorded)],
                ))
                .color(tokens.text_secondary),
            );
        }
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
        kit::status(ui, tokens, &self.flow);
        ui.add_space(12.0);
        if let Some(cmd) = kit::actions_row(ui, enabled, enabled) {
            self.after_frame = Some(CommandId::new(cmd));
        }
    }

    fn method_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        kit::above(ui, tokens, &t("owner_money.paid_by"));
        let f = self.header.current().clone();
        let is_check = matches!(f.method, OwnerMethod::Check(_));
        let mut set = None;
        ui.horizontal(|ui| {
            if ui.radio(is_check, t("owner_money.paid_by.check")).clicked() && !is_check {
                set = Some(OwnerMethod::Check(String::new()));
            }
            if ui
                .radio(!is_check, t("owner_money.paid_by.transfer"))
                .clicked()
                && is_check
            {
                set = Some(OwnerMethod::Electronic);
            }
        });
        if let Some(m) = set {
            self.header.edit_step(|h| h.method = m);
        }
        if let OwnerMethod::Check(n) = &self.header.current().method {
            let mut text = n.clone();
            let r = kit::text_field(ui, tokens, &t("owner_money.check_no"), &mut text, width, 10);
            if r.changed() {
                self.header
                    .edit(field::NUMBER, |h| h.method = OwnerMethod::Check(text));
            }
            kit::track(&mut self.focused, &r, field::NUMBER);
        }
    }

    fn panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        kit::panel_card(ui, tokens, &t("owner_money.panel.title"), |ui| {
            ui.add(egui::Label::new(t(self.note_key())).wrap());
            ui.add(
                egui::Label::new(
                    RichText::new(t("owner_money.ask_accountant")).color(tokens.text_secondary),
                )
                .wrap(),
            );
            let bank = self
                .header
                .current()
                .bank
                .and_then(|b| self.balances.shown(b, Some(AccountClass::Bank)))
                .map(|m| m.format(style));
            kit::balance_line(ui, tokens, &t("owner_money.bank_balance"), bank);
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
                self.inputs.who.open = true;
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
        let f = self.header.current();
        let mut v = json!({
            "screen": "owner_money",
            "company_open": self.tenant.is_some(),
            "loaded": self.books.loaded,
            "direction": match f.direction {
                Direction::PayYourself => "pay_yourself",
                Direction::PutMoneyIn => "put_money_in",
            },
            "entity_type": self.books.entity.map(cl_model::forms::owner_money::Entity::name),
            "roles_supported": self.books.roles_supported,
            "note": self.note_key(),
            "equity_id": f.equity.map(|a| a.to_string()),
            "equity_source": self.equity_source.map(|s| match s {
                EquitySource::Engine => "engine",
                EquitySource::Remembered => "remembered",
            }),
            "bank_id": f.bank.map(|a| a.to_string()),
            "method": match f.method { OwnerMethod::Check(_) => "check", OwnerMethod::Electronic => "electronic" },
            "has_who": f.who.is_some(),
            "amount_set": f.amount.is_some(),
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

impl Screen for OwnerMoneyScreen {
    fn title(&self) -> String {
        t("cmd.banking.owner_money")
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
        if let Some(cmd) = self.after_frame.take() {
            self.command(cmd);
        }
        if self.books.busy() || self.balances.busy() || self.flow.busy() || self.records.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.open_saved(txn)
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
        self.form_command(id)
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

impl RecordForm for OwnerMoneyScreen {
    fn parts(&mut self) -> (&mut Records, &Arc<CompanyLink>, Option<&RequestGroup>) {
        (&mut self.records, &self.link, self.group.as_ref())
    }

    /// The records of the "Recorded in" account.
    fn record_scope(&self) -> Option<RecordScope> {
        self.header.current().equity.map(|account| RecordScope {
            account,
            types: OWNER_TYPES,
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
        self.header.revert_form();
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

#[cfg(test)]
mod tests;
