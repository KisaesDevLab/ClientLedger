//! The ledger grid demo (developer-only, `--demo`): a check's split lines
//! in a [`GridForm`], a 200,000-row register and a fixed-row Pay bills
//! example, all synthetic and saved only in memory (P10 plan §2.12).
//!
//! `state` carries positions, counts and flags only: no amounts, names or
//! memo text. Exactness is proven by the displayed cell values and footer
//! (`Money::format` never rounds) and by in-process tests.

use std::sync::Arc;

use cl_core::{CalendarDate, Money};
use cl_grid::{
    Grid, GridCommand, GridEvent, GridHeight, GridProblem, GridSnapshot, ModelRef, NoLookups,
};
use cl_model::forms::{AfterSave, FormState, SaveIntent, SaveStart};
use cl_model::{CommandId, Scope};
use eframe::egui::{RichText, Ui};
use serde_json::json;

use super::lines::{self, DemoCheck, DemoLists, LinesState};
use super::register::{self, Bills, REGISTER_ROWS, Register};
use crate::company::{CompanyEvent, CompanyLink};
use crate::grid_env::EnvParts;
use crate::grid_form::{GridForm, Routed};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "dev.grid_demo";

/// Header field id for the check amount (undo merging).
const CHECK_FIELD: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Lines,
    Register,
    Bills,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Lines => "lines",
            Mode::Register => "register",
            Mode::Bills => "bills",
        }
    }
}

/// The demo screen.
pub struct GridDemo {
    mode: Mode,
    lists: DemoLists,
    lines: GridForm<DemoCheck, LinesState>,
    check_text: String,
    saved: Vec<DemoCheck>,
    last_save_balanced: bool,
    problems: Vec<GridProblem>,
    register: Option<(Grid, Register)>,
    one_line: bool,
    bills: Option<(Grid, Bills)>,
    parts: EnvParts,
    snap: GridSnapshot,
    today: CalendarDate,
    link: Option<Arc<CompanyLink>>,
}

fn new_grid(salt: &str, spec: cl_grid::GridSpec) -> Grid {
    // The specs are fixed and covered by tests; a failure is a programming
    // error in this file.
    Grid::new(salt, spec).expect("the demo grid specs are valid")
}

/// The money style in force (for the footer).
fn money_text(m: Money, parts: &EnvParts) -> String {
    m.format(parts.frame.money_style)
}

/// Row 1, first column: where the register and Pay bills grids start.
fn first_cell() -> cl_grid::CellPos {
    cl_grid::CellPos {
        row: cl_grid::RowKey(1),
        col: cl_grid::ColumnId(1),
    }
}

/// Grid commands for the register and Pay bills grids (they have no form).
fn plain_command(id: CommandId) -> Option<GridCommand> {
    Some(match id.as_str() {
        "form.revert" => GridCommand::Revert,
        "form.prev_record" => GridCommand::PageUp,
        "form.next_record" => GridCommand::PageDown,
        "form.insert_line" => GridCommand::InsertLine,
        "form.delete_line" => GridCommand::DeleteLine,
        "edit.open_list" => GridCommand::OpenList,
        "edit.undo" => GridCommand::Undo,
        "edit.redo" => GridCommand::Redo,
        _ => return None,
    })
}

impl GridDemo {
    /// A fresh demo.
    #[must_use]
    pub fn new(today: CalendarDate, link: Option<Arc<CompanyLink>>) -> GridDemo {
        let mut state = LinesState::new();
        let check = state.new_check();
        let lines = GridForm::new(
            FormState::new(check),
            state,
            new_grid("dev.grid_demo/lines", lines::spec()),
            t("dev.grid_demo.lines_label"),
            GridHeight::Rows { min: 4, max: 8 },
        )
        .with_widths_key("dev.grid_demo/lines");
        GridDemo {
            mode: Mode::Lines,
            lists: DemoLists::default(),
            lines,
            check_text: String::new(),
            saved: Vec::new(),
            last_save_balanced: false,
            problems: Vec::new(),
            register: None,
            one_line: false,
            bills: None,
            parts: EnvParts::default(),
            snap: GridSnapshot::default(),
            today,
            link,
        }
    }

    fn show_register(&mut self) {
        self.mode = Mode::Register;
        let mut grid = new_grid("dev.grid_demo/register", register::spec(self.one_line));
        let mut model = Register::new(REGISTER_ROWS, self.today);
        let label = String::new();
        let env = self
            .parts
            .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
        grid.focus(first_cell(), ModelRef::Browse(&mut model), &env);
        self.register = Some((grid, model));
    }

