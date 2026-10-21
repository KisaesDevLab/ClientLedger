//! The screen's polling, loading, the control-channel state and the
//! [`Screen`] implementation.

use cl_api::{Capabilities, DailyEntry, DailyTemplate};
use cl_core::CalendarDate;
use cl_core::ids::{DailyEntryId, DailyLineId, DailyTemplateId, TransactionId};
use cl_model::forms::daily_sales::{DailyLayout, DailySalesForm, DailyTotals, OverShort, totals};
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use super::{Call, DailySalesScreen, Field, Reply, Saved, field, layout, layout_of};
use crate::company::Task;
use crate::i18n::t;
use crate::screens::company::form::DateInput;
use crate::windows::Screen;

impl DailySalesScreen {
    /// The engine takes the closing-date password for a day
    /// (`daily-sales-lock-v1`; the demo always does).
    pub(crate) fn lock_supported(&self) -> bool {
        self.books.is_some() || {
            let caps = self.link.capabilities();
            caps.has(Capabilities::DAILY_SALES_LOCK) && caps.has(Capabilities::CLOSING_PASSWORD)
        }
    }

    /// The day in the form is posted or void: shown, never changed.
    pub(crate) fn read_only(&self) -> bool {
        self.saved
            .as_ref()
            .is_some_and(|s| s.status != cl_api::EntryStatus::Draft)
    }

