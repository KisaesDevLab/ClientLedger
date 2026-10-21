//! Set up lines: the layout editor (add, rename, reorder with Ctrl+Insert
//! and Ctrl+Delete, section, account, remove), "Update this layout" and
//! "Save as new layout" (Q-256, Q-251: the layout only, never amounts),
//! deleting a layout, and the empty state's "Start from: Retail ·
//! Restaurant · Blank". The manual "Sales tax collected" line is added to
//! the sales tax liability account when a layout has none (Q-260).
//!
//! The side of each line is fixed by its section (Q-P14s) and never
//! shown.

use cl_api::{DailyLineInput, NormalSide, Preset};
use cl_core::ids::{AccountId, DailyTemplateId};
use cl_model::CommandId;
use cl_model::forms::daily_sales::{
    DailyLayout, DailySection, DailySide, LayoutLine, LayoutProblem, side_for,
};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::{Call, DailySalesScreen, Reply, engine_section, system_account};
use crate::company::Task;
use crate::i18n::t;
use crate::screens::company::form;
use crate::screens::register::account_text;
use crate::theme::Tokens;

/// The layout being set up.
#[derive(Clone, Debug)]
pub(crate) struct Setup {
    /// The saved layout, or `None` for one not saved yet (Blank).
    pub template: Option<DailyTemplateId>,
    pub original: DailyLayout,
    pub layout: DailyLayout,
    /// The row the keyboard is in (Ctrl+Insert / Ctrl+Delete).
    pub row: Option<usize>,
    pub problems: Vec<LayoutProblem>,
    /// A line in a section this version doesn't know: can't be saved here.
    pub unknown: bool,
    pub confirm_delete: bool,
    pub error: Option<cl_api::ApiError>,
}

impl Setup {
    /// Unsaved changes to the layout.
    pub(crate) fn dirty(&self) -> bool {
        self.layout != self.original
    }

    /// The control-channel state (counts only, never a label).
    pub(crate) fn state_json(&self) -> Value {
        json!({
            "saved_layout": self.template.is_some(),
            "lines": self.layout.lines.iter().filter(|l| l.active).count(),
            "dirty": self.dirty(),
            "problems": self.problems.iter().map(|p| p.key()).collect::<Vec<_>>(),
            "error": self.error.is_some(),
            "confirm_delete": self.confirm_delete,
        })
    }

    /// The active lines as the engine takes them (removed lines are left
    /// out, which keeps them removed).
    pub(crate) fn inputs(&self, keep_ids: bool) -> Vec<DailyLineInput> {
        self.layout
            .lines
            .iter()
            .filter(|l| l.active)
            .filter_map(|l| {
                let section = l.section?;
                Some(DailyLineInput {
                    id: if keep_ids { l.id } else { None },
                    section: engine_section(section),
                    label: l.label.trim().to_string(),
                    account: l.account,
                    tag: None,
                    side: match l.side {
                        DailySide::Debit => NormalSide::Debit,
                        DailySide::Credit => NormalSide::Credit,
                    },
                    required: false,
                    active: true,
                })
            })
            .collect()
    }
}

const fn section_name(s: DailySection) -> &'static str {
    super::form::section_key(s)
}

impl DailySalesScreen {
    /// Opens Set up lines for the layout in use.
    pub(crate) fn open_setup(&mut self) {
        let Some(tpl) = &self.template else { return };
        let original = self.layout.clone();
        let mut layout = original.clone();
        // Only active lines are edited; removed lines stay removed.
        layout.lines.retain(|l| l.active);
        let tax = system_account(&self.accounts, "sales_tax_payable");
        layout.ensure_tax_line(tax);
        let unknown = layout.lines.iter().any(|l| l.section.is_none());
        let mut active = original.clone();
        active.lines.retain(|l| l.active);
        self.setup = Some(Setup {
            template: Some(tpl.id),
            original: active,
            layout,
            row: None,
            problems: Vec::new(),
            unknown,
            confirm_delete: false,
            error: None,
        });
    }

    /// The empty state's choice: a preset layout from the engine, or a
    /// blank one set up here first.
    pub(crate) fn start_from(&mut self, preset: Option<Preset>) {
        self.save.error = None;
        if let Some(p) = preset {
            let name = match p {
                Preset::Restaurant => t("daily.starter.name.restaurant"),
                _ => t("daily.starter.name.retail"),
            };
            self.setup_task = self.run(Call::CreateTemplate(name, Some(p)), true);
            self.poll_setup();
            return;
        }
        let clearing = system_account(&self.accounts, "payments_clearing");
        let tax = system_account(&self.accounts, "sales_tax_payable");
        let mut layout = DailyLayout {
            name: t("daily.starter.name.blank"),
            lines: vec![
                LayoutLine::new(DailySection::Sales, &t("daily.starter.line.sales"), None),
                LayoutLine::new(
                    DailySection::Payment,
                    &t("daily.starter.line.cash"),
                    clearing,
                ),
                LayoutLine::new(
                    DailySection::Payment,
                    &t("daily.starter.line.cards"),
                    clearing,
                ),
            ],
        };
        layout.ensure_tax_line(tax);
        self.setup = Some(Setup {
            template: None,
            original: DailyLayout::default(),
            layout,
            row: None,
            problems: Vec::new(),
            unknown: false,
            confirm_delete: false,
            error: None,
        });
    }

