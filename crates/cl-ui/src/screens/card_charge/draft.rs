//! Record Card Charge in **draft mode** (A04 D2, S12, step 12): a card
//! charge or a cash expense ("Expense paid from": bank and cash accounts)
//! from Review Documents. The stored reading fills the empty fields
//! (`cl_model::forms::card::apply`: payee, date, amount, the card whose
//! printed digits matched, the lines), each *read, not accepted*: Accept
//! (Alt+A) or an edit accepts it, and `ReadNotAccepted` blocks the save
//! until then. Credit is hidden (convert refuses it; refunds: Q-A04g).
//! **Accept is Save & Close / Save & New**: the form's own checks and its
//! own `CardChargeDraft`, then [`DraftSession::accept`]. Extra files go to
//! the draft's own key, so convert moves them; no post-save move.

use std::collections::BTreeMap;
use std::sync::Arc;

use cl_api::{CardChargeDraft, LocalValidation, SplitLine};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use cl_docread::correct::{Final, FinalValues};
use cl_model::CommandId;
use cl_model::doc_draft::{DraftForm, FillKind, Step, TargetKey};
use cl_model::forms::SaveIntent;
use cl_model::forms::card::{CardForm, CardMode, CheckedCard};
use cl_model::forms::doc_read::{ReadField, ReadTarget};
use cl_model::forms::lines::FormLines;
use cl_model::register::AccountClass;
use cl_model::rowkey::RowKey;
use eframe::egui;

use super::{CardChargeScreen, field};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::reading::learn::{CardLearn, LearnInput};
use crate::reading::map;
use crate::screens::doc_review::attach::MatchProbe;
use crate::screens::doc_review::review::{
    self, Accept, BannerClick, BannerRead, Canon, DraftEvent, DraftOpen, DraftSession, FormValues,
    PostBody, TargetIn,
};
use crate::widgets::compare::{CompareAction, CompareEvent, FillValue};

#[cfg(test)]
#[path = "draft_kind_ai_tests.rs"]
mod kind_ai_tests;

/// The values a reading put in the form, to notice the owner's edits (an
/// edit accepts that field).
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct ReadSnap {
    card: Option<AccountId>,
    payee: Option<ContactId>,
    date: Option<CalendarDate>,
    amount: Option<Money>,
    lines: BTreeMap<RowKey, Option<Money>>,
}

/// The form's state in draft mode.
#[derive(Default)]
pub(crate) struct CardDraft {
    /// The session (`None`: not a draft form).
    pub session: Option<DraftSession>,
    /// "Expense paid from": bank and cash accounts (a cash expense).
    pub expense: bool,
    /// What the reading filled.
    snap: Option<ReadSnap>,
    /// Put the keyboard on Accept (a save was refused for it).
    focus_accept: bool,
}

impl CardDraft {
    /// The form is in draft mode.
    pub(crate) fn on(&self) -> bool {
        self.session.is_some()
    }
}

/// The card field id of a read target.
fn field_of(t: ReadTarget) -> Option<u64> {
    Some(match t {
        ReadTarget::PaidFrom => field::CARD,
        ReadTarget::Payee => field::PAYEE,
        ReadTarget::Date => field::DATE,
        ReadTarget::Total => field::AMOUNT,
        _ => return None,
    })
}

/// The read target of a card field id.
fn target_of(f: u64) -> Option<ReadTarget> {
    Some(match f {
        field::CARD => ReadTarget::PaidFrom,
        field::PAYEE => ReadTarget::Payee,
        field::DATE => ReadTarget::Date,
        field::AMOUNT => ReadTarget::Total,
        _ => return None,
    })
}

impl CardChargeScreen {
    /// Opens draft `o` here: a card charge, or a cash expense ("Expense
    /// paid from").
    pub(crate) fn open_draft(&mut self, o: DraftOpen) {
        self.new_form(false);
        // A draft starts with no card and no date: the document's fill
        // them; Credit is hidden.
        let mut f = CardForm::new(None, None);
        f.mode = CardMode::Charge;
        self.header.load(f);
        self.lock_columns();
        self.sync_inputs();
        let tenant = self.tenant.as_ref().map(|(t, _)| *t);
        self.draft = CardDraft {
            session: Some(DraftSession::new(Arc::clone(&self.link), tenant, o)),
            expense: o.form == DraftForm::CashExpense,
            snap: None,
            focus_accept: false,
        };
    }

