//! Memorize and Save changes (plan §3.6, Q-A08c, Q-A08n, Q-310).
//!
//! - Memorize (Ctrl+M) makes a new memorized report (`kind: builder`), or a
//!   copy; the name is the report's name and title (Q-A08r).
//! - Save changes updates the memorized report it came from: only a
//!   builder row this user may change, with changes, at the version read.
//! - Refusals: the name is taken (said at the name); someone changed it
//!   (Memorize yours as a copy / Load their version / Cancel); only the
//!   author or an admin may change it (the button turns off, the edits
//!   stay); it was deleted (Memorize as new); too big; a rule broken (a
//!   problem on its row).

use cl_api::ApiError;
use cl_api::OrphanLabel;
use cl_api::resources::reports::definitions::{
    DefinitionRefusal, DefinitionRow, DefinitionWrite, definition_refusal,
};
use cl_model::report_def::editor::{Op, Saved, SavedKind};
use cl_model::report_def::{MAX_BYTES, to_text};
use eframe::egui::{self, Ui};

use super::BuilderScreen;
use super::picker;
use crate::i18n::t;
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::widgets::primary_button;

/// What a save job was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Memorize,
    Update,
}

/// The Memorize dialog.
#[derive(Clone, Debug, Default)]
pub(crate) struct MemoDialog {
    pub(crate) name: String,
    pub(crate) group: Option<String>,
    /// A problem said at the name (an i18n key).
    pub(crate) problem: Option<&'static str>,
}

/// Saves under way and their dialogs.
#[derive(Default)]
pub(crate) struct SaveState {
    pub(crate) memorize: Option<MemoDialog>,
    /// 409: someone changed the report since it was read.
    pub(crate) conflict: bool,
    job: Option<(Kind, Job<DefinitionRow>)>,
    theirs: Option<Job<DefinitionRow>>,
    groups: Option<Vec<(String, String)>>,
    groups_job: Option<Job<Vec<(String, String)>>>,
    /// Close the window once saved (the close prompt's Save).
    close_after: bool,
}

impl SaveState {
    /// True while a save or a read is under way.
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some() || self.theirs.is_some() || self.groups_job.is_some()
    }

    /// True while a dialog is open (the editor's keys wait).
    pub(crate) fn dialog_open(&self) -> bool {
        self.memorize.is_some() || self.conflict
    }

    /// For the control channel.
    pub(crate) fn state_name(&self) -> &'static str {
        match (&self.job, &self.memorize, self.conflict) {
            (Some(_), ..) => "saving",
            (None, Some(_), _) => "memorize",
            (None, None, true) => "conflict",
            _ => "idle",
        }
    }
}

/// The write for the editor's report under `name`.
fn write_of(s: &BuilderScreen, name: &str, group: Option<String>) -> Option<DefinitionWrite> {
    let definition = serde_json::to_value(s.ed.definition()).ok()?;
    Some(DefinitionWrite {
        name: name.trim().to_string(),
        group_id: group,
        kind: "builder".into(),
        definition,
    })
}

/// True when the definition fits the engine's 256 KB.
fn fits(s: &BuilderScreen) -> bool {
    to_text(s.ed.definition()).is_ok_and(|t| t.len() <= MAX_BYTES)
}

/// Opens the Memorize dialog, prefilled with the report's name and group.
pub(crate) fn open_memorize(s: &mut BuilderScreen, closing: bool) {
    if s.save.job.is_some() {
        return;
    }
    s.save.close_after = closing;
    s.save.memorize = Some(MemoDialog {
        name: s.ed.name().to_string(),
        group: s.ed.group_id().map(str::to_string),
        problem: None,
    });
    if s.save.groups.is_none() && s.save.groups_job.is_none() {
        s.save.groups_job = Some(engine::read(
            &s.link,
            s.group.as_ref(),
            |d| {
                Ok(crate::company::demo::report_store::definitions(d)
                    .1
                    .into_iter()
                    .map(|g| (g.id, g.name))
                    .collect())
            },
            |s| async move {
                Ok(s.report_definitions()
                    .groups()
                    .await?
                    .into_iter()
                    .map(|g| (g.id, g.name))
                    .collect())
            },
        ));
    }
}

fn label() -> OrphanLabel {
    OrphanLabel {
        what: "orphan.report_memorize",
        key: None,
    }
}