    /// Saves the layout: `as_new` makes a new layout (ids dropped),
    /// otherwise the layout in use is updated.
    pub(crate) fn save_setup(&mut self, as_new: bool) {
        let Some(s) = self.setup.as_mut() else { return };
        s.error = None;
        s.problems = s.layout.problems();
        if s.layout.name.trim().is_empty() {
            s.problems
                .push(LayoutProblem::EmptyLabel { index: usize::MAX });
        }
        if s.unknown || !s.problems.is_empty() {
            return;
        }
        let update = !as_new && s.template.is_some();
        let renamed = s.layout.name.trim() != s.original.name.trim();
        let call = Call::SaveLayout {
            id: if update { s.template } else { None },
            name: (!update || renamed).then(|| s.layout.name.trim().to_string()),
            lines: s.inputs(update),
        };
        self.setup_task = self.run(call, true);
        self.poll_setup();
    }

    /// Deletes the layout being set up (its old days keep it).
    pub(crate) fn delete_setup(&mut self) {
        let Some(id) = self.setup.as_ref().and_then(|s| s.template) else {
            return;
        };
        self.setup_task = self.run(Call::DeleteTemplate(id), true);
        self.poll_setup();
    }

    pub(crate) fn poll_setup(&mut self) {
        let Some(r) = self.setup_task.as_mut().and_then(Task::poll_result) else {
            return;
        };
        self.setup_task = None;
        match r {
            Ok(Reply::Template(tpl)) => {
                self.setup = None;
                if !self.templates.iter().any(|x| x.id == tpl.id) {
                    self.templates.push(tpl.clone());
                }
                let id = tpl.id;
                if self.header.current().template == Some(id) {
                    self.apply_template(tpl);
                } else if self.header.is_dirty() {
                    self.pick_layout(id);
                } else {
                    let mut f = self.header.current().clone();
                    f.template = Some(id);
                    f.values.clear();
                    self.header.load(f);
                    self.apply_template(tpl);
                }
                self.save.last_action = Some("layout_saved");
                self.load_templates();
            }
            Ok(Reply::Unit) => {
                if let Some(gone) = self.setup.take().and_then(|s| s.template) {
                    self.templates.retain(|t| t.id != gone);
                }
                let mut f = self.header.current().clone();
                f.template = None;
                f.values.clear();
                self.header.load(f);
                self.template = None;
                self.layout = DailyLayout::default();
                self.save.last_action = Some("layout_deleted");
                self.load_templates();
            }
            Ok(_) => {}
            Err(e) => match self.setup.as_mut() {
                Some(s) => s.error = Some(e),
                None => self.save.error = Some(e),
            },
        }
    }

    /// Keymap commands while Set up lines is open.
    pub(crate) fn setup_command(&mut self, id: CommandId) -> Option<bool> {
        let s = self.setup.as_mut()?;
        match id.as_str() {
            "form.insert_line" => {
                let at = s.row.map_or(s.layout.lines.len(), |r| r + 1);
                let section = s
                    .row
                    .and_then(|r| s.layout.lines.get(r))
                    .and_then(|l| l.section)
                    .unwrap_or(DailySection::Other);
                s.layout.lines.insert(
                    at.min(s.layout.lines.len()),
                    LayoutLine::new(section, "", None),
                );
                s.row = Some(at);
                Some(true)
            }
            "form.delete_line" => {
                if let Some(r) = s.row.filter(|r| *r < s.layout.lines.len()) {
                    s.layout.lines.remove(r);
                    s.row = None;
                }
                Some(true)
            }
            "form.revert" => {
                if s.confirm_delete {
                    s.confirm_delete = false;
                } else if !s.dirty() {
                    self.setup = None;
                } else {
                    return Some(false);
                }
                Some(true)
            }
            "form.save_new" | "form.save_close" => Some(true),
            _ => None,
        }
    }

