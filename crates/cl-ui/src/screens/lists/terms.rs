//! Terms and Payment Methods (P11 step 6): the engine's fixed lists, shown
//! read-only. Editable lists (with discount terms such as 2% 10 Net 30)
//! wait for U-43; the window says so in plain words.

use cl_model::lists::terms::{PAYMENT_METHODS, TERMS};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The Terms window kind.
pub const TERMS_KIND: &str = "lists.terms";
/// The Payment Methods window kind.
pub const METHODS_KIND: &str = "lists.payment_methods";

/// Which fixed list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedList {
    /// Payment terms.
    Terms,
    /// Payment methods.
    Methods,
}

/// A read-only fixed list window.
pub struct FixedListScreen {
    which: FixedList,
}

impl FixedListScreen {
    /// The window for `which`.
    #[must_use]
    pub fn new(which: FixedList) -> FixedListScreen {
        FixedListScreen { which }
    }

    fn kind(&self) -> &'static str {
        match self.which {
            FixedList::Terms => TERMS_KIND,
            FixedList::Methods => METHODS_KIND,
        }
    }

    /// (name, detail) rows.
    fn rows(&self) -> Vec<(String, String)> {
        match self.which {
            FixedList::Terms => TERMS
                .iter()
                .map(|term| {
                    let due = match term.due_days {
                        Some(0) => t("terms.due.now"),
                        Some(d) => t_with("terms.due.days", &[("days", &d.to_string())]),
                        None => t("terms.due.custom"),
                    };
                    (t(term.label.as_str()), due)
                })
                .collect(),
            FixedList::Methods => PAYMENT_METHODS
                .iter()
                .map(|m| (t(m.label.as_str()), String::new()))
                .collect(),
        }
    }
}

impl Screen for FixedListScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{}", self.kind()))
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        form::heading(ui, &tokens, &self.title());
        ui.add_space(4.0);
        let (col1, col2, note) = match self.which {
            FixedList::Terms => ("terms.col.name", Some("terms.col.due"), "terms.fixed_note"),
            FixedList::Methods => ("methods.col.name", None, "methods.fixed_note"),
        };
        form::hint(ui, &tokens, &t(note));
        ui.add_space(8.0);
        let rows = self.rows();
        egui::Grid::new(("fixed-list", self.kind()))
            .num_columns(2)
            .striped(true)
            .min_col_width(160.0)
            .spacing([24.0, 8.0])
            .show(ui, |ui| {
                ui.label(RichText::new(t(col1)).strong());
                ui.label(RichText::new(col2.map(t).unwrap_or_default()).strong());
                ui.end_row();
                for (name, detail) in &rows {
                    ui.label(name);
                    ui.label(RichText::new(detail).color(tokens.text_secondary));
                    ui.end_row();
                }
            });
    }

    fn state(&self) -> Value {
        json!({
            "screen": match self.which {
                FixedList::Terms => "terms",
                FixedList::Methods => "payment_methods",
            },
            "rows": self.rows().len(),
            "editable": false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_lists_render_every_row() {
        for (which, kind, rows) in [
            (FixedList::Terms, TERMS_KIND, 6),
            (FixedList::Methods, METHODS_KIND, 5),
        ] {
            let mut s = FixedListScreen::new(which);
            assert_eq!(s.kind(), kind);
            assert_eq!(s.state()["rows"], rows);
            assert_eq!(s.state()["editable"], false);
            assert_ne!(s.title(), "");
            let ctx = egui::Context::default();
            crate::shell::Shell::install(&ctx);
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
            });
            out.textures_delta.clear();
        }
        let terms = FixedListScreen::new(FixedList::Terms).rows();
        assert_eq!(terms[0].1, t("terms.due.now"));
        assert!(terms[2].1.contains("30"));
        assert_eq!(terms[5].1, t("terms.due.custom"));
    }
}
