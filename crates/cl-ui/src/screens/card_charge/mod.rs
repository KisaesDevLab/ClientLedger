//! Record Card Charge / Credit (P14 lane C, `vendors.card_charge`,
//! Ctrl+Shift+C; Q-60, Q-241, Q-P14i, Q-P14j), on the Write Check pattern
//! (Q-261): the header card, the shared [`LinesGrid`], the totals line and
//! Clear · Save & New · Save & Close, with the card's balance owed beside.
//!
//! A charge posts as an expense paid from the card
//! (`BankPosting::card_charge_with`, line customers and billable as on a
//! check); a credit posts as a deposit into the card. The one exact gate is
//! `cl_model::forms::card::check` (the lines equal the amount to the cent);
//! the save maps the checked value field by field, with no arithmetic. The
//! balance owed is the engine's, read again after every save (Q-262).

pub(crate) mod kit;
mod payee;
pub(crate) mod records;
mod template;

use std::sync::Arc;

use cl_api::{
    ApiError, BankPosting, Capabilities, CardChargeDraft, DepositDraft, LocalValidation,
    RequestGroup, SplitLine, Transaction, TxnType, Versioned,
};
use cl_core::ids::{AccountId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle, Secret};
use cl_model::amount_entry::AmountRules;
use cl_model::forms::card::{CardField, CardForm, CardMode, CardProblem, CheckedCard, check};
use cl_model::forms::lines::{FormLine, FormLines, TotalsStatus, lines_total};
use cl_model::forms::{FormState, SaveIntent, SaveStart};
use cl_model::register::AccountClass;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Ui};
use serde_json::{Value, json};

use crate::company::demo::register::DemoPosting;
use crate::company::{CompanyEvent, CompanyLink};
use crate::forms::lines_grid::{L_ACCOUNT, L_AMOUNT, L_BILLABLE, L_CUSTOMER, LinesGrid, LinesSpec};
use crate::i18n::t;
use crate::screens::company::form::{self, DateInput};
use crate::screens::register::lists::RegisterLists;
use crate::theme::Tokens;
use crate::windows::{PromptAnswer, Screen};
use kit::{
    AmountInput, Balances, Books, MessageClick, NameInput, NameOutcome, Outbound, Pending,
    SaveFlow, Sent,
};
use records::{RecordForm, RecordScope, Records};

/// The command and window kind.
pub const KIND: &str = "vendors.card_charge";

/// The lines (every column; Billable is locked on a credit).
pub const LINES: LinesSpec = LinesSpec::all("card_charge/lines", "card.lines.label");

/// The Record Card Charge window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(CardChargeScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// Header fields (undo merges typing within one field).
mod field {
    pub const CARD: u64 = 1;
    pub const DATE: u64 = 2;
    pub const PAYEE: u64 = 3;
    pub const REF: u64 = 4;
    pub const AMOUNT: u64 = 5;
    pub const MEMO: u64 = 6;
}

/// The texts being typed in the header.
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub date: DateInput,
    pub payee: NameInput,
    pub amount: AmountInput,
    pub rev: u64,
}

/// The Record Card Charge window.
pub struct CardChargeScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    pub(crate) books: Books,
    pub(crate) balances: Balances,
    /// The header (its `lines` stay empty: the grid holds them).
    pub(crate) header: FormState<CardForm>,
    pub(crate) lines: LinesGrid<RegisterLists>,
    pub(crate) inputs: Inputs,
    pub(crate) flow: SaveFlow,
    pub(crate) problems: Vec<CardProblem>,
    pub(crate) bus: crate::data_bus::Subscription,
    lists_stale: bool,
    in_grid: bool,
    after_frame: Option<CommandId>,
    /// The fixed bars' sizes last frame (Q-265).
    layout: crate::layout::FrameReport,
    pub(crate) money_style: MoneyStyle,
    focus_amount: bool,
    /// Prev/Next and the saved record being read.
    pub(crate) records: Records,
    /// The header field that had the keyboard (Esc reverts it).
    pub(crate) focused: Option<u64>,
    /// Payee prefill and the recent charges.
    pub(crate) payee: payee::PayeeState,
    /// Save as Template and the template this form came from (P15).
    pub(crate) template: crate::screens::templates::SaveFlow,
}

/// The kinds of record this form shows: charges (expenses paid from the
/// card) and credits (deposits into it).
const CARD_TYPES: &[TxnType] = &[TxnType::Expense, TxnType::Deposit];

