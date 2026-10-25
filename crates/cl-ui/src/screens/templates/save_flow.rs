//! Save as Template on a form (P15 T4): the toolbar button's state, the
//! Template dialog, "Replace" or "Save as new" for a form opened from a
//! template (Q-P15l), and the banner for a template's warnings.
//!
//! The form hands over its checked draft (`TemplateSource::template_draft`,
//! the same check as Save); this component only stores it. A Replace sends
//! the version the form was opened at (409 `STALE` → "This template
//! changed in another window"). After a form opened from a Templates Due
//! occurrence saves, [`SaveFlow::form_saved`] marks the occurrence entered
//! (Q-P15p: a failed mark leaves the reminder showing).

use std::sync::Arc;

use cl_api::{
    ApiError, ConflictReason, DraftWarning, NewTemplate, RequestGroup, Template, TemplateChange,
    TemplateDraft, TemplateGroup,
};
use cl_core::CalendarDate;
use cl_core::ids::{TemplateGroupId, TemplateId, TransactionId};
use cl_model::templates::{CheckedDialog, GroupChoice};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::dialog::{DialogClick, DialogUi, api_schedule};
use super::engine::{Job, finished, run, templates_on};
use crate::company::CompanyLink;
use crate::company::demo::template_store;
use crate::forms::template::{SaveAsTemplate, TemplateOpen};
use crate::i18n::t;
use crate::theme::Tokens;

/// The template a form was opened from ("Use").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Origin {
    /// The template.
    pub id: TemplateId,
    /// Its version when it was read (Replace sends it).
    pub version: u32,
}

/// One form's Save as Template.
#[derive(Debug, Default)]
pub struct SaveFlow {
    dialog: Option<DialogUi>,
    draft: Option<(TemplateDraft, String)>,
    pub(crate) origin: Option<Origin>,
    /// The Templates Due occurrence this form fills in.
    pub(crate) occurrence: Option<(TemplateId, CalendarDate)>,
    replace_prompt: bool,
    job: Option<Job<Template>>,
    groups_job: Option<Job<Vec<TemplateGroup>>>,
    resolve_job: Option<Job<()>>,
    /// Templates saved from this form.
    pub(crate) saved_count: u32,
    /// `created` or `replaced`.
    pub(crate) last: Option<&'static str>,
    pub(crate) error: Option<ApiError>,
    pub(crate) stale: bool,
    /// What the engine flagged in the template the form was filled from.
    pub(crate) warnings: Vec<DraftWarning>,
    /// The occurrence could not be marked entered.
    pub(crate) resolve_failed: bool,
    /// The occurrence being marked entered.
    resolving: Option<(TemplateId, CalendarDate)>,
    /// Marked entered; Templates Due drops it at the next look.
    resolved: Option<(TemplateId, CalendarDate)>,
}

/// The template's warnings as the owner reads them.
fn warning_text(w: &[DraftWarning]) -> Option<String> {
    use cl_api::DraftWarningCode as C;
    let first = w.first()?;
    Some(t(match first.code {
        C::AccountInactive | C::NotFound => "templates.use.warn.account",
        C::ContactInactive => "templates.use.warn.contact",
        C::TagInactive => "templates.use.warn.tag",
        _ => "templates.use.warn.other",
    }))
}

impl SaveFlow {
    /// The toolbar button: off without `templates-v1` (and for `blocked`,
    /// a form-specific reason such as a grouped deposit).
    #[must_use]
    pub(crate) fn button(&self, link: &CompanyLink, blocked: Option<String>) -> SaveAsTemplate {
        if !templates_on(link) {
            return SaveAsTemplate::Disabled(t("template.save.unavailable"));
        }
        match blocked {
            Some(why) => SaveAsTemplate::Disabled(why),
            None if self.busy() => SaveAsTemplate::Disabled(t("templates.save.busy")),
            None => SaveAsTemplate::Enabled,
        }
    }

