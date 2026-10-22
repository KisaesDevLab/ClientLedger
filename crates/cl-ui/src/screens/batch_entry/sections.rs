//! The section strip (plan §3 WP-D1): a chip per section ("Checks · 1010
//! Operating Checking (12)", with its problem count), Add section and
//! Remove section (asked first when it has rows), the section's Type and
//! Account pickers (a click opens the list, Q-250; the Account picker
//! offers only bank or card accounts as the type needs), and Ctrl+PageUp /
//! Ctrl+PageDown between sections.

use std::sync::Arc;

use cl_core::ids::AccountId;
use cl_grid::Grid;
use cl_model::forms::batch::{ContextKind, Section, SectionKind};
use cl_model::register::AccountClass;
use eframe::egui::{self, RichText, Ui};

use super::{BulkEntryScreen, KIND, grid};
use crate::i18n::{t, t_with};
use crate::screens::card_charge::kit::{self, Books};
use crate::theme::Tokens;

/// The i18n key of a section type's name.
#[must_use]
pub(crate) const fn kind_key(k: SectionKind) -> &'static str {
    match k {
        SectionKind::Checks => "batch_entry.kind.checks",
        SectionKind::Expenses => "batch_entry.kind.expenses",
        SectionKind::CardCharges => "batch_entry.kind.card_charges",
        SectionKind::CardCredits => "batch_entry.kind.card_credits",
        SectionKind::Deposits => "batch_entry.kind.deposits",
        SectionKind::JournalEntries => "batch_entry.kind.journal_entries",
    }
}

/// The grid's accessible label for a section type ("Checks rows").
#[must_use]
pub(crate) fn grid_label(k: SectionKind) -> String {
    t_with("batch_entry.grid.label", &[("kind", &t(kind_key(k)))])
}

/// The accounts a section of this class may post against.
#[must_use]
pub(crate) fn accounts_for(books: &Books, class: ContextKind) -> Vec<(AccountId, String)> {
    match class {
        ContextKind::Bank => books.accounts_of(&[AccountClass::Bank]),
        ContextKind::Card => books.accounts_of(&[AccountClass::CreditCard]),
    }
}

impl BulkEntryScreen {
    /// The grid shows the selected section with its type's columns (after
    /// an undo, a type change or a removed section too).
    pub(crate) fn sync_shown_section(&mut self) {
        let n = self.batch().sections.len();
        let lists = Arc::clone(&self.books.lists);
        if self.selected() >= n && n > 0 {
            self.grid.adapter_mut().selected = n - 1;
            self.grid.reset_view(&*lists);
        }
        let Some(kind) = self.batch().sections.get(self.selected()).map(|s| s.kind) else {
            return;
        };
        if kind == self.shown_kind {
            return;
        }
        let next = self.spare.remove(&kind).unwrap_or_else(|| {
            // Fixed specs, covered by `every_section_spec_is_valid_with_its_own_headers`.
            Grid::new(grid::grid_id(kind), grid::spec(kind)).expect("the Bulk Entry spec is valid")
        });
        let old = self.grid.swap_grid(next, grid_label(kind), &*lists);
        self.grid
            .set_widths_key(format!("{KIND}/{}", grid::grid_id(kind)));
        self.spare.insert(self.shown_kind, old);
        self.shown_kind = kind;
    }

    /// Shows section `i` (its rows in the grid, the cursor on its first
    /// row).
    pub(crate) fn select_section(&mut self, i: usize) {
        if i >= self.batch().sections.len() || i == self.selected() {
            return;
        }
        let lists = Arc::clone(&self.books.lists);
        let _ = self.grid.commit_pending(&*lists);
        self.mark_cursor_row_left();
        self.grid.adapter_mut().selected = i;
        self.sync_shown_section();
        self.grid.reset_view(&*lists);
        self.confirm_remove = None;
    }

    /// Ctrl+PageDown (`forward`) / Ctrl+PageUp: the next or previous
    /// section, wrapping around.
    pub(crate) fn step_section(&mut self, forward: bool) {
        let n = self.batch().sections.len();
        if n < 2 {
            return;
        }
        let i = self.selected();
        let next = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        self.select_section(next);
    }

    /// The account a new section of `kind` starts on: the shown section's
    /// when it fits, else the first of its class.
    fn default_context(&self, kind: SectionKind) -> Option<AccountId> {
        let class = kind.needs_context()?;
        let shown = self
            .batch()
            .sections
            .get(self.selected())
            .filter(|s| s.kind.needs_context() == Some(class))
            .and_then(|s| s.context);
        shown.or_else(|| accounts_for(&self.books, class).first().map(|(a, _)| *a))
    }

