//! Memorized reports (U-118, `report-defs-v1`, Q-P28d): the catalog's
//! "Memorized" section with its groups, and the Memorize dialog (name and
//! group; "Save changes" or "Save as new" when the options changed; on a
//! 409 "Someone changed this report: Reload / Save as a copy"). Hidden on
//! an engine without the capability and in the demo (WP5 adds its store).

use cl_api::resources::reports::definitions::{
    DefinitionGroup, DefinitionRefusal, DefinitionRow, DefinitionWrite, definition_refusal,
};
use cl_api::{Capabilities, OrphanLabel};
use cl_model::reports::{ReportOptions, StandardDefinition};
use eframe::egui::{self, RichText, Ui};

use super::window::ReportWindow;
use crate::company::CompanyLink;
use crate::i18n::t;
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;

/// How the dialog saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveAs {
    /// A new memorized report.
    New,
    /// Changes to the one shown.
    Changes,
}

/// The Memorize dialog.
#[derive(Debug, Default)]
pub(crate) struct Dialog {
    pub(crate) name: String,
    pub(crate) group: Option<String>,
    /// The engine refused with 409 `STALE_VERSION`.
    pub(crate) conflict: bool,
    pub(crate) problem: Option<&'static str>,
    /// Delete was clicked: waiting for the yes (QA-12).
    pub(crate) confirm_delete: bool,
    /// The Name field takes the focus once, when the dialog opens.
    focused: bool,
}

/// The memorized reports and the dialog.
#[derive(Debug, Default)]
pub(crate) struct Memo {
    pub(crate) list: Vec<DefinitionRow>,
    pub(crate) groups: Vec<DefinitionGroup>,
    load: Option<Job<(Vec<DefinitionRow>, Vec<DefinitionGroup>)>>,
    save: Option<Job<DefinitionRow>>,
    /// The options the save in flight sends (S7): marked saved on success,
    /// whatever changed on screen meanwhile.
    sending: Option<ReportOptions>,
    remove: Option<Job<()>>,
    pub(crate) dialog: Option<Dialog>,
    /// The memorized report on screen, as read.
    pub(crate) current: Option<DefinitionRow>,
    /// The options it was saved with.
    pub(crate) saved: Option<ReportOptions>,
    /// The `save` job is a Reload after a 409.
    reloading: bool,
}

#[cfg(test)]
impl Dialog {
    /// A dialog with `name` typed (tests).
    pub(crate) fn named(name: &str) -> Dialog {
        Dialog {
            name: name.into(),
            ..Dialog::default()
        }
    }
}

impl Memo {
    pub(crate) fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub(crate) fn busy(&self) -> bool {
        self.load.is_some() || self.save.is_some() || self.remove.is_some()
    }
}

/// The engine stores memorized reports (the demo keeps its own, WP5).
pub(crate) fn available(link: &CompanyLink) -> bool {
    link.is_demo() || link.capabilities().has(Capabilities::REPORT_DEFS)
}

/// Reads the memorized reports and groups.
pub(super) fn load(w: &mut ReportWindow) {
    if !available(&w.link) {
        return;
    }
    w.memo.load = Some(engine::read(
        &w.link,
        w.group.as_ref(),
        |d| Ok(crate::company::demo::report_store::definitions(d)),
        |s| async move {
            let d = s.report_definitions();
            Ok((d.list().await?, d.groups().await?))
        },
    ));
}

