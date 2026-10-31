//! "This company" panels backed by the engine's `/company/settings`
//! (basis, fiscal year, default sales tax rate) and the local setup flags
//! (Q-P08c), plus the placeholders for settings whose engine endpoints
//! arrive with later phases (Q-P09d): check options and default accounts
//! (P13/P19), finance charges (P22), 1099 box mapping (P11/P28).

use cl_api::{AccountingMethod, CompanySettings, CompanySettingsUpdate};
use cl_core::{Rate, UserInput};
use eframe::egui::{self, Ui};

use super::{Edits, Loaded, SaveBlock};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// Month names for the fiscal-year picker.
pub const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The engine-owned accounting settings as edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountingEdits {
    /// Basis (Q-44).
    pub method: AccountingMethod,
    /// First month of the fiscal year (1-12).
    pub fiscal_year_start_month: u8,
    /// Default sales tax rate in percent, as typed (`8.25`).
    pub tax_percent: String,
    /// The income account markups on billable costs post to (Q-P20b).
    pub markup_account: Option<cl_core::ids::AccountId>,
}

impl Default for AccountingEdits {
    fn default() -> Self {
        AccountingEdits {
            method: AccountingMethod::Accrual,
            fiscal_year_start_month: 1,
            tax_percent: String::new(),
            markup_account: None,
        }
    }
}

/// `0.0825` → `8.25` (exact; trailing zeros trimmed).
#[must_use]
pub fn rate_to_percent(rate: Rate) -> String {
    let units = rate.to_ten_thousandths() * 100; // percent in ten-thousandths
    let whole = units / 10_000;
    let frac = (units % 10_000).unsigned_abs();
    let mut s = format!("{whole}.{frac:04}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

/// `8.25` (percent, up to two decimals) → `0.0825`.
///
/// # Errors
/// `BadRate` for anything else.
pub fn percent_to_rate(text: &str) -> Result<Rate, SaveBlock> {
    let t = text.trim().trim_end_matches('%').trim();
    let percent = Rate::parse_user(t, UserInput::CENTS).map_err(|_| SaveBlock::BadRate)?;
    let units = percent.to_ten_thousandths();
    if units < 0 || units % 100 != 0 {
        return Err(SaveBlock::BadRate);
    }
    Rate::from_ten_thousandths(units / 100).map_err(|_| SaveBlock::BadRate)
}

impl AccountingEdits {
    /// From the engine's settings (defaults when none).
    #[must_use]
    pub fn from_settings(s: Option<&CompanySettings>) -> AccountingEdits {
        let Some(s) = s else {
            return AccountingEdits::default();
        };
        AccountingEdits {
            method: if s.accounting_method.as_deref() == Some("cash") {
                AccountingMethod::Cash
            } else {
                AccountingMethod::Accrual
            },
            fiscal_year_start_month: s.fiscal_year_start_month.unwrap_or(1).clamp(1, 12),
            tax_percent: s
                .default_sales_tax_rate
                .filter(|r| !r.is_zero())
                .map(rate_to_percent)
                .unwrap_or_default(),
            markup_account: s.billable_markup_account_id,
        }
    }

    /// Checks the rate text.
    ///
    /// # Errors
    /// `BadRate`.
    pub fn validate(&self) -> Result<(), SaveBlock> {
        if !self.tax_percent.trim().is_empty() {
            percent_to_rate(&self.tax_percent)?;
        }
        Ok(())
    }

    /// The write for what changed against the engine's values (`None`
    /// when nothing did).
    #[must_use]
    pub fn update_for(&self, loaded: Option<&CompanySettings>) -> Option<CompanySettingsUpdate> {
        let before = AccountingEdits::from_settings(loaded);
        let same = AccountingEdits {
            markup_account: before.markup_account,
            ..self.clone()
        };
        if same == before {
            return None;
        }
        let rate = if self.tax_percent.trim().is_empty() {
            Some(Rate::ZERO)
        } else {
            percent_to_rate(&self.tax_percent).ok()
        };
        Some(CompanySettingsUpdate {
            fiscal_year_start_month: Some(self.fiscal_year_start_month),
            accounting_method: Some(self.method),
            default_sales_tax_rate: rate,
            ..CompanySettingsUpdate::default()
        })
    }
}

/// Basis, fiscal year, the closing date (shown).
pub fn accounting(ui: &mut Ui, tokens: &Tokens, edits: &mut Edits, loaded: &Loaded) {
    form::heading(ui, tokens, &t("prefs.cat.accounting"));
    form::hint(ui, tokens, &t("prefs.company.accounting_intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Label::new(t("prefs.company.basis")).selectable(false));
            ui.radio_value(
                &mut edits.accounting.method,
                AccountingMethod::Accrual,
                t("prefs.company.accrual"),
            );
            ui.radio_value(
                &mut edits.accounting.method,
                AccountingMethod::Cash,
                t("prefs.company.cash"),
            );
        });
        form::hint(ui, tokens, &t("prefs.company.basis_hint"));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let l = ui.add(egui::Label::new(t("prefs.company.fy_month")).selectable(false));
            let m = &mut edits.accounting.fiscal_year_start_month;
            egui::ComboBox::from_id_salt("prefs-fy")
                .selected_text(MONTHS[usize::from((*m).clamp(1, 12) - 1)])
                .show_ui(ui, |ui| {
                    for (i, name) in MONTHS.iter().enumerate() {
                        ui.selectable_value(m, u8::try_from(i + 1).unwrap_or(1), *name);
                    }
                })
                .response
                .labelled_by(l.id);
        });
        ui.add_space(6.0);
        let closed = loaded
            .settings
            .as_ref()
            .and_then(|s| s.lock_date)
            .map_or_else(
                || t("prefs.company.closing_none"),
                |d| {
                    t_with(
                        "prefs.company.closing_through",
                        &[("date", &crate::shell::us_date(d))],
                    )
                },
            );
        ui.add(egui::Label::new(closed).selectable(false));
        form::hint(ui, tokens, &t("prefs.company.closing_hint"));
        ui.add_space(6.0);
        // Q-79: a company preference kept on the company record.
        ui.checkbox(
            &mut edits.company.account_numbers,
            t("prefs.company.account_numbers"),
        );
        form::hint(ui, tokens, &t("prefs.company.account_numbers_hint"));
    });
}