    /// The accounts a charge is paid from: the cards, or in "Expense paid
    /// from" mode the bank and cash accounts.
    pub(crate) fn paid_from_accounts(&self) -> Vec<(AccountId, String)> {
        if !self.draft.expense {
            return self.cards();
        }
        let mut v = self.books.accounts_of(&[AccountClass::Bank]);
        let cash: Vec<(AccountId, String)> = self
            .books
            .accounts
            .iter()
            .filter(|a| {
                a.is_active
                    && a.fields
                        .detail_type
                        .as_deref()
                        .is_some_and(|d| d.contains("cash"))
                    && !v.iter().any(|(id, _)| *id == a.id)
            })
            .map(|a| (a.id, crate::screens::register::account_text(a)))
            .collect();
        v.extend(cash);
        v
    }

    /// Nothing may change while the session is busy or asks.
    pub(crate) fn draft_locked(&self) -> bool {
        self.draft
            .session
            .as_ref()
            .is_some_and(DraftSession::locked)
    }

    /// Every frame: the session's events.
    pub(crate) fn poll_draft(&mut self, ctx: &egui::Context) {
        // The queue's Enter (WP-F1): a card charge or a cash expense (one
        // window; `form` picks the mode).
        if let Some(o) = crate::screens::doc_review::take_open(ctx, DraftForm::CardCharge) {
            self.open_draft(DraftOpen {
                id: o.id,
                form: o.form,
            });
        }
        let cards = crate::reading::service::card_accounts_of(&self.books.accounts);
        let today = self.today;
        let Some(s) = self.draft.session.as_mut() else {
            return;
        };
        s.rematch_card(&cards);
        let events = s.tick(ctx, &cards, today);
        for ev in events {
            self.draft_event(ev);
        }
        let refused = self
            .problems
            .contains(&cl_model::forms::card::CardProblem::ReadNotAccepted);
        if refused {
            self.draft.focus_accept = true;
        }
    }

    fn draft_event(&mut self, ev: DraftEvent) {
        match ev {
            DraftEvent::Loaded | DraftEvent::Read => self.draft_loaded(),
            DraftEvent::Posted {
                transaction,
                intent,
            } => self.draft_posted(transaction, intent),
            DraftEvent::Attached { transaction } => {
                self.draft_posted(transaction, SaveIntent::SaveAndClose);
            }
            DraftEvent::Failed { error, intent } => {
                let closing = self.books.closing;
                self.flow.intent = Some(intent);
                self.flow.failed(&self.link, error, closing);
            }
            DraftEvent::Cancelled => {}
            DraftEvent::Reload => {
                let mut f = CardForm::new(None, None);
                f.mode = CardMode::Charge;
                self.header.load(f);
                let lists = Arc::clone(&self.books.lists);
                let fresh = FormLines::from_lines(
                    Vec::new(),
                    &mut self.lines.form_mut().adapter_mut().keys,
                );
                self.lines.form_mut().load(fresh, &*lists);
                self.draft.snap = None;
                self.sync_inputs();
            }
            DraftEvent::Next(Some(o))
                if matches!(o.form, DraftForm::CardCharge | DraftForm::CashExpense) =>
            {
                self.open_draft(o);
            }
            DraftEvent::Next(Some(o)) => {
                review::open(&self.link, o);
                self.close_window();
            }
            DraftEvent::Close | DraftEvent::Next(None) => self.close_window(),
            DraftEvent::Helped => self.draft_helped(),
        }
    }

    /// The form's values for "Enter this as".
    fn form_values(&self) -> FormValues {
        let f = self.compose();
        FormValues {
            payee: f.payee,
            date: f.date,
            total: f.amount,
            paid_from: f.card,
            lines: f.lines.lines,
            marks: f.read,
        }
    }