impl CardChargeScreen {
    /// A new card charge in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> CardChargeScreen {
        let group = link.group().and_then(Result::ok);
        let books = Books::default();
        let lines = LinesGrid::new(&LINES, Arc::clone(&books.lists), Vec::new());
        let mut s = CardChargeScreen {
            link,
            tenant,
            group,
            today,
            books,
            balances: Balances::default(),
            header: FormState::new(CardForm::new(None, Some(today))),
            lines,
            inputs: Inputs {
                date: DateInput::new(today),
                payee: NameInput::default(),
                amount: AmountInput::default(),
                rev: u64::MAX,
            },
            flow: SaveFlow::default(),
            problems: Vec::new(),
            bus: crate::data_bus::Subscription::detached(),
            lists_stale: false,
            in_grid: false,
            after_frame: None,
            layout: crate::layout::FrameReport::default(),
            money_style: MoneyStyle::default(),
            focus_amount: false,
            records: Records::default(),
            focused: None,
            payee: payee::PayeeState::default(),
            template: crate::screens::templates::SaveFlow::default(),
        };
        s.bus = s.link.bus().subscribe();
        s.books.start(&s.link, s.group.as_ref());
        s.poll();
        s
    }

    /// The whole form: the header with the grid's lines.
    #[must_use]
    pub(crate) fn compose(&self) -> CardForm {
        let mut f = self.header.current().clone();
        f.lines = self.lines.value().clone();
        f
    }

    /// The engine takes customers and billable marks on lines.
    pub(crate) fn line_billable(&self) -> bool {
        match self.link.demo_state() {
            Some(d) => !d.checks.line_billable_off,
            None => self.link.capabilities().has(Capabilities::LINE_BILLABLE),
        }
    }

    /// The card shown.
    pub(crate) fn card(&self) -> Option<AccountId> {
        self.header.current().card
    }

    /// The active credit card accounts.
    pub(crate) fn cards(&self) -> Vec<(AccountId, String)> {
        self.books.accounts_of(&[AccountClass::CreditCard])
    }

    pub(crate) fn poll(&mut self) {
        if self.books.poll() {
            self.lines.set_lists(Arc::clone(&self.books.lists));
            self.lock_columns();
            self.inputs.rev = u64::MAX;
            if self.card().is_none()
                && let Some((card, _)) = self.cards().first().cloned()
            {
                // Not an edit: the starting account is part of the blank form.
                let mut f = self.header.current().clone();
                f.card = Some(card);
                self.header.load(f);
                self.balances.fetch(&self.link, self.group.as_ref(), card);
            }
        }
        self.balances.poll();
        let (saved, voided) = self.flow.poll();
        match saved {
            Some(Ok(r)) => self.saved(r),
            Some(Err(e)) => self.save_failed(e),
            None => {}
        }
        if let Some(ev) = self.flow.lost.poll(&mut self.header) {
            self.lost_event(ev);
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
        self.poll_payee();
    }

    /// Billable is a charge's column only; without `line-billable-v1` the
    /// customer columns are locked too.
    fn lock_columns(&mut self) {
        let mut locked = Vec::new();
        if !self.line_billable() {
            locked.extend([L_CUSTOMER, L_BILLABLE]);
        } else if self.header.current().mode == CardMode::Credit {
            locked.push(L_BILLABLE);
        }
        self.lines.set_locked(locked);
    }

    /// Saves in other windows (Q-262): the balance owed again; the lists
    /// again once the form is clean.
    fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if changes.lists() {
            self.lists_stale = true;
        }
        if self.lists_stale && !self.is_dirty() && !self.books.busy() {
            self.lists_stale = false;
            self.books.start(&self.link, self.group.as_ref());
        }
        if let Some(card) = self.card()
            && changes.ledger_touches(card)
        {
            self.balances.fetch(&self.link, self.group.as_ref(), card);
        }
    }

    /// The header texts from the form.
    pub(crate) fn sync_inputs(&mut self) {
        let f = self.header.current();
        if let Some(d) = f.date {
            self.inputs.date = DateInput::new(d);
        }
        self.inputs.payee.text = f
            .payee
            .and_then(|p| self.books.lists.name_text(p))
            .unwrap_or_default()
            .to_string();
        self.inputs.payee.problem = false;
        self.inputs.amount.show(f.amount, self.money_style);
        self.inputs.rev = self.header.revision();
    }

    /// The form has unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.header.is_dirty() || self.lines.dirty()
    }

    /// The owner left the lines: a blank amount takes the lines total.
    fn left_lines(&mut self) {
        let mut f = self.compose();
        f.leave_lines();
        let (amount, from) = (f.amount, f.amount_from_lines);
        let h = self.header.current();
        if h.amount != amount || h.amount_from_lines != from {
            self.header.edit_step(|h| {
                h.amount = amount;
                h.amount_from_lines = from;
            });
            self.sync_inputs();
        }
    }

    /// The owner typed the amount: one line follows it.
    pub(crate) fn amount_typed(&mut self, amount: Option<Money>) {
        let mut f = self.compose();
        f.set_amount(amount);
        let lines = f.lines;
        self.header.edit(field::AMOUNT, |h| {
            h.amount = amount;
            h.amount_from_lines = false;
        });
        if &lines != self.lines.value() {
            self.lines.form_mut().edit_step(|v| *v = lines);
        }
        self.inputs.amount.show(amount, self.money_style);
        self.inputs.rev = self.header.revision();
    }

    fn amount_left(&mut self) {
        if let kit::Settled::Value(m) = self
            .inputs
            .amount
            .settle(self.money_style, AmountRules::MONEY)
            && (self.header.current().amount != m || self.header.current().amount_from_lines)
        {
            self.amount_typed(m);
        }
    }

    /// Charge or credit.
    pub(crate) fn set_mode(&mut self, mode: CardMode) {
        if self.header.current().mode == mode {
            return;
        }
        self.header.edit_step(|h| {
            h.mode = mode;
            // A credit is a deposit into the card, which has no reference
            // number: the field empties and is off (never dropped
            // silently at the save). Undo brings it back with Charge.
            if mode == CardMode::Credit {
                h.ref_no.clear();
            }
        });
        if mode == CardMode::Credit {
            // A credit has no billable lines (the engine takes none on a
            // deposit); the customer stays.
            let lines = self.lines.value().clone();
            if lines.lines.iter().any(|l| l.billable) {
                self.lines.form_mut().edit_step(|v| {
                    for l in &mut v.lines {
                        l.billable = false;
                    }
                });
            }
        }
        self.lock_columns();
    }

    fn set_card(&mut self, card: AccountId) {
        self.header.edit(field::CARD, |h| h.card = Some(card));
        self.balances.fetch(&self.link, self.group.as_ref(), card);
    }

    /// The tab title: "Record Card Charge" or "Record Card Credit".
    fn title_text(&self) -> String {
        match self.header.current().mode {
            CardMode::Charge => t("cmd.vendors.card_charge"),
            CardMode::Credit => t("card.title.credit"),
        }
    }

    fn busy(&self) -> bool {
        self.books.busy()
            || self.balances.busy()
            || self.flow.busy()
            || self.records.busy()
            || self.payee.busy()
    }

    // ------------------------------------------------------------- saving

    /// Commits the amount and payee text still being typed. False (nothing
    /// saved) when the typed text is not accepted.
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
        let payee_shown = self
            .header
            .current()
            .payee
            .and_then(|p| self.books.lists.name_text(p))
            .unwrap_or_default()
            .to_string();
        if self.inputs.payee.text.trim() != payee_shown.trim() {
            let lists = Arc::clone(&self.books.lists);
            let out = kit::resolve_name(&mut self.inputs.payee, &lists);
            self.apply_payee(out);
        }
        self.inputs.amount.problem.is_none() && !self.inputs.payee.problem
    }

    fn apply_payee(&mut self, out: NameOutcome) {
        match out {
            NameOutcome::Picked(id) => {
                let changed = self.header.current().payee != Some(id);
                self.header.edit(field::PAYEE, |h| h.payee = Some(id));
                self.inputs.payee.text = self
                    .books
                    .lists
                    .name_text(id)
                    .unwrap_or_default()
                    .to_string();
                if changed {
                    self.payee_picked(id);
                }
            }
            NameOutcome::Cleared => {
                self.header.edit(field::PAYEE, |h| h.payee = None);
            }
            NameOutcome::None => return,
        }
        self.inputs.rev = self.header.revision();
    }

    /// The checked form as the engine posting and the demo's copy, field
    /// by field (no arithmetic).
    pub(crate) fn outbound(&self, c: &CheckedCard) -> Result<Outbound, LocalValidation> {
        let split = |l: &cl_model::forms::lines::PostingLine| SplitLine {
            account: l.account,
            amount: l.amount,
            memo: l.memo.clone(),
            contact: l.contact,
            billable: l.billable,
            tag: l.tag,
        };
        let posting = match c.mode {
            CardMode::Charge => BankPosting::card_charge_with(
                CardChargeDraft {
                    pay_from: c.card,
                    date: c.date,
                    number: c.ref_no.clone(),
                    payee: c.payee,
                    memo: c.memo.clone(),
                    lines: c.lines.iter().map(split).collect(),
                },
                self.line_billable(),
            )?,
            CardMode::Credit => BankPosting::deposit(DepositDraft::plain(
                c.card,
                c.date,
                c.memo.clone(),
                c.lines
                    .iter()
                    .map(|l| SplitLine {
                        contact: l.contact.or(c.payee),
                        billable: false,
                        ..split(l)
                    })
                    .collect(),
            ))?,
        };
        let demo = DemoPosting {
            kind: match c.mode {
                CardMode::Charge => "card_charge",
                CardMode::Credit => "deposit",
            },
            date: c.date,
            // As the engine: a deposit (credit) has no number.
            number: match c.mode {
                CardMode::Charge => c.ref_no.clone().unwrap_or_default(),
                CardMode::Credit => String::new(),
            },
            contact: c.payee.and_then(|p| self.books.contact_name(p)),
            memo: c.memo.clone().unwrap_or_default(),
            own: c.card,
            own_debit: c.mode == CardMode::Credit,
            lines: c.lines.iter().map(|l| (l.account, l.amount)).collect(),
        };
        Ok(Outbound { posting, demo })
    }

    /// Checks the form and starts the save. Nothing is sent when the check
    /// doesn't pass (above all when the lines and the amount disagree by
    /// any amount). True when it saved at once or had nothing to save.
    pub(crate) fn start_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        if !self.flow.idle() && password.is_none() {
            return false;
        }
        if !self.commit_header() || !self.lines.commit() {
            return false;
        }
        self.left_lines();
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
                self.refuse(problems);
                return false;
            }
        };
        let out = match self.outbound(&checked) {
            Ok(o) => o,
            Err(LocalValidation::LineBillableUnavailable) => {
                self.flow.notice = Some("helper.line_billable_unavailable");
                return false;
            }
            Err(v) => {
                self.flow.error = Some(ApiError::Local(v));
                return false;
            }
        };
        let mut touched: Vec<AccountId> = vec![checked.card];
        touched.extend(checked.lines.iter().map(|l| l.account));
        self.flow.published = touched;
        let closing = self.books.closing;
        let sent = self.flow.send(
            &self.link,
            self.group.as_ref(),
            out,
            intent,
            password,
            closing,
        );
        match sent {
            Sent::Saved(r) => {
                self.saved(r);
                true
            }
            Sent::Queued => {
                let lists = Arc::clone(&self.books.lists);
                let _ = self.lines.form_mut().begin_save(intent, &*lists);
                let _: SaveStart<CardForm> = self.header.begin_save(intent);
                false
            }
            Sent::Asked | Sent::Failed => false,
        }
    }

    fn refuse(&mut self, problems: Vec<CardProblem>) {
        if let Some(first) = problems.first() {
            let lists = Arc::clone(&self.books.lists);
            let cell = |row, col| cl_grid::CellPos { row, col };
            match first.field() {
                CardField::Amount => self.focus_amount = true,
                CardField::Lines => self.lines.focus_first(),
                CardField::Line(k) => {
                    let col = match first {
                        CardProblem::Line(
                            _,
                            cl_model::forms::card::LineIssue::AmountNotPositive,
                        ) => L_AMOUNT,
                        _ => L_ACCOUNT,
                    };
                    self.lines.form_mut().focus(cell(k, col), &*lists);
                }
                CardField::Card | CardField::Date | CardField::RefNo => {}
            }
        }
        self.problems = problems;
    }

    /// The engine took the save (Q-262: every window showing these
    /// accounts reads them again).
    fn saved(&mut self, r: kit::SavedRecord) {
        self.template
            .form_saved(&self.link, self.group.as_ref(), r.id);
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
        let lines = self.lines.value().clone();
        self.lines.form_mut().save_ok(lines.clone(), &*lists);
        self.lines.form_mut().load(lines, &*lists);
        if let Some(card) = self.card() {
            self.balances.fetch(&self.link, self.group.as_ref(), card);
        }
        self.records.forget();
        self.payee.recent_for = None;
        self.follow_up(intent);
        self.after_saved();
    }

    fn save_failed(&mut self, e: ApiError) {
        let closing = self.books.closing;
        let failure = cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        };
        kit::header_failed(&mut self.header, &e, failure.clone());
        self.lines.form_mut().save_failed(failure);
        self.flow.failed(&self.link, e, closing);
    }

    /// The recheck or the owner answered an unknown save (B2). Found: the
    /// charge is in the books, so the form opens it.
    fn lost_event(&mut self, ev: crate::forms::lost_save::LostEvent) {
        if let crate::forms::lost_save::LostEvent::Saved(found) = ev {
            self.flow.found();
            crate::forms::publish_saved(&self.link, &self.bus, &self.flow.published);
            let lists = Arc::clone(&self.books.lists);
            let header = self.header.current().clone();
            self.header.load(header);
            let lines = self.lines.value().clone();
            self.lines.form_mut().load(lines, &*lists);
            if let Some(card) = self.card() {
                self.balances.fetch(&self.link, self.group.as_ref(), card);
            }
            self.records.forget();
            if let Some(id) = found {
                self.template
                    .form_saved(&self.link, self.group.as_ref(), id);
                self.open_saved(id);
            }
        }
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

    /// A blank form; Save & New keeps the card, the mode and the date.
    pub(crate) fn new_form(&mut self, keep_date: bool) {
        let current = self.header.current();
        let date = if keep_date {
            current.date.unwrap_or(self.today)
        } else {
            self.today
        };
        let mut f = CardForm::new(current.card, Some(date));
        f.mode = current.mode;
        self.header.load(f);
        let lists = Arc::clone(&self.books.lists);
        let fresh =
            FormLines::from_lines(Vec::new(), &mut self.lines.form_mut().adapter_mut().keys);
        self.lines.form_mut().load(fresh, &*lists);
        self.template.forget();
        self.problems.clear();
        self.inputs.payee = NameInput::default();
        self.flow.new_record();
        self.records.new_record();
        self.payee.prefill = None;
        self.payee.applied = false;
        self.payee.source = None;
        self.payee.dropped = false;
        self.lock_columns();
        self.sync_inputs();
    }

    /// Clear: throw the unsaved edits away.
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

    fn void_now(&mut self, password: Option<Secret>) {
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
        crate::forms::publish_saved(&self.link, &self.bus, &accounts);
        self.records.forget();
        if was_open {
            // The voided record leaves the form: a new one (as Write Check).
            self.new_form(true);
        }
        if let Some(card) = self.card() {
            self.balances.fetch(&self.link, self.group.as_ref(), card);
        }
    }

    /// The saved record `v` as this form: a charge is one credit on a card
    /// with debit lines; a credit is one debit on a card with credit lines.
    /// Anything else is shown read-only (`false`). Amounts are the
    /// engine's, copied as they are.
    pub(crate) fn form_of(&mut self, v: &Versioned<Transaction>) -> (CardForm, FormLines, bool) {
        let txn = &v.record;
        let is_card = |id| self.books.class(id) == Some(AccountClass::CreditCard);
        let mode = if txn.txn_type == TxnType::Deposit {
            CardMode::Credit
        } else {
            CardMode::Charge
        };
        // The card's side: credited on a charge, debited on a credit.
        let card_side = |l: &cl_api::TransactionLine| match mode {
            CardMode::Charge => l.credit.is_positive() && l.debit.is_zero(),
            CardMode::Credit => l.debit.is_positive() && l.credit.is_zero(),
        };
        let own: Vec<usize> = txn
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| card_side(l) && is_card(l.account_id))
            .map(|(i, _)| i)
            .collect();
        let card_line = match own.as_slice() {
            [i] => Some(*i),
            _ => None,
        };
        let line_amount = |l: &cl_api::TransactionLine| match mode {
            CardMode::Charge => l.debit,
            CardMode::Credit => l.credit,
        };
        let fits = card_line.is_some()
            && matches!(txn.txn_type, TxnType::Expense | TxnType::Deposit)
            && txn.lines.iter().enumerate().all(|(i, l)| {
                Some(i) == card_line
                    || (line_amount(l).is_positive() && !card_side(l) && {
                        let other = match mode {
                            CardMode::Charge => l.credit,
                            CardMode::Credit => l.debit,
                        };
                        other.is_zero()
                    })
            });
        let card = card_line.map(|i| txn.lines[i].account_id);
        let mut f = CardForm::new(card, Some(txn.txn_date));
        f.mode = mode;
        f.payee = txn.contact_id;
        f.ref_no = txn.txn_number.clone().unwrap_or_default();
        // The card's own line is on the other side from the lines.
        f.amount = card_line.map(|i| match mode {
            CardMode::Charge => txn.lines[i].credit,
            CardMode::Credit => txn.lines[i].debit,
        });
        f.memo = txn.memo.clone().unwrap_or_default();
        let keys = &mut self.lines.form_mut().adapter_mut().keys;
        let seeds = txn
            .lines
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != card_line)
            .map(|(_, l)| FormLine {
                account: Some(l.account_id),
                amount: Some(line_amount(l)),
                memo: l.description.clone().unwrap_or_default(),
                customer: l.contact_id,
                billable: l.is_billable,
                tag: l.tag_id,
                ..FormLine::blank(keys.next_key())
            })
            .collect();
        (f, FormLines::from_lines(seeds, keys), fits)
    }

    /// Shows a saved charge or credit (Prev/Next, the register, Reload).
    pub(crate) fn show(&mut self, v: &Versioned<Transaction>) {
        let (form, lines, fits) = self.form_of(v);
        let lists = Arc::clone(&self.books.lists);
        self.header.load(form);
        self.lines.form_mut().load(lines, &*lists);
        self.template.forget();
        self.flow.opened_txn(v, fits);

        self.problems.clear();
        self.payee.prefill = None;
        self.payee.applied = false;
        self.payee.dropped = false;
        self.focused = None;
        self.lock_columns();
        self.sync_inputs();
        if let Some(card) = self.card() {
            self.balances.fetch(&self.link, self.group.as_ref(), card);
        }
    }

    /// Reads the open record again (it changed elsewhere).
    fn reload(&mut self) {
        if let Some(id) = self.flow.editing.map(|e| e.id) {
            self.open_saved(id);
        }
    }

    /// Esc: the question, the void card or the names list closes first;
    /// then the focused field goes back to its saved value (or drops the
    /// text typed in it). With nothing to revert the shell closes the
    /// window, asking first when the form is dirty.
    pub(crate) fn revert_field(&mut self) -> bool {
        if self.records.prompt.is_some() {
            self.answer_nav(PromptAnswer::Keep);
            return true;
        }
        if self.flow.void_open {
            self.flow.void_open = false;
            return true;
        }
        if self.inputs.payee.open {
            self.inputs.payee.open = false;
            return true;
        }
        let Some(f) = self.focused else { return false };
        let saved = self.header.original().clone();
        let now = self.header.current().clone();
        let typed = match f {
            field::AMOUNT => {
                self.inputs.amount.text.trim()
                    != now
                        .amount
                        .map(|m| m.format(self.money_style))
                        .unwrap_or_default()
            }
            field::PAYEE => {
                self.inputs.payee.text.trim()
                    != now
                        .payee
                        .and_then(|p| self.books.lists.name_text(p))
                        .unwrap_or_default()
            }
            field::DATE => now
                .date
                .is_some_and(|d| DateInput::new(d).text != self.inputs.date.text),
            _ => false,
        };
        let differs = match f {
            field::CARD => now.card != saved.card,
            field::DATE => now.date != saved.date,
            field::PAYEE => now.payee != saved.payee,
            field::REF => now.ref_no != saved.ref_no,
            field::AMOUNT => {
                now.amount != saved.amount || now.amount_from_lines != saved.amount_from_lines
            }
            field::MEMO => now.memo != saved.memo,
            _ => false,
        };
        if differs {
            self.header.edit_step(|h| match f {
                field::CARD => h.card = saved.card,
                field::DATE => h.date = saved.date,
                field::PAYEE => h.payee = saved.payee,
                field::REF => h.ref_no.clone_from(&saved.ref_no),
                field::AMOUNT => {
                    h.amount = saved.amount;
                    h.amount_from_lines = saved.amount_from_lines;
                }
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
        let empty = self.cards().is_empty();
        self.layout = kit::form_frame(ui, "card_charge", empty, |ui, col| match col {
            kit::Column::Top => {
                self.toolbar(ui, &tokens);
                match kit::messages(ui, &tokens, &mut self.flow, self.books.closing) {
                    MessageClick::SubmitClosing => self.submit_closing(),
                    MessageClick::Void => self.void_now(None),
                    MessageClick::Reload => self.reload(),
                    MessageClick::None => {}
                }
                if self.records.prompt.is_some()
                    && let Some(a) = records::prompt_ui(ui, KIND, &self.title_text())
                {
                    self.answer_nav(a);
                }
            }
            kit::Column::Empty => {
                ui.add_space(8.0);
                ui.label(RichText::new(t("card.no_card")).color(tokens.text_secondary));
                if ui.button(t("helper.open_chart")).clicked() {
                    self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                        crate::screens::lists::accounts::KIND,
                    )));
                }
            }
            kit::Column::Form => self.form_ui(ui, &tokens),
            kit::Column::Panel => self.panel_ui(ui, &tokens),
            kit::Column::Actions => {
                let enabled = self.flow.idle();
                if let Some(cmd) = kit::actions_row(ui, enabled, enabled) {
                    self.after_frame = Some(CommandId::new(cmd));
                }
            }
        });
    }

    fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let idle = self.flow.idle();
        let mode = self.header.current().mode;
        let mut picked = None;
        let clicked = kit::toolbar(ui, tokens, idle, self.is_dirty(), |ui| {
            ui.separator();
            ui.add_enabled_ui(idle, |ui| {
                if ui
                    .radio(mode == CardMode::Charge, t("card.mode.charge"))
                    .clicked()
                {
                    picked = Some(CardMode::Charge);
                }
                if ui
                    .radio(mode == CardMode::Credit, t("card.mode.credit"))
                    .on_hover_text(t("card.mode.credit.tip"))
                    .clicked()
                {
                    picked = Some(CardMode::Credit);
                }
            });
            self.template_slot(ui);
        });
        if let Some(cmd) = clicked {
            self.after_frame = Some(CommandId::new(cmd));
        }
        if let Some(m) = picked {
            self.set_mode(m);
        }
    }

    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let enabled = self.flow.idle();
        self.card_row(ui, tokens, enabled);
        ui.add_space(10.0);
        ui.add_enabled_ui(enabled, |ui| self.header_ui(ui, tokens));
        ui.add_space(12.0);
        let total = lines_total(self.lines.value()).unwrap_or(Money::ZERO);
        let job = kit::with_figure(
            ui,
            "card.lines.heading",
            "total",
            &total.format(self.money_style),
            14.0,
            tokens.text_primary,
        );
        ui.add(egui::Label::new(job).selectable(false));
        ui.add_space(4.0);
        ui.add_enabled_ui(enabled, |ui| {
            let _ = self.lines.ui(ui);
        });
        if !self.line_billable() {
            form::hint(ui, tokens, &t("helper.lines.billable_later"));
        }
        ui.add_space(6.0);
        self.totals_ui(ui, tokens);
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
        if let Some(ev) = kit::status(
            ui,
            tokens,
            &mut self.flow,
            &mut self.header,
            &self.link,
            self.group.as_ref(),
            "card_charge",
        ) {
            self.lost_event(ev);
        }
    }

    fn card_row(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let cards = self.cards();
        let current = self.card();
        let mut picked = None;
        let title = self.title_text();
        kit::title_row(ui, tokens, "card_charge", &title, |ui| {
            kit::wrap_item(ui, "card_charge/card", |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    let (p, r) = kit::account_combo(
                        ui,
                        tokens,
                        "card_charge/card",
                        &t("card.card"),
                        &cards,
                        current,
                        300.0,
                    );
                    picked = p;
                    kit::track(&mut self.focused, &r, field::CARD);
                });
            });
            kit::wrap_item(ui, "card_charge/balance", |ui| {
                ui.add_space(20.0);
                let owed = current
                    .and_then(|c| self.balances.shown(c, Some(AccountClass::CreditCard)))
                    .map(|m| m.format(self.money_style));
                kit::balance_line(ui, tokens, &t("card.balance_owed"), owed);
            });
        });
        if let Some(id) = picked {
            self.set_card(id);
        }
    }

    fn header_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        egui::Frame::new()
            .fill(tokens.surface_form)
            .stroke(Stroke::new(1.0, tokens.accent_border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let width = ui.available_width();
                let sp = ui.spacing().item_spacing.x;
                let third = ((width - 2.0 * (16.0 + sp)) / 3.0).max(120.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(third * 2.0 + 16.0);
                        let lists = Arc::clone(&self.books.lists);
                        let current = self.header.current().payee;
                        let (out, r) = kit::name_field(
                            ui,
                            tokens,
                            &t("card.payee"),
                            &mut self.inputs.payee,
                            &lists,
                            current,
                            third * 2.0 + 16.0,
                        );
                        self.apply_payee(out);
                        kit::track(&mut self.focused, &r, field::PAYEE);
                        if crate::forms::enter::entered(&r).saves() {
                            self.after_frame = Some(CommandId::new("form.save_new"));
                        }
                    });
                    ui.add_space(16.0);
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
                        if crate::forms::enter::entered(&r).saves() {
                            self.after_frame = Some(CommandId::new("form.save_new"));
                        }
                    });
                });
                ui.add_space(8.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(third);
                        let mut text = self.header.current().ref_no.clone();
                        let charge = self.header.current().mode == CardMode::Charge;
                        let r = ui
                            .add_enabled_ui(charge, |ui| {
                                kit::text_field(
                                    ui,
                                    tokens,
                                    &t("card.ref_no"),
                                    &mut text,
                                    third,
                                    cl_model::forms::card::REF_MAX,
                                )
                            })
                            .inner
                            .on_disabled_hover_text(t("card.ref_no.credit"));
                        if charge && r.changed() {
                            self.header.edit(field::REF, |h| h.ref_no = text);
                            self.inputs.rev = self.header.revision();
                        }
                        kit::track(&mut self.focused, &r, field::REF);
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(third);
                        let mut text = self.header.current().memo.clone();
                        let r = kit::text_field(
                            ui,
                            tokens,
                            &t("helper.memo"),
                            &mut text,
                            third,
                            cl_api::PRINTED_MEMO_MAX,
                        );
                        if r.changed() {
                            self.header.edit(field::MEMO, |h| h.memo = text);
                            self.inputs.rev = self.header.revision();
                        }
                        kit::track(&mut self.focused, &r, field::MEMO);
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(third);
                        let r = kit::amount_field(
                            ui,
                            tokens,
                            &t("helper.amount"),
                            &mut self.inputs.amount,
                            third,
                            true,
                        );
                        if std::mem::take(&mut self.focus_amount) {
                            r.request_focus();
                        }
                        kit::track(&mut self.focused, &r, field::AMOUNT);
                        if r.lost_focus() {
                            self.amount_left();
                            if crate::forms::enter::entered(&r).saves()
                                && self.inputs.amount.problem.is_none()
                            {
                                self.after_frame = Some(CommandId::new("form.save_new"));
                            }
                        }
                    });
                });
                ui.add_space(8.0);
                let header = self.header.current().header_tags.clone();
                if let Some(edit) =
                    kit::tags_row(ui, tokens, "card_charge/tags", &self.books.tags, &header)
                {
                    let tags = self.books.tags.clone();
                    self.header
                        .edit_step(|h| kit::apply_tag_edit(&tags, &mut h.header_tags, edit));
                }
            });
    }

    fn totals_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let form = self.compose();
        let total = lines_total(&form.lines).unwrap_or(Money::ZERO);
        let (key, var, figure, warn) = match form.totals_status() {
            Some(TotalsStatus::Balanced) => ("helper.totals.balanced", "total", total, false),
            Some(TotalsStatus::Remaining(m)) => ("helper.totals.remaining", "amount", m, true),
            Some(TotalsStatus::Over(m)) => ("helper.totals.over", "amount", m, true),
            None => ("helper.totals.lines", "total", total, false),
        };
        let color = if warn {
            tokens.warn_text
        } else {
            tokens.text_secondary
        };
        let job = kit::with_figure(ui, key, var, &figure.format(self.money_style), 12.0, color);
        crate::widgets::label_with_check(ui, job, false);
    }

    fn panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        kit::panel_card(ui, tokens, &t("card.panel.title"), |ui| {
            let owed = self
                .card()
                .and_then(|c| self.balances.shown(c, Some(AccountClass::CreditCard)))
                .map(|m| m.format(self.money_style));
            kit::balance_line(ui, tokens, &t("card.balance_owed"), owed);
            form::hint(ui, tokens, &t("card.panel.explain"));
        });
        self.payee_panel_ui(ui, tokens);
    }

    /// Keymap commands the grid and the closing dialog did not take.
    fn form_command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "form.save_as_template" => {
                self.save_as_template();
                true
            }
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
                self.inputs.payee.open = true;
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

    /// The control-channel state: flags and counts only (never an amount,
    /// a name or a memo).
    pub(crate) fn state_json(&self) -> Value {
        let f = self.compose();
        let totals = f.totals_status().map(|s| match s {
            TotalsStatus::Balanced => "balanced",
            TotalsStatus::Remaining(_) => "remaining",
            TotalsStatus::Over(_) => "over",
        });
        let mut v = json!({
            "screen": "card_charge",
            "layout": self.layout.json(),
            "company_open": self.tenant.is_some(),
            "loaded": self.books.loaded,
            "mode": match f.mode { CardMode::Charge => "charge", CardMode::Credit => "credit" },
            "card_id": f.card.map(|c| c.to_string()),
            "balance_owed_shown": f.card.is_some_and(|c| self.balances.known.contains_key(&c)),
            "has_payee": f.payee.is_some(),
            "amount_set": f.amount.is_some(),
            "lines": f.lines.used().count(),
            "totals": totals,
            "dirty": self.is_dirty(),
            "line_billable": self.line_billable(),
            "published_accounts": self.flow.published.len(),
            "header_tags": f.header_tags.len(),
            "save": {
                "refused": self.problems.first().map(|p| p.name()),
                "problems": self.problems.len(),
            },
            "grid": { "focused": self.lines.has_focus() },
            "nav_prompt": self.records.prompt.is_some(),
            "nav_at": self.records.at,
            "focused": self.focused,
            "prefill": {
                "applied": self.payee.applied,
                "source": self.payee.source,
                "dropped": self.payee.dropped,
                "pending": self.payee.prefill.is_some(),
            },
            "recent_charges": self.payee.recent.as_ref().map(Vec::len),
            "template": self.template.state_json(),
        });
        if let (Value::Object(m), Value::Object(c)) = (&mut v, self.flow.state_json(&self.link)) {
            m.extend(c);
        }
        v
    }
}

