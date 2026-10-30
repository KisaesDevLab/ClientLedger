//! Enter Bill in **draft mode** (A04 D2, step 12): a document draft from
//! Review Documents opens here. The stored reading fills the empty fields
//! through the bill's own reading machinery (`fill`: every value *read,
//! not accepted*, A03), the document shows in the source card (its own
//! file locked: Discard document is its action), extra files go to the
//! draft's own key (convert moves them atomically; no post-save move), and
//! **Accept is Save & Close / Save & New**: the same checks and the same
//! `to_draft` as a manual bill, then [`DraftSession::accept`] (the register
//! match, then `convert`). The compare tools run over the source card's
//! viewer, docked or popped out.
//!
//! Only a bill converts here (a vendor credit can't be a draft's convert
//! body): the Bill / Vendor credit choice is off in draft mode.

use std::sync::Arc;

use cl_core::ids::TransactionId;
use cl_docread::correct::{Final, FinalValues};
use cl_model::CommandId;
use cl_model::doc_draft::{DraftForm, FillKind, Step, TargetKey};
use cl_model::forms::SaveIntent;
use cl_model::forms::bill::{BillKind, CheckedBill};
use cl_model::forms::doc_read::ReadTarget;
use cl_model::forms::item_lines::ItemLines;
use cl_model::forms::lines::FormLines;
use cl_model::rowkey::RowKey;
use eframe::egui;

use super::doc_read::Incoming;
use super::save::{Draft, to_draft};
use super::{EnterBillScreen, Field, LastSaved, Pending};
use crate::company::CompanyEvent;
use crate::i18n::{t, t_with};
use crate::reading::learn::LearnInput;
use crate::reading::map;
use crate::screens::doc_review::attach::MatchProbe;
use crate::screens::doc_review::review::{
    self, Accept, Canon, DraftEvent, DraftOpen, DraftSession, FormValues, PostBody, TargetIn,
};
use crate::widgets::compare::{CompareAction, CompareEvent, FillValue};

#[cfg(test)]
#[path = "draft_kind_ai_tests.rs"]
mod kind_ai_tests;

/// The bill field a read target is.
fn field_of(t: ReadTarget) -> Option<Field> {
    Some(match t {
        ReadTarget::Vendor => Field::Vendor,
        ReadTarget::BillDate => Field::Date,
        ReadTarget::DueDate => Field::DueDate,
        ReadTarget::Reference => Field::Reference,
        ReadTarget::AmountDue => Field::Amount,
        _ => return None,
    })
}

/// The read target a bill field is.
fn target_of(f: Field) -> Option<ReadTarget> {
    Some(match f {
        Field::Vendor => ReadTarget::Vendor,
        Field::Date => ReadTarget::BillDate,
        Field::DueDate => ReadTarget::DueDate,
        Field::Reference => ReadTarget::Reference,
        Field::Amount => ReadTarget::AmountDue,
        _ => return None,
    })
}

impl EnterBillScreen {
    /// Opens draft `o` here (a fresh bill in draft mode; the draft loads).
    pub(crate) fn open_draft(&mut self, o: DraftOpen) {
        self.new_bill(false);
        let tenant = self.tenant.as_ref().map(|(t, _)| *t);
        self.draft = Some(DraftSession::new(Arc::clone(&self.link), tenant, o));
    }

    /// In draft mode, nothing may change while the session is busy or asks.
    pub(crate) fn draft_locked(&self) -> bool {
        self.draft.as_ref().is_some_and(DraftSession::locked)
    }

    /// Every frame (before the reading's own poll): the session's events.
    pub(crate) fn poll_draft(&mut self, ctx: &egui::Context) {
        // The queue's Enter (WP-F1) hands its draft over here.
        if let Some(o) = crate::screens::doc_review::take_open(ctx, DraftForm::EnterBill) {
            self.open_draft(DraftOpen {
                id: o.id,
                form: o.form,
            });
        }
        let cards = crate::reading::service::card_accounts_of(&self.accounts);
        let today = self.today;
        let Some(s) = self.draft.as_mut() else {
            return;
        };
        // A Read document of a `new` draft lends its words and spots.
        if let Some(last) = self.read.local.last.as_deref() {
            s.adopt_reading(last);
        } else if self.read.problem.is_some() {
            // Its Read document failed: the banner says so (UI item 4).
            s.read_failed();
        }
        let events = s.tick(ctx, &cards, today);
        for ev in events {
            self.draft_event(ev);
        }
        self.feed_assist();
    }

