//! Review matches (step 3): how the engine matched each account the files
//! name (E2b `accountMatches`; by number, then name). An account that
//! matches nothing blocks the step until the owner picks one of the
//! company's accounts (Q-P31c: never suspense), or fixes the account list
//! file and adds it again. `system_import` re-exports this view.

use cl_api::imports::{AccountMatch, MatchStatus};
use cl_core::ids::AccountId;
use eframe::egui::{self, RichText, Ui};

use super::ImportScreen;
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// The counts the step's summary shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchCounts {
    /// Already in the company.
    pub existing: usize,
    /// Made from the account list file.
    pub created: usize,
    /// Matching nothing (picked or not).
    pub unmatched: usize,
    /// Matching nothing and not picked yet (these block the step).
    pub open: usize,
}

/// Counts `matches`, `picked` naming the source keys the owner gave an
/// account.
#[must_use]
pub fn counts<'a>(
    matches: impl IntoIterator<Item = &'a AccountMatch>,
    picked: impl Fn(&str) -> bool,
) -> MatchCounts {
    let mut c = MatchCounts::default();
    for m in matches {
        match m.status {
            MatchStatus::Existing => c.existing += 1,
            MatchStatus::Create => c.created += 1,
            MatchStatus::Unmatched => {
                c.unmatched += 1;
                if !picked(&m.source_key) {
                    c.open += 1;
                }
            }
        }
    }
    c
}

/// The account's name as the list shows it ("1000 · Checking").
fn account_text(a: &cl_api::Account) -> String {
    a.fields.account_number.as_ref().map_or_else(
        || a.fields.name.clone(),
        |n| format!("{n} · {}", a.fields.name),
    )
}

/// The source account as the file names it.
fn source_text(m: &AccountMatch) -> String {
    match &m.number {
        Some(n) if *n != m.name => format!("{n} · {}", m.name),
        _ => m.name.clone(),
    }
}

/// Draws the step.
pub fn mapping_ui(s: &mut ImportScreen, ui: &mut Ui, tokens: &Tokens) {
    let matches: Vec<AccountMatch> = s.matches().into_iter().cloned().collect();
    let c = counts(&matches, |k| s.account_map.contains_key(k));
    ui.add(
        egui::Label::new(RichText::new(t("imports.mapping.intro")).color(tokens.text_secondary))
            .wrap(),
    );
    ui.add_space(8.0);
    if matches.is_empty() {
        ui.label(t("imports.mapping.none"));
        return;
    }
    ui.label(t_with(
        "imports.mapping.summary",
        &[
            ("existing", &c.existing.to_string()),
            ("created", &c.created.to_string()),
            ("unmatched", &c.unmatched.to_string()),
        ],
    ));
    if c.open > 0 {
        ui.add(
            egui::Label::new(
                RichText::new(t_with(
                    "imports.mapping.open",
                    &[("n", &c.open.to_string())],
                ))
                .color(tokens.warn_text),
            )
            .wrap(),
        );
    }
    ui.add_space(8.0);
    let unmatched: Vec<&AccountMatch> = matches
        .iter()
        .filter(|m| m.status == MatchStatus::Unmatched)
        .collect();
    if unmatched.is_empty() {
        return;
    }
    let chart: Vec<(AccountId, String)> = s
        .chart
        .iter()
        .filter(|a| a.is_active)
        .map(|a| (a.id, account_text(a)))
        .collect();
    egui::Grid::new("imports/mapping")
        .num_columns(2)
        .striped(true)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            ui.label(RichText::new(t("imports.mapping.col_source")).strong());
            ui.label(RichText::new(t("imports.mapping.col_account")).strong());
            ui.end_row();
            for m in unmatched {
                let l = ui.label(source_text(m));
                let current = s.account_map.get(&m.source_key).copied();
                let shown = current
                    .and_then(|id| chart.iter().find(|(c, _)| *c == id))
                    .map_or_else(|| t("imports.mapping.choose"), |(_, n)| n.clone());
                let mut chosen = None;
                egui::ComboBox::from_id_salt(("imports/map", &m.source_key))
                    .selected_text(shown)
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for (id, name) in &chart {
                            if ui.selectable_label(current == Some(*id), name).clicked() {
                                chosen = Some(*id);
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
                if let Some(id) = chosen {
                    s.account_map.insert(m.source_key.clone(), id);
                    s.sync_mapping();
                }
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(RichText::new(t("imports.mapping.fix_file")).color(tokens.text_secondary))
            .wrap(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(key: &str, status: MatchStatus) -> AccountMatch {
        AccountMatch {
            source_key: key.into(),
            number: None,
            name: key.into(),
            status,
            account: None,
        }
    }

    #[test]
    fn unmatched_accounts_count_until_picked() {
        let list = [
            m("1000", MatchStatus::Existing),
            m("6100", MatchStatus::Create),
            m("Odd", MatchStatus::Unmatched),
            m("Other", MatchStatus::Unmatched),
        ];
        let c = counts(&list, |k| k == "Odd");
        assert_eq!(
            c,
            MatchCounts {
                existing: 1,
                created: 1,
                unmatched: 2,
                open: 1
            }
        );
        assert_eq!(source_text(&m("Odd", MatchStatus::Unmatched)), "Odd");
    }
}
