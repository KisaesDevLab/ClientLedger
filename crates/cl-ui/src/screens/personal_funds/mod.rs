//! Paid with Personal Funds (P14 lane C, `vendors.personal_funds`; Q-85,
//! Q-P14n, Q-P14o), on the Write Check pattern (Q-261).
//!
//! The owner paid a business expense with their own money. "How should we
//! record it?": "Money I put into the business" (an expense paid from the
//! owner's contribution account) or "The business will pay me back" (paid
//! from a liability such as "Due to owner", with a quick add). The default
//! follows the entity type and says so ("Default for a single-member
//! LLC"). The lines must equal the amount to the cent
//! (`cl_model::forms::personal_funds::check`); it posts as an expense paid
//! from that account (`BankPosting::card_charge_with`).

use std::sync::Arc;

use cl_api::{
    Account, AccountFields, AccountType, ApiError, BankPosting, Capabilities, CardChargeDraft,
    IdempotencyKey, LocalValidation, OrphanLabel, RequestGroup, SplitLine,
};
use cl_core::ids::{AccountId, TenantId};
use cl_core::{CalendarDate, Money, MoneyStyle, Secret};
use cl_model::amount_entry::AmountRules;
use cl_model::forms::lines::{FormLines, TotalsStatus, lines_total};
use cl_model::forms::owner_money::{Entity, EquitySource};
use cl_model::forms::personal_funds::{
    CheckedPersonalFunds, PersonalFundsForm, PersonalFundsProblem, Treatment, check,
    default_treatment, pick_offset,
};
use cl_model::forms::{FormState, SaveIntent};
use cl_model::register::AccountClass;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Ui};
use serde_json::{Value, json};

use crate::company::demo::register::DemoPosting;
use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::forms::lines_grid::{L_BILLABLE, L_CUSTOMER, LinesGrid, LinesSpec};
use crate::i18n::t;
use crate::screens::card_charge::kit::{
    self, AmountInput, Balances, Books, MessageClick, NameInput, NameOutcome, Outbound, Pending,
    SaveFlow, Sent, Settled,
};
use crate::screens::company::form::{self, DateInput};
use crate::screens::owner_money::{company_key, remember};
use crate::screens::register::lists::RegisterLists;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "vendors.personal_funds";

/// What the money was for (every column, as on a check).
pub const LINES: LinesSpec = LinesSpec::all("personal_funds/lines", "personal_funds.lines.label");

/// The number the demo gives "Due to owner" (synthetic).
const DEMO_DUE_TO_OWNER: u32 = 2410;

/// The window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(PersonalFundsScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

mod field {
    pub const WHO: u64 = 1;
    pub const PAYEE: u64 = 2;
    pub const DATE: u64 = 3;
    pub const MEMO: u64 = 4;
    pub const AMOUNT: u64 = 5;
    pub const OFFSET: u64 = 6;
}

/// The texts being typed.
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub date: DateInput,
    pub who: NameInput,
    pub payee: NameInput,
    pub amount: AmountInput,
    pub rev: u64,
}

/// The "default for …" label of an entity type.
pub(crate) const fn default_label(entity: Option<Entity>) -> &'static str {
    match entity {
        Some(Entity::SoleProp) => "personal_funds.default.sole_prop",
        Some(Entity::SingleMemberLlc) => "personal_funds.default.smllc",
        Some(Entity::Partnership) => "personal_funds.default.partnership",
        Some(Entity::SCorp) => "personal_funds.default.s_corp",
        Some(Entity::CCorp) => "personal_funds.default.c_corp",
        None => "personal_funds.default.unknown",
    }
}

