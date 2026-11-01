//! Invoice templates in Company Information (U-130, Q-P20ac/ae/af): the
//! company's print layouts, the default first, archived ones apart with
//! Unarchive (never Delete); New, Edit (name, columns, ship-to / P.O. /
//! terms, footer), Make default and Archive. Every write goes to the
//! engine (`invoice-layouts-v1`) and the list is read again after it.

use std::sync::Arc;

use cl_api::{ApiError, InvoiceLayout, InvoiceLayoutId, LayoutDraft, OrphanLabel, RequestGroup};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;

/// What the owner is editing.
#[derive(Clone)]
pub(crate) struct Editing {
    /// The layout being changed (`None`: a new one).
    pub(crate) of: Option<InvoiceLayout>,
    pub(crate) draft: LayoutDraft,
}

/// The layouts card.
#[derive(Default)]
pub(crate) struct LayoutsPanel {
    on: bool,
    pub(crate) layouts: Vec<InvoiceLayout>,
    read: Option<Job<Vec<InvoiceLayout>>>,
    write: Option<Job<InvoiceLayout>>,
    pub(crate) editing: Option<Editing>,
    pub(crate) problem: Option<&'static str>,
}

fn label() -> OrphanLabel {
    OrphanLabel {
        what: "orphan.invoice_layout",
        key: None,
    }
}

/// The plain message for a layout refusal.
fn refusal(e: &ApiError) -> &'static str {
    let code = match e {
        ApiError::Conflict {
            reason: cl_api::ConflictReason::Stale,
            ..
        } => return "layouts.stale",
        ApiError::Conflict { code: Some(c), .. } | ApiError::NotFound { code: Some(c) } => {
            c.as_str().to_string()
        }
        ApiError::Validation(v) => v.code().map(|c| c.as_str().to_string()).unwrap_or_default(),
        ApiError::Local(_) => return "layouts.name_needed",
        _ => String::new(),
    };
    match code.as_str() {
        "INVOICE_LAYOUT_NAME_IN_USE" => "layouts.name_in_use",
        "INVOICE_LAYOUT_IS_DEFAULT" => "layouts.default_not_archived",
        "INVOICE_LAYOUT_ARCHIVED" => "layouts.archived_not_default",
        "STALE" => "layouts.stale",
        _ => "layouts.failed",
    }
}

/// A click on the card, carried out after drawing.
enum Act {
    New,
    Edit(InvoiceLayoutId),
    Default(InvoiceLayoutId),
    Archive(InvoiceLayoutId, bool),
    Save,
    Cancel,
}

impl LayoutsPanel {
    /// The panel of the open company (off without `invoice-layouts-v1`).
    pub(crate) fn open(link: &Arc<CompanyLink>, group: Option<&RequestGroup>) -> LayoutsPanel {
        let mut p = LayoutsPanel {
            on: link.is_demo()
                || link
                    .capabilities()
                    .has(cl_api::Capabilities::INVOICE_LAYOUTS),
            ..LayoutsPanel::default()
        };
        p.refresh(link, group);
        p
    }

    fn refresh(&mut self, link: &Arc<CompanyLink>, group: Option<&RequestGroup>) {
        if !self.on {
            return;
        }
        self.read = Some(engine::read(
            link,
            group,
            |d| Ok(crate::company::demo::layouts::list(d, true)),
            |s| async move { s.invoice_layouts().list(true).await },
        ));
        self.poll(link, group);
    }

    pub(crate) fn busy(&self) -> bool {
        self.read.is_some() || self.write.is_some()
    }

    pub(crate) fn poll(&mut self, link: &Arc<CompanyLink>, group: Option<&RequestGroup>) {
        if let Some(r) = engine::poll(&mut self.read) {
            match r {
                Ok(l) => self.layouts = l,
                Err(_) => self.problem = Some("layouts.load_failed"),
            }
        }
        if let Some(r) = engine::poll(&mut self.write) {
            match r {
                Ok(_) => {
                    self.problem = None;
                    self.editing = None;
                    self.refresh(link, group);
                }
                Err(e) => self.problem = Some(refusal(&e)),
            }
        }
    }

