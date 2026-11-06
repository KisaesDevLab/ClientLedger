//! Unit Sets (P24 plan §3 `units.rs`, `lists.unit_sets`; engine U-161,
//! `units-of-measure-v1`; Q-P24i, Q-P24w): the company's unit sets on the
//! left, the chosen one on the right with its base unit (stock is counted
//! in it) and its other units, each "1 {unit} = {factor} {base}".
//!
//! A unit already on saved transactions keeps its size: its factor is read
//! only and it can't be taken out ("add a new unit instead"). The engine
//! refuses the same (409 `UNIT_IN_USE`) and a name used twice (409
//! `DUPLICATE_NAME`); both show in plain words.

use std::sync::Arc;

use cl_api::{ApiError, Capabilities, NewUnitSet, RequestGroup, Unit, UnitRefusal, UnitSet};
use cl_core::ids::UnitSetId;
use cl_model::units::{UnitProblem, UnitRow, UnitSetDraft};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job, label};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "lists.unit_sets";

/// The Unit Sets window.
pub struct UnitsScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    available: bool,
    sets: Option<Vec<UnitSet>>,
    list_job: Option<Job<Vec<UnitSet>>>,
    save_job: Option<Job<UnitSet>>,
    /// The set being edited (`None`: a new one).
    editing: Option<UnitSetId>,
    draft: UnitSetDraft,
    saved_draft: UnitSetDraft,
    problems: Vec<UnitProblem>,
    note: Option<String>,
    error: Option<ApiError>,
    saved_count: u32,
}

fn draft_of(s: &UnitSet) -> UnitSetDraft {
    UnitSetDraft {
        name: s.name.clone(),
        base_name: s.base_unit_name.clone(),
        base_abbreviation: s.base_unit_abbreviation.clone().unwrap_or_default(),
        units: s
            .units
            .iter()
            .map(|u| UnitRow {
                id: u.id,
                name: u.name.clone(),
                abbreviation: u.abbreviation.clone().unwrap_or_default(),
                factor: u.factor.format(),
                // The engine says the set is in use; every saved unit of a
                // set in use keeps its factor (the engine checks each one).
                locked: s.in_use.then_some(u.factor),
            })
            .collect(),
    }
}

fn blank() -> UnitSetDraft {
    let mut d = UnitSetDraft::default();
    d.add_row();
    d
}

