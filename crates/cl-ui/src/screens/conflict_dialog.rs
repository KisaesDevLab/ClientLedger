//! The shared save-conflict dialog (P33 D4 step 11, review S2 and M5,
//! Q-P33r). A save came back 409 `STALE`: someone else saved the record
//! after this window opened it. The form reads their version and builds a
//! [`cl_model::lan::Conflict`] of its editable fields (original, mine,
//! theirs):
//!
//! - nothing editable changed on their side: the form saves again on the
//!   new stamp without asking ([`Conflict::silent_resave`]);
//! - otherwise this dialog lists every field that differs (as opened,
//!   yours, theirs), the lines aligned side by side, and offers **Use
//!   theirs**, **Keep mine** (their record plus your changed fields, lines
//!   whole from one side; it names each of their changes it would undo) and
//!   **Back**.
//!
//! No mockup exists (Q-P33aj); the dialog follows DESIGN.md's modal pattern.
//! Values are compared as exact text (money as the engine's strings); the
//! dialog never adds anything up.

use cl_model::lan::{Conflict, Merge, Undone, canonical};
use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// What the owner chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Show their record; my changes are dropped.
    UseTheirs,
    /// Save the merge on their stamp.
    KeepMine(Merge),
    /// Back to the form, nothing saved.
    Back,
}

/// How one line reads in the dialog (account, amount, memo), made by the
/// form from its own lists.
pub type LineText = Box<dyn Fn(&Value) -> String + Send>;

/// The dialog over one conflict.
pub struct ConflictDialog {
    conflict: Conflict,
    line_text: LineText,
}

impl std::fmt::Debug for ConflictDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConflictDialog").finish_non_exhaustive()
    }
}

/// A field's label: `lan.field.<name>`, else the name itself.
#[must_use]
pub fn field_label(name: &str) -> String {
    let key = format!("lan.field.{name}");
    if crate::i18n::has(&key) {
        t(&key)
    } else {
        name.to_string()
    }
}

/// A value as the owner reads it (exact text; never re-computed).
#[must_use]
pub fn value_text(v: &Value) -> String {
    match v {
        Value::Null => t("lan.conflict.empty"),
        Value::String(s) if s.is_empty() => t("lan.conflict.empty"),
        Value::String(s) => s.clone(),
        Value::Bool(true) => t("lan.yes"),
        Value::Bool(false) => t("lan.no"),
        other => canonical(other),
    }
}

impl ConflictDialog {
    /// The dialog for `conflict`; `line_text` renders one line.
    #[must_use]
    pub fn new(conflict: Conflict, line_text: LineText) -> ConflictDialog {
        ConflictDialog {
            conflict,
            line_text,
        }
    }

    /// The conflict shown.
    #[must_use]
    pub fn conflict(&self) -> &Conflict {
        &self.conflict
    }

    /// Their changes "Keep mine" would undo, in words.
    #[must_use]
    pub fn undone_texts(merge: &Merge) -> Vec<String> {
        merge
            .undone
            .iter()
            .map(|u| match u {
                Undone::Field(f) => field_label(f),
                Undone::Lines { changed } => t_with(
                    "lan.conflict.undoes.lines",
                    &[("n", &crate::i18n::count_text(*changed as u64))],
                ),
            })
            .collect()
    }