    /// New: a draft copying the default layout's options.
    pub(crate) fn start_new(&mut self) {
        let base = self.layouts.iter().find(|l| l.is_default);
        let mut draft = base.map_or_else(
            || LayoutDraft {
                name: String::new(),
                show_ship_to: true,
                show_po_number: true,
                show_terms: true,
                footer_text: String::new(),
                columns: cl_api::LayoutColumns::default(),
            },
            LayoutDraft::of,
        );
        draft.name.clear();
        self.editing = Some(Editing { of: None, draft });
        self.problem = None;
    }

    pub(crate) fn start_edit(&mut self, id: InvoiceLayoutId) {
        if let Some(l) = self.layouts.iter().find(|l| l.id == id) {
            self.editing = Some(Editing {
                of: Some(l.clone()),
                draft: LayoutDraft::of(l),
            });
            self.problem = None;
        }
    }

    /// Saves the open draft (create or edit).
    pub(crate) fn save(&mut self, link: &Arc<CompanyLink>, group: Option<&RequestGroup>) {
        let Some(e) = self.editing.clone() else {
            return;
        };
        if e.draft.name.trim().is_empty() {
            self.problem = Some("layouts.name_needed");
            return;
        }
        let (of, draft) = (e.of, e.draft);
        let (of2, draft2) = (of.clone(), draft.clone());
        self.write = Some(engine::write(
            link,
            group,
            label(),
            move |d| match &of2 {
                None => crate::company::demo::layouts::create(d, &draft2),
                Some(l) => crate::company::demo::layouts::update(d, l, &draft2),
            },
            move |s| async move {
                match of {
                    None => s.invoice_layouts().create(&draft).await,
                    Some(l) => s.invoice_layouts().update(&l, &draft).await,
                }
            },
        ));
        self.poll(link, group);
    }

    /// Archive (`true`) or Unarchive.
    pub(crate) fn set_archived(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        id: InvoiceLayoutId,
        archived: bool,
    ) {
        let Some(l) = self.layouts.iter().find(|l| l.id == id).cloned() else {
            return;
        };
        let l2 = l.clone();
        self.write = Some(engine::write(
            link,
            group,
            label(),
            move |d| crate::company::demo::layouts::set_archived(d, &l2, archived),
            move |s| async move { s.invoice_layouts().set_archived(&l, archived).await },
        ));
        self.poll(link, group);
    }

    /// Make default.
    pub(crate) fn set_default(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        id: InvoiceLayoutId,
    ) {
        self.write = Some(engine::write(
            link,
            group,
            label(),
            move |d| crate::company::demo::layouts::set_default(d, id),
            move |s| async move { s.invoice_layouts().set_default(id).await },
        ));
        self.poll(link, group);
    }

    pub(crate) fn state_json(&self) -> Value {
        json!({
            "on": self.on,
            "active": self.layouts.iter().filter(|l| !l.is_archived).count(),
            "archived": self.layouts.iter().filter(|l| l.is_archived).count(),
            "default": self.layouts.iter().find(|l| l.is_default).map(|l| l.name.clone()),
            "editing": self.editing.is_some(),
            "problem": self.problem,
        })
    }

