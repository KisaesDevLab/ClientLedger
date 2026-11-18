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
    /// Account list rows that go into one of the company's own system
    /// accounts (A/R, A/P, retained earnings; Q-P31k, U-272) unless the
    /// owner sends them elsewhere.
    pub merged: usize,
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
            MatchStatus::Create if m.system_role.is_some() => c.merged += 1,
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
    let matches: Vec<AccountMatch> = s.matches();
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
            ("merged", &c.merged.to_string()),
            ("unmatched", &c.unmatched.to_string()),
        ],
    ));
    if !s.unmapped_failed.is_empty() {
        ui.add(
            egui::Label::new(RichText::new(t("imports.mapping.failed")).color(tokens.warn_text))
                .wrap(),
        );
    }
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
    for m in matches
        .iter()
        .filter(|m| m.status == MatchStatus::Create && m.system_role.is_some())
    {
        if let Some(text) = merge_text(m, &s.chart, s.account_map.get(&m.source_key).copied()) {
            ui.add(egui::Label::new(RichText::new(text).color(tokens.text_secondary)).wrap());
        }
    }
    ui.add_space(8.0);
    let chart: Vec<(AccountId, String)> = s
        .chart
        .iter()
        .filter(|a| a.is_active)
        .map(|a| (a.id, account_text(a)))
        .collect();
    let unmatched: Vec<&AccountMatch> = matches
        .iter()
        .filter(|m| m.status == MatchStatus::Unmatched)
        .collect();
    if !unmatched.is_empty() {
        unmatched_grid(s, ui, tokens, &unmatched, &chart);
    }
    redirect_section(s, ui, &matches, &chart);
}

/// What a merged account list row becomes ("3900 · Retained Earnings goes
/// into your Retained Earnings account"); `None` for a role the screen has
/// no words for. `picked` is the owner's own choice, which wins.
fn merge_text(
    m: &AccountMatch,
    chart: &[cl_api::Account],
    picked: Option<AccountId>,
) -> Option<String> {
    use cl_api::imports::SystemRole;
    let source = source_text(m);
    if let Some(id) = picked {
        let to = chart
            .iter()
            .find(|a| a.id == id)
            .map_or_else(|| t("imports.mapping.choose"), account_text);
        return Some(t_with(
            "imports.mapping.redirected",
            &[("source", &source), ("account", &to)],
        ));
    }
    let tag = match m.system_role? {
        SystemRole::AccountsReceivable => "accounts_receivable",
        SystemRole::AccountsPayable => "accounts_payable",
        SystemRole::RetainedEarnings => "retained_earnings",
    };
    let own = chart
        .iter()
        .find(|a| a.is_active && a.system_tag.as_deref() == Some(tag));
    Some(match own {
        Some(a) => t_with(
            "imports.mapping.merges",
            &[("source", &source), ("account", &account_text(a))],
        ),
        None => t_with(
            &format!("imports.mapping.becomes.{tag}"),
            &[("source", &source)],
        ),
    })
}