impl Screen for CardChargeScreen {
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
        if self.inputs.rev != self.header.revision() {
            self.sync_inputs();
        }
        self.draw(ui);
        // A refused save's problems follow the owner's fixes.
        self.recheck_problems();
        let in_grid = self.lines.has_focus();
        if self.in_grid && !in_grid {
            self.left_lines();
        }
        self.in_grid = in_grid;
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

    fn save_unconfirmed(&self) -> bool {
        self.flow.lost.pending()
    }

    fn save(&mut self) -> bool {
        self.start_save(SaveIntent::Save, None)
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(handled) = self.flow.closing_prompt.command(id) {
            return handled;
        }
        if let Some(handled) = self.lines.command(id) {
            return handled;
        }
        self.form_command(id)
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.open_saved(txn)
    }

    fn open_template(&mut self, t: crate::forms::template::TemplateOpen) -> bool {
        self.fill_template(&t)
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        true
    }
}

impl RecordForm for CardChargeScreen {
    fn parts(&mut self) -> (&mut Records, &Arc<CompanyLink>, Option<&RequestGroup>) {
        (&mut self.records, &self.link, self.group.as_ref())
    }

    fn record_scope(&self) -> Option<RecordScope> {
        self.card().map(|account| RecordScope {
            account,
            types: CARD_TYPES,
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
        self.lines.form_mut().revert_form(&*lists);
        self.problems.clear();
        self.lock_columns();
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

impl CardChargeScreen {
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
pub(crate) mod tests;
