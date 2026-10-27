//! Write Check in **draft mode** (A04 D2, S12, step 12): a check already
//! written, from Review Documents. Print later is off. The stored reading
//! fills the empty fields (`cl_model::forms::check::apply`: payee, date,
//! amount, the printed check number over a proposed one, the lines), each
//! *read, not accepted*: Accept (Alt+A) or an edit accepts it, and
//! `ReadNotAccepted` blocks the save until then. **Accept is Save & Close
//! / Save & New**: the check's own checks and its own `CheckDraft` (the
//! same `to_draft` and `BankPosting::check_with` as a manual check), then
//! [`DraftSession::accept`]. The document rides on the draft's own key, so
//! convert moves it.

use std::collections::BTreeMap;
use std::sync::Arc;

use cl_api::CheckDraft;
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use cl_docread::correct::{Final, FinalValues};
use cl_model::CommandId;
use cl_model::doc_draft::{DraftForm, FillKind, Step, TargetKey};
use cl_model::forms::SaveIntent;
use cl_model::forms::check::{AttachmentRef, CheckedCheck, NumberField};
use cl_model::forms::doc_read::{ReadField, ReadTarget};
use cl_model::forms::lines::FormLines;
use cl_model::rowkey::RowKey;
use eframe::egui;

use super::{Field, Pending, WriteCheckScreen, field};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::reading::learn::LearnInput;
use crate::reading::map;
use crate::screens::doc_review::attach::MatchProbe;
use crate::screens::doc_review::review::{
    self, Accept, BannerClick, BannerRead, Canon, DraftEvent, DraftOpen, DraftSession, FormValues,
    PostBody, TargetIn,
};
use crate::widgets::compare::{CompareAction, CompareEvent, FillValue};

/// The values a reading put in the form, to notice the owner's edits.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct ReadSnap {
    bank: Option<AccountId>,
    number: Option<String>,
    payee: Option<ContactId>,
    date: Option<CalendarDate>,
    amount: Option<Money>,
    lines: BTreeMap<RowKey, Option<Money>>,
}

/// The form's state in draft mode.
#[derive(Default)]
pub(crate) struct CheckDraftMode {
    /// The session (`None`: not a draft form).
    pub session: Option<DraftSession>,
    snap: Option<ReadSnap>,
    focus_accept: bool,
}

impl CheckDraftMode {
    /// The form is in draft mode.
    pub(crate) fn on(&self) -> bool {
        self.session.is_some()
    }
}

fn field_of(t: ReadTarget) -> Option<Field> {
    Some(match t {
        ReadTarget::PaidFrom => Field::Bank,
        ReadTarget::CheckNumber => Field::Number,
        ReadTarget::Payee => Field::Payee,
        ReadTarget::Date => Field::Date,
        ReadTarget::Total => Field::Amount,
        _ => return None,
    })
}

fn target_of(f: Field) -> Option<ReadTarget> {
    Some(match f {
        Field::Bank => ReadTarget::PaidFrom,
        Field::Number => ReadTarget::CheckNumber,
        Field::Payee => ReadTarget::Payee,
        Field::Date => ReadTarget::Date,
        Field::Amount => ReadTarget::Total,
        _ => return None,
    })
}