    /// Enter Bill's own Ask AI to help (A03) works on the draft too, once
    /// the page words are ready: the stored values with the words go to
    /// the reading's `last` (the AI amount gate needs the words).
    fn feed_assist(&mut self) {
        let Some(s) = self.draft.as_ref() else {
            return;
        };
        if self.read.local.last.is_some() || self.read.local.helped || self.read.local.busy() {
            return;
        }
        let Some(base) = s.assist_base() else {
            return;
        };
        let bytes = s.file.as_ref().map(|f| Arc::clone(&f.bytes));
        self.read.ensure_switches(&self.link);
        self.read.local.needs_help = crate::reading::assist::needs_help(&base);
        self.read.local.bytes = bytes;
        self.read.local.vendors = map::vendor_candidates(&self.contacts);
        self.read.local.helped = s.assist.helped;
        self.read.local.last = Some(Box::new(base));
    }

    /// The form's values for "Enter this as".
    fn form_values(&self) -> FormValues {
        let f = self.compose();
        FormValues {
            payee: f.vendor,
            date: f.date,
            total: f.amount_due,
            paid_from: None,
            lines: f.expenses.lines,
            marks: f.read,
        }
    }

    /// "Enter this as" `to`: the draft opens in that form's window with the
    /// owner's work.
    pub(crate) fn enter_as(&mut self, ctx: &egui::Context, to: DraftForm) {
        let v = self.form_values();
        let Some(s) = self.draft.as_ref() else {
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
        let Some(s) = self.draft.as_mut() else {
            return;
        };
        let Some(c) = s.take_carry() else {
            return;
        };
        let rows: Vec<Option<RowKey>> = (0..c.lines.len()).map(|i| s.line_key(i)).collect();
        let form = s.form;
        let mut v = self.form_values();
        c.apply_to(form, &mut v, |i| rows.get(i).copied().flatten(), |_| false);
        let mut f = self.compose();
        if c.total.is_some() {
            f.amount_from_lines = false;
        }
        f.vendor = v.payee;
        f.date = v.date;
        f.amount_due = v.total;
        f.read = v.marks;
        f.expenses.lines = v.lines;
        self.take_form(f);
        self.refresh_terms_dates();
        self.track_read();
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
            // Enter Bill asks AI through A03's own path (`feed_assist`).
            DraftEvent::Cancelled | DraftEvent::Helped => {}
            DraftEvent::Reload => {
                // The draft is read again; its Loaded refills the form.
                self.new_bill(false);
            }
            DraftEvent::Next(Some(o)) if o.form == DraftForm::EnterBill => self.open_draft(o),
            DraftEvent::Next(Some(o)) => {
                review::open(&self.link, o);
                self.close_window();
            }
            DraftEvent::Close | DraftEvent::Next(None) => self.close_window(),
        }
    }

    fn close_window(&mut self) {
        self.link
            .push(CompanyEvent::CloseWindow(CommandId::new(super::KIND)));
    }

    /// The document into the source card (locked) and, once, the stored
    /// values into the empty fields.
    fn draft_loaded(&mut self) {
        self.draft_show_file();
        let Some(s) = self.draft.as_mut() else {
            return;
        };
        let file_name = s.file.as_ref().map(|f| f.name.clone()).unwrap_or_default();
        let Some(local) = s.take_fill() else {
            return;
        };
        let m = map::to_doc(&local);
        self.read.job_file.clone_from(&file_name);
        self.read.local.file_name = file_name;
        self.fill(&Incoming {
            doc: m.doc,
            unknown_vendor: m.unknown_vendor,
            unreadable: false,
            default_account: None,
            checks: m.checks,
            local: true,
            balance_due: m.balance_due,
        });
        // Read with the vendor's saved layout: the one "Filled from" line
        // (the reading banner's) names it (UI check item 3).
        if let Some(vendor) = self.draft_vendor_name()
            && let Some(f) = self.read.filled.as_mut()
        {
            f.file_name = t_with("doc_form.banner.layout_name", &[("vendor", &vendor)]);
        }
        // The read lines, in the reading's order (their spots).
        let read = self.header.current().read.clone();
        let lines: Vec<ReadTarget> = self
            .expenses
            .value()
            .lines
            .iter()
            .map(|l| ReadTarget::Line(l.key))
            .filter(|t| read.state(*t).is_some())
            .collect();
        if let Some(s) = self.draft.as_mut() {
            s.filled(&lines);
        }
        self.apply_carry();
    }

    /// The draft's file in the source card, its key the form's draft key.
    pub(crate) fn draft_show_file(&mut self) {
        let Some(s) = self.draft.as_ref() else {
            return;
        };
        let (Some(file), Some(a)) = (
            s.file.clone(),
            s.draft.as_ref().and_then(|d| d.attachment_id),
        ) else {
            return;
        };
        let key = s.draft_key;
        if self.attach.shown == Some(a) {
            return;
        }
        self.attach.draft = key;
        let r = cl_model::forms::check::AttachmentRef {
            id: a.to_string(),
            name: file.name.clone(),
        };
        if !self
            .header
            .current()
            .attachments
            .iter()
            .any(|x| x.id == r.id)
        {
            let mut f = self.header.current().clone();
            f.attachments.push(r);
            // Part of the opened form, not an edit.
            self.header.load(f);
        }
        self.attach.shown = Some(a);
        self.attach.viewer.set_full_size_button(true);
        self.attach.viewer.open(file);
        self.load_attachments();
    }

