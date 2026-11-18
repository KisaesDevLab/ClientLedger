//! Check that it matches (step 6). D4 builds the full view (tiles, the
//! Trial balance / A/R / A/P / Inventory switch, "Show only differences",
//! the rows, Save PDF); until then this shows the engine's tie-out summary
//! only: how many accounts were compared, the total difference, and ✓
//! only when the engine says `allZero` (the UI never re-derives a
//! balance). `system_import` re-exports this view.

use cl_api::imports::TieOut;
use cl_model::imports::tieout::diff_text;
use eframe::egui::{self, RichText, Ui};

use super::{ImportScreen, show_date};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// The summary line's parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    /// "Trial balance at 10/27/2026".
    pub as_of: String,
    /// Accounts compared.
    pub accounts: usize,
    /// The engine's total difference, exact (`0.0040` stays so).
    pub difference: String,
    /// ✓: the engine said every difference is zero.
    pub all_zero: bool,
    /// Rows with a difference.
    pub differences: usize,
}

/// The summary of `t` (the engine's numbers only).
#[must_use]
pub fn summary(t: &TieOut) -> Summary {
    Summary {
        as_of: show_date(t.as_of),
        accounts: t.accounts.len(),
        difference: t.totals.diff.map_or_else(String::new, diff_text),
        all_zero: t.all_zero,
        differences: t.accounts.iter().filter(|a| !a.diff.is_zero()).count()
            + t.balance_rows.iter().filter(|r| !r.diff.is_zero()).count(),
    }
}

/// Draws the step.
pub fn tieout_ui(s: &mut ImportScreen, ui: &mut Ui, tokens: &Tokens) {
    let source = t(&format!("imports.source.{}", s.wizard.source.key()));
    ui.add(
        egui::Label::new(
            RichText::new(t_with("imports.tieout.intro", &[("source", &source)]))
                .color(tokens.text_secondary),
        )
        .wrap(),
    );
    ui.add_space(10.0);
    let Some(tie) = s.tieout.as_ref() else {
        ui.label(t("imports.tieout.loading"));
        return;
    };
    let sum = summary(tie);
    egui::Frame::new()
        .fill(tokens.accent_tint)
        .stroke(egui::Stroke::new(1.0, tokens.accent_border))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.label(
                RichText::new(t_with("imports.tieout.tile", &[("date", &sum.as_of)]))
                    .color(tokens.text_secondary),
            );
            let mark = if sum.all_zero { " ✓" } else { "" };
            ui.label(
                RichText::new(format!("{}{mark}", sum.difference))
                    .monospace()
                    .size(20.0)
                    .color(if sum.all_zero {
                        tokens.accent
                    } else {
                        tokens.warn_text
                    }),
            );
            ui.label(
                RichText::new(t_with(
                    "imports.tieout.compared",
                    &[("n", &sum.accounts.to_string())],
                ))
                .color(tokens.text_secondary),
            );
        });
    ui.add_space(8.0);
    if sum.all_zero {
        ui.label(t("imports.tieout.all_zero"));
    } else {
        ui.add(
            egui::Label::new(
                RichText::new(t_with(
                    "imports.tieout.differences",
                    &[("n", &sum.differences.to_string())],
                ))
                .color(tokens.warn_text),
            )
            .wrap(),
        );
    }
    if !s.allowed() {
        ui.label(t("imports.tieout.admin_only"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::imports::{TieOutAccount, TieOutTotals};
    use cl_core::{CalendarDate, Money};

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    #[test]
    fn a_difference_under_a_cent_is_shown_exactly_and_never_ticked() {
        let row = |d: &str| TieOutAccount {
            account: None,
            name: "1000".into(),
            source_row: None,
            source_keys: Vec::new(),
            source: money("10.00"),
            books: money("10.00"),
            diff: money(d),
        };
        let mut tie = TieOut {
            as_of: CalendarDate::from_ymd(2026, 10, 27).expect("date"),
            accounts: vec![row("0.0040"), row("0")],
            ar_items: Vec::new(),
            ap_items: Vec::new(),
            inventory_items: Vec::new(),
            balance_rows: Vec::new(),
            totals: TieOutTotals {
                source: None,
                books: None,
                diff: Some(money("0.0040")),
            },
            all_zero: false,
        };
        let s = summary(&tie);
        assert_eq!(s.difference, "0.0040");
        assert!(!s.all_zero);
        assert_eq!(s.differences, 1);
        assert_eq!(s.as_of, "10/27/2026");
        // ✓ only on the engine's word, whatever the rows say.
        tie.accounts = vec![row("0")];
        assert!(!summary(&tie).all_zero);
        tie.all_zero = true;
        assert!(summary(&tie).all_zero);
    }
}