impl WriteCheckScreen {
    /// Opens draft `o` here: a check already written (Print later off).
    pub(crate) fn open_draft(&mut self, o: DraftOpen) {
        self.new_check(false);
        let mut f = super::blank(None, self.today, NumberField::Typed(String::new()));
        // No date: the document's own fills it (else the owner types it).
        f.date = None;
        f.print_later = false;
        self.header.load(f);
        self.sync_inputs();
        let tenant = self.tenant.as_ref().map(|(t, _)| *t);
        self.draft = CheckDraftMode {
            session: Some(DraftSession::new(Arc::clone(&self.link), tenant, o)),
            snap: None,
            focus_accept: false,
        };
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
        // The queue's Enter (WP-F1) hands its draft over here.
        if let Some(o) = crate::screens::doc_review::take_open(ctx, DraftForm::WriteCheck) {
            self.open_draft(DraftOpen {
                id: o.id,
                form: o.form,
            });
        }
        let today = self.today;
        let Some(s) = self.draft.session.as_mut() else {
            return;
        };
        let events = s.tick(ctx, &[], today);
        for ev in events {
            self.draft_event(ev);
        }
        if self
            .save
            .problems
            .contains(&cl_model::forms::check::CheckProblem::ReadNotAccepted)
        {
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
                if !self.closing_failed(&error, Pending::Save(intent)) {
                    self.save.error = Some(error);
                }
            }
            DraftEvent::Cancelled => {}
            DraftEvent::Reload => {
                let session = self.draft.session.take();
                if let Some(s) = session {
                    let o = DraftOpen {
                        id: s.id,
                        form: s.form,
                    };
                    self.open_draft(o);
                }
            }
            DraftEvent::Next(Some(o)) if o.form == DraftForm::WriteCheck => self.open_draft(o),
            DraftEvent::Next(Some(o)) => {
                review::open(&self.link, o);
                self.close_window();
            }
            DraftEvent::Close | DraftEvent::Next(None) => self.close_window(),
            DraftEvent::Helped => self.draft_helped(),
        }
    }

    fn close_window(&mut self) {
        self.link
            .push(CompanyEvent::CloseWindow(CommandId::new(super::KIND)));
    }

    /// The document into the pane (on the draft's own key) and, once, the
    /// stored values into the empty fields.
    fn draft_loaded(&mut self) {
        let Some(s) = self.draft.session.as_mut() else {
            return;
        };
        if let (Some(key), Some(file)) = (s.draft_key, s.file.clone())
            && self.attach.draft != Some(key)
        {
            self.attach.draft = Some(key);
            let mut f = self.header.current().clone();
            f.draft_attachment = Some(AttachmentRef {
                id: key.to_string(),
                name: file.name,
            });
            self.header.load(f);
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
            cl_model::forms::check::apply(&doc, &mut form, keys)
        };
        let lines = std::mem::take(&mut form.lines);
        self.header.load(form);
        let lists = Arc::clone(&self.lists);
        self.lines.form_mut().load(lines, &*lists);
        if let Some(s) = self.draft.session.as_mut() {
            s.filled(&filled);
        }
        self.draft.snap = Some(self.snap());
        self.sync_inputs();
        self.apply_carry();
    }

    fn snap(&self) -> ReadSnap {
        let f = self.compose();
        ReadSnap {
            bank: f.bank,
            number: f.number.text(),
            payee: f.payee,
            date: f.date,
            amount: f.amount,
            lines: f.lines.lines.iter().map(|l| (l.key, l.amount)).collect(),
        }
    }

    /// After drawing: a read field the owner changed counts as accepted.
    pub(crate) fn track_draft_read(&mut self) {
        let Some(before) = self.draft.snap.clone() else {
            return;
        };
        let now = self.snap();
        if now == before {
            return;
        }
        let mut edited = Vec::new();
        if now.bank != before.bank {
            edited.push(ReadTarget::PaidFrom);
        }
        if now.number != before.number {
            edited.push(ReadTarget::CheckNumber);
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
            let mut r = self.header.current().read.clone();
            for t in pending {
                r.edited(t);
            }
            self.header.edit_step(|h| h.read = r);
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
                ReadTarget::PaidFrom => f.bank = None,
                ReadTarget::CheckNumber => f.number = NumberField::Typed(String::new()),
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

    /// Accept: the checked check (its engine draft built by `to_draft`)
    /// through the draft.
    pub(crate) fn accept_draft(
        &mut self,
        c: &CheckedCheck,
        body: CheckDraft,
        intent: SaveIntent,
        password: Option<Secret>,
    ) -> bool {
        let payee_name = self.lists.name_text(c.payee).map(str::to_string);
        let account_name = self
            .bank_accounts()
            .into_iter()
            .find(|a| a.id == c.bank)
            .map(crate::screens::register::account_text);
        let marks = self.header.current().read.clone();
        let billable = self.line_billable();
        let Some(s) = self.draft.session.as_mut() else {
            return false;
        };
        let learn = LearnInput {
            vendor: Some(c.payee),
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
                reference: c.number.map(|n| {
                    let n = n.to_string();
                    Final {
                        entry: s.entry(ReadTarget::CheckNumber, &marks, &n),
                        value: n,
                    }
                }),
                total: Some(Final {
                    value: c.amount,
                    entry: s.entry(ReadTarget::Total, &marks, &c.amount),
                }),
                line_amounts: Some(c.lines.iter().map(|l| l.amount).collect()),
            },
            card: None,
        };
        let accept = Accept {
            form_digest: Canon::of_check(c).digest(),
            body: PostBody::Check(body),
            billable,
            intent,
            password,
            probe: MatchProbe {
                amount: c.amount,
                date: c.date,
                payee: payee_name,
                contact: Some(c.payee),
                account: Some(c.bank),
                account_name,
            },
            learn: Some(learn),
        };
        s.accept(accept)
    }

    /// The document posted (or was attached): the form is saved; then the
    /// next draft or close.
    fn draft_posted(&mut self, _id: TransactionId, intent: SaveIntent) {
        self.closing_prompt.saved();
        self.save.count += 1;
        self.save.last_action = Some("save");
        self.save.error = None;
        self.save.problems.clear();
        self.publish_ledger();
        let lists = Arc::clone(&self.lists);
        let header = self.header.current().clone();
        self.header.load(header);
        let lines: FormLines = self.lines.value().clone();
        self.lines.form_mut().load(lines, &*lists);
        // The file moved with the convert.
        self.attach.draft = None;
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
            // A check already written: no Print later, templates or Void.
            "check.print_later" | "form.save_as_template" | "edit.void_transaction" => Some(true),
            _ => None,
        }
    }

    /// The draft banner (above the form).
    pub(crate) fn draft_banner(&mut self, ui: &mut egui::Ui, tokens: &crate::theme::Tokens) {
        let unaccepted = self.header.current().read.unaccepted().len();
        let vendor = self
            .draft
            .session
            .as_ref()
            .and_then(DraftSession::layout_vendor)
            .and_then(|v| self.lists.name_text(v))
            .map(str::to_string);
        let vendors = map::vendor_candidates(&self.contacts);
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
                card_offer: false,
            }),
            vendor.as_deref(),
        );
        match click {
            BannerClick::Accept => self.accept_read(),
            BannerClick::Clear => self.clear_read(),
            BannerClick::ReadDocument => {
                let _ = s.read_document(vendors, None, today);
            }
            BannerClick::AskAi => {
                let _ = s.ask_ai(vendors);
            }
            BannerClick::EnterAs(to) => self.enter_as(ui.ctx(), to),
            BannerClick::None | BannerClick::Discard | BannerClick::TryAgain => {}
        }
    }

    /// The form's values for "Enter this as".
    fn form_values(&self) -> FormValues {
        let f = self.compose();
        FormValues {
            payee: f.payee,
            date: f.date,
            total: f.amount,
            paid_from: f.bank,
            lines: f.lines.lines,
            marks: f.read,
        }
    }

    /// "Enter this as" `to`: the draft opens in that form's window with the
    /// owner's work.
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
        review::open(&self.link, o);
        self.close_window();
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
        let allowed: Vec<AccountId> = self.bank_accounts().into_iter().map(|a| a.id).collect();
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
        f.bank = v.paid_from;
        f.read = v.marks;
        f.lines.lines = v.lines;
        let lines = std::mem::take(&mut f.lines);
        self.header.load(f);
        let lists = Arc::clone(&self.lists);
        self.lines.form_mut().load(lines, &*lists);
        self.draft.snap = Some(self.snap());
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
        let bank = f
            .bank
            .and_then(|b| self.bank_accounts().into_iter().find(|a| a.id == b))
            .map(crate::screens::register::account_text)
            .unwrap_or_default();
        let mut fields = vec![
            TargetIn {
                target: ReadTarget::Payee,
                label: t("check.payee"),
                fill: None,
                value: f
                    .payee
                    .and_then(|p| self.lists.name_text(p))
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
                target: ReadTarget::CheckNumber,
                label: t("check.number"),
                fill: Some(FillKind::Text),
                value: f.number.text().unwrap_or_default(),
            },
            TargetIn {
                target: ReadTarget::Total,
                label: t("helper.amount"),
                fill: Some(FillKind::Money),
                value: f.amount.map(|m| m.format(style)).unwrap_or_default(),
            },
            TargetIn {
                target: ReadTarget::PaidFrom,
                label: t("check.bank"),
                fill: None,
                value: bank,
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
                    let lists = Arc::clone(&self.lists);
                    self.lines.form_mut().focus(
                        cl_grid::CellPos {
                            row: k,
                            col: crate::forms::lines_grid::L_AMOUNT,
                        },
                        &*lists,
                    );
                }
                Some(t) => self.focus = field_of(t),
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
                    (ReadTarget::CheckNumber, FillValue::Text(n)) => {
                        self.header
                            .edit(field::NUMBER, |h| h.number = NumberField::Typed(n));
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