    /// Accept: the checked bill through the draft (instead of `POST
    /// /bills`). True when it was sent.
    pub(crate) fn accept_draft(
        &mut self,
        c: &CheckedBill,
        intent: SaveIntent,
        password: Option<cl_core::Secret>,
    ) -> bool {
        if c.kind != BillKind::Bill {
            self.save.notice = Some("doc_form.bill_only");
            return false;
        }
        let Draft::Bill(body) = to_draft(c, self.attach.draft, self.caps().lines) else {
            return false;
        };
        let vendor_name = self.lists.name_text(c.vendor).map(str::to_string);
        let marks = self.header.current().read.clone();
        let Some(s) = self.draft.as_mut() else {
            return false;
        };
        let lines: Vec<cl_core::Money> = c.expense_lines.iter().map(|l| l.amount).collect();
        let learn = LearnInput {
            vendor: Some(c.vendor),
            values: FinalValues {
                vendor: vendor_name.clone().map(|v| Final {
                    entry: s.entry(ReadTarget::Vendor, &marks, &v),
                    value: v,
                }),
                date: Some(Final {
                    value: c.date,
                    entry: s.entry(ReadTarget::BillDate, &marks, &c.date),
                }),
                due_date: c.due_date.map(|d| Final {
                    value: d,
                    entry: s.entry(ReadTarget::DueDate, &marks, &d),
                }),
                reference: c.reference.clone().map(|r| Final {
                    entry: s.entry(ReadTarget::Reference, &marks, &r),
                    value: r,
                }),
                total: Some(Final {
                    value: c.amount,
                    entry: s.entry(ReadTarget::AmountDue, &marks, &c.amount),
                }),
                line_amounts: Some(lines),
            },
            card: None,
        };
        let accept = Accept {
            form_digest: Canon::of_bill(c).digest(),
            body: PostBody::Bill(body),
            billable: true,
            intent,
            password,
            probe: MatchProbe {
                amount: c.amount,
                date: c.date,
                payee: vendor_name,
                contact: Some(c.vendor),
                account: None,
                account_name: None,
            },
            learn: Some(learn),
        };
        s.accept(accept)
    }

    /// The document posted (or was attached): the form is saved; then the
    /// next draft (Save & New) or close.
    fn draft_posted(&mut self, id: TransactionId, intent: SaveIntent) {
        self.closing_prompt.saved();
        self.save.count += 1;
        self.save.last = Some(LastSaved {
            kind: BillKind::Bill,
            id,
        });
        self.save.last_action = Some("save");
        self.save.problems.clear();
        self.save.error = None;
        let vendor = self.vendor();
        self.publish(vendor);
        // The values are in the books now: the form is clean.
        let lists = Arc::clone(&self.lists);
        let header = self.header.current().clone();
        self.header.load(header);
        let lines: FormLines = self.expenses.value().clone();
        self.expenses.load(lines, &*lists);
        let items: ItemLines = self.items.value().clone();
        self.items.load(items);
        // The draft's files moved with the convert.
        self.attach.draft = None;
        match intent {
            SaveIntent::SaveAndNew => {
                if let Some(s) = self.draft.as_mut() {
                    s.find_next();
                }
            }
            _ => self.close_window(),
        }
    }