/// The accounts that match nothing, each with a picker (they block the
/// step until picked).
fn unmatched_grid(
    s: &mut ImportScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    unmatched: &[&AccountMatch],
    chart: &[(AccountId, String)],
) {
    egui::Grid::new("imports/mapping")
        .num_columns(2)
        .striped(true)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            ui.label(RichText::new(t("imports.mapping.col_source")).strong());
            ui.label(RichText::new(t("imports.mapping.col_account")).strong());
            ui.end_row();
            for m in unmatched.iter().copied() {
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
                        for (id, name) in chart {
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

/// Account list rows the import would create (or merge into a system
/// account) that the owner may send to one of the company's accounts
/// instead: a renamed Retained Earnings the engine does not recognise
/// (U-272 open point) is mapped here. Choosing the first entry undoes the
/// choice.
fn redirect_section(
    s: &mut ImportScreen,
    ui: &mut Ui,
    matches: &[AccountMatch],
    chart: &[(AccountId, String)],
) {
    let rows: Vec<&AccountMatch> = matches
        .iter()
        .filter(|m| m.status == MatchStatus::Create)
        .collect();
    if rows.is_empty() || chart.is_empty() {
        return;
    }
    ui.add_space(8.0);
    egui::CollapsingHeader::new(t("imports.mapping.redirect"))
        .id_salt("imports/redirect")
        .show(ui, |ui| {
            ui.add(egui::Label::new(t("imports.mapping.redirect_hint")).wrap());
            egui::Grid::new("imports/redirect_grid")
                .num_columns(2)
                .striped(true)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    for m in rows {
                        let l = ui.label(source_text(m));
                        let current = s.account_map.get(&m.source_key).copied();
                        let default = if m.system_role.is_some() {
                            t("imports.mapping.keep_merge")
                        } else {
                            t("imports.mapping.keep_new")
                        };
                        let shown = current
                            .and_then(|id| chart.iter().find(|(c, _)| *c == id))
                            .map_or_else(|| default.clone(), |(_, n)| n.clone());
                        let mut chosen: Option<Option<AccountId>> = None;
                        egui::ComboBox::from_id_salt(("imports/redirect", &m.source_key))
                            .selected_text(shown)
                            .width(280.0)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(current.is_none(), &default).clicked() {
                                    chosen = Some(None);
                                }
                                for (id, name) in chart {
                                    if ui.selectable_label(current == Some(*id), name).clicked() {
                                        chosen = Some(Some(*id));
                                    }
                                }
                            })
                            .response
                            .labelled_by(l.id);
                        match chosen {
                            Some(Some(id)) => {
                                s.account_map.insert(m.source_key.clone(), id);
                                s.sync_mapping();
                            }
                            Some(None) => {
                                s.account_map.remove(&m.source_key);
                                s.sync_mapping();
                            }
                            None => {}
                        }
                        ui.end_row();
                    }
                });
        });
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
            matched_by: None,
            rows: 1,
            system_role: None,
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
                merged: 0,
                unmatched: 2,
                open: 1
            }
        );
        assert_eq!(source_text(&m("Odd", MatchStatus::Unmatched)), "Odd");
    }

    fn account(n: u32, name: &str, tag: Option<&str>) -> cl_api::Account {
        cl_api::Account {
            id: crate::company::demo::account_id(n),
            fields: cl_api::AccountFields {
                name: name.to_string(),
                account_number: None,
                account_type: cl_api::AccountType::Equity,
                detail_type: None,
                description: None,
                parent_id: None,
            },
            is_active: true,
            is_system: tag.is_some(),
            system_tag: tag.map(str::to_string),
            balance: None,
            created_at: None,
            updated_at: None,
            card_last4: None,
            is_sensitive: false,
        }
    }

    /// U-272: the source's Retained Earnings says `create` but carries the
    /// role; it is counted as merged and the screen says where it goes.
    #[test]
    fn retained_earnings_merges_into_the_companys_own() {
        let mut re = m("32000", MatchStatus::Create);
        re.name = "Retained Earnings".into();
        re.number = Some("32000".into());
        re.system_role = Some(cl_api::imports::SystemRole::RetainedEarnings);
        let list = [m("6100", MatchStatus::Create), re.clone()];
        let c = counts(&list, |_| false);
        assert_eq!((c.created, c.merged), (1, 1));
        let own = account(901, "Retained Earnings", Some("retained_earnings"));
        let text = merge_text(&re, std::slice::from_ref(&own), None).expect("text");
        assert!(
            text.contains("32000 · Retained Earnings") && text.contains("goes into"),
            "{text}"
        );
        let none = merge_text(&re, &[], None).expect("text");
        assert!(none.contains("becomes"), "{none}");
        // A renamed one the engine missed, or this one sent elsewhere: the
        // owner's pick wins.
        let other = account(902, "Owner Equity", None);
        let to = merge_text(&re, &[own, other.clone()], Some(other.id)).expect("text");
        assert!(to.contains("Owner Equity"), "{to}");
        assert_eq!(merge_text(&m("6100", MatchStatus::Create), &[], None), None);
    }
}