/// Memorize: a new report under the dialog's name.
fn memorize(s: &mut BuilderScreen) {
    let Some(d) = s.save.memorize.clone() else {
        return;
    };
    if d.name.trim().is_empty() {
        set_problem(s, "builder.save.name_needed");
        return;
    }
    if !fits(s) {
        set_problem(s, "builder.save.too_big");
        return;
    }
    // The name is the title too (Q-A08r); one undo step.
    if d.name.trim() != s.ed.name() {
        s.apply(Op::Name(d.name.clone()));
    }
    if d.group.as_deref() != s.ed.group_id() {
        s.apply(Op::MemorizedGroup(d.group.clone()));
    }
    let Some(w) = write_of(s, &d.name, d.group) else {
        return;
    };
    let live = w.clone();
    // One key per owner action: a retry of this Memorize is the same one.
    let key = cl_api::IdempotencyKey::new();
    s.save.job = Some((
        Kind::Memorize,
        engine::write(
            &s.link,
            s.group.as_ref(),
            label(),
            move |d| crate::company::demo::report_store::save_definition(d, None, &w),
            move |s| async move { s.report_definitions().create(&live, key).await },
        ),
    ));
}

/// Memorize as the dialog's button does (tests).
#[cfg(test)]
pub(crate) fn memorize_for_test(s: &mut BuilderScreen) {
    memorize(s);
}

fn set_problem(s: &mut BuilderScreen, key: &'static str) {
    if let Some(d) = s.save.memorize.as_mut() {
        d.problem = Some(key);
    }
}

/// Save changes: the memorized builder report, at the version read.
pub(crate) fn save_changes(s: &mut BuilderScreen, closing: bool) {
    if s.save.job.is_some() || !s.ed.can_save_changes() {
        return;
    }
    let Some(saved) = s.ed.saved().cloned() else {
        return;
    };
    if !fits(s) {
        s.status = Some(t("builder.save.too_big"));
        return;
    }
    s.save.close_after = closing;
    let Some(w) = write_of(s, s.ed.name(), s.ed.group_id().map(str::to_string)) else {
        return;
    };
    let live = w.clone();
    let (id, version) = (saved.id.clone(), saved.version);
    let id2 = id.clone();
    s.save.job = Some((
        Kind::Update,
        engine::write(
            &s.link,
            s.group.as_ref(),
            label(),
            move |d| {
                crate::company::demo::report_store::save_definition(d, Some((id2, version)), &w)
            },
            move |s| async move { s.report_definitions().update(&id, version, &live).await },
        ),
    ));
}

/// The saved row as the editor's baseline, with the engine's `canEdit`.
fn saved_of(s: &BuilderScreen, row: &DefinitionRow) -> Saved {
    Saved {
        id: row.id.clone(),
        version: row.version,
        kind: SavedKind::Builder,
        can_edit: row.can_edit,
        def: s.ed.definition().clone(),
        name: row.name.clone(),
        group_id: row.group_id.clone(),
        author: row.created_by_name.clone(),
        updated_at: row.updated_at,
    }
}

/// Takes finished saves and reads.
pub(crate) fn poll(s: &mut BuilderScreen) {
    if let Some(r) = engine::poll(&mut s.save.groups_job) {
        s.save.groups = Some(r.unwrap_or_default());
    }
    if let Some(r) = engine::poll(&mut s.save.theirs) {
        match r {
            Ok(row) => load_theirs(s, &row),
            Err(_) => s.status = Some(t("builder.save.failed")),
        }
    }
    let Some((kind, job)) = s.save.job.as_mut() else {
        return;
    };
    let kind = *kind;
    let Some(r) = job.take() else {
        return;
    };
    s.save.job = None;
    match r {
        Ok(row) => {
            let saved = saved_of(s, &row);
            s.ed.mark_saved(saved);
            s.save.memorize = None;
            s.save.conflict = false;
            s.status = Some(t("builder.save.saved"));
            if std::mem::take(&mut s.save.close_after) {
                s.link.push(crate::company::CompanyEvent::CloseWindow(
                    cl_model::CommandId::new(super::KIND),
                ));
            }
        }
        Err(e) => refused(s, kind, &e),
    }
}

fn refused(s: &mut BuilderScreen, kind: Kind, e: &ApiError) {
    s.save.close_after = false;
    match (definition_refusal(e), kind) {
        (Some(DefinitionRefusal::NameTaken), Kind::Memorize) => {
            set_problem(s, "builder.save.name_taken");
        }
        (Some(DefinitionRefusal::NameTaken), Kind::Update) => {
            s.status = Some(t("builder.save.name_taken"));
        }
        (Some(DefinitionRefusal::Stale), Kind::Update) => s.save.conflict = true,
        (Some(DefinitionRefusal::NotAuthor), _) => {
            s.ed.not_author();
            s.status = Some(t("builder.save.not_author"));
        }
        (Some(DefinitionRefusal::TooLarge), _) => {
            set_problem(s, "builder.save.too_big");
            s.status = Some(t("builder.save.too_big"));
        }
        (Some(DefinitionRefusal::Invalid { path, code }), _) => {
            s.save.memorize = None;
            s.save.conflict = false;
            match (path, code) {
                (Some(p), Some(c)) => s.ed.set_engine_problem(&p, &c),
                _ => s.status = Some(t("builder.save.invalid")),
            }
        }
        _ if matches!(e, ApiError::NotFound { .. }) && kind == Kind::Update => {
            // Deleted since it was read: Memorize as new (Q-A08n).
            s.ed.forget_saved();
            s.status = Some(t("builder.save.deleted"));
            open_memorize(s, false);
        }
        _ => {
            set_problem(s, "builder.save.failed");
            s.status = Some(t("builder.save.failed"));
        }
    }
}