    /// Keys in draft mode before the form's own: the session's questions,
    /// Page Up/Down through the queue, no Bill / Vendor credit switch.
    pub(crate) fn draft_command(&mut self, id: CommandId) -> Option<bool> {
        let dirty = self.is_dirty();
        let s = self.draft.as_mut()?;
        if let Some(h) = s.command(id) {
            return Some(h);
        }
        match id.as_str() {
            "form.prev_record" => Some(s.step(Step::Prev, dirty)),
            "form.next_record" => Some(s.step(Step::Next, dirty)),
            "bill.kind_toggle" => {
                self.save.notice = Some("doc_form.bill_only");
                Some(true)
            }
            "form.clear" => {
                self.new_bill(false);
                self.draft_show_file();
                Some(true)
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------ compare

    /// The compare targets and the focused field, then the viewer gets the
    /// view (docked) before it is drawn.
    pub(crate) fn compare_before(&mut self) {
        let style = self.money_style;
        let Some(s) = self.draft.as_ref() else {
            self.attach.viewer.set_compare(None);
            return;
        };
        let f = self.header.current().clone();
        let marks = f.read.clone();
        let date =
            |d: Option<cl_core::CalendarDate>| d.map(crate::shell::us_date).unwrap_or_default();
        let mut fields = vec![
            TargetIn {
                target: ReadTarget::Vendor,
                label: t("bill.vendor"),
                fill: None,
                value: f
                    .vendor
                    .and_then(|v| self.lists.name_text(v))
                    .unwrap_or_default()
                    .to_string(),
            },
            TargetIn {
                target: ReadTarget::BillDate,
                label: t("bill.date"),
                fill: Some(FillKind::Date),
                value: date(f.date),
            },
            TargetIn {
                target: ReadTarget::Reference,
                label: t("bill.ref"),
                fill: Some(FillKind::Text),
                value: f.reference.clone(),
            },
            TargetIn {
                target: ReadTarget::DueDate,
                label: t("bill.due"),
                fill: Some(FillKind::Date),
                value: date(f.due_date),
            },
            TargetIn {
                target: ReadTarget::AmountDue,
                label: t("bill.amount_due"),
                fill: Some(FillKind::Money),
                value: f.amount_due.map(|m| m.format(style)).unwrap_or_default(),
            },
        ];
        for l in &self.expenses.value().lines {
            let target = ReadTarget::Line(l.key);
            if marks.state(target).is_some() || s.clicked.contains_key(&target) {
                fields.push(TargetIn {
                    target,
                    label: t("doc_form.line_amount"),
                    fill: Some(FillKind::Money),
                    value: l.amount.map(|m| m.format(style)).unwrap_or_default(),
                });
            }
        }
        let focused = self.focused.and_then(target_of).map(TargetKey::from);
        let Some(s) = self.draft.as_mut() else {
            return;
        };
        s.set_targets(&marks, fields);
        s.compare.focus(focused);
        let view = s.compare.view();
        self.attach.viewer.set_compare(Some(view));
    }

    /// The docked viewer's compare events, and a box re-read's answer.
    pub(crate) fn compare_after(&mut self) {
        let events = self.attach.viewer.take_compare_events();
        for ev in events {
            self.on_compare_event(ev);
        }
        let today = self.today;
        if let Some(a) = self.draft.as_mut().and_then(|s| s.compare_poll(today)) {
            self.apply_compare(a);
        }
    }

    /// One compare event (the docked viewer, or the pop-out's outbox).
    pub(crate) fn on_compare_event(&mut self, ev: CompareEvent) {
        let today = self.today;
        if let Some(a) = self.draft.as_mut().and_then(|s| s.compare_event(ev, today)) {
            self.apply_compare(a);
        }
    }

    fn apply_compare(&mut self, action: CompareAction) {
        if self.draft_locked() {
            return;
        }
        let Some(s) = self.draft.as_ref() else {
            return;
        };
        match action {
            CompareAction::Focus(key) => match s.target_of(key) {
                Some(ReadTarget::Line(k)) => {
                    let lists = Arc::clone(&self.lists);
                    self.tab = super::Tab::Expenses;
                    self.expenses.form_mut().focus(
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
                    (ReadTarget::AmountDue, FillValue::Money(m)) => self.amount_typed(Some(m)),
                    (ReadTarget::BillDate, FillValue::Date(d)) => {
                        self.header.edit(super::field::DATE, |h| h.date = Some(d));
                        self.refresh_terms_dates();
                    }
                    (ReadTarget::DueDate, FillValue::Date(d)) => {
                        self.header.edit(super::field::DUE, |h| {
                            h.due_date = Some(d);
                            h.due_overridden = true;
                        });
                    }
                    (ReadTarget::Reference, FillValue::Text(r)) => {
                        self.header
                            .edit(super::field::REFERENCE, |h| h.reference = r);
                    }
                    (ReadTarget::Line(k), FillValue::Money(m)) => {
                        self.expenses.form_mut().edit_step(|v| {
                            if let Some(l) = v.lines.iter_mut().find(|l| l.key == k) {
                                l.amount = Some(m);
                            }
                        });
                        self.header.edit_step(|h| h.read.edited(t));
                    }
                    _ => return,
                }
                self.header.edit_step(|h| h.read.edited(t));
                self.sync_inputs();
            }
        }
    }

    /// The control channel's `draft` (no amounts or names).
    pub(crate) fn draft_state(&self) -> serde_json::Value {
        self.draft
            .as_ref()
            .map_or(serde_json::Value::Null, DraftSession::state_json)
    }

    /// The suggested vendor's name (the "saved layout" banner).
    pub(crate) fn draft_vendor_name(&self) -> Option<String> {
        let v = self.draft.as_ref()?.layout_vendor()?;
        self.lists.name_text(v).map(str::to_string)
    }
}