    /// The card. Clicks are carried out after drawing.
    #[allow(clippy::too_many_lines, reason = "one card, drawn top to bottom")]
    pub(crate) fn ui(
        &mut self,
        ui: &mut Ui,
        tokens: &Tokens,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
    ) {
        if !self.on {
            return;
        }
        let mut act = None;
        let busy = self.busy();
        // Folded until opened, so the form above keeps its place.
        egui::CollapsingHeader::new(RichText::new(t("layouts.title")).strong())
            .id_salt("company/layouts")
            .default_open(false)
            .show(ui, |ui| {
                form::hint(ui, tokens, &t("layouts.intro"));
                form::card(ui, tokens, |ui| {
                    for archived in [false, true] {
                        let rows: Vec<&InvoiceLayout> = self
                            .layouts
                            .iter()
                            .filter(|l| l.is_archived == archived)
                            .collect();
                        if archived && !rows.is_empty() {
                            ui.add_space(6.0);
                            ui.label(RichText::new(t("layouts.archived")).strong());
                        }
                        for l in rows {
                            ui.horizontal(|ui| {
                                ui.label(&l.name);
                                if l.is_default {
                                    ui.label(
                                        RichText::new(t("layouts.default_badge"))
                                            .color(tokens.text_secondary),
                                    );
                                }
                                ui.add_enabled_ui(!busy, |ui| {
                                    if !archived && ui.small_button(t("layouts.edit")).clicked() {
                                        act = Some(Act::Edit(l.id));
                                    }
                                    if !archived
                                        && !l.is_default
                                        && ui.small_button(t("layouts.make_default")).clicked()
                                    {
                                        act = Some(Act::Default(l.id));
                                    }
                                    if !l.is_default {
                                        let k = if archived {
                                            "layouts.unarchive"
                                        } else {
                                            "layouts.archive"
                                        };
                                        if ui.small_button(t(k)).clicked() {
                                            act = Some(Act::Archive(l.id, !archived));
                                        }
                                    }
                                });
                            });
                        }
                    }
                    ui.add_space(6.0);
                    if ui
                        .add_enabled(
                            !busy && self.editing.is_none(),
                            egui::Button::new(t("layouts.new")),
                        )
                        .clicked()
                    {
                        act = Some(Act::New);
                    }
                    if let Some(e) = &mut self.editing {
                        ui.separator();
                        ui.horizontal(|ui| {
                            let l = ui.label(t("layouts.name"));
                            ui.add(
                                egui::TextEdit::singleline(&mut e.draft.name)
                                    .desired_width(220.0)
                                    .char_limit(cl_api::LAYOUT_NAME_MAX),
                            )
                            .labelled_by(l.id);
                        });
                        ui.label(RichText::new(t("layouts.columns")).strong());
                        ui.horizontal_wrapped(|ui| {
                            let c = &mut e.draft.columns;
                            ui.checkbox(&mut c.item, t("layouts.col.item"));
                            ui.checkbox(&mut c.description, t("layouts.col.description"));
                            ui.checkbox(&mut c.service_date, t("layouts.col.service_date"));
                            ui.checkbox(&mut c.quantity, t("layouts.col.quantity"));
                            ui.checkbox(&mut c.rate, t("layouts.col.rate"));
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut e.draft.show_ship_to, t("layouts.show_ship_to"));
                            ui.checkbox(&mut e.draft.show_po_number, t("layouts.show_po"));
                            ui.checkbox(&mut e.draft.show_terms, t("layouts.show_terms"));
                        });
                        let l = ui.label(t("layouts.footer"));
                        ui.add(
                            egui::TextEdit::multiline(&mut e.draft.footer_text)
                                .desired_rows(2)
                                .desired_width(360.0),
                        )
                        .labelled_by(l.id);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!busy, egui::Button::new(t("layouts.save")))
                                .clicked()
                            {
                                act = Some(Act::Save);
                            }
                            if ui.button(t("layouts.cancel")).clicked() {
                                act = Some(Act::Cancel);
                            }
                        });
                    }
                    if let Some(p) = self.problem {
                        ui.label(RichText::new(t(p)).color(tokens.warn_text));
                    }
                });
            });
        match act {
            Some(Act::New) => self.start_new(),
            Some(Act::Edit(id)) => self.start_edit(id),
            Some(Act::Default(id)) => self.set_default(link, group, id),
            Some(Act::Archive(id, on)) => self.set_archived(link, group, id, on),
            Some(Act::Save) => self.save(link, group),
            Some(Act::Cancel) => {
                self.editing = None;
                self.problem = None;
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_are_kept_archived_and_made_default() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let mut p = LayoutsPanel::open(&link, None);
        assert_eq!(p.layouts.len(), 2);
        p.start_new();
        p.save(&link, None);
        assert_eq!(p.problem, Some("layouts.name_needed"));
        if let Some(e) = p.editing.as_mut() {
            e.draft.name = "Detailed".into();
            e.draft.columns.item = true;
        }
        p.save(&link, None);
        assert_eq!(p.layouts.len(), 3);
        let id = p
            .layouts
            .iter()
            .find(|l| l.name == "Detailed")
            .map(|l| l.id)
            .expect("new");
        p.set_default(&link, None, id);
        assert_eq!(p.state_json()["default"], "Detailed");
        p.set_archived(&link, None, id, true);
        assert_eq!(p.problem, Some("layouts.default_not_archived"));
        let simple = p
            .layouts
            .iter()
            .find(|l| l.name == "Simple")
            .map(|l| l.id)
            .expect("s");
        p.set_archived(&link, None, simple, true);
        assert_eq!(p.state_json()["archived"], 1);
        p.set_archived(&link, None, simple, false);
        assert_eq!(p.state_json()["archived"], 0);
    }
}