/// "Load their version": the row as it is now replaces the edits, one
/// undo step (Q-A08n).
fn load_theirs(s: &mut BuilderScreen, row: &DefinitionRow) {
    let text = serde_json::to_string(&row.definition).unwrap_or_default();
    let Ok(def) = cl_model::report_def::parse(&text) else {
        s.status = Some(t("reports.builder.unreadable"));
        return;
    };
    s.apply(Op::Load {
        def: Box::new(def),
        name: row.name.clone(),
        group_id: row.group_id.clone(),
    });
    let saved = saved_of(s, row);
    s.ed.mark_saved(saved);
    s.save.conflict = false;
}

fn read_theirs(s: &mut BuilderScreen) {
    let Some(id) = s.ed.saved().map(|x| x.id.clone()) else {
        s.save.conflict = false;
        return;
    };
    let id2 = id.clone();
    s.save.theirs = Some(engine::read(
        &s.link,
        s.group.as_ref(),
        move |d| crate::company::demo::report_store::definition(d, &id2),
        move |s| async move { s.report_definitions().get(&id).await },
    ));
}

/// The Memorize and "someone changed it" dialogs.
pub(crate) fn dialogs(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let ctx = ui.ctx().clone();
    if s.save.memorize.is_some() {
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new(t("builder.save.memorize_title"))
            .id(egui::Id::unique(("builder-memorize", s.id.0)))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(&ctx, |ui| {
                ui.set_width(360.0);
                let groups = s.save.groups.clone().unwrap_or_default();
                let Some(d) = s.save.memorize.as_mut() else {
                    return;
                };
                let l = picker::caption(ui, &t("builder.report.name"));
                let r = ui
                    .add(egui::TextEdit::singleline(&mut d.name).desired_width(340.0))
                    .labelled_by(l.id);
                if ui.memory(|m| m.focused().is_none()) {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if let Some(p) = d.problem {
                    form::inline_note(ui, tokens, &t(p));
                }
                let mut choices = vec![picker::on(t("builder.report.no_group"))];
                choices.extend(groups.iter().map(|(_, n)| picker::on(n.clone())));
                let current = d
                    .group
                    .as_ref()
                    .and_then(|g| groups.iter().position(|(id, _)| id == g))
                    .map_or(0, |i| i + 1);
                if let Some(i) = picker::pick(
                    ui,
                    ("memo-group", 0),
                    &t("builder.report.group"),
                    &choices,
                    Some(current),
                    300.0,
                ) {
                    d.group = i
                        .checked_sub(1)
                        .and_then(|i| groups.get(i))
                        .map(|g| g.0.clone());
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if primary_button(ui, &t("builder.save.memorize")).clicked() {
                        submit = true;
                    }
                    if ui.button(t("builder.save.cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if submit && s.save.job.is_none() {
            memorize(s);
        }
        if cancel {
            s.save.memorize = None;
            s.save.close_after = false;
        }
    }
    if s.save.conflict {
        let mut pick = None;
        egui::Window::new(t("builder.save.conflict_title"))
            .id(egui::Id::unique(("builder-conflict", s.id.0)))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(&ctx, |ui| {
                ui.set_width(380.0);
                ui.add(egui::Label::new(t("builder.save.conflict")).wrap());
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if primary_button(ui, &t("builder.save.copy")).clicked() {
                        pick = Some(0);
                    }
                    if ui.button(t("builder.save.load_theirs")).clicked() {
                        pick = Some(1);
                    }
                    if ui.button(t("builder.save.cancel")).clicked() {
                        pick = Some(2);
                    }
                });
            });
        match pick {
            Some(0) => {
                s.save.conflict = false;
                open_memorize(s, false);
                if let Some(d) = s.save.memorize.as_mut() {
                    d.name =
                        crate::i18n::t_with("builder.save.copy_name", &[("name", s.ed.name())]);
                }
            }
            Some(1) => read_theirs(s),
            Some(_) => s.save.conflict = false,
            None => {}
        }
    }
}