    /// "Enter this as" `to`: the draft opens there with the owner's work
    /// (a cash expense and a card charge share this window).
    pub(crate) fn enter_as(&mut self, ctx: &egui::Context, to: DraftForm) {
        let v = self.form_values();
        let Some(s) = self.draft.session.as_ref() else {
            return;
        };
        let Some(carry) = s.enter_as(to, &v) else {
            return;
        };
        let o = DraftOpen { id: s.id, form: to };
        review::publish_carry(ctx, carry);
        if matches!(to, DraftForm::CardCharge | DraftForm::CashExpense) {
            self.open_draft(o);
        } else {
            review::open(&self.link, o);
            self.close_window();
        }
    }

    /// After the fill: what "Enter this as" brought from the other form.
    fn apply_carry(&mut self) {
        let Some(s) = self.draft.session.as_mut() else {
            return;
        };
        let Some(c) = s.take_carry() else {
            return;
        };
        let rows: Vec<Option<RowKey>> = (0..c.lines.len()).map(|i| s.line_key(i)).collect();
        let form = s.form;
        let allowed: Vec<AccountId> = self
            .paid_from_accounts()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let mut v = self.form_values();
        c.apply_to(
            form,
            &mut v,
            |i| rows.get(i).copied().flatten(),
            |a| allowed.contains(&a),
        );
        let mut f = self.compose();
        if c.total.is_some() {
            f.amount_from_lines = false;
        }
        f.payee = v.payee;
        f.date = v.date;
        f.amount = v.total;
        f.card = v.paid_from;
        f.read = v.marks;
        f.lines.lines = v.lines;
        let lines = std::mem::take(&mut f.lines);
        self.header.load(f);
        let lists = Arc::clone(&self.books.lists);
        self.lines.form_mut().load(lines, &*lists);
        self.draft.snap = Some(self.snap());
        if let Some(card) = self.card() {
            self.balances.fetch(&self.link, self.group.as_ref(), card);
        }
        self.sync_inputs();
    }

    /// Ask AI to help answered: the values not yet accepted give way to the
    /// merged reading; what the owner accepted or changed stays, marked.
    fn draft_helped(&mut self) {
        let kept: Vec<(ReadTarget, ReadField)> = self
            .header
            .current()
            .read
            .entries()
            .filter(|(_, s)| !matches!(s, ReadField::Read { .. }))
            .collect();
        self.clear_read();
        self.draft_loaded();
        let mut f = self.header.current().clone();
        for (t, s) in kept {
            f.read.restore(t, s);
        }
        self.header.load(f);
        self.draft.snap = Some(self.snap());
    }

    fn close_window(&mut self) {
        self.link
            .push(CompanyEvent::CloseWindow(CommandId::new(super::KIND)));
    }

    /// The document into the pane and, once, the stored values into the
    /// empty fields.
    fn draft_loaded(&mut self) {
        let Some(s) = self.draft.session.as_mut() else {
            return;
        };
        if let (Some(file), Some(a)) = (
            s.file.clone(),
            s.draft.as_ref().and_then(|d| d.attachment_id),
        ) && self.attach.shown != Some(a)
        {
            self.attach.draft = s.draft_key;
            self.attach.show(a, file);
            self.load_attachments();
        }
        let Some(s) = self.draft.session.as_mut() else {
            return;
        };
        let Some(local) = s.take_fill() else {
            return;
        };
        let doc = map::to_doc(&local).doc;
        let mut form = self.compose();
        let filled = {
            let keys = &mut self.lines.form_mut().adapter_mut().keys;
            cl_model::forms::card::apply(&doc, &mut form, keys)
        };
        // A paid-from account the mode doesn't list is no card here.
        if let Some(c) = form.card
            && !self.paid_from_accounts().iter().any(|(id, _)| *id == c)
        {
            form.card = None;
            form.read = {
                let mut r = form.read.clone();
                r.edited(ReadTarget::PaidFrom);
                r
            };
        }
        let lines = std::mem::take(&mut form.lines);
        self.header.load(form);
        let lists = Arc::clone(&self.books.lists);
        self.lines.form_mut().load(lines, &*lists);
        if let Some(s) = self.draft.session.as_mut() {
            s.filled(&filled);
        }
        self.draft.snap = Some(self.snap());
        if let Some(card) = self.card() {
            self.balances.fetch(&self.link, self.group.as_ref(), card);
        }
        self.lock_columns();
        self.sync_inputs();
        self.apply_carry();
    }