pub(super) fn poll(w: &mut ReportWindow) {
    if let Some(r) = engine::poll(&mut w.memo.load)
        && let Ok((list, groups)) = r
    {
        w.memo.list = list;
        w.memo.groups = groups;
    }
    if let Some(r) = engine::poll(&mut w.memo.save) {
        match r {
            Ok(row) if w.memo.reloading => {
                w.memo.reloading = false;
                open_row(w, &row);
            }
            Ok(row) => {
                w.memo.saved = w.memo.sending.take().or_else(|| Some(w.options.clone()));
                if row.kind == "builder" {
                    super::builder_run::memorized(w, &row);
                }
                w.memo.current = Some(row);
                w.memo.dialog = None;
                load(w);
                // Other open report windows list it too (QA-16).
                w.link
                    .bus()
                    .publish_from(&w.bus, crate::data_bus::Change::Reports);
            }
            Err(e) => {
                w.memo.sending = None;
                if std::mem::take(&mut w.memo.reloading) && w.memo.dialog.is_none() {
                    w.notice = Some(t("reports.memorize.failed"));
                }
                if let Some(d) = w.memo.dialog.as_mut() {
                    match definition_refusal(&e) {
                        Some(DefinitionRefusal::Stale) => d.conflict = true,
                        Some(DefinitionRefusal::NameTaken) => {
                            d.problem = Some("reports.memorize.name_taken");
                        }
                        Some(DefinitionRefusal::NotAuthor) => {
                            d.problem = Some("reports.memorize.not_author");
                        }
                        Some(DefinitionRefusal::TooLarge | DefinitionRefusal::Invalid { .. }) => {
                            d.problem = Some("reports.memorize.invalid");
                        }
                        None => d.problem = Some("reports.memorize.failed"),
                    }
                }
            }
        }
    }
}

/// Polls a delete.
fn poll_remove(w: &mut ReportWindow) {
    if let Some(r) = engine::poll(&mut w.memo.remove) {
        match r {
            Ok(()) => {
                w.memo.current = None;
                w.memo.saved = None;
                w.memo.dialog = None;
                w.notice = Some(t("reports.memorize.deleted"));
                load(w);
                w.link
                    .bus()
                    .publish_from(&w.bus, crate::data_bus::Change::Reports);
            }
            Err(e) => {
                if let Some(d) = w.memo.dialog.as_mut() {
                    d.confirm_delete = false;
                    d.problem = Some(match definition_refusal(&e) {
                        Some(DefinitionRefusal::Stale) => "reports.memorize.conflict",
                        Some(DefinitionRefusal::NotAuthor) => "reports.memorize.not_author",
                        _ => "reports.memorize.failed",
                    });
                }
            }
        }
    }
}

/// Deletes the memorized report shown.
fn remove(w: &mut ReportWindow) {
    let Some((id, version)) = w.memo.current.as_ref().map(|c| (c.id.clone(), c.version)) else {
        return;
    };
    w.memo.remove = Some(engine::write(
        &w.link,
        w.group.as_ref(),
        OrphanLabel {
            what: "orphan.report_memorize",
            key: None,
        },
        {
            let id = id.clone();
            move |d| crate::company::demo::report_store::delete_definition(d, &id, version)
        },
        move |s| async move { s.report_definitions().delete(&id, version).await },
    ));
}

/// Opens the dialog on the report shown (Ctrl+M). A builder report (a
/// starter, a memorized builder report, an editor's Run report) is
/// memorized as a builder report (A08 lifts Q-A07t(1)); a detail tab or an
/// unreadable definition has nothing to memorize.
pub(super) fn open_dialog(w: &mut ReportWindow) {
    if !available(&w.link)
        || (w.builder.is_some() && super::builder_run::memorize_body(w).is_none())
    {
        return;
    }
    let (name, group) = match &w.memo.current {
        Some(c) => (c.name.clone(), c.group_id.clone()),
        None => (w.title_text(), None),
    };
    w.memo.dialog = Some(Dialog {
        name,
        group,
        conflict: false,
        problem: None,
        confirm_delete: false,
        focused: false,
    });
}

/// The kind of report the window shows: `builder` or `standard`.
fn shown_kind(w: &ReportWindow) -> &'static str {
    if w.builder.is_some() {
        "builder"
    } else {
        "standard"
    }
}

/// The memorized report shown changed since it was read: its options, and
/// so the definition Memorize would save (a builder report's date preset,
/// basis or period columns, `builder_run::memorize_body`). Only for a row
/// of the kind shown.
pub(crate) fn changed(w: &ReportWindow) -> bool {
    w.memo
        .current
        .as_ref()
        .is_some_and(|c| c.kind == shown_kind(w))
        && w.memo.saved.as_ref() != Some(&w.options)
}