/// Check options live in File > Printer Setup (P19); finance charges and
/// 1099 mapping arrive later.
pub fn checks_forms(ui: &mut Ui, tokens: &Tokens) {
    form::heading(ui, tokens, &t("prefs.cat.checks"));
    form::hint(ui, tokens, &t("prefs.company.checks_intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        ui.add(egui::Label::new(t("prefs.company.checks_moved")).selectable(false));
        form::hint(ui, tokens, &t("prefs.company.finance_later"));
        form::hint(ui, tokens, &t("prefs.company.ten99_later"));
    });
}

/// Sales tax on/off and the default rate.
pub fn sales_tax(ui: &mut Ui, tokens: &Tokens, edits: &mut Edits) {
    form::heading(ui, tokens, &t("prefs.cat.sales_tax"));
    form::hint(ui, tokens, &t("prefs.company.tax_intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        ui.checkbox(
            &mut edits.flags.sales_tax_enabled,
            t("prefs.company.tax_on"),
        );
        ui.add_enabled_ui(edits.flags.sales_tax_enabled, |ui| {
            form::field(
                ui,
                &t("prefs.company.tax_rate"),
                &mut edits.accounting.tax_percent,
            );
        });
        form::hint(ui, tokens, &t("prefs.company.tax_hint"));
    });
}