    fn snap(&self) -> ReadSnap {
        let f = self.compose();
        ReadSnap {
            card: f.card,
            payee: f.payee,
            date: f.date,
            amount: f.amount,
            lines: f.lines.lines.iter().map(|l| (l.key, l.amount)).collect(),
        }
    }

    /// After drawing: a read field the owner changed counts as accepted
    /// (a line only by its amount, non-negotiable 4).
    pub(crate) fn track_draft_read(&mut self) {
        let Some(before) = self.draft.snap.clone() else {
            return;
        };
        let now = self.snap();
        if now == before {
            return;
        }
        let mut edited = Vec::new();
        if now.card != before.card {
            edited.push(ReadTarget::PaidFrom);
        }
        if now.payee != before.payee {
            edited.push(ReadTarget::Payee);
        }
        if now.date != before.date {
            edited.push(ReadTarget::Date);
        }
        if now.amount != before.amount {
            edited.push(ReadTarget::Total);
        }
        for (k, a) in &before.lines {
            if now.lines.get(k).is_some_and(|n| n != a) {
                edited.push(ReadTarget::Line(*k));
            }
        }
        let read = &self.header.current().read;
        let pending: Vec<ReadTarget> = edited
            .into_iter()
            .filter(|t| matches!(read.state(*t), Some(ReadField::Read { .. })))
            .collect();
        if !pending.is_empty() {
            let mut f = self.header.current().clone();
            for t in pending {
                f.read.edited(t);
            }
            // Not an undo step of its own: it follows the edit.
            self.header.edit_step(|h| h.read = f.read);
        }
        self.draft.snap = Some(now);
    }

    /// Accept these values (Alt+A).
    pub(crate) fn accept_read(&mut self) {
        if self.header.current().read.unaccepted().is_empty() {
            return;
        }
        self.header.edit_step(|h| h.read.accept_all());
        self.draft.focus_accept = false;
    }

    /// "Clear and type it myself": the values not yet accepted go.
    pub(crate) fn clear_read(&mut self) {
        let mut f = self.header.current().clone();
        let mut lines = self.lines.value().clone();
        for target in f.read.unaccepted() {
            match target {
                ReadTarget::PaidFrom => f.card = None,
                ReadTarget::Payee => f.payee = None,
                ReadTarget::Date => f.date = None,
                ReadTarget::Total => {
                    f.amount = None;
                    f.amount_from_lines = false;
                }
                ReadTarget::Line(k) => lines.lines.retain(|l| l.key != k),
                _ => {}
            }
        }
        f.read.clear();
        self.header.edit_step(|h| *h = f);
        if &lines != self.lines.value() {
            self.lines.form_mut().edit_step(|v| *v = lines);
            self.lines.lines_replaced();
        }
        self.draft.snap = None;
        if let Some(s) = self.draft.session.as_mut() {
            s.compare.clear();
        }
        self.sync_inputs();
    }

    /// The checked charge as the engine draft (no arithmetic).
    pub(crate) fn charge_draft(c: &CheckedCard) -> CardChargeDraft {
        CardChargeDraft {
            pay_from: c.card,
            date: c.date,
            number: c.ref_no.clone(),
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
        }
    }