    /// Draws the dialog (modal); the owner's choice, once.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<ConflictChoice> {
        let tokens = Tokens::of(ctx);
        let merge = self.conflict.keep_mine();
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("lan_conflict_dialog")).show(ctx, |ui| {
            ui.set_max_width(720.0);
            ui.label(RichText::new(t("lan.conflict.title")).strong().size(16.0));
            ui.label(t_with(
                "lan.conflict.explain",
                &[("who", &t("lan.conflict.someone"))],
            ));
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| self.diff(ui, &tokens));
            ui.add_space(6.0);
            let undone = Self::undone_texts(&merge);
            if undone.is_empty() {
                ui.label(t("lan.conflict.undoes.none"));
            } else {
                ui.label(RichText::new(t("lan.conflict.undoes")).color(tokens.warn_text));
                for u in &undone {
                    ui.label(format!("\u{2022} {u}"));
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if crate::widgets::primary_button(ui, &t("lan.conflict.keep_mine"))
                    .on_hover_text(t("lan.conflict.keep_mine.tip"))
                    .clicked()
                {
                    choice = Some(ConflictChoice::KeepMine(merge.clone()));
                }
                if ui
                    .button(t("lan.conflict.use_theirs"))
                    .on_hover_text(t("lan.conflict.use_theirs.tip"))
                    .clicked()
                {
                    choice = Some(ConflictChoice::UseTheirs);
                }
                if ui
                    .button(t("lan.conflict.back"))
                    .on_hover_text(t("lan.conflict.back.tip"))
                    .clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    choice = Some(ConflictChoice::Back);
                }
            });
        });
        choice
    }

    fn diff(&self, ui: &mut egui::Ui, tokens: &Tokens) {
        let c = &self.conflict;
        let changes = c.field_changes();
        if !changes.is_empty() {
            egui::Grid::new("lan_conflict_fields")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    for h in [
                        "lan.conflict.field",
                        "lan.conflict.original",
                        "lan.conflict.mine",
                        "lan.conflict.theirs",
                    ] {
                        ui.label(RichText::new(t(h)).color(tokens.text_secondary));
                    }
                    ui.end_row();
                    for f in &changes {
                        ui.label(field_label(&f.field));
                        ui.label(value_text(&f.original));
                        let mine = RichText::new(value_text(&f.mine));
                        ui.label(if f.mine_changed() {
                            mine.strong()
                        } else {
                            mine
                        });
                        let theirs = RichText::new(value_text(&f.theirs));
                        ui.label(if f.theirs_changed() {
                            theirs.strong()
                        } else {
                            theirs
                        });
                        ui.end_row();
                    }
                });
        }
        if c.mine_changed_lines() || c.theirs_changed_lines() {
            ui.add_space(6.0);
            ui.label(RichText::new(t("lan.conflict.lines")).strong());
            let (mine, theirs) = (c.mine.lines(), c.theirs.lines());
            egui::Grid::new("lan_conflict_lines")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new(t("lan.conflict.mine")).color(tokens.text_secondary));
                    ui.label(RichText::new(t("lan.conflict.theirs")).color(tokens.text_secondary));
                    ui.label("");
                    ui.end_row();
                    for p in c.line_diff() {
                        let text = |side: &[Value], i: Option<usize>| {
                            i.and_then(|i| side.get(i))
                                .map_or_else(String::new, |v| (self.line_text)(v))
                        };
                        ui.label(text(mine, p.left));
                        ui.label(text(theirs, p.right));
                        let what = match (p.same, p.left, p.right) {
                            (true, _, _) => String::new(),
                            (false, None, _) => t("lan.conflict.line.added"),
                            (false, _, None) => t("lan.conflict.line.removed"),
                            (false, _, _) => t("lan.conflict.line.changed"),
                        };
                        ui.label(RichText::new(what).color(tokens.warn_text));
                        ui.end_row();
                    }
                });
            ui.label(
                RichText::new(t(if c.mine_changed_lines() {
                    "lan.conflict.lines_mine"
                } else {
                    "lan.conflict.lines_theirs"
                }))
                .color(tokens.text_secondary),
            );
        }
    }

    /// For the control channel: counts only (no values).
    #[must_use]
    pub fn state(&self) -> Value {
        let m = self.conflict.keep_mine();
        json!({
            "fields": self.conflict.field_changes().len(),
            "lines_differ": self.conflict.mine_changed_lines() || self.conflict.theirs_changed_lines(),
            "undoes": m.undone.len(),
            "lines_from_mine": m.lines_from_mine,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_values_and_undone_read_in_plain_words() {
        assert_eq!(field_label("memo"), "Memo");
        assert_eq!(field_label("unknownField"), "unknownField");
        assert_eq!(value_text(&Value::Null), t("lan.conflict.empty"));
        assert_eq!(value_text(&json!("10.50")), "10.50");
        let c = Conflict::new(
            &json!({"memo": "a", "lines": ["x"]}),
            &json!({"memo": "b", "lines": ["y"]}),
            &json!({"memo": "c", "lines": ["z"]}),
            "lines",
        );
        let d = ConflictDialog::new(c, Box::new(|v| v.as_str().unwrap_or_default().to_string()));
        let texts = ConflictDialog::undone_texts(&d.conflict().keep_mine());
        assert_eq!(
            texts,
            vec!["Memo".to_string(), "their lines (1 changed)".to_string()]
        );
        assert_eq!(d.state()["undoes"], 2);
    }
}