    /// Add section: a new section of `kind` after the others, shown.
    pub(crate) fn add_section(&mut self, kind: SectionKind) {
        if self.busy() {
            return;
        }
        let context = self.default_context(kind);
        let today = self.today;
        let a = self.grid.adapter_mut();
        let highest = a.highest.clone();
        let mut keys = std::mem::take(&mut a.keys);
        let mut index = 0;
        self.grid.edit_step(|b| {
            b.sections.push(Section::new(kind, context));
            index = b.sections.len() - 1;
            grid::ensure_entry(b, index, &mut keys, today, &highest);
        });
        self.grid.adapter_mut().keys = keys;
        if let Some(bank) = context.filter(|_| kind == SectionKind::Checks) {
            self.fetch_numbers(bank);
        }
        self.last_action = Some("add_section");
        self.select_section(index);
    }

    /// Remove section: asks first when the section has rows; the last
    /// section stays.
    pub(crate) fn remove_section(&mut self, i: usize, confirmed: bool) {
        if self.busy() || self.batch().sections.len() < 2 {
            return;
        }
        let has_rows = self
            .batch()
            .sections
            .get(i)
            .is_some_and(|s| s.filled_count() > 0);
        if has_rows && !confirmed {
            self.confirm_remove = Some(i);
            return;
        }
        self.confirm_remove = None;
        let lists = Arc::clone(&self.books.lists);
        let _ = self.grid.commit_pending(&*lists);
        self.grid.edit_step(|b| {
            if i < b.sections.len() {
                b.sections.remove(i);
            }
        });
        let sel = self.selected();
        let n = self.batch().sections.len();
        self.grid.adapter_mut().selected = if sel > i || sel >= n {
            sel.saturating_sub(1).min(n - 1)
        } else {
            sel
        };
        self.checks.prune(self.grid.form().current());
        self.sync_shown_section();
        self.grid.reset_view(&*lists);
        self.last_action = Some("remove_section");
    }

    /// The Type picker: only while the section has no rows (the columns
    /// change).
    pub(crate) fn set_kind(&mut self, i: usize, kind: SectionKind) {
        let Some(s) = self.batch().sections.get(i) else {
            return;
        };
        if s.kind == kind || s.filled_count() > 0 || self.busy() {
            return;
        }
        let context = if s.kind.needs_context() == kind.needs_context() {
            s.context
        } else {
            self.default_context(kind)
        };
        let today = self.today;
        let a = self.grid.adapter_mut();
        let highest = a.highest.clone();
        let mut keys = std::mem::take(&mut a.keys);
        self.grid.edit_step(|b| {
            if let Some(s) = b.sections.get_mut(i) {
                *s = Section::new(kind, context);
            }
            grid::ensure_entry(b, i, &mut keys, today, &highest);
        });
        self.grid.adapter_mut().keys = keys;
        if let Some(bank) = context.filter(|_| kind == SectionKind::Checks) {
            self.fetch_numbers(bank);
        }
        self.sync_shown_section();
        let lists = Arc::clone(&self.books.lists);
        self.grid.reset_view(&*lists);
    }

    /// The Account picker.
    pub(crate) fn set_context(&mut self, i: usize, account: AccountId) {
        if self.busy() {
            return;
        }
        self.grid.edit_step(|b| {
            if let Some(s) = b.sections.get_mut(i) {
                s.context = Some(account);
            }
        });
        if self
            .batch()
            .sections
            .get(i)
            .is_some_and(|s| s.kind == SectionKind::Checks)
        {
            self.fetch_numbers(account);
        }
        self.renumber_entry_rows();
    }

    /// A chip's text: "Checks · 1010 · Operating Checking (12)".
    fn chip_text(&self, s: &Section) -> String {
        let account = s
            .context
            .and_then(|a| self.books.lists.account_text(a))
            .map(str::to_string);
        let kind = t(kind_key(s.kind));
        let name = match account {
            Some(a) => format!("{kind} \u{b7} {a}"),
            None => kind,
        };
        t_with(
            "batch_entry.section.chip",
            &[("name", &name), ("count", &s.filled_count().to_string())],
        )
    }

