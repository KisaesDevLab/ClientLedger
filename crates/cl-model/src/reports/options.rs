//! The owner's choices for one report (plan §4), saved as a memorized
//! report's `definition.options` (U-118). Ids sit only at the fixed paths
//! the engine's tenant export remaps: `filters.{accountIds, contactIds,
//! tagIds, itemIds}`, `tagSplit.tagGroupId`, `budgetId` (and
//! `columnTagGroupId`, see QUESTIONS Q-P28s).

use std::collections::BTreeSet;

use cl_core::CalendarDate;
use cl_core::ids::{AccountId, ContactId, ItemId, TagGroupId, TagId};
use cl_core::report::{Adjusting, Basis, Cleared, Extra, RowPath};
use serde::{Deserialize, Serialize};

use super::presets::DatePreset;
use super::spec::{BasisMode, ReportSpec};

/// The report's dates: relative (memorized reports keep "last month") or
/// fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DateChoice {
    /// A preset, resolved against today when run.
    Preset(DatePreset),
    /// Fixed dates.
    Fixed {
        /// First day.
        from: CalendarDate,
        /// Last day.
        to: CalendarDate,
    },
    /// A fixed as-of date.
    AsOf(CalendarDate),
}

/// How the columns are split.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnBy {
    /// One total column.
    Total,
    /// Calendar months.
    Month,
    /// Fiscal periods (4-4-5 aware).
    FiscalPeriod,
    /// Fiscal quarters.
    Quarter,
    /// Fiscal years.
    Year,
    /// One column per client (U-201 `compare=by_contact`).
    Client,
    /// One column per tag of a group (U-201 `compare=by_tag`).
    TagGroup,
    /// Projects: greyed until U-35.
    Project,
}

impl ColumnBy {
    /// Its i18n key.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            ColumnBy::Total => "reports.columns.total",
            ColumnBy::Month => "reports.columns.month",
            ColumnBy::FiscalPeriod => "reports.columns.fiscal_period",
            ColumnBy::Quarter => "reports.columns.quarter",
            ColumnBy::Year => "reports.columns.year",
            ColumnBy::Client => "reports.columns.client",
            ColumnBy::TagGroup => "reports.columns.tag_group",
            ColumnBy::Project => "reports.columns.project",
        }
    }
}

/// The amount filter (owner input only; never AI, Q-P28g): exact decimal
/// strings, absolute line amounts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmountFilter {
    /// Smallest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<String>,
    /// Largest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
}

/// Filters (ids at the fixed remap paths).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filters {
    /// Accounts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub account_ids: Vec<AccountId>,
    /// Clients and vendors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contact_ids: Vec<ContactId>,
    /// Tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tag_ids: Vec<TagId>,
    /// Items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub item_ids: Vec<ItemId>,
    /// Amounts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<AmountFilter>,
    /// Cleared state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared: Option<Cleared>,
    /// Transaction types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub txn_types: Vec<String>,
    /// The adjusting lens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjusting: Option<Adjusting>,
}

/// How a tag row's caption is written (formatting only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagLabel {
    /// `Account - Tag`.
    #[default]
    Append,
    /// `Tag - Account`.
    Prepend,
    /// `Tag`.
    None,
}

/// Split each account by the tags of a group (Q-P28h).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagSplit {
    /// The tag group.
    pub tag_group_id: TagGroupId,
    /// Show the account total row (off by default).
    #[serde(default)]
    pub show_account_total: bool,
    /// Tag row captions.
    #[serde(default)]
    pub label: TagLabel,
    /// Between account and tag (default `-`).
    #[serde(default = "dash")]
    pub separator: String,
}

fn dash() -> String {
    "-".to_string()
}

impl TagSplit {
    /// A tag row's caption (formatting only).
    #[must_use]
    pub fn caption(&self, account: &str, tag: &str) -> String {
        let sep = &self.separator;
        match self.label {
            TagLabel::Append => format!("{account} {sep} {tag}"),
            TagLabel::Prepend => format!("{tag} {sep} {account}"),
            TagLabel::None => tag.to_string(),
        }
    }
}

/// A client-side sort within groups (Q-P28k).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sort {
    /// Column key.
    pub column: String,
    /// Largest first.
    #[serde(default)]
    pub descending: bool,
}

/// The report header (Q-107: company, logo, basis, dates, pages, run time).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderOptions {
    /// A title of the owner's (caption rename).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Leave the logo out.
    #[serde(default)]
    pub hide_logo: bool,
    /// Leave the run time out.
    #[serde(default)]
    pub hide_run_time: bool,
}

/// The owner's options for one report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportOptions {
    /// Dates.
    pub dates: DateChoice,
    /// Basis (reports with the toggle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<Basis>,
    /// Columns.
    pub columns: ColumnBy,
    /// The group of tag columns ([`ColumnBy::TagGroup`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_tag_group_id: Option<TagGroupId>,
    /// Extra columns.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub extras: BTreeSet<Extra>,
    /// Filters.
    #[serde(default)]
    pub filters: Filters,
    /// Tag split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_split: Option<TagSplit>,
    /// Budget (Budget vs. Actual).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_id: Option<String>,
    /// Sort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Sort>,
    /// Collapsed rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collapsed: Vec<RowPath>,
    /// Header.
    #[serde(default)]
    pub header: HeaderOptions,
}

impl ReportOptions {
    /// The defaults for `spec`.
    #[must_use]
    pub fn for_spec(spec: &ReportSpec) -> ReportOptions {
        ReportOptions {
            dates: DateChoice::Preset(spec.default_dates),
            basis: match spec.basis {
                BasisMode::Toggle => Some(Basis::Accrual),
                BasisMode::NotApplicable => None,
            },
            columns: spec.default_columns(),
            column_tag_group_id: None,
            extras: spec.default_extras.iter().copied().collect(),
            filters: Filters::default(),
            tag_split: None,
            budget_id: None,
            sort: None,
            collapsed: Vec::new(),
            header: HeaderOptions::default(),
        }
    }

    /// True when a change to `other` must run the report again (collapse,
    /// sort and header are local).
    #[must_use]
    pub fn needs_run(&self, other: &ReportOptions) -> bool {
        self.dates != other.dates
            || self.basis != other.basis
            || self.columns != other.columns
            || self.column_tag_group_id != other.column_tag_group_id
            || self.extras != other.extras
            || self.filters != other.filters
            || self.tag_split.as_ref().map(|t| t.tag_group_id)
                != other.tag_split.as_ref().map(|t| t.tag_group_id)
            || self.budget_id != other.budget_id
    }
}