pub(super) fn save(w: &mut ReportWindow, how: SaveAs) {
    let kind = shown_kind(w);
    // The definition to save. A builder report saves its own definition; a
    // row is never overwritten by another kind (S-L1; the engine refuses a
    // kind change too).
    let body = if kind == "builder" {
        super::builder_run::memorize_body(w)
    } else {
        serde_json::to_value(StandardDefinition::new(w.spec.key, w.options.clone())).ok()
    };
    let same_kind = w
        .memo
        .current
        .as_ref()
        .is_none_or(|c| c.kind == kind && c.definition["kind"].as_str() == Some(kind));
    let Some(d) = w.memo.dialog.as_mut() else {
        return;
    };
    let Some(mut body) = body else {
        d.problem = Some("reports.memorize.invalid");
        return;
    };
    if how == SaveAs::Changes && !same_kind {
        d.problem = Some("reports.memorize.invalid");
        return;
    }
    let name = d.name.trim().to_string();
    if name.is_empty() {
        d.problem = Some("reports.memorize.name_needed");
        return;
    }
    d.problem = None;
    // A builder report's name is its title too (Q-A08r).
    if kind == "builder"
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("title".into(), serde_json::Value::from(name.clone()));
    }
    w.memo.sending = Some(w.options.clone());
    let write = DefinitionWrite {
        name,
        group_id: d.group.clone(),
        kind: kind.into(),
        definition: body,
    };
    let target = match how {
        SaveAs::Changes => w.memo.current.as_ref().map(|c| (c.id.clone(), c.version)),
        SaveAs::New => None,
    };
    // One key per Memorize press: the engine stores one row however often
    // the transport retries it.
    let key = cl_api::IdempotencyKey::new();
    w.memo.save = Some(engine::write(
        &w.link,
        w.group.as_ref(),
        OrphanLabel {
            what: "orphan.report_memorize",
            key: None,
        },
        {
            let (target, write) = (target.clone(), write.clone());
            move |d| crate::company::demo::report_store::save_definition(d, target, &write)
        },
        move |s| async move {
            let d = s.report_definitions();
            match target {
                Some((id, version)) => d.update(&id, version, &write).await,
                None => d.create(&write, key).await,
            }
        },
    ));
}

/// Opens a memorized report: its catalog report with its options.
pub(super) fn open_row(w: &mut ReportWindow, row: &DefinitionRow) {
    let read = cl_model::reports::definition::read_memorized(&row.definition);
    // A Report Builder definition runs as a builder report (A07). It is
    // sent inline at the version read (Q-A07t). One that can't be read
    // (saved by a newer version, #8) opens the builder window with the
    // message, never a blank one.
    let builder = matches!(
        read,
        Ok(cl_model::reports::definition::Memorized::Builder(_))
    ) || row.definition["kind"] == "builder";
    if builder {
        let s = cl_api::resources::report_builder::Starter {
            key: row.id.clone(),
            title: row.name.clone(),
            ties_to: None,
            definition: row.definition.clone(),
        };
        super::builder_run::show_starter(w, &s);
        w.memo.saved = Some(w.options.clone());
        w.memo.current = Some(row.clone());
        return;
    }
    let def = match read {
        Ok(cl_model::reports::definition::Memorized::Standard(def)) => *def,
        Err(e) => {
            w.notice = Some(t(if e.is_newer() {
                "reports.memorize.newer"
            } else {
                "reports.memorize.unreadable"
            }));
            return;
        }
        Ok(cl_model::reports::definition::Memorized::Builder(_)) => return,
    };
    let Some(spec) = cl_model::reports::find(&def.report_key) else {
        w.notice = Some(t("reports.memorize.unreadable"));
        return;
    };
    // A Balance Sheet or Trial Balance saved with a tag filter runs
    // without it now, and says so (S2, Q-PR6b).
    let mut options = def.options;
    let dropped = !spec.filters.contains(&cl_model::reports::FilterKind::Tags)
        && (!options.filters.tag_ids.is_empty() || options.filters.untagged);
    if dropped {
        options.filters.tag_ids.clear();
        options.filters.untagged = false;
    }
    w.show_report(spec, options.clone());
    if dropped {
        w.notice = Some(t("reports.memorize.tag_dropped"));
    }
    w.memo.saved = Some(options);
    w.memo.current = Some(row.clone());
}