    /// Draws Set up lines.
    pub(crate) fn setup_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let accounts: Vec<(AccountId, String)> = self
            .accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| (a.id, account_text(a)))
            .collect();
        let busy = self.setup_task.is_some();
        let Some(s) = self.setup.as_mut() else { return };
        let mut action: Option<&'static str> = None;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("daily.setup.title"));
            form::hint(ui, tokens, &t("daily.setup.explain"));
            form::field(ui, &t("daily.setup.name"), &mut s.layout.name);
            ui.add_space(6.0);
            let mut moves: Option<(usize, isize)> = None;
            let mut remove = None;
            let count = s.layout.lines.len();
            egui::Grid::new("daily_sales/setup_grid")
                .num_columns(5)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new(t("daily.setup.col.section")).size(12.0));
                    // The line names are labelled by this header (AccessKit).
                    let line_header = ui
                        .label(RichText::new(t("daily.setup.col.line")).size(12.0))
                        .id;
                    ui.label(RichText::new(t("daily.setup.col.account")).size(12.0));
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                    for (i, line) in s.layout.lines.iter_mut().enumerate() {
                        let shown = line.section.map_or_else(
                            || t("daily.setup.section.unknown"),
                            |sec| t(section_name(sec)),
                        );
                        let mut picked_section = None;
                        let r = egui::ComboBox::from_id_salt(("daily_sales/setup_section", i))
                            .width(170.0)
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                for sec in DailySection::ALL {
                                    if ui
                                        .selectable_label(
                                            line.section == Some(sec),
                                            t(section_name(sec)),
                                        )
                                        .clicked()
                                    {
                                        picked_section = Some(sec);
                                    }
                                }
                            })
                            .response
                            .on_hover_text(t("daily.setup.col.section"));
                        if r.has_focus() {
                            s.row = Some(i);
                        }
                        if let Some(sec) = picked_section {
                            line.section = Some(sec);
                            line.side = side_for(sec);
                        }
                        let r = ui
                            .add(
                                egui::TextEdit::singleline(&mut line.label)
                                    .id_salt(("daily_sales/setup_label", i))
                                    .desired_width(200.0)
                                    // A grid cell offers last frame's column
                                    // width; without a floor the box stays
                                    // as narrow as the first frame's.
                                    .min_size(egui::vec2(200.0, 0.0))
                                    .char_limit(cl_model::forms::daily_sales::LABEL_MAX)
                                    .hint_text(t("daily.setup.col.line")),
                            )
                            .labelled_by(line_header);
                        if r.has_focus() {
                            s.row = Some(i);
                        }
                        let account = line
                            .account
                            .and_then(|a| accounts.iter().find(|(id, _)| *id == a))
                            .map_or_else(|| t("daily.account.unmapped"), |(_, n)| n.clone());
                        let mut picked_account = None;
                        egui::ComboBox::from_id_salt(("daily_sales/setup_account", i))
                            .width(220.0)
                            .selected_text(account)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        line.account.is_none(),
                                        t("daily.account.unmapped"),
                                    )
                                    .clicked()
                                {
                                    picked_account = Some(None);
                                }
                                for (id, name) in &accounts {
                                    if ui
                                        .selectable_label(line.account == Some(*id), name)
                                        .clicked()
                                    {
                                        picked_account = Some(Some(*id));
                                    }
                                }
                            })
                            .response
                            .on_hover_text(t("daily.setup.col.account"));
                        if let Some(a) = picked_account {
                            line.account = a;
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(i > 0, egui::Button::new(t("daily.setup.up")))
                                .clicked()
                            {
                                moves = Some((i, -1));
                            }
                            if ui
                                .add_enabled(
                                    i + 1 < count,
                                    egui::Button::new(t("daily.setup.down")),
                                )
                                .clicked()
                            {
                                moves = Some((i, 1));
                            }
                        });
                        if ui
                            .button(t("daily.setup.remove"))
                            .on_hover_text(t("daily.setup.remove.tip"))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            if let Some((i, d)) = moves {
                let j = i.saturating_add_signed(d);
                if j < s.layout.lines.len() {
                    s.layout.lines.swap(i, j);
                }
            }
            if let Some(i) = remove {
                s.layout.lines.remove(i);
            }
            if ui
                .button(t("daily.setup.add"))
                .on_hover_text(t("daily.setup.add.tip"))
                .clicked()
            {
                s.layout
                    .lines
                    .push(LayoutLine::new(DailySection::Other, "", None));
            }
            for p in &s.problems {
                let key = match p {
                    LayoutProblem::EmptyLabel { index: usize::MAX } => {
                        "daily.layout.problem.no_name"
                    }
                    other => other.key(),
                };
                ui.label(RichText::new(t(key)).color(tokens.warn_text));
            }
            if s.unknown {
                ui.label(RichText::new(t("daily.setup.unknown")).color(tokens.warn_text));
            }
            if let Some(e) = &s.error {
                form::inline_error(ui, tokens, e);
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if s.template.is_some()
                    && ui
                        .add_enabled(!busy, egui::Button::new(t("daily.setup.update")))
                        .clicked()
                {
                    action = Some("update");
                }
                if ui
                    .add_enabled(!busy, egui::Button::new(t("daily.setup.save_new")))
                    .clicked()
                {
                    action = Some("new");
                }
                if ui.button(t("daily.setup.cancel")).clicked() {
                    action = Some("cancel");
                }
                if s.template.is_some() {
                    ui.separator();
                    if s.confirm_delete {
                        ui.label(t("daily.setup.delete.confirm"));
                        if ui.button(t("daily.setup.delete.yes")).clicked() {
                            action = Some("delete");
                        }
                        if ui.button(t("daily.setup.delete.no")).clicked() {
                            s.confirm_delete = false;
                        }
                    } else if ui.button(t("daily.setup.delete")).clicked() {
                        s.confirm_delete = true;
                    }
                }
            });
        });
        match action {
            Some("update") => self.save_setup(false),
            Some("new") => self.save_setup(true),
            Some("delete") => self.delete_setup(),
            Some("cancel") => self.setup = None,
            _ => {}
        }
    }
}
