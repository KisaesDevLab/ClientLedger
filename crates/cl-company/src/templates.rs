//! The four starter charts of accounts (Q-49): General service,
//! Contractor / trades, Retail / product sales, Farm / ranch (Schedule F).
//!
//! They are `ClientLedger` JSON files in `assets/coa/`, embedded at build
//! time, not engine templates (U-28 would register them upstream). A
//! company is created with the engine's system accounts only, then the
//! chart is imported with `POST /accounts/import`; the engine's seven
//! system numbers are never in a template, and an account marked
//! `requires` is imported only when that feature is on at setup.

use cl_api::{AccountImportRow, AccountType};
use serde::Deserialize;

/// Which starter chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StarterChart {
    /// Consultants, service trades, professionals.
    GeneralService,
    /// Builders and trades that do jobs.
    Contractor,
    /// Shops and online stores.
    Retail,
    /// Schedule F farms and ranches.
    Farm,
}

impl StarterChart {
    /// Every chart, in the order the interview offers them.
    pub const ALL: [StarterChart; 4] = [
        StarterChart::GeneralService,
        StarterChart::Contractor,
        StarterChart::Retail,
        StarterChart::Farm,
    ];

    /// Stable id (the file name and the settings value).
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            StarterChart::GeneralService => "general_service",
            StarterChart::Contractor => "contractor",
            StarterChart::Retail => "retail",
            StarterChart::Farm => "farm",
        }
    }

    /// Parses an id.
    #[must_use]
    pub fn from_id(id: &str) -> Option<StarterChart> {
        StarterChart::ALL.into_iter().find(|c| c.id() == id)
    }

    fn json(self) -> &'static str {
        match self {
            StarterChart::GeneralService => {
                include_str!("../../../assets/coa/general_service.json")
            }
            StarterChart::Contractor => include_str!("../../../assets/coa/contractor.json"),
            StarterChart::Retail => include_str!("../../../assets/coa/retail.json"),
            StarterChart::Farm => include_str!("../../../assets/coa/farm.json"),
        }
    }

    /// The parsed template.
    ///
    /// # Panics
    /// Never for the embedded files (a unit test parses all four).
    #[must_use]
    pub fn template(self) -> Template {
        serde_json::from_str(self.json()).expect("embedded starter chart is valid JSON")
    }
}

/// A feature an account depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requires {
    /// Inventory features on.
    Inventory,
    /// Sales tax features on.
    SalesTax,
}

/// One template account.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TemplateAccount {
    /// Account number.
    pub number: String,
    /// Name.
    pub name: String,
    /// Type.
    #[serde(rename = "type")]
    pub account_type: AccountType,
    /// Detail type, when the engine knows one.
    #[serde(default)]
    pub detail: Option<String>,
    /// Only with this feature.
    #[serde(default)]
    pub requires: Option<Requires>,
    /// Help text.
    #[serde(default)]
    pub description: Option<String>,
}

/// A starter chart file.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Template {
    /// Stable id.
    pub id: String,
    /// Shown to the owner.
    pub name: String,
    /// One-line description for the interview.
    pub description: String,
    /// The accounts.
    pub accounts: Vec<TemplateAccount>,
}

/// The engine's system account numbers (seeded by every company; never
/// imported, never overwritten).
pub const ENGINE_SYSTEM_NUMBERS: [&str; 7] = [
    "10100", "10150", "10200", "20100", "20900", "30000", "30120",
];

impl Template {
    /// Rows to import for a company with these features.
    #[must_use]
    pub fn import_rows(&self, sales_tax: bool, inventory: bool) -> Vec<AccountImportRow> {
        self.accounts
            .iter()
            .filter(|a| match a.requires {
                None => true,
                Some(Requires::Inventory) => inventory,
                Some(Requires::SalesTax) => sales_tax,
            })
            .filter(|a| !ENGINE_SYSTEM_NUMBERS.contains(&a.number.as_str()))
            .map(|a| AccountImportRow {
                account_number: Some(a.number.clone()),
                name: a.name.clone(),
                account_type: a.account_type,
                detail_type: a.detail.clone(),
            })
            .collect()
    }

    /// Account numbers must be unique (Q-209) and never a system number.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for a in &self.accounts {
            if !seen.insert(a.number.as_str()) {
                out.push(format!("duplicate number {}", a.number));
            }
            if ENGINE_SYSTEM_NUMBERS.contains(&a.number.as_str()) {
                out.push(format!("system number {}", a.number));
            }
            if a.number.is_empty() || a.number.len() > 20 || a.name.is_empty() || a.name.len() > 255
            {
                out.push(format!("bad row {}", a.number));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_four_charts_parse_with_unique_non_system_numbers() {
        for chart in StarterChart::ALL {
            let t = chart.template();
            assert_eq!(t.id, chart.id());
            assert_eq!(StarterChart::from_id(&t.id), Some(chart));
            assert!(t.accounts.len() >= 40, "{}: {}", t.id, t.accounts.len());
            assert_eq!(t.problems(), Vec::<String>::new(), "{}", t.id);
            // Every chart has a checking account, income, and expenses.
            assert!(
                t.accounts
                    .iter()
                    .any(|a| a.detail.as_deref() == Some("checking"))
            );
            assert!(
                t.accounts
                    .iter()
                    .any(|a| a.account_type == AccountType::Revenue)
            );
            assert!(
                t.accounts
                    .iter()
                    .any(|a| a.account_type == AccountType::Expense)
            );
        }
    }

    #[test]
    fn feature_flags_filter_rows() {
        let t = StarterChart::GeneralService.template();
        let plain = t.import_rows(false, false);
        let with_inventory = t.import_rows(false, true);
        assert!(with_inventory.len() > plain.len());
        assert!(!plain.iter().any(|r| r.name == "Inventory Asset"));
        assert!(with_inventory.iter().any(|r| r.name == "Inventory Asset"));
        assert!(
            plain
                .iter()
                .all(|r| r.account_number.as_deref().is_some_and(|n| n.len() <= 20))
        );
        assert!(StarterChart::from_id("nope").is_none());
    }
}