/// The catalog's "Memorized" section, narrowed by the catalog's search
/// (every word in the name). Returns how many it lists.
pub(super) fn catalog(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) -> usize {
    if !available(&w.link) {
        return 0;
    }
    let words: Vec<String> = w.search.split_whitespace().map(str::to_lowercase).collect();
    let matches = |r: &DefinitionRow| {
        let name = r.name.to_lowercase();
        words.iter().all(|x| name.contains(x.as_str()))
    };
    let shown = w.memo.list.iter().filter(|r| matches(r)).count();
    if !words.is_empty() && shown == 0 {
        return 0;
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(t("reports.group.memorized").to_uppercase())
                .size(11.0)
                .color(tokens.text_secondary),
        );
        // A09 (Q-A09k): the Memorized Reports list.
        if ui
            .link(RichText::new(t("reports.memorized.manage")).size(11.0))
            .on_hover_text(t("reports.memorized.manage.tip"))
            .clicked()
        {
            w.link.push(crate::company::CompanyEvent::OpenWindow(
                cl_model::CommandId::new(super::memorized_list::KIND),
            ));
        }
    });
    if w.memo.list.is_empty() {
        ui.label(
            RichText::new(t("reports.memorize.none"))
                .size(12.0)
                .color(tokens.text_secondary),
        );
        return 0;
    }
    let mut pick = None;
    let mut groups: Vec<Option<&DefinitionGroup>> = w.memo.groups.iter().map(Some).collect();
    groups.push(None);
    for g in groups {
        let rows: Vec<&DefinitionRow> = w
            .memo
            .list
            .iter()
            .filter(|r| r.group_id.as_deref() == g.map(|g| g.id.as_str()) && matches(r))
            .collect();
        if rows.is_empty() {
            continue;
        }
        if let Some(g) = g {
            ui.label(RichText::new(&g.name).size(12.0).strong());
        }
        for r in rows {
            let on = w.memo.current.as_ref().is_some_and(|c| c.id == r.id);
            if ui.selectable_label(on, &r.name).clicked() {
                pick = Some(r.clone());
            }
        }
    }
    if let Some(r) = pick {
        open_row(w, &r);
    }
    shown
}