    fn show_bills(&mut self) {
        self.mode = Mode::Bills;
        if self.bills.is_none() {
            let mut grid = new_grid("dev.grid_demo/bills", register::bills_spec());
            let mut model = Bills::new(self.today);
            let label = String::new();
            let env = self
                .parts
                .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
            grid.focus(first_cell(), ModelRef::Edit(&mut model), &env);
            self.bills = Some((grid, model));
        }
    }

    /// Save locally (no engine): commit the open cell, check the rules,
    /// then keep a copy. Returns true when it saved.
    fn save_with(&mut self, intent: SaveIntent) -> bool {
        if self.lines.commit_pending(&self.lists).is_err() {
            return false;
        }
        self.last_save_balanced = lines::balanced(self.lines.form().current());
        self.problems = self.lines.validate();
        if !self.problems.is_empty() {
            return false;
        }
        let after = match self.lines.begin_save(intent, &self.lists) {
            Ok(SaveStart::Send(value)) => {
                self.saved.push(lines::for_save(&value));
                self.lines.save_ok(value, &self.lists)
            }
            Ok(SaveStart::Skip(after)) => after,
            Ok(SaveStart::Busy) | Err(_) => return false,
        };
        match after {
            AfterSave::Stay => {}
            AfterSave::NewForm => {
                let fresh = self.lines.adapter_mut().new_check();
                self.lines.load(fresh, &self.lists);
                self.check_text.clear();
            }
            AfterSave::CloseWindow => {
                if let Some(link) = &self.link {
                    link.push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
                }
            }
        }
        true
    }

    fn lines_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let parsed = crate::screens::company::form::money_field(
            ui,
            &t("dev.grid_demo.check_amount"),
            &mut self.check_text,
        );
        match parsed {
            Ok(amount) => {
                if amount != self.lines.form().current().amount {
                    self.lines.edit(CHECK_FIELD, |c| c.amount = amount);
                }
            }
            Err(_) => {
                ui.label(RichText::new(t("grid.amount.invalid")).color(tokens.warn_text));
            }
        }
        ui.add_space(6.0);
        let out = self.lines.ui(ui, &self.lists);
        for e in &out.events {
            if matches!(e, GridEvent::EnterOnEmptyEntry) {
                self.save_with(SaveIntent::SaveAndNew);
            }
        }
        self.snap = self.lines.snapshot();
        ui.add_space(6.0);
        let check = self.lines.form().current();
        let total = lines::lines_total(check).map_or_else(
            || t("dev.grid_demo.too_large"),
            |m| money_text(m, &self.parts),
        );
        ui.horizontal(|ui| {
            ui.label(t_with("dev.grid_demo.lines_total", &[("total", &total)]));
            let (key, color) = if lines::balanced(check) {
                ("dev.grid_demo.matches", tokens.accent)
            } else {
                ("dev.grid_demo.does_not_match", tokens.warn_text)
            };
            ui.label(RichText::new(t(key)).color(color));
        });
        for p in &self.problems {
            let mut text = t(p.key.0);
            if let Some(fix) = p.fix {
                text.push(' ');
                text.push_str(&t(fix.0));
            }
            ui.label(RichText::new(text).color(tokens.warn_text));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if crate::widgets::primary_button(ui, &t("dev.grid_demo.save")).clicked() {
                self.save_with(SaveIntent::Save);
            }
            if ui.button(t("dev.grid_demo.save_new")).clicked() {
                self.save_with(SaveIntent::SaveAndNew);
            }
            ui.label(
                RichText::new(t_with(
                    "dev.grid_demo.saved_count",
                    &[("n", &self.saved.len().to_string())],
                ))
                .color(tokens.text_secondary),
            );
        });
    }

    fn register_ui(&mut self, ui: &mut Ui) {
        let label = t("dev.grid_demo.register_label");
        if let Some((grid, model)) = &mut self.register {
            let env = self.parts.env(
                &NoLookups,
                &label,
                GridHeight::Fill {
                    reserve_below: 8.0,
                    min_rows: 6,
                },
            );
            grid.show(ui, ModelRef::Browse(model), &env);
            self.snap = grid.snapshot(&ModelRef::Browse(model));
        }
    }

    fn bills_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let label = t("dev.grid_demo.bills_label");
        if let Some((grid, model)) = &mut self.bills {
            let env = self
                .parts
                .env(&NoLookups, &label, GridHeight::Rows { min: 6, max: 6 });
            grid.show(ui, ModelRef::Edit(model), &env);
            self.snap = grid.snapshot(&ModelRef::Edit(model));
            ui.label(
                RichText::new(t_with(
                    "dev.grid_demo.bills_selected",
                    &[("n", &model.selected().to_string())],
                ))
                .color(tokens.text_secondary),
            );
        }
    }

    fn plain_grid_command(&mut self, cmd: GridCommand) -> bool {
        let label = String::new();
        let height = GridHeight::Rows { min: 1, max: 1 };
        let env = self.parts.env(&NoLookups, &label, height);
        match self.mode {
            Mode::Register => self.register.as_mut().is_some_and(|(g, m)| {
                let handled = g.command(cmd, ModelRef::Browse(m), &env).0.yes();
                self.snap = g.snapshot(&ModelRef::Browse(m));
                handled
            }),
            Mode::Bills => self.bills.as_mut().is_some_and(|(g, m)| {
                let handled = g.command(cmd, ModelRef::Edit(m), &env).0.yes();
                self.snap = g.snapshot(&ModelRef::Edit(m));
                handled
            }),
            Mode::Lines => false,
        }
    }
}