/// The Paid with Personal Funds window.
pub struct PersonalFundsScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    pub(crate) books: Books,
    pub(crate) balances: Balances,
    /// The header (its `lines` stay empty: the grid holds them).
    pub(crate) header: FormState<PersonalFundsForm>,
    pub(crate) lines: LinesGrid<RegisterLists>,
    pub(crate) inputs: Inputs,
    pub(crate) flow: SaveFlow,
    pub(crate) problems: Vec<PersonalFundsProblem>,
    /// Where the account came from (`None`: the owner picked it).
    pub(crate) offset_source: Option<EquitySource>,
    /// The treatment is still the entity type's default.
    pub(crate) treatment_default: bool,
    quick_add: Option<Task<AccountId>>,
    /// The quick-added account to pick once the lists arrive again.
    pending_offset: Option<AccountId>,
    pub(crate) bus: crate::data_bus::Subscription,
    lists_stale: bool,
    in_grid: bool,
    after_frame: Option<CommandId>,
    pub(crate) money_style: MoneyStyle,
}

impl PersonalFundsScreen {
    /// A new form in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> PersonalFundsScreen {
        let group = link.group().and_then(Result::ok);
        let books = Books::default();
        let lines = LinesGrid::new(&LINES, Arc::clone(&books.lists), Vec::new());
        let mut s = PersonalFundsScreen {
            link,
            tenant,
            group,
            today,
            books,
            balances: Balances::default(),
            header: FormState::new(PersonalFundsForm::new(Some(today), Treatment::PutIn)),
            lines,
            inputs: Inputs {
                date: DateInput::new(today),
                who: NameInput::default(),
                payee: NameInput::default(),
                amount: AmountInput::default(),
                rev: u64::MAX,
            },
            flow: SaveFlow::default(),
            problems: Vec::new(),
            offset_source: None,
            treatment_default: true,
            quick_add: None,
            pending_offset: None,
            bus: crate::data_bus::Subscription::detached(),
            lists_stale: false,
            in_grid: false,
            after_frame: None,
            money_style: MoneyStyle::default(),
        };
        s.bus = s.link.bus().subscribe();
        s.books.start(&s.link, s.group.as_ref());
        s.poll();
        s
    }

    /// The whole form: the header with the grid's lines.
    #[must_use]
    pub(crate) fn compose(&self) -> PersonalFundsForm {
        let mut f = self.header.current().clone();
        f.lines = self.lines.value().clone();
        f
    }

    fn line_billable(&self) -> bool {
        match self.link.demo_state() {
            Some(d) => !d.checks.line_billable_off,
            None => self.link.capabilities().has(Capabilities::LINE_BILLABLE),
        }
    }

    /// The accounts a treatment records into: equity for "put in",
    /// liabilities other than cards and payables for "pay back".
    pub(crate) fn offsets(&self, treatment: Treatment) -> Vec<(AccountId, String)> {
        match treatment {
            Treatment::PutIn => self.books.accounts_of(&[AccountClass::Equity]),
            Treatment::PayBack => self.books.accounts_of(&[AccountClass::OtherLiability]),
        }
    }

    fn remembered(&self, treatment: Treatment) -> Option<AccountId> {
        let slot = match treatment {
            Treatment::PutIn => remember::Slot::In,
            Treatment::PayBack => remember::Slot::PayBack,
        };
        remember::get(
            remember::dir_of(&self.link).as_deref(),
            &company_key(self.tenant.as_ref()),
            slot,
        )
    }

    /// The account the form starts on for `treatment`.
    pub(crate) fn default_offset(&self, treatment: Treatment) -> Option<(AccountId, EquitySource)> {
        let usable: Vec<AccountId> = self
            .offsets(treatment)
            .into_iter()
            .map(|(a, _)| a)
            .collect();
        pick_offset(
            treatment,
            &self.books.roles,
            self.books.entity,
            self.remembered(treatment),
            &usable,
        )
    }

    pub(crate) fn poll(&mut self) {
        if self.books.poll() {
            self.lines.set_lists(Arc::clone(&self.books.lists));
            if !self.line_billable() {
                self.lines.set_locked(vec![L_CUSTOMER, L_BILLABLE]);
            }
            self.inputs.rev = u64::MAX;
            if let Some(a) = self.pending_offset.take() {
                self.pick_offset(a);
            } else if !self.header.is_dirty() {
                let treatment = if self.treatment_default {
                    default_treatment(self.books.entity)
                } else {
                    self.header.current().treatment
                };
                let mut f = self.header.current().clone();
                f.treatment = treatment;
                if let Some((a, src)) = self.default_offset(treatment) {
                    f.offset = Some(a);
                    self.offset_source = Some(src);
                }
                self.header.load(f);
            }
        }
        self.balances.poll();
        if let Some(r) = self.quick_add.as_mut().and_then(Task::try_take) {
            self.quick_add = None;
            match r {
                Ok(id) => {
                    self.pending_offset = Some(id);
                    self.books.start(&self.link, self.group.as_ref());
                    if self.link.is_demo() {
                        self.poll();
                    }
                }
                Err(e) => self.flow.error = Some(e),
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
        if let Some(a) = self.header.current().offset
            && changes.ledger_touches(a)
        {
            self.balances.fetch(&self.link, self.group.as_ref(), a);
        }
    }

    pub(crate) fn sync_inputs(&mut self) {
        let f = self.header.current();
        let lists = &self.books.lists;
        if let Some(d) = f.date {
            self.inputs.date = DateInput::new(d);
        }
        self.inputs.who.text = f
            .who
            .and_then(|p| lists.name_text(p))
            .unwrap_or_default()
            .to_string();
        self.inputs.payee.text = f
            .payee
            .and_then(|p| lists.name_text(p))
            .unwrap_or_default()
            .to_string();
        self.inputs.who.problem = false;
        self.inputs.payee.problem = false;
        self.inputs.amount.show(f.amount, self.money_style);
        self.inputs.rev = self.header.revision();
    }

    /// The form has unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.header.is_dirty() || self.lines.dirty()
    }

    /// "How should we record it?": the account follows.
    pub(crate) fn set_treatment(&mut self, treatment: Treatment) {
        if self.header.current().treatment == treatment {
            return;
        }
        self.treatment_default = treatment == default_treatment(self.books.entity);
        let next = self.default_offset(treatment);
        self.header.edit_step(|h| {
            h.treatment = treatment;
            h.offset = next.map(|(a, _)| a);
        });
        self.offset_source = next.map(|(_, s)| s);
    }

    /// The owner picked the account: remembered for this company.
    pub(crate) fn pick_offset(&mut self, account: AccountId) {
        let treatment = self.header.current().treatment;
        self.header
            .edit(field::OFFSET, |h| h.offset = Some(account));
        self.offset_source = None;
        let slot = match treatment {
            Treatment::PutIn => remember::Slot::In,
            Treatment::PayBack => remember::Slot::PayBack,
        };
        remember::set(
            remember::dir_of(&self.link).as_deref(),
            &company_key(self.tenant.as_ref()),
            slot,
            account,
        );
    }

    /// Quick add "Due to owner" (a current liability) and pick it.
    pub(crate) fn quick_add_due_to_owner(&mut self) {
        if self.quick_add.is_some() {
            return;
        }
        let fields = AccountFields {
            name: t("personal_funds.due_to_owner"),
            account_number: None,
            account_type: AccountType::Liability,
            detail_type: Some("other_current_liability".into()),
            description: None,
            parent_id: None,
        };
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let id = crate::company::demo::account_id(DEMO_DUE_TO_OWNER);
            let chart = demo.chart.get_or_insert_with(crate::company::demo::chart);
            if !chart.iter().any(|a| a.id == id) {
                chart.push(Account {
                    id,
                    fields: AccountFields {
                        account_number: Some(DEMO_DUE_TO_OWNER.to_string()),
                        ..fields
                    },
                    is_active: true,
                    is_system: false,
                    system_tag: None,
                    balance: Some(Money::ZERO),
                    created_at: None,
                    updated_at: None,
                });
            }
            drop(demo);
            self.quick_add = Some(Task::ready(id));
            self.poll();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.helper_account",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            Ok(s.accounts().create(&fields, key).await?.record.id)
        }) {
            self.quick_add = Some(Task::Pending(p));
        }
    }

    fn apply_name(&mut self, which: u64, out: NameOutcome) {
        let value = match out {
            NameOutcome::Picked(id) => Some(id),
            NameOutcome::Cleared => None,
            NameOutcome::None => return,
        };
        self.header.edit(which, |h| {
            if which == field::WHO {
                h.who = value;
            } else {
                h.payee = value;
            }
        });
        self.inputs.rev = self.header.revision();
    }

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

    fn amount_left(&mut self) {
        if let Settled::Value(m) = self
            .inputs
            .amount
            .settle(self.money_style, AmountRules::MONEY)
        {
            self.header.edit(field::AMOUNT, |h| {
                h.amount = m;
                h.amount_from_lines = false;
            });
            // One used line with no amount follows the typed amount.
            let lines = self.lines.value().clone();
            let used: Vec<usize> = lines
                .lines
                .iter()
                .enumerate()
                .filter(|(_, l)| !l.is_blank())
                .map(|(i, _)| i)
                .collect();
            if let [i] = used.as_slice()
                && lines.lines[*i].amount.is_none()
            {
                let i = *i;
                self.lines.form_mut().edit_step(|v| v.lines[i].amount = m);
            }
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
        let lists = Arc::clone(&self.books.lists);
        for which in [field::WHO, field::PAYEE] {
            let (current, input) = if which == field::WHO {
                (self.header.current().who, &mut self.inputs.who)
            } else {
                (self.header.current().payee, &mut self.inputs.payee)
            };
            let shown = current
                .and_then(|p| lists.name_text(p))
                .unwrap_or_default()
                .to_string();
            if input.text.trim() != shown.trim() {
                let out = kit::resolve_name(input, &lists);
                self.apply_name(which, out);
            }
        }
        self.inputs.amount.problem.is_none()
            && !self.inputs.who.problem
            && !self.inputs.payee.problem
    }

    /// The checked form as the engine posting and the demo's copy.
    pub(crate) fn outbound(&self, c: &CheckedPersonalFunds) -> Result<Outbound, LocalValidation> {
        let posting = BankPosting::card_charge_with(
            CardChargeDraft {
                pay_from: c.offset,
                date: c.date,
                number: None,
                payee: c.payee,
                memo: c.memo.clone(),
                lines: c
                    .lines
                    .iter()
                    .map(|l| SplitLine {
                        account: l.account,
                        amount: l.amount,
                        memo: l.memo.clone(),
                        contact: l.contact,
                        billable: l.billable,
                        tag: l.tag,
                    })
                    .collect(),
            },
            self.line_billable(),
        )?;
        let demo = DemoPosting {
            kind: "card_charge",
            date: c.date,
            number: String::new(),
            contact: c.payee.and_then(|p| self.books.contact_name(p)),
            memo: c.memo.clone().unwrap_or_default(),
            own: c.offset,
            own_debit: false,
            lines: c.lines.iter().map(|l| (l.account, l.amount)).collect(),
        };
        Ok(Outbound { posting, demo })
    }

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
            Err(p) => {
                self.problems = p;
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
        let mut touched = vec![checked.offset];
        touched.extend(checked.lines.iter().map(|l| l.account));
        self.flow.published = touched;
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
                let _ = self.lines.form_mut().begin_save(intent, &*lists);
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
        if let Some(a) = self.header.current().offset {
            self.balances.fetch(&self.link, self.group.as_ref(), a);
        }
        self.follow_up(intent);
    }

    fn save_failed(&mut self, e: ApiError) {
        let closing = self.books.closing;
        let failure = cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        };
        self.header.save_failed(failure.clone());
        self.lines.form_mut().save_failed(failure);
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

    /// A blank form; keeps who, the treatment and the account.
    pub(crate) fn new_form(&mut self, keep_date: bool) {
        let c = self.header.current().clone();
        let mut f = PersonalFundsForm::new(
            Some(if keep_date {
                c.date.unwrap_or(self.today)
            } else {
                self.today
            }),
            c.treatment,
        );
        f.who = c.who;
        f.offset = c.offset;
        self.header.load(f);
        let lists = Arc::clone(&self.books.lists);
        let fresh =
            FormLines::from_lines(Vec::new(), &mut self.lines.form_mut().adapter_mut().keys);
        self.lines.form_mut().load(fresh, &*lists);
        self.problems.clear();
        self.inputs.payee = NameInput::default();
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
        let accounts = self.flow.published.clone();
        self.flow.voided();
        crate::forms::publish_saved(&self.link, &self.bus, &accounts);
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.books.loaded {
            form::progress_line(ui, &tokens, &t("helper.loading"));
            return;
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(t("personal_funds.intro"))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
            kit::heading_right(ui, &tokens, &t("cmd.vendors.personal_funds"));
        });
        ui.separator();
        match kit::messages(ui, &tokens, &mut self.flow) {
            MessageClick::SubmitClosing => self.submit_closing(),
            MessageClick::Void => self.void_now(None),
            MessageClick::None => {}
        }
        kit::two_columns(ui, "personal_funds/scroll", |ui, col| match col {
            kit::Column::Form => self.form_ui(ui, &tokens),
            kit::Column::Panel => Self::panel_ui(ui, &tokens),
        });
    }

    #[allow(clippy::too_many_lines)] // One card of fields, top to bottom.
    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let enabled = self.flow.idle();
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
                    let lists = Arc::clone(&self.books.lists);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let (out, _) = kit::name_field(
                                ui,
                                tokens,
                                &t("personal_funds.who"),
                                &mut self.inputs.who,
                                &lists,
                                f.who,
                                half,
                            );
                            self.apply_name(field::WHO, out);
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            let (out, r) = kit::name_field(
                                ui,
                                tokens,
                                &t("personal_funds.payee"),
                                &mut self.inputs.payee,
                                &lists,
                                f.payee,
                                half,
                            );
                            self.apply_name(field::PAYEE, out);
                            if kit::entered(&r) {
                                self.after_frame = Some(CommandId::new("form.save_new"));
                            }
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal_top(|ui| {
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
                            if kit::entered(&r) {
                                self.after_frame = Some(CommandId::new("form.save_new"));
                            }
                        });
                        ui.add_space(16.0);
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
                            if r.lost_focus() {
                                self.amount_left();
                            }
                        });
                    });
                    ui.add_space(8.0);
                    let mut memo = f.memo.clone();
                    let r = kit::text_field(
                        ui,
                        tokens,
                        &t("helper.memo"),
                        &mut memo,
                        width,
                        cl_api::PRINTED_MEMO_MAX,
                    );
                    if r.changed() {
                        self.header.edit(field::MEMO, |h| h.memo = memo);
                    }
                    ui.add_space(8.0);
                    self.treatment_ui(ui, tokens, half);
                });
        });
        ui.add_space(12.0);
        let total = lines_total(self.lines.value()).unwrap_or(Money::ZERO);
        let job = kit::with_figure(
            ui,
            "personal_funds.lines.heading",
            "total",
            &total.format(self.money_style),
            14.0,
            tokens.text_primary,
        );
        ui.add(egui::Label::new(job).selectable(false));
        ui.add_enabled_ui(enabled, |ui| {
            let _ = self.lines.ui(ui);
        });
        if !self.line_billable() {
            form::hint(ui, tokens, &t("helper.lines.billable_later"));
        }
        let f = self.compose();
        let (key, var, figure, warn) = match f.totals_status() {
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
        ui.label(kit::with_figure(
            ui,
            key,
            var,
            &figure.format(self.money_style),
            12.0,
            color,
        ));
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
        kit::status(ui, tokens, &self.flow);
        ui.add_space(12.0);
        if let Some(cmd) = kit::actions_row(ui, enabled, enabled) {
            self.after_frame = Some(CommandId::new(cmd));
        }
    }

    fn treatment_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        kit::above(ui, tokens, &t("personal_funds.how"));
        let f = self.header.current().clone();
        let default = default_treatment(self.books.entity);
        let mut set = None;
        for (treatment, key) in [
            (Treatment::PutIn, "personal_funds.put_in"),
            (Treatment::PayBack, "personal_funds.pay_back"),
        ] {
            ui.horizontal(|ui| {
                if ui.radio(f.treatment == treatment, t(key)).clicked() {
                    set = Some(treatment);
                }
                if treatment == default {
                    ui.label(
                        RichText::new(t(default_label(self.books.entity)))
                            .size(12.0)
                            .color(tokens.text_secondary),
                    );
                }
            });
        }
        if let Some(tr) = set {
            self.set_treatment(tr);
        }
        let offsets = self.offsets(f.treatment);
        ui.horizontal(|ui| {
            let (p, _) = kit::account_combo(
                ui,
                tokens,
                "personal_funds/offset",
                &t("personal_funds.offset"),
                &offsets,
                self.header.current().offset,
                width - 8.0,
            );
            if let Some(id) = p {
                self.pick_offset(id);
            }
            if f.treatment == Treatment::PayBack
                && ui
                    .add_enabled(
                        self.quick_add.is_none(),
                        egui::Button::new(t("personal_funds.quick_add")),
                    )
                    .clicked()
            {
                self.quick_add_due_to_owner();
            }
        });
    }

    fn panel_ui(ui: &mut Ui, tokens: &Tokens) {
        kit::panel_card(ui, tokens, &t("personal_funds.panel.title"), |ui| {
            ui.add(egui::Label::new(t("personal_funds.panel.explain")).wrap());
            ui.add(
                egui::Label::new(
                    RichText::new(t("owner_money.ask_accountant")).color(tokens.text_secondary),
                )
                .wrap(),
            );
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
            "edit.void_transaction" => self.flow.start_void(),
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
            "form.revert" => {
                if self.flow.void_open {
                    self.flow.void_open = false;
                    return true;
                }
                for input in [&mut self.inputs.who, &mut self.inputs.payee] {
                    if input.open {
                        input.open = false;
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    /// The control-channel state (no amounts or names).
    pub(crate) fn state_json(&self) -> Value {
        let f = self.compose();
        let totals = f.totals_status().map(|s| match s {
            TotalsStatus::Balanced => "balanced",
            TotalsStatus::Remaining(_) => "remaining",
            TotalsStatus::Over(_) => "over",
        });
        let mut v = json!({
            "screen": "personal_funds",
            "company_open": self.tenant.is_some(),
            "loaded": self.books.loaded,
            "entity_type": self.books.entity.map(Entity::name),
            "treatment": f.treatment.name(),
            "treatment_default": default_treatment(self.books.entity).name(),
            "default_label": default_label(self.books.entity),
            "offset_id": f.offset.map(|a| a.to_string()),
            "offset_source": self.offset_source.map(|s| match s {
                EquitySource::Engine => "engine",
                EquitySource::Remembered => "remembered",
            }),
            "has_who": f.who.is_some(),
            "has_payee": f.payee.is_some(),
            "amount_set": f.amount.is_some(),
            "lines": f.lines.used().count(),
            "totals": totals,
            "dirty": self.is_dirty(),
            "published_accounts": self.flow.published.len(),
            "save": {
                "refused": self.problems.first().map(|p| p.name()),
                "problems": self.problems.len(),
            },
        });
        if let (Value::Object(m), Value::Object(c)) = (&mut v, self.flow.state_json(&self.link)) {
            m.extend(c);
        }
        v
    }
}

impl Screen for PersonalFundsScreen {
    fn title(&self) -> String {
        t("cmd.vendors.personal_funds")
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
        let in_grid = self.lines.has_focus();
        if self.in_grid && !in_grid {
            self.left_lines();
        }
        self.in_grid = in_grid;
        if let Some(cmd) = self.after_frame.take() {
            self.command(cmd);
        }
        if self.books.busy() || self.balances.busy() || self.flow.busy() || self.quick_add.is_some()
        {
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
        if let Some(handled) = self.lines.command(id) {
            return handled;
        }
        self.form_command(id)
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
mod tests;