    /// Accept: the checked charge through the draft. The register search
    /// comes first; the closing-date password is asked only when the
    /// convert itself is refused for the closed period (the engine's 423,
    /// review B1): attaching to a record already in the books posts
    /// nothing and needs no password (Q-A04f).
    pub(crate) fn accept_draft(
        &mut self,
        c: &CheckedCard,
        intent: SaveIntent,
        password: Option<Secret>,
    ) -> bool {
        if c.mode != CardMode::Charge {
            // Credit is hidden in draft mode (convert refuses it, Q-A04g);
            // never a silent no-op (review L4).
            self.flow.notice = Some("doc_form.credit_unavailable");
            return false;
        }
        let expense = self.draft.expense;
        let raw = Self::charge_draft(c);
        // cl-api's own checks before anything goes (the posting must add up
        // and the engine take line customers).
        if let Err(v) = cl_api::BankPosting::card_charge_with(raw.clone(), self.line_billable()) {
            if v == LocalValidation::LineBillableUnavailable {
                self.flow.notice = Some("helper.line_billable_unavailable");
            } else {
                self.flow.error = Some(cl_api::ApiError::Local(v));
            }
            return false;
        }
        let kind = if expense {
            cl_api::DraftKind::CashExpense
        } else {
            cl_api::DraftKind::CardCharge
        };
        let payee_name = c
            .payee
            .and_then(|p| self.books.lists.name_text(p))
            .map(str::to_string);
        let account_name = self
            .paid_from_accounts()
            .into_iter()
            .find(|(id, _)| *id == c.card)
            .map(|(_, n)| n);
        let marks = self.header.current().read.clone();
        let card_offer = self.card_offer();
        let billable = self.line_billable();
        let mut touched: Vec<AccountId> = vec![c.card];
        touched.extend(c.lines.iter().map(|l| l.account));
        self.flow.published = touched;
        let Some(s) = self.draft.session.as_mut() else {
            return false;
        };
        let card = s
            .printed_card()
            .filter(|_| card_offer && s.remember_card)
            .map(|digits| CardLearn {
                account: c.card,
                digits,
            });
        let learn = LearnInput {
            vendor: c.payee,
            values: FinalValues {
                vendor: payee_name.clone().map(|v| Final {
                    entry: s.entry(ReadTarget::Payee, &marks, &v),
                    value: v,
                }),
                date: Some(Final {
                    value: c.date,
                    entry: s.entry(ReadTarget::Date, &marks, &c.date),
                }),
                due_date: None,
                reference: c.ref_no.clone().map(|r| Final {
                    entry: cl_docread::correct::Entry::Typed,
                    value: r,
                }),
                total: Some(Final {
                    value: c.amount,
                    entry: s.entry(ReadTarget::Total, &marks, &c.amount),
                }),
                line_amounts: Some(c.lines.iter().map(|l| l.amount).collect()),
            },
            card,
        };
        let accept = Accept {
            form_digest: Canon::of_card(kind, c).digest(),
            body: if expense {
                PostBody::CashExpense(raw)
            } else {
                PostBody::CardCharge(raw)
            },
            billable,
            intent,
            password,
            probe: MatchProbe {
                amount: c.amount,
                date: c.date,
                payee: payee_name,
                contact: c.payee,
                account: Some(c.card),
                account_name,
            },
            learn: Some(learn),
        };
        s.accept(accept)
    }

    /// "Remember card ending 1234": a card charge whose account has no
    /// digits yet (Q-A04j).
    fn card_offer(&self) -> bool {
        !self.draft.expense
            && self.card().is_some_and(|c| {
                self.books
                    .accounts
                    .iter()
                    .any(|a| a.id == c && a.card_last4.is_none())
            })
    }

    /// The document posted (or was attached): the form is saved; then the
    /// next draft or close.
    fn draft_posted(&mut self, _id: TransactionId, intent: SaveIntent) {
        self.flow.closing_prompt.saved();
        self.flow.count += 1;
        self.flow.last_action = Some("save");
        self.flow.error = None;
        self.problems.clear();
        crate::forms::publish_saved(&self.link, &self.bus, &self.flow.published);
        let lists = Arc::clone(&self.books.lists);
        let header = self.header.current().clone();
        self.header.load(header);
        let lines = self.lines.value().clone();
        self.lines.form_mut().load(lines, &*lists);
        // The files moved with the convert: nothing moves after it.
        self.attach.draft = None;
        self.records.forget();
        match intent {
            SaveIntent::SaveAndNew => {
                if let Some(s) = self.draft.session.as_mut() {
                    s.find_next();
                }
            }
            _ => self.close_window(),
        }
    }