    /// The form has unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.header.is_dirty()
    }

    /// The form's totals (the exact twin of the engine's).
    pub(crate) fn totals(&self) -> Option<DailyTotals> {
        totals(self.header.current(), &self.layout).ok()
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = self.accounts_task.as_mut().and_then(Task::try_take) {
            self.accounts_task = None;
            if let Ok(a) = r {
                self.accounts = a;
            }
        }
        if let Some(r) = self.tags_task.as_mut().and_then(Task::try_take) {
            self.tags_task = None;
            if let Ok(tags) = r {
                self.tags = tags;
            }
        }
        if let Some(r) = self.settings_task.as_mut().and_then(Task::try_take) {
            self.settings_task = None;
            if let Ok(s) = r {
                self.closing = s.lock_date;
            }
        }
        if let Some(r) = self.templates_task.as_mut().and_then(Task::poll_result) {
            self.templates_task = None;
            self.templates_loaded = true;
            if let Ok(Reply::Templates(list)) = r {
                self.templates = list;
                self.templates_arrived();
            }
        }
        if let Some(r) = self.template_task.as_mut().and_then(Task::poll_result) {
            self.template_task = None;
            match r {
                Ok(Reply::Template(t)) => self.apply_template(t),
                Ok(_) => {}
                Err(e) => self.save.error = Some(e),
            }
        }
        if let Some(r) = self.load_task.as_mut().and_then(Task::poll_result) {
            self.load_task = None;
            match r {
                Ok(Reply::Entry(e)) => self.show_entry(e),
                Ok(_) => {}
                Err(e) => self.save.error = Some(e),
            }
        }
        if let Some(r) = self.open_task.as_mut().and_then(Task::poll_result) {
            self.open_task = None;
            if let Ok(Reply::Found(Some(id))) = r {
                self.open_entry(id);
            }
        }
        if let Some(r) = self.panel.recent_task.as_mut().and_then(Task::poll_result) {
            self.panel.recent_task = None;
            match r {
                Ok(Reply::Days(d)) => self.panel.recent = d,
                Ok(_) => {}
                Err(_) => self.panel.recent_failed = true,
            }
        }
        if let Some(r) = self.nav.task.as_mut().and_then(Task::poll_result) {
            self.nav.task = None;
            match r {
                Ok(Reply::Days(mut d)) => {
                    // Newest first, whatever order the list came in.
                    d.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
                    self.nav.list = d;
                    self.nav.loaded = true;
                    if let Some(older) = self.nav.want_older.take() {
                        self.step(older);
                    }
                }
                Ok(_) => self.nav.want_older = None,
                Err(e) => {
                    self.nav.want_older = None;
                    self.save.error = Some(e);
                }
            }
        }
        if let Some(r) = self.panel.drafts_task.as_mut().and_then(Task::poll_result) {
            self.panel.drafts_task = None;
            if let Ok(Reply::Days(d)) = r {
                self.panel.drafts = d;
            }
        }
        self.poll_save();
        self.poll_setup();
        if self.inputs.rev != self.header.revision() {
            self.sync_inputs();
        }
    }

    /// The layouts arrived: a form without one takes the first, and the
    /// one in use is read again (it may have changed).
    fn templates_arrived(&mut self) {
        let current = self.header.current().template;
        let keep = current.filter(|id| self.templates.iter().any(|t| t.id == *id));
        match (keep, self.templates.first().map(|t| t.id)) {
            (Some(id), _) => self.load_template(id),
            (None, Some(first)) if !self.read_only() => {
                let mut f = self.header.current().clone();
                f.template = Some(first);
                f.values.clear();
                if self.header.is_dirty() {
                    self.header.edit_step(|h| *h = f);
                } else {
                    self.header.load(f);
                }
                self.load_template(first);
            }
            _ => {
                if current.is_some() && !self.read_only() {
                    self.template = None;
                    self.layout = DailyLayout::default();
                    let mut f = self.header.current().clone();
                    f.template = None;
                    f.values.clear();
                    self.header.load(f);
                }
            }
        }
        self.inputs.rev = u64::MAX;
    }

    /// A layout with its lines arrived (first load, a pick, or a reload
    /// after it changed elsewhere).
    pub(crate) fn apply_template(&mut self, t: DailyTemplate) {
        if self.header.current().template != Some(t.id) {
            return;
        }
        self.layout = layout_of(&t);
        if let Some(i) = self.templates.iter().position(|x| x.id == t.id) {
            self.templates[i].name.clone_from(&t.name);
        }
        // Values of lines no longer on the layout are dropped.
        let ids: Vec<DailyLineId> = t.lines.iter().map(|l| l.id).collect();
        let stale = self
            .header
            .current()
            .values
            .keys()
            .any(|k| !ids.contains(k));
        if stale && !self.read_only() {
            self.header
                .edit_step(|h| h.values.retain(|k, _| ids.contains(k)));
        }
        self.template = Some(t);
        self.inputs.rev = u64::MAX;
    }

    /// The owner picked a layout.
    pub(crate) fn pick_layout(&mut self, id: DailyTemplateId) {
        self.layout_list = false;
        self.inputs.layout_problem = false;
        if self.header.current().template == Some(id) {
            self.inputs.rev = u64::MAX;
            return;
        }
        self.header.edit(field::LAYOUT, |h| {
            h.template = Some(id);
            h.values.clear();
        });
        self.template = None;
        self.layout = DailyLayout::default();
        self.load_template(id);
        self.inputs.rev = u64::MAX;
    }

    /// A saved day: a draft opens for posting, a posted or void day is
    /// shown read-only.
    pub(crate) fn show_entry(&mut self, e: DailyEntry) {
        let values = e.values.iter().map(|v| (v.line, Some(v.amount))).collect();
        let form = DailySalesForm {
            template: Some(e.template),
            date: Some(e.date),
            tag: e.tag,
            notes: e.notes.clone().unwrap_or_default(),
            values,
        };
        self.saved = Some(Saved {
            id: e.id,
            status: e.status.clone(),
            transaction: e.transaction,
            date: e.date,
        });
        self.header.load(form);
        match e.layout {
            Some(t) => self.apply_template(t),
            None => self.load_template(e.template),
        }
        self.save.problems.clear();
        self.save.error = None;
        self.save.notice = None;
        self.save.last_action = Some("load");
        self.inputs.rev = u64::MAX;
    }

    /// Reads a saved day into the form.
    pub(crate) fn open_entry(&mut self, id: DailyEntryId) {
        self.load_task = self.run(Call::Entry(id), false);
    }

    /// The texts from the form (after a load, clear, undo or a change the
    /// screen made).
    pub(crate) fn sync_inputs(&mut self) {
        let f = self.header.current();
        let style = self.money_style;
        if let Some(d) = f.date {
            self.inputs.date = DateInput::new(d);
        }
        self.inputs.layout = f
            .template
            .and_then(|id| {
                self.templates
                    .iter()
                    .find(|t| t.id == id)
                    .or(self.template.as_ref().filter(|t| t.id == id))
                    .map(|t| t.name.clone())
            })
            .unwrap_or_default();
        self.inputs.amounts = f
            .values
            .iter()
            .filter_map(|(k, v)| v.map(|m| (*k, m.format(style))))
            .collect();
        self.inputs.amount_problems.clear();
        self.inputs.layout_problem = false;
        self.inputs.rev = self.header.revision();
    }

    /// Saves in other windows (Q-262): the panel's days are read again;
    /// the layouts too, once the form has no unsaved edits (a change heard
    /// while dirty waits for then, as on the other forms).
    pub(crate) fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if changes.ledger_any() {
            self.load_panel();
            self.nav = super::DayNav::default();
        }
        if changes.lists() {
            self.lists_stale = true;
        }
        if self.lists_stale && !self.is_dirty() && self.templates_task.is_none() {
            self.lists_stale = false;
            self.load_templates();
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.accounts_task.is_some()
            || self.templates_task.is_some()
            || self.template_task.is_some()
            || self.load_task.is_some()
            || self.save.task.is_some()
            || self.void.task.is_some()
            || self.setup_task.is_some()
            || self.panel.recent_task.is_some()
            || self.panel.drafts_task.is_some()
            || self.open_task.is_some()
            || self.nav.task.is_some()
    }

    /// The control-channel state: flags and counts only (never an amount,
    /// a label or a note).
    pub(crate) fn state_json(&self) -> Value {
        let f = self.header.current();
        let totals = self.totals();
        let over_short = totals.as_ref().map(|t| match t.over_short {
            OverShort::Even => "even",
            OverShort::Over(_) => "over",
            OverShort::Short(_) => "short",
        });
        let mode = match self.saved.as_ref().map(|s| &s.status) {
            None => "new",
            Some(cl_api::EntryStatus::Draft) => "draft",
            Some(cl_api::EntryStatus::Posted) => "posted",
            Some(_) => "void",
        };
        let mut v = json!({
            "screen": "daily_sales",
            "company_open": self.tenant.is_some(),
            "loaded": self.templates_loaded,
            "books": self.books.is_some() || self.link.session().is_some(),
            "layouts": self.templates.len(),
            "layout_set": self.template.is_some(),
            "empty_state": self.templates_loaded && self.templates.is_empty(),
            "date": f.date.map(CalendarDate::to_wire),
            "has_tag": f.tag.is_some(),
            "lines": self.layout.lines.iter().filter(|l| l.active).count(),
            "values": f.values.values().filter(|v| v.is_some_and(|m| !m.is_zero())).count(),
            "over_short": over_short,
            "unmapped": totals.as_ref().map_or(0, |t| t.unmapped.len()),
            "mode": mode,
            "read_only": self.read_only(),
            "dirty": self.is_dirty(),
            "saving": self.save.task.is_some(),
            "saved_count": self.save.count,
            "save": {
                "refused": self.save.problems.first().map(|p| p.code()),
                "problems": self.save.problems.len(),
                "error": self.save.error.is_some(),
            },
            "last_action": self.save.last_action,
            "notice": self.save.notice,
            "lock_supported": self.lock_supported(),
            "void_open": self.void.open,
            "setup_open": self.setup.is_some(),
            "setup": self.setup.as_ref().map(layout::Setup::state_json),
            "recent": self.panel.recent.len(),
            "recent_failed": self.panel.recent_failed,
            "drafts": self.panel.drafts.len(),
            "layout_list": self.layout_list,
            "nav_at": self.nav.at,
            "nav_prompt": self.nav_prompt.is_some(),

            "focused": self.focused.map(|f| match f {
                Field::Layout => "layout",
                Field::Date => "date",
                Field::Tag => "tag",
                Field::Notes => "notes",
                Field::Amount(_) => "amount",
            }),
        });
        if let (Value::Object(m), Value::Object(c)) = (
            &mut v,
            self.closing_prompt.state_json(self.lock_supported()),
        ) {
            m.extend(c);
        }
        v
    }
}

impl Screen for DailySalesScreen {
    fn title(&self) -> String {
        t("cmd.clients.daily_sales")
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
        self.is_dirty() || self.setup.as_ref().is_some_and(layout::Setup::dirty)
    }

    fn save(&mut self) -> bool {
        self.start_save(cl_model::forms::SaveIntent::Save, None)
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(handled) = self.closing_prompt.command(id) {
            return handled;
        }
        if self.setup.is_some()
            && let Some(handled) = self.setup_command(id)
        {
            return handled;
        }
        self.form_command(id)
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.open_task = self.run(Call::FindTransaction(txn), false);
        true
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}