impl Screen for GridDemo {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.parts = EnvParts::capture(ui.ctx());
        let tokens = Tokens::of(ui.ctx());
        crate::screens::company::form::heading(ui, &tokens, &t("dev.grid_demo.title"));
        crate::screens::company::form::hint(ui, &tokens, &t("dev.grid_demo.synthetic"));
        ui.add_space(6.0);
        ui.horizontal(|ui| match self.mode {
            Mode::Lines => {
                if ui.button(t("dev.grid_demo.show_register")).clicked() {
                    self.show_register();
                }
                if ui.button(t("dev.grid_demo.show_bills")).clicked() {
                    self.show_bills();
                }
            }
            Mode::Register | Mode::Bills => {
                if ui.button(t("dev.grid_demo.show_lines")).clicked() {
                    self.mode = Mode::Lines;
                }
                if self.mode == Mode::Register
                    && ui
                        .checkbox(&mut self.one_line, t("dev.grid_demo.one_line"))
                        .changed()
                {
                    self.show_register();
                }
            }
        });
        ui.add_space(8.0);
        match self.mode {
            Mode::Lines => self.lines_ui(ui, &tokens),
            Mode::Register => self.register_ui(ui),
            Mode::Bills => self.bills_ui(ui, &tokens),
        }
    }

    fn dirty(&self) -> bool {
        self.lines.dirty()
    }

    fn scope(&self) -> Scope {
        // Every screen that holds a grid uses Form scope, so Esc arrives as
        // `form.revert` (plan §2.3).
        Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        if self.mode != Mode::Lines {
            return plain_command(id).is_some_and(|c| self.plain_grid_command(c));
        }
        // Saves check the demo's rules first; `save_with` commits the open
        // cell before anything else.
        match id.as_str() {
            "form.save_new" => {
                self.save_with(SaveIntent::SaveAndNew);
                return true;
            }
            "form.save_close" => {
                self.save_with(SaveIntent::SaveAndClose);
                return true;
            }
            _ => {}
        }
        let routed = self.lines.command(id, &self.lists);
        self.snap = self.lines.snapshot();
        match routed {
            Routed::NotHandled => false,
            // The demo has one record: Prev/Next only commit (no records
            // to move to).
            Routed::Handled | Routed::Blocked(_) | Routed::Navigate { .. } | Routed::Save(_) => {
                true
            }
        }
    }

    fn save(&mut self) -> bool {
        // The close or quit prompt closes the window itself once this
        // returns true: a plain save, so no second CloseWindow is queued.
        self.save_with(SaveIntent::Save)
    }

    fn state(&self) -> serde_json::Value {
        let cursor = match (self.snap.cursor_row, self.snap.cursor_col) {
            (Some(row), Some(col)) => json!({ "row": row, "col": col }),
            _ => serde_json::Value::Null,
        };
        json!({
            "mode": self.mode.name(),
            "cursor": cursor,
            "rows": self.snap.rows,
            "top_row": self.snap.top_row,
            "popup_open": self.snap.popup_open,
            "selected_count": self.snap.selected,
            "focused": self.snap.focused,
            "dirty": self.dirty(),
            "saved_count": self.saved.len(),
            "last_save_balanced": self.last_save_balanced,
            "problems": self.problems.len(),
            "register_rows": self.register.as_ref().map_or(0, |(_, r)| {
                use cl_grid::RowModel;
                r.row_count()
            }),
        })
    }
}

#[cfg(test)]
mod tests;