    /// Keys in draft mode before the form's own.
    pub(crate) fn draft_command(&mut self, id: CommandId) -> Option<bool> {
        let dirty = self.is_dirty();
        let s = self.draft.session.as_mut()?;
        if let Some(h) = s.command(id) {
            return Some(h);
        }
        match id.as_str() {
            "form.prev_record" => Some(s.step(Step::Prev, dirty)),
            "form.next_record" => Some(s.step(Step::Next, dirty)),
            "form.accept_read" => {
                self.accept_read();
                Some(true)
            }
            "form.clear" => {
                self.clear_read();
                Some(true)
            }
            // Templates and Void don't apply to a document being entered.
            "form.save_as_template" | "edit.void_transaction" => Some(true),
            _ => None,
        }
    }

    /// The draft banner (above the header).
    pub(crate) fn draft_banner(&mut self, ui: &mut egui::Ui, tokens: &crate::theme::Tokens) {
        let unaccepted = self.header.current().read.unaccepted().len();
        let card_offer = self.card_offer();
        let vendor = self
            .draft
            .session
            .as_ref()
            .and_then(DraftSession::layout_vendor)
            .and_then(|v| self.books.lists.name_text(v))
            .map(str::to_string);
        let vendors = map::vendor_candidates(&self.books.contacts);
        let cards = crate::reading::service::card_accounts_of(&self.books.accounts);
        let today = self.today;
        let focus_accept = std::mem::take(&mut self.draft.focus_accept);
        let Some(s) = self.draft.session.as_mut() else {
            return;
        };
        let click = s.banner_ui(
            ui,
            tokens,
            Some(BannerRead {
                unaccepted,
                focus_accept,
                card_offer,
            }),
            vendor.as_deref(),
        );
        match click {
            BannerClick::Accept => self.accept_read(),
            BannerClick::Clear => self.clear_read(),
            BannerClick::ReadDocument => {
                let _ = s.read_document(vendors, Some(cards), today);
            }
            BannerClick::AskAi => {
                let _ = s.ask_ai(vendors);
            }
            BannerClick::EnterAs(to) => self.enter_as(ui.ctx(), to),
            BannerClick::None
            | BannerClick::Discard
            | BannerClick::TryAgain
            | BannerClick::EnterAsNew
            | BannerClick::SearchBack => {}
        }
    }

    // ------------------------------------------------------------ compare

    /// The compare targets, then the docked viewer gets the view.
    pub(crate) fn compare_before(&mut self) {
        let style = self.money_style;
        if self.draft.session.is_none() {
            self.attach.viewer.set_compare(None);
            return;
        }
        let f = self.compose();
        let marks = f.read.clone();
        let account = |id: Option<AccountId>| {
            id.and_then(|c| {
                self.paid_from_accounts()
                    .into_iter()
                    .find(|(a, _)| *a == c)
                    .map(|(_, n)| n)
            })
            .unwrap_or_default()
        };
        let mut fields = vec![
            TargetIn {
                target: ReadTarget::Payee,
                label: t("card.payee"),
                fill: None,
                value: f
                    .payee
                    .and_then(|p| self.books.lists.name_text(p))
                    .unwrap_or_default()
                    .to_string(),
            },
            TargetIn {
                target: ReadTarget::Date,
                label: t("helper.date"),
                fill: Some(FillKind::Date),
                value: f.date.map(crate::shell::us_date).unwrap_or_default(),
            },
            TargetIn {
                target: ReadTarget::Total,
                label: t("helper.amount"),
                fill: Some(FillKind::Money),
                value: f.amount.map(|m| m.format(style)).unwrap_or_default(),
            },
            TargetIn {
                target: ReadTarget::PaidFrom,
                label: if self.draft.expense {
                    t("doc_form.expense_paid_from")
                } else {
                    t("card.card")
                },
                fill: None,
                value: account(f.card),
            },
        ];
        let clicked: Vec<ReadTarget> = self
            .draft
            .session
            .as_ref()
            .map(|s| s.clicked.keys().copied().collect())
            .unwrap_or_default();
        for l in &f.lines.lines {
            let target = ReadTarget::Line(l.key);
            if marks.state(target).is_some() || clicked.contains(&target) {
                fields.push(TargetIn {
                    target,
                    label: t("doc_form.line_amount"),
                    fill: Some(FillKind::Money),
                    value: l.amount.map(|m| m.format(style)).unwrap_or_default(),
                });
            }
        }
        let focused = self.focused.and_then(target_of).map(TargetKey::from);
        let Some(s) = self.draft.session.as_mut() else {
            return;
        };
        s.set_targets(&marks, fields);
        s.compare.focus(focused);
        let view = s.compare.view();
        self.attach.viewer.set_compare(Some(view));
    }