    /// The strip of section chips with Add and Remove.
    pub(crate) fn strip_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let mut pick = None;
        let mut add: Option<SectionKind> = None;
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            let selected = self.selected();
            let chips: Vec<(String, usize)> = self
                .batch()
                .sections
                .iter()
                .enumerate()
                .map(|(i, s)| (self.chip_text(s), self.problem_rows_in(i).len()))
                .collect();
            for (i, (text, problems)) in chips.into_iter().enumerate() {
                let label = if problems > 0 {
                    RichText::new(t_with(
                        "batch_entry.section.chip_problems",
                        &[("chip", &text), ("problems", &problems.to_string())],
                    ))
                    .color(tokens.warn_text)
                } else {
                    RichText::new(text)
                };
                let r = ui
                    .selectable_label(i == selected, label)
                    .on_hover_text(t_with(
                        "batch_entry.section.tip",
                        &[
                            ("next", &kit::chip("batch.next_section").replace(' ', "+")),
                            ("prev", &kit::chip("batch.prev_section").replace(' ', "+")),
                        ],
                    ));
                if r.clicked() {
                    pick = Some(i);
                }
            }
            ui.add_enabled_ui(enabled, |ui| {
                ui.menu_button(t("batch_entry.section.add"), |ui| {
                    for k in SectionKind::ALL {
                        if !self.caps().offers(k) {
                            continue;
                        }
                        if ui.button(t(kind_key(k))).clicked() {
                            add = Some(k);
                            ui.close();
                        }
                    }
                });
                let can_remove = self.batch().sections.len() > 1;
                if ui
                    .add_enabled(
                        can_remove,
                        egui::Button::new(t("batch_entry.section.remove")),
                    )
                    .on_disabled_hover_text(t("batch_entry.section.remove.last"))
                    .clicked()
                {
                    remove = Some(self.selected());
                }
            });
        });
        if let Some(i) = self.confirm_remove {
            let mut yes = false;
            let mut no = false;
            crate::screens::company::form::card(ui, tokens, |ui| {
                ui.label(t("batch_entry.section.remove.confirm"));
                ui.horizontal(|ui| {
                    yes = crate::widgets::destructive_button(
                        ui,
                        &t("batch_entry.section.remove.yes"),
                    )
                    .clicked();
                    no = ui.button(t("batch_entry.section.remove.no")).clicked();
                });
            });
            if yes {
                self.remove_section(i, true);
            } else if no {
                self.confirm_remove = None;
            }
        }
        if let Some(k) = add {
            self.add_section(k);
        } else if let Some(i) = remove {
            self.remove_section(i, false);
        } else if let Some(i) = pick {
            self.select_section(i);
        }
    }

    /// The selected section's Type and Account pickers and "Add new names
    /// to the list".
    pub(crate) fn section_header_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let i = self.selected();
        let Some(s) = self.batch().sections.get(i).cloned() else {
            return;
        };
        let mut kind_pick = None;
        let mut account_pick = None;
        let mut add_names = None;
        ui.add_enabled_ui(enabled, |ui| {
            crate::forms::header::wrap_row(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                kit::wrap_item(ui, "batch_entry/type", |ui| {
                    ui.vertical(|ui| {
                        let l = kit::above(ui, tokens, &t("batch_entry.type"));
                        let locked = s.filled_count() > 0;
                        let r = ui
                            .add_enabled_ui(!locked, |ui| {
                                egui::ComboBox::from_id_salt(("batch_entry/type", i))
                                    .width(170.0)
                                    .selected_text(t(kind_key(s.kind)))
                                    .show_ui(ui, |ui| {
                                        for k in SectionKind::ALL {
                                            if self.caps().offers(k)
                                                && ui
                                                    .selectable_label(k == s.kind, t(kind_key(k)))
                                                    .clicked()
                                            {
                                                kind_pick = Some(k);
                                            }
                                        }
                                    })
                                    .response
                                    .labelled_by(l.id)
                            })
                            .inner;
                        if locked {
                            r.on_disabled_hover_text(t("batch_entry.type.locked"));
                        }
                    });
                });
                if let Some(class) = s.kind.needs_context() {
                    kit::wrap_item(ui, "batch_entry/account", |ui| {
                        ui.vertical(|ui| {
                            let accounts = accounts_for(&self.books, class);
                            let label = match class {
                                ContextKind::Bank => t("batch_entry.account.bank"),
                                ContextKind::Card => t("batch_entry.account.card"),
                            };
                            let (picked, _) = kit::account_combo(
                                ui,
                                tokens,
                                &format!("batch_entry/account/{i}"),
                                &label,
                                &accounts,
                                s.context,
                                260.0,
                            );
                            account_pick = picked;
                        });
                    });
                }
                kit::wrap_item(ui, "batch_entry/add_names", |ui| {
                    ui.add_space(18.0);
                    let mut on = self.batch().add_new_names;
                    if ui
                        .checkbox(&mut on, t("batch_entry.add_names"))
                        .on_hover_text(t("batch_entry.add_names.tip"))
                        .changed()
                    {
                        add_names = Some(on);
                    }
                });
            });
        });
        if let Some(k) = kind_pick {
            self.set_kind(i, k);
        }
        if let Some(a) = account_pick {
            self.set_context(i, a);
        }
        if let Some(on) = add_names {
            self.grid.edit_step(|b| b.add_new_names = on);
        }
    }
}
