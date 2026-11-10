//! Memorized reports (U-118, `report-defs-v1`, Q-P28d): the catalog's
//! "Memorized" section with its groups, and the Memorize dialog (name and
//! group; "Save changes" or "Save as new" when the options changed; on a
//! 409 "Someone changed this report: Reload / Save as a copy"). Hidden on
//! an engine without the capability and in the demo (WP5 adds its store).

use cl_api::resources::reports::definitions::{
    DefinitionGroup, DefinitionRefusal, DefinitionRow, DefinitionWrite, definition_refusal,
};
use cl_api::{ApiError, Capabilities, LocalValidation, OrphanLabel};
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
}

/// The memorized reports and the dialog.
#[derive(Debug, Default)]
pub(crate) struct Memo {
    pub(crate) list: Vec<DefinitionRow>,
    pub(crate) groups: Vec<DefinitionGroup>,
    load: Option<Job<(Vec<DefinitionRow>, Vec<DefinitionGroup>)>>,
    save: Option<Job<DefinitionRow>>,
    pub(crate) dialog: Option<Dialog>,
    /// The memorized report on screen, as read.
    pub(crate) current: Option<DefinitionRow>,
    /// The options it was saved with.
    pub(crate) saved: Option<ReportOptions>,
    /// The `save` job is a Reload after a 409.
    reloading: bool,
}

impl Memo {
    pub(crate) fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub(crate) fn busy(&self) -> bool {
        self.load.is_some() || self.save.is_some()
    }
}

/// The engine stores memorized reports (never the demo, WP1).
pub(crate) fn available(link: &CompanyLink) -> bool {
    !link.is_demo() && link.capabilities().has(Capabilities::REPORT_DEFS)
}

/// Reads the memorized reports and groups.
pub(super) fn load(w: &mut ReportWindow) {
    if !available(&w.link) {
        return;
    }
    w.memo.load = Some(engine::read(
        &w.link,
        w.group.as_ref(),
        |_| Err(ApiError::Local(LocalValidation::Unsupported)),
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
                w.memo.saved = Some(w.options.clone());
                w.memo.current = Some(row);
                w.memo.dialog = None;
                load(w);
            }
            Err(e) => {
                w.memo.reloading = false;
                if let Some(d) = w.memo.dialog.as_mut() {
                    match definition_refusal(&e) {
                        Some(DefinitionRefusal::Stale) => d.conflict = true,
                        Some(DefinitionRefusal::NameTaken) => {
                            d.problem = Some("reports.memorize.name_taken");
                        }
                        Some(DefinitionRefusal::NotAuthor) => {
                            d.problem = Some("reports.memorize.not_author");
                        }
                        Some(DefinitionRefusal::TooLarge | DefinitionRefusal::Invalid) => {
                            d.problem = Some("reports.memorize.invalid");
                        }
                        None => d.problem = Some("reports.memorize.failed"),
                    }
                }
            }
        }
    }
}

/// Opens the dialog on the report shown (Ctrl+M).
pub(super) fn open_dialog(w: &mut ReportWindow) {
    if !available(&w.link) {
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
    });
}

/// The options changed since the memorized report was read.
pub(crate) fn changed(w: &ReportWindow) -> bool {
    w.memo.current.is_some() && w.memo.saved.as_ref() != Some(&w.options)
}

fn save(w: &mut ReportWindow, how: SaveAs) {
    let Some(d) = w.memo.dialog.as_mut() else {
        return;
    };
    let name = d.name.trim().to_string();
    if name.is_empty() {
        d.problem = Some("reports.memorize.name_needed");
        return;
    }
    d.problem = None;
    let def = StandardDefinition::new(w.spec.key, w.options.clone());
    let Ok(body) = serde_json::to_value(&def) else {
        d.problem = Some("reports.memorize.invalid");
        return;
    };
    let write = DefinitionWrite {
        name,
        group_id: d.group.clone(),
        kind: "standard".into(),
        definition: body,
    };
    let target = match how {
        SaveAs::Changes => w.memo.current.as_ref().map(|c| (c.id.clone(), c.version)),
        SaveAs::New => None,
    };
    w.memo.save = Some(engine::write(
        &w.link,
        w.group.as_ref(),
        OrphanLabel {
            what: "orphan.report_memorize",
            key: None,
        },
        |_| Err(ApiError::Local(LocalValidation::Unsupported)),
        move |s| async move {
            let d = s.report_definitions();
            match target {
                Some((id, version)) => d.update(&id, version, &write).await,
                None => d.create(&write).await,
            }
        },
    ));
}

/// Opens a memorized report: its catalog report with its options.
fn open_row(w: &mut ReportWindow, row: &DefinitionRow) {
    let def: Result<StandardDefinition, _> = serde_json::from_value(row.definition.clone());
    let Some((spec, def)) = def
        .ok()
        .filter(|d| d.check().is_ok())
        .and_then(|d| cl_model::reports::find(&d.report_key).map(|s| (s, d)))
    else {
        w.notice = Some(t("reports.memorize.unreadable"));
        return;
    };
    w.show_report(spec, def.options.clone());
    w.memo.saved = Some(def.options);
    w.memo.current = Some(row.clone());
}

/// The catalog's "Memorized" section.
pub(super) fn catalog(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    if !available(&w.link) {
        return;
    }
    ui.add_space(6.0);
    ui.label(
        RichText::new(t("reports.group.memorized").to_uppercase())
            .size(11.0)
            .color(tokens.text_secondary),
    );
    if w.memo.list.is_empty() {
        ui.label(
            RichText::new(t("reports.memorize.none"))
                .size(12.0)
                .color(tokens.text_secondary),
        );
        return;
    }
    let mut pick = None;
    let mut groups: Vec<Option<&DefinitionGroup>> = w.memo.groups.iter().map(Some).collect();
    groups.push(None);
    for g in groups {
        let rows: Vec<&DefinitionRow> = w
            .memo
            .list
            .iter()
            .filter(|r| r.group_id.as_deref() == g.map(|g| g.id.as_str()))
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
}

/// The dialog, above the report.
pub(super) fn ui(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    enum Click {
        Save(SaveAs),
        Reload,
        Close,
    }
    let changes = w.memo.current.is_some();
    let saving = w.memo.save.is_some();
    let groups = w.memo.groups.clone();
    let Some(d) = w.memo.dialog.as_mut() else {
        return;
    };
    let mut click = None;
    form::card(ui, tokens, |ui| {
        form::heading(ui, tokens, &t("reports.memorize.title"));
        form::field(ui, &t("reports.memorize.name"), &mut d.name);
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
        });
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
                    |_| Err(ApiError::Local(LocalValidation::Unsupported)),
                    move |s| async move { s.report_definitions().get(&id).await },
                ));
            }
        }
        Some(Click::Close) => w.memo.dialog = None,
        None => {}
    }
}