/// Billable costs (Q-P20b): the income account markups post to.
pub fn billable(
    ui: &mut Ui,
    tokens: &Tokens,
    edits: &mut Edits,
    incomes: &[(cl_core::ids::AccountId, String)],
) {
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        let current = edits.accounting.markup_account;
        let shown = current
            .and_then(|a| incomes.iter().find(|(id, _)| *id == a))
            .map_or_else(|| t("prefs.company.markup_none"), |(_, n)| n.clone());
        ui.horizontal(|ui| {
            let l = ui.add(egui::Label::new(t("prefs.company.markup_account")).selectable(false));
            egui::ComboBox::from_id_salt("prefs-markup-account")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut edits.accounting.markup_account,
                        None,
                        t("prefs.company.markup_none"),
                    );
                    for (id, name) in incomes {
                        ui.selectable_value(&mut edits.accounting.markup_account, Some(*id), name);
                    }
                })
                .response
                .labelled_by(l.id);
        });
        form::hint(ui, tokens, &t("prefs.company.markup_hint"));
    });
}

/// Inventory on/off.
pub fn inventory(ui: &mut Ui, tokens: &Tokens, edits: &mut Edits) {
    form::heading(ui, tokens, &t("prefs.cat.inventory"));
    form::hint(ui, tokens, &t("prefs.company.inventory_intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        ui.checkbox(
            &mut edits.flags.inventory_enabled,
            t("prefs.company.inventory_on"),
        );
        form::hint(ui, tokens, &t("prefs.company.inventory_hint"));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_and_rate_convert_exactly_both_ways() {
        let r = percent_to_rate("8.25").expect("rate");
        assert_eq!(r.to_wire(), "0.0825");
        assert_eq!(rate_to_percent(r), "8.25");
        assert_eq!(percent_to_rate(" 7 % ").expect("rate").to_wire(), "0.0700");
        assert_eq!(rate_to_percent(Rate::parse_wire("0.0700").expect("r")), "7");
        assert_eq!(
            rate_to_percent(Rate::parse_wire("0.1000").expect("r")),
            "10"
        );
        assert_eq!(rate_to_percent(Rate::ZERO), "0");
        for bad in ["8.255", "-1", "abc", "8,25"] {
            assert_eq!(percent_to_rate(bad), Err(SaveBlock::BadRate), "{bad}");
        }
    }

    #[test]
    fn settings_round_trip_and_only_changes_are_written() {
        let s = CompanySettings {
            fiscal_year_start_month: Some(7),
            accounting_method: Some("cash".into()),
            default_sales_tax_rate: Some(Rate::parse_wire("0.0825").expect("r")),
            ..serde_json::from_str("{}").expect("empty settings")
        };
        let e = AccountingEdits::from_settings(Some(&s));
        assert_eq!(e.method, AccountingMethod::Cash);
        assert_eq!(e.fiscal_year_start_month, 7);
        assert_eq!(e.tax_percent, "8.25");
        assert!(e.update_for(Some(&s)).is_none(), "unchanged");
        let mut changed = e.clone();
        changed.tax_percent = String::new();
        let u = changed.update_for(Some(&s)).expect("update");
        assert_eq!(u.default_sales_tax_rate, Some(Rate::ZERO));
        assert_eq!(u.accounting_method, Some(AccountingMethod::Cash));
        assert_eq!(
            AccountingEdits::from_settings(None),
            AccountingEdits::default()
        );
        let mut bad = e.clone();
        bad.tax_percent = "x".into();
        assert_eq!(bad.validate(), Err(SaveBlock::BadRate));
        // The markup account goes by its own write (Q-P20b).
        let mut markup = e;
        markup.markup_account = Some(
            cl_core::ids::AccountId::parse("d2000000-0000-4000-8000-000000004000").expect("id"),
        );
        assert_eq!(markup.update_for(Some(&s)), None);
        let with: CompanySettings = serde_json::from_value(serde_json::json!({
            "billableMarkupAccountId": "d2000000-0000-4000-8000-000000004000",
        }))
        .expect("settings");
        assert_eq!(
            AccountingEdits::from_settings(Some(&with)).markup_account,
            markup.markup_account
        );
    }
}