    /// The docked viewer's events and a box re-read's answer.
    pub(crate) fn compare_after(&mut self) {
        for ev in self.attach.viewer.take_compare_events() {
            self.on_compare_event(ev);
        }
        let today = self.today;
        if let Some(a) = self
            .draft
            .session
            .as_mut()
            .and_then(|s| s.compare_poll(today))
        {
            self.apply_compare(a);
        }
    }

    /// One compare event (docked, or the pop-out's outbox).
    pub(crate) fn on_compare_event(&mut self, ev: CompareEvent) {
        let today = self.today;
        if let Some(a) = self
            .draft
            .session
            .as_mut()
            .and_then(|s| s.compare_event(ev, today))
        {
            self.apply_compare(a);
        }
    }

    fn apply_compare(&mut self, action: CompareAction) {
        if self.draft_locked() {
            return;
        }
        let Some(s) = self.draft.session.as_ref() else {
            return;
        };
        match action {
            CompareAction::Focus(key) => match s.target_of(key) {
                Some(ReadTarget::Line(k)) => {
                    let lists = Arc::clone(&self.books.lists);
                    self.lines.form_mut().focus(
                        cl_grid::CellPos {
                            row: k,
                            col: crate::forms::lines_grid::L_AMOUNT,
                        },
                        &*lists,
                    );
                }
                Some(ReadTarget::Total) => self.focus_amount = true,
                Some(t) => self.focused = field_of(t),
                None => {}
            },
            CompareAction::LooksRight(key) => {
                if let Some(t) = s.target_of(key) {
                    self.header.edit_step(|h| h.read.edited(t));
                }
            }
            CompareAction::Fill { key, value, .. } => {
                let Some(t) = s.target_of(key) else {
                    return;
                };
                match (t, value) {
                    (ReadTarget::Total, FillValue::Money(m)) => self.amount_typed(Some(m)),
                    (ReadTarget::Date, FillValue::Date(d)) => {
                        self.header.edit(field::DATE, |h| h.date = Some(d));
                    }
                    (ReadTarget::Line(k), FillValue::Money(m)) => {
                        self.lines.form_mut().edit_step(|v| {
                            if let Some(l) = v.lines.iter_mut().find(|l| l.key == k) {
                                l.amount = Some(m);
                            }
                        });
                    }
                    _ => return,
                }
                self.header.edit_step(|h| h.read.edited(t));
                self.sync_inputs();
            }
        }
    }

    /// The control channel's `draft`.
    pub(crate) fn draft_state(&self) -> serde_json::Value {
        match &self.draft.session {
            None => serde_json::Value::Null,
            Some(s) => {
                let mut v = s.state_json();
                if let serde_json::Value::Object(m) = &mut v {
                    m.insert("expense_mode".into(), self.draft.expense.into());
                    m.insert(
                        "unaccepted".into(),
                        self.header.current().read.unaccepted().len().into(),
                    );
                }
                v
            }
        }
    }
}