impl UnitsScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> UnitsScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::UNITS_OF_MEASURE);
        let mut s = UnitsScreen {
            link,
            group,
            available,
            sets: None,
            list_job: None,
            save_job: None,
            editing: None,
            draft: blank(),
            saved_draft: blank(),
            problems: Vec::new(),
            note: None,
            error: None,
            saved_count: 0,
        };
        s.refresh();
        s
    }

    fn refresh(&mut self) {
        if !self.available {
            return;
        }
        self.list_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(d.units.list()),
            |s| async move { s.unit_sets().list().await },
        ));
        self.poll();
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.list_job) {
            match r {
                Ok(v) => self.sets = Some(v),
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.save_job) {
            match r {
                Ok(s) => {
                    self.saved_count += 1;
                    self.editing = Some(s.id);
                    self.draft = draft_of(&s);
                    self.saved_draft = self.draft.clone();
                    self.note = Some(t("units.saved"));
                    self.link.bus().publish(crate::data_bus::Change::Items);
                    self.refresh();
                }
                Err(e) => self.failed(e),
            }
        }
    }

    fn failed(&mut self, e: ApiError) {
        let key = match cl_api::unit_refusal(&e) {
            Some(UnitRefusal::InUse) => Some("units.refusal.in_use"),
            Some(UnitRefusal::DuplicateName) => Some("units.refusal.duplicate"),
            Some(UnitRefusal::NotInSet) => Some("units.refusal.not_in_set"),
            Some(UnitRefusal::QuantityTooSmall) => Some("units.refusal.too_small"),
            None => None,
        };
        match key {
            Some(k) => self.note = Some(t(k)),
            None => self.error = Some(e),
        }
    }

    fn open(&mut self, id: Option<UnitSetId>) {
        let found = id.and_then(|id| self.sets.iter().flatten().find(|s| s.id == id).cloned());
        self.editing = found.as_ref().map(|s| s.id);
        self.draft = found.as_ref().map_or_else(blank, draft_of);
        self.saved_draft = self.draft.clone();
        self.problems.clear();
        self.note = None;
        self.error = None;
    }

    fn save(&mut self) {
        self.problems.clear();
        self.note = None;
        self.error = None;
        let checked = match self.draft.check() {
            Ok(c) => c,
            Err(p) => {
                self.problems = p;
                return;
            }
        };
        let current = self
            .editing
            .and_then(|id| self.sets.iter().flatten().find(|s| s.id == id).cloned());
        let n = NewUnitSet {
            name: checked.name,
            base_unit_name: checked.base_name,
            base_unit_abbreviation: checked.base_abbreviation,
            is_active: current.as_ref().is_none_or(|s| s.is_active),
            units: checked
                .units
                .into_iter()
                .map(|u| Unit {
                    id: u.id,
                    name: u.name,
                    abbreviation: u.abbreviation,
                    factor: u.factor,
                })
                .collect(),
        };
        let editing = self.editing;
        let expected = current.and_then(|s| s.updated_at);
        let live = n.clone();
        self.save_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label("orphan.unit_set_save", None),
            move |d| {
                match editing {
                    None => d.units.create(&n),
                    Some(id) => d.units.update(id, &n),
                }
                .map_err(crate::company::demo::units::UnitError::api)
            },
            move |s| async move {
                match editing {
                    None => s.unit_sets().create(&live).await,
                    Some(id) => s.unit_sets().update(id, &live, expected).await,
                }
            },
        ));
        self.poll();
    }

    fn dirty(&self) -> bool {
        self.draft != self.saved_draft
    }

    fn list_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mut open: Option<Option<UnitSetId>> = None;
        ui.vertical(|ui| {
            ui.set_width(220.0);
            if ui.button(t("units.new")).clicked() {
                open = Some(None);
            }
            ui.add_space(4.0);
            match &self.sets {
                None => form::progress_line(ui, tokens, &t("inventory.loading")),
                Some(v) if v.is_empty() => form::hint(ui, tokens, &t("units.empty")),
                Some(v) => {
                    for s in v {
                        let on = self.editing == Some(s.id);
                        if ui.selectable_label(on, &s.name).clicked() {
                            open = Some(Some(s.id));
                        }
                    }
                }
            }
        });
        if let Some(id) = open {
            self.open(id);
        }
    }

    fn editor_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.vertical(|ui| {
            let title = if self.editing.is_some() {
                t("units.edit")
            } else {
                t("units.new_title")
            };
            ui.label(RichText::new(title).size(16.0).strong());
            egui::Grid::new("unit_set_head")
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    let name = ui.label(t("units.name"));
                    ui.add_sized(
                        [220.0, 22.0],
                        egui::TextEdit::singleline(&mut self.draft.name),
                    )
                    .labelled_by(name.id);
                    ui.end_row();
                    let base = ui.label(t("units.base"));
                    ui.add_sized(
                        [160.0, 22.0],
                        egui::TextEdit::singleline(&mut self.draft.base_name),
                    )
                    .labelled_by(base.id);
                    ui.end_row();
                    let abbr = ui.label(t("units.base_abbreviation"));
                    ui.add_sized(
                        [80.0, 22.0],
                        egui::TextEdit::singleline(&mut self.draft.base_abbreviation),
                    )
                    .labelled_by(abbr.id);
                    ui.end_row();
                });
            ui.add_space(6.0);
            form::hint(ui, tokens, &t("units.intro"));
            let base = if self.draft.base_name.trim().is_empty() {
                t("units.base_placeholder")
            } else {
                self.draft.base_name.trim().to_string()
            };
            let mut remove = None;
            egui::Grid::new("unit_set_units")
                .striped(true)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for h in [
                        "units.col.unit",
                        "units.col.abbreviation",
                        "units.col.factor",
                        "",
                    ] {
                        ui.label(
                            RichText::new(if h.is_empty() { String::new() } else { t(h) })
                                .strong()
                                .size(12.0),
                        );
                    }
                    ui.end_row();
                    for (i, r) in self.draft.units.iter_mut().enumerate() {
                        let n = (i + 1).to_string();
                        ui.add_sized([140.0, 22.0], egui::TextEdit::singleline(&mut r.name))
                            .widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::TextInput,
                                    true,
                                    t_with("units.row.name", &[("n", &n)]),
                                )
                            });
                        ui.add_sized(
                            [60.0, 22.0],
                            egui::TextEdit::singleline(&mut r.abbreviation),
                        )
                        .widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::TextInput,
                                true,
                                t_with("units.row.abbreviation", &[("n", &n)]),
                            )
                        });
                        ui.horizontal(|ui| {
                            let locked = r.locked.is_some();
                            ui.add_enabled_ui(!locked, |ui| {
                                ui.add_sized(
                                    [90.0, 22.0],
                                    egui::TextEdit::singleline(&mut r.factor),
                                )
                            })
                            .inner
                            .widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::TextInput,
                                    !locked,
                                    t_with("units.row.factor", &[("n", &n)]),
                                )
                            });
                            ui.label(&base);
                            if locked {
                                ui.label(
                                    RichText::new(t("units.locked"))
                                        .size(12.0)
                                        .color(tokens.text_secondary),
                                )
                                .on_hover_text(t("units.locked.tip"));
                            }
                        });
                        if r.locked.is_none()
                            && ui
                                .button(t_with("units.row.remove", &[("n", &n)]))
                                .clicked()
                        {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            if let Some(i) = remove {
                self.draft.remove_row(i);
            }
            ui.horizontal(|ui| {
                if ui.button(t("units.add_unit")).clicked() {
                    self.draft.add_row();
                }
                if ui.button(t("units.save")).clicked() {
                    self.save();
                }
            });
            for p in &self.problems {
                form::inline_warning(ui, tokens, &t(p.key().0));
            }
            if let Some(n) = &self.note {
                ui.label(RichText::new(n).color(tokens.warn_text));
            }
            if let Some(e) = &self.error {
                form::inline_error(ui, tokens, e);
            }
        });
    }

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(RichText::new(t("units.title")).size(22.0).strong());
        if !self.available {
            form::hint(ui, &tokens, &t("inventory.needs_update"));
            return;
        }
        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            self.list_ui(ui, &tokens);
            ui.separator();
            self.editor_ui(ui, &tokens);
        });
    }
}

impl Screen for UnitsScreen {
    fn title(&self) -> String {
        t("units.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        egui::ScrollArea::both().show(ui, |ui| self.draw(ui));
        if self.list_job.is_some() || self.save_job.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn dirty(&self) -> bool {
        UnitsScreen::dirty(self)
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "view.refresh" => {
                self.refresh();
                true
            }
            "form.save_close" | "form.save_new" => {
                self.save();
                true
            }
            super::DEV_RESET => {
                let ok = super::demo_reset(&self.link);
                self.saved_count = 0;
                self.open(None);
                self.refresh();
                ok
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        json!({
            "screen": "unit_sets",
            "available": self.available,
            "loaded": self.sets.is_some(),
            "sets": self.sets.as_ref().map(Vec::len),
            "mode": if self.editing.is_some() { "edit" } else { "new" },
            "units": self.draft.units.len(),
            "locked": self.draft.units.iter().filter(|u| u.locked.is_some()).count(),
            "problems": self.problems.iter().map(|p| p.name()).collect::<Vec<_>>(),
            "note": self.note.is_some(),
            "error": self.error.is_some(),
            "saved_count": self.saved_count,
            "dirty": self.dirty(),
        })
    }
}