    /// A call is on its way.
    #[must_use]
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some() || self.groups_job.is_some() || self.resolve_job.is_some()
    }

    /// The form was filled from a template.
    pub(crate) fn opened(&mut self, t: &TemplateOpen) {
        self.origin = Some(Origin {
            id: t.template,
            version: t.version,
        });
        self.occurrence = t.occurrence.map(|d| (t.template, d));
        self.warnings.clone_from(&t.warnings);
    }

    /// The form was cleared or moved to another record: it no longer
    /// comes from a template.
    pub(crate) fn forget(&mut self) {
        self.origin = None;
        self.occurrence = None;
        self.warnings.clear();
    }

    /// Save as Template with the form's checked draft.
    pub(crate) fn start(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        draft: TemplateDraft,
        client_form: &str,
        name: String,
        today: CalendarDate,
    ) {
        if !templates_on(link) || self.busy() {
            return;
        }
        self.error = None;
        self.stale = false;
        let auto = draft.allows_auto();
        self.draft = Some((draft, client_form.to_string()));
        if self.origin.is_some() {
            self.replace_prompt = true;
            return;
        }
        self.open_dialog(link, group, name, auto, today);
    }

    fn open_dialog(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        name: String,
        auto: bool,
        today: CalendarDate,
    ) {
        self.dialog = Some(DialogUi::new(name, today, auto, Vec::new(), today));
        self.groups_job = Some(run(
            link,
            group,
            None,
            |s| Ok(template_store::groups(s)),
            |s| async move { s.templates().groups().await },
        ));
    }

    fn create(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        c: CheckedDialog<TemplateGroupId>,
    ) {
        let Some((draft, form)) = self.draft.clone() else {
            return;
        };
        let new = NewTemplate {
            name: c.name.clone(),
            group: None,
            client_form: Some(form),
            draft,
            schedule: api_schedule(&c),
        };
        let choice = c.group;
        let (new2, choice2) = (new.clone(), choice.clone());
        self.job = Some(run(
            link,
            group,
            Some("orphan.template_save"),
            move |s| {
                let mut new = new;
                new.group = match choice {
                    GroupChoice::None => None,
                    GroupChoice::Existing(g) => Some(g),
                    GroupChoice::New(n) => Some(template_store::create_group(s, &n)?.id),
                };
                template_store::create(s, &new)
            },
            move |s| async move {
                let mut new = new2;
                new.group = match choice2 {
                    GroupChoice::None => None,
                    GroupChoice::Existing(g) => Some(g),
                    GroupChoice::New(n) => Some(s.templates().create_group(&n).await?.id),
                };
                s.templates().create(&new).await
            },
        ));
        self.last = Some("created");
    }

    fn replace(&mut self, link: &Arc<CompanyLink>, group: Option<&RequestGroup>) {
        let (Some(origin), Some((draft, form))) = (self.origin, self.draft.clone()) else {
            return;
        };
        let change = TemplateChange {
            client_form: Some(form),
            draft: Some(draft),
            ..TemplateChange::default()
        };
        let change2 = change.clone();
        self.job = Some(run(
            link,
            group,
            Some("orphan.template_save"),
            move |s| template_store::update(s, origin.id, origin.version, &change),
            move |s| async move {
                s.templates()
                    .update(origin.id, origin.version, &change2)
                    .await
            },
        ));
        self.last = Some("replaced");
    }

    /// The form saved transaction `txn`: an occurrence it filled in is
    /// marked entered (Templates Due "Open in form").
    pub(crate) fn form_saved(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        txn: TransactionId,
    ) {
        let Some((id, date)) = self.occurrence.take() else {
            return;
        };
        // Until the link takes, Templates Due shows the date as already
        // entered and the runner tries again with this transaction (S2).
        link.due().linking(id, date, txn);
        self.resolving = Some((id, date));
        self.resolve_job = Some(run(
            link,
            group,
            Some("orphan.template_resolve"),
            move |s| crate::company::demo::templates::resolve(s, id, date, txn),
            move |s| async move { s.templates().resolve(id, date, txn).await },
        ));
        self.poll(link);
        self.settle_resolved(link);
    }

    /// A marked occurrence leaves Templates Due's lists (P15 lane C).
    fn settle_resolved(&mut self, link: &CompanyLink) {
        if let Some((id, date)) = self.resolved.take() {
            let mut board = link.due();
            board.linked(id, date);
            board.handled(id, date);
        }
    }

    fn poll(&mut self, link: &CompanyLink) {
        if let Some(r) = finished(&mut self.groups_job)
            && let Some(d) = self.dialog.as_mut()
        {
            d.groups = r.unwrap_or_default();
        }
        if let Some(r) = finished(&mut self.job) {
            if let Some(d) = self.dialog.as_mut() {
                d.busy = false;
            }
            match r {
                Ok(tpl) => {
                    self.saved_count += 1;
                    // An open Templates list shows it at once.
                    link.bus().publish(crate::data_bus::Change::Templates);
                    self.dialog = None;
                    self.draft = None;
                    // The form now stands for the saved template.
                    self.origin = Some(Origin {
                        id: tpl.id,
                        version: tpl.version,
                    });
                }
                Err(ApiError::Conflict {
                    reason: ConflictReason::Stale,
                    ..
                }) => {
                    self.stale = true;
                    self.last = None;
                }
                Err(e) => {
                    self.last = None;
                    self.error = Some(e);
                }
            }
        }
        if let Some(r) = finished(&mut self.resolve_job) {
            let done = self.resolving.take();
            match r {
                Ok(()) => {
                    self.resolve_failed = false;
                    self.resolved = done;
                }
                // A retry after a lost reply: the date is entered.
                Err(e) if cl_api::is_occurrence_taken(&e) => {
                    self.resolve_failed = false;
                    self.resolved = done;
                }
                // Another try can't change this: the reminder stays and
                // can be dismissed (Q-P15p).
                Err(e)
                    if matches!(
                        e,
                        ApiError::Validation(_)
                            | ApiError::NotFound { .. }
                            | ApiError::Forbidden { .. }
                    ) =>
                {
                    self.resolve_failed = true;
                    if let Some((id, date)) = done {
                        link.due().linked(id, date);
                    }
                }
                // Anything else is tried again by the runner (S2).
                Err(_) => {
                    self.resolve_failed = false;
                    if let Some((id, date)) = done {
                        link.due().link_failed(id, date);
                    }
                }
            }
        }
    }

    /// The toolbar slot: the button (true when clicked: the form then runs
    /// Save as Template), the warnings banner and the open dialogs.
    pub(crate) fn slot(
        &mut self,
        ui: &mut Ui,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        state: &SaveAsTemplate,
        today: CalendarDate,
    ) -> bool {
        self.poll(link);
        self.settle_resolved(link);
        let clicked = crate::forms::header::save_as_template_button(ui, state);
        let tokens = Tokens::of(ui.ctx());
        if let Some(text) = warning_text(&self.warnings) {
            ui.label(RichText::new(text).color(tokens.warn_text));
            if ui
                .small_button(t("templates.use.warn.dismiss"))
                .on_hover_text(t("templates.use.warn.dismiss"))
                .clicked()
            {
                self.warnings.clear();
            }
        }
        if self.stale {
            ui.label(RichText::new(t("templates.save.stale")).color(tokens.warn_text));
        }
        if let Some(e) = &self.error {
            let text = super::auto_start_text(e, today)
                .unwrap_or_else(|| t(cl_core::OwnerFacing::owner_message(e).key));
            ui.label(RichText::new(text).color(tokens.warn_text));
        }
        if self.resolve_failed {
            ui.label(RichText::new(t("templates.save.resolve_failed")).color(tokens.warn_text));
        }
        self.windows(ui.ctx(), link, group, today);
        clicked
    }

    fn windows(
        &mut self,
        ctx: &egui::Context,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        today: CalendarDate,
    ) {
        if self.replace_prompt {
            let mut choice = None;
            egui::Window::new(t("templates.replace.title"))
                .id(egui::Id::unique("templates/replace"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(t("templates.replace.body"));
                    ui.horizontal(|ui| {
                        if ui.button(t("templates.replace.replace")).clicked() {
                            choice = Some(0);
                        }
                        if ui.button(t("templates.replace.new")).clicked() {
                            choice = Some(1);
                        }
                        if ui.button(t("templates.dialog.cancel")).clicked() {
                            choice = Some(2);
                        }
                    });
                });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                choice = Some(2);
            }
            if let Some(c) = choice {
                self.choose(c, link, group, today);
            }
        }
        if let Some(d) = self.dialog.as_mut() {
            match d.show(ctx) {
                DialogClick::None => {}
                DialogClick::Cancel => {
                    self.dialog = None;
                    self.draft = None;
                }
                DialogClick::Save(c) => {
                    d.busy = true;
                    self.create(link, group, c);
                    self.poll(link);
                }
            }
        }
    }

    /// The Replace prompt's answer: 0 Replace, 1 Save as new, 2 Cancel.
    pub(crate) fn choose(
        &mut self,
        choice: u8,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        today: CalendarDate,
    ) {
        self.replace_prompt = false;
        match choice {
            0 => {
                self.replace(link, group);
                self.poll(link);
            }
            1 => {
                let auto = self.draft.as_ref().is_some_and(|(d, _)| d.allows_auto());
                self.open_dialog(link, group, String::new(), auto, today);
                self.poll(link);
            }
            _ => self.draft = None,
        }
    }

    /// The dialog (control channel and tests).
    #[cfg(test)]
    pub(crate) fn dialog_mut(&mut self) -> Option<&mut DialogUi> {
        self.dialog.as_mut()
    }

    /// The dialog's Save (control channel and tests).
    #[cfg(test)]
    pub(crate) fn dialog_save(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
    ) -> bool {
        let Some(c) = self.dialog.as_mut().and_then(DialogUi::save) else {
            return false;
        };
        self.create(link, group, c);
        self.poll(link);
        true
    }

    /// The control-channel state (ids and flags only).
    #[must_use]
    pub(crate) fn state_json(&self) -> Value {
        json!({
            "dialog": self.dialog.as_ref().map(DialogUi::state_json),
            "replace_prompt": self.replace_prompt,
            "from_template": self.origin.map(|o| o.id.to_string()),
            "occurrence": self.occurrence.map(|(_, d)| d.to_wire()),
            "saved_count": self.saved_count,
            "last": self.last,
            "stale": self.stale,
            "error": self.error.is_some(),
            "auto_start_too_early": self.error.as_ref().is_some_and(cl_api::is_auto_start_too_early),
            "warnings": self.warnings.len(),
            "busy": self.busy(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::TransferDraft;
    use cl_core::Money;

    fn d(y: i32, m: u32, day: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, day).expect("date")
    }

    fn draft() -> TemplateDraft {
        TemplateDraft::Transfer(TransferDraft {
            from: crate::company::demo::account_id(1010),
            to: crate::company::demo::account_id(1020),
            date: d(2026, 11, 4),
            amount: Money::parse_wire("25.00").expect("m"),
            memo: None,
        })
    }

    fn demo() -> (tempfile::TempDir, Arc<CompanyLink>) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        if let Some(mut s) = link.demo_state() {
            s.open = s.companies.first().map(|c| c.tenant_id);
        }
        (dir, link)
    }

    #[test]
    fn save_then_replace_with_the_version_it_was_read_at() {
        let (_dir, link) = demo();
        let mut f = SaveFlow::default();
        assert_eq!(f.button(&link, None), SaveAsTemplate::Enabled);
        assert!(matches!(
            f.button(&link, Some("grouped".into())),
            SaveAsTemplate::Disabled(_)
        ));
        let today = d(2026, 11, 4);
        f.start(&link, None, draft(), "transfer", "Savings".into(), today);
        f.poll(&link);
        assert!(f.dialog_mut().is_some());
        let dlg = f.dialog_mut().expect("dialog");
        assert!(dlg.set("new_group", "Monthly bills"));
        let mut list = link.bus().subscribe();
        assert!(f.dialog_save(&link, None));
        assert_eq!(f.saved_count, 1);
        // An open Templates list hears about it.
        assert!(link.bus().take(&mut list).templates);
        assert_eq!(f.last, Some("created"));
        let id = f.origin.expect("origin").id;
        let saved = link
            .demo_state()
            .map(|s| template_store::get(&s, id))
            .expect("demo");
        assert_eq!(
            saved.expect("t").group_name.as_deref(),
            Some("Monthly bills")
        );
        // Replace from the form.
        f.start(&link, None, draft(), "transfer", String::new(), today);
        assert!(f.state_json()["replace_prompt"].as_bool().unwrap_or(false));
        f.choose(0, &link, None, today);
        assert_eq!((f.saved_count, f.last), (2, Some("replaced")));
        // Another window changed it: Replace with the old version is stale.
        f.origin = Some(Origin { id, version: 1 });
        f.start(&link, None, draft(), "transfer", String::new(), today);
        f.choose(0, &link, None, today);
        assert!(f.stale);
        f.choose(2, &link, None, today);
        assert!(f.state_json()["dialog"].is_null());
        f.forget();
        assert!(f.origin.is_none());
    }

    #[test]
    fn an_occurrence_is_marked_entered_after_the_form_saves() {
        let (_dir, link) = demo();
        let mut f = SaveFlow::default();
        let tid = link
            .demo_state()
            .map(|mut s| {
                template_store::create(
                    &mut s,
                    &cl_api::NewTemplate {
                        name: "Savings".into(),
                        group: None,
                        client_form: None,
                        draft: draft(),
                        schedule: Some(cl_api::TemplateSchedule {
                            mode: cl_api::ScheduleMode::Remind,
                            frequency: cl_api::Frequency::Monthly,
                            interval: 1,
                            next_date: d(2026, 11, 1),
                            end: cl_api::ScheduleEnd::Never,
                        }),
                    },
                )
            })
            .expect("demo")
            .expect("template")
            .id;
        f.opened(&TemplateOpen {
            kind: cl_model::CommandId::new("banking.transfer"),
            template: tid,
            version: 1,
            occurrence: Some(d(2026, 11, 1)),
            draft: draft(),
            warnings: vec![DraftWarning {
                field: "fromAccountId".into(),
                code: cl_api::DraftWarningCode::AccountInactive,
            }],
        });
        assert_eq!(f.state_json()["warnings"], 1);
        assert!(warning_text(&f.warnings).is_some());
        let txn = TransactionId::parse("d0000000-0000-4000-8000-b00000000001").expect("t");
        f.form_saved(&link, None, txn);
        f.poll(&link);
        assert!(!f.resolve_failed);
        assert!(
            link.demo_state()
                .is_some_and(|s| s.occurrences.contains_key(&(tid, d(2026, 11, 1))))
        );
    }
}