/// The dialog, above the report.
pub(super) fn ui(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    enum Click {
        Save(SaveAs),
        Reload,
        Close,
        Delete,
    }
    poll_remove(w);
    // Save changes only over a row of the kind shown (a standard row
    // customized into a builder report is memorized new).
    let changes = w
        .memo
        .current
        .as_ref()
        .is_some_and(|c| c.kind == shown_kind(w));
    let saving = w.memo.save.is_some() || w.memo.remove.is_some();
    let groups = w.memo.groups.clone();
    let Some(d) = w.memo.dialog.as_mut() else {
        return;
    };
    let mut click = None;
    form::card(ui, tokens, |ui| {
        form::heading(ui, tokens, &t("reports.memorize.title"));
        let name = form::field(ui, &t("reports.memorize.name"), &mut d.name);
        // QA-12: the cursor starts in Name; Enter there saves.
        if !d.focused {
            name.request_focus();
            d.focused = true;
        }
        if name.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !saving {
            click = Some(Click::Save(if changes && !d.conflict {
                SaveAs::Changes
            } else {
                SaveAs::New
            }));
        }
        let shown = d
            .group
            .as_ref()
            .and_then(|id| groups.iter().find(|g| &g.id == id))
            .map_or_else(|| t("reports.memorize.no_group"), |g| g.name.clone());
        let l = ui.label(t("reports.memorize.group"));
        egui::ComboBox::from_id_salt("memorize_group")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.group, None, t("reports.memorize.no_group"));
                for g in &groups {
                    ui.selectable_value(&mut d.group, Some(g.id.clone()), &g.name);
                }
            })
            .response
            .labelled_by(l.id);
        if let Some(p) = d.problem {
            form::inline_warning(ui, tokens, &t(p));
        }
        if d.conflict {
            form::inline_warning(ui, tokens, &t("reports.memorize.conflict"));
        }
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!saving, |ui| {
                if d.conflict {
                    if ui.button(t("reports.memorize.reload")).clicked() {
                        click = Some(Click::Reload);
                    }
                    if ui.button(t("reports.memorize.save_copy")).clicked() {
                        click = Some(Click::Save(SaveAs::New));
                    }
                } else if changes {
                    if ui.button(t("reports.memorize.save_changes")).clicked() {
                        click = Some(Click::Save(SaveAs::Changes));
                    }
                    if ui.button(t("reports.memorize.save_new")).clicked() {
                        click = Some(Click::Save(SaveAs::New));
                    }
                } else if ui.button(t("reports.memorize.save")).clicked() {
                    click = Some(Click::Save(SaveAs::New));
                }
            });
            if ui.button(t("reports.memorize.cancel")).clicked() {
                click = Some(Click::Close);
            }
            if changes && !d.confirm_delete {
                ui.separator();
                if ui
                    .add_enabled(!saving, egui::Button::new(t("reports.memorize.delete")))
                    .clicked()
                {
                    d.confirm_delete = true;
                }
            }
        });
        if d.confirm_delete {
            form::inline_warning(ui, tokens, &t("reports.memorize.delete_question"));
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !saving,
                        egui::Button::new(t("reports.memorize.delete_confirm")),
                    )
                    .clicked()
                {
                    click = Some(Click::Delete);
                }
                if ui.button(t("reports.memorize.delete_cancel")).clicked() {
                    d.confirm_delete = false;
                }
            });
        }
    });
    match click {
        Some(Click::Save(how)) => save(w, how),
        Some(Click::Reload) => {
            w.memo.dialog = None;
            if let Some(cur) = w.memo.current.clone() {
                let id = cur.id.clone();
                w.memo.reloading = true;
                w.memo.save = Some(engine::read(
                    &w.link,
                    w.group.as_ref(),
                    {
                        let id = id.clone();
                        move |d| crate::company::demo::report_store::definition(d, &id)
                    },
                    move |s| async move { s.report_definitions().get(&id).await },
                ));
            }
        }
        Some(Click::Close) => w.memo.dialog = None,
        Some(Click::Delete) => remove(w),
        None => {}
    }
}

#[cfg(test)]
mod tag_tests {
    use std::sync::Arc;

    use cl_model::CommandId;
    use cl_model::reports::{ReportOptions, StandardDefinition, find};

    /// S2 (Q-PR6b): a Balance Sheet memorized with a tag runs for all tags
    /// and says so.
    #[test]
    fn a_saved_tag_on_a_balance_report_is_dropped_with_a_notice() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(crate::company::CompanyLink::demo(dir.path()));
        let today = cl_core::CalendarDate::from_ymd(2026, 11, 4).expect("d");
        let spec = find("balance_sheet").expect("bs");
        let mut w = super::ReportWindow::new(
            Arc::clone(&link),
            CommandId::new(super::super::KIND),
            None,
            spec,
            today,
        );
        let mut o = ReportOptions::for_spec(spec);
        o.filters.tag_ids = vec!["33333333-3333-4333-8333-333333333333".parse().expect("t")];
        let row = cl_api::resources::reports::definitions::DefinitionRow {
            id: "r1".into(),
            group_id: None,
            name: "Sheet by tag".into(),
            kind: "standard".into(),
            definition: serde_json::to_value(StandardDefinition::new("balance_sheet", o))
                .expect("json"),
            version: 1,
            created_by: None,
            created_by_name: None,
            updated_at: None,
            can_edit: true,
        };
        super::open_row(&mut w, &row);
        assert_eq!(
            w.notice.as_deref(),
            Some(crate::i18n::t("reports.memorize.tag_dropped").as_str())
        );
        assert_eq!(w.options.filters.tag_ids, Vec::<cl_core::ids::TagId>::new());
    }
}
