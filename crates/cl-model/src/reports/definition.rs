//! A memorized report as the engine stores it (U-118, Q-P28d):
//! `{schemaVersion: 1, kind: 'standard', reportKey, options}`. A newer
//! `schemaVersion` is refused (the owner is asked to update), never guessed at.

use serde::{Deserialize, Serialize};

use super::options::ReportOptions;

/// The definition schema this build writes and reads.
pub const SCHEMA_VERSION: u32 = 1;

/// The engine's size cap on a definition body (64 KB).
pub const MAX_BYTES: usize = 64 * 1024;

/// Why a definition can't be used.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DefinitionError {
    /// Saved by a newer version of the app.
    #[error("this report was saved by a newer version of the app")]
    Newer,
    /// Not a standard report (a Report Builder definition, A07).
    #[error("this is not a standard report")]
    NotStandard,
    /// The report key is malformed or not in the catalog.
    #[error("unknown report")]
    UnknownReport,
}

/// `kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionKind {
    /// A catalog report with options (P28).
    Standard,
    /// A Report Builder definition (A07).
    Builder,
}

/// A standard memorized report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandardDefinition {
    /// Schema version (1).
    pub schema_version: u32,
    /// `standard`.
    pub kind: DefinitionKind,
    /// The catalog report.
    pub report_key: String,
    /// Its options.
    pub options: ReportOptions,
}

impl StandardDefinition {
    /// A new definition of `report_key` with `options`.
    #[must_use]
    pub fn new(report_key: &str, options: ReportOptions) -> StandardDefinition {
        StandardDefinition {
            schema_version: SCHEMA_VERSION,
            kind: DefinitionKind::Standard,
            report_key: report_key.to_string(),
            options,
        }
    }

    /// Checks a definition read from the engine.
    ///
    /// # Errors
    /// [`DefinitionError::Newer`] for a later schema;
    /// [`DefinitionError::NotStandard`] for a builder definition;
    /// [`DefinitionError::UnknownReport`] for a key outside the catalog.
    pub fn check(&self) -> Result<(), DefinitionError> {
        if self.schema_version > SCHEMA_VERSION {
            return Err(DefinitionError::Newer);
        }
        if self.kind != DefinitionKind::Standard {
            return Err(DefinitionError::NotStandard);
        }
        if !valid_key(&self.report_key) || super::catalog::find(&self.report_key).is_none() {
            return Err(DefinitionError::UnknownReport);
        }
        Ok(())
    }
}

/// A memorized report of either kind, routed on its `kind` (A07).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Memorized {
    /// A catalog report with options.
    Standard(Box<StandardDefinition>),
    /// A Report Builder definition.
    Builder(Box<crate::report_def::BuilderDefinition>),
}

/// Why a memorized report can't be opened.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MemorizedError {
    /// A standard definition's problem.
    #[error(transparent)]
    Standard(#[from] DefinitionError),
    /// A builder definition's problem.
    #[error(transparent)]
    Builder(#[from] crate::report_def::DefinitionError),
    /// Neither kind, or not readable.
    #[error("this memorized report can't be read")]
    Unreadable,
}

impl MemorizedError {
    /// Saved by a newer version of the app (either kind).
    #[must_use]
    pub const fn is_newer(&self) -> bool {
        matches!(
            self,
            MemorizedError::Standard(DefinitionError::Newer)
                | MemorizedError::Builder(crate::report_def::DefinitionError::Newer)
        )
    }
}

/// Reads a stored definition body: `standard` goes to
/// [`StandardDefinition`], `builder` through the strict schema v1 parse.
///
/// # Errors
/// [`MemorizedError`].
pub fn read_memorized(body: &serde_json::Value) -> Result<Memorized, MemorizedError> {
    match body.get("kind").and_then(serde_json::Value::as_str) {
        Some("standard") => {
            let def: StandardDefinition =
                serde_json::from_value(body.clone()).map_err(|_| MemorizedError::Unreadable)?;
            def.check()?;
            Ok(Memorized::Standard(Box::new(def)))
        }
        Some("builder") => {
            let text = serde_json::to_string(body).map_err(|_| MemorizedError::Unreadable)?;
            Ok(Memorized::Builder(Box::new(crate::report_def::parse(
                &text,
            )?)))
        }
        _ => Err(MemorizedError::Unreadable),
    }
}

/// `^[a-z0-9_]{1,64}$`.
#[must_use]
pub fn valid_key(k: &str) -> bool {
    (1..=64).contains(&k.len())
        && k.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reports::options::{AmountFilter, ColumnBy, DateChoice, TagLabel, TagSplit};
    use crate::reports::presets::DatePreset;
    use cl_core::report::{Basis, Extra, RowPath};

    const A: &str = "11111111-1111-4111-8111-111111111111";
    const C: &str = "22222222-2222-4222-8222-222222222222";
    const G: &str = "33333333-3333-4333-8333-333333333333";

    /// The wire shape the engine stores and its tenant export remaps: ids
    /// at `options.filters.{accountIds,contactIds,...}`,
    /// `options.tagSplit.tagGroupId` and `options.budgetId` (PR-F).
    fn fixture() -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 1,
            "kind": "standard",
            "reportKey": "profit_loss",
            "options": {
                "dates": {"preset": "last_month"},
                "basis": "accrual",
                "columns": "month",
                "extras": ["pct_of_income", "ytd"],
                "filters": {
                    "accountIds": [A],
                    "contactIds": [C],
                    "amount": {"min": "100.00"}
                },
                "tagSplit": {
                    "tagGroupId": G,
                    "showAccountTotal": true,
                    "label": "prepend",
                    "separator": "-"
                },
                "budgetId": "budget-1",
                "collapsed": [["income"]],
                "header": {"title": "Monthly P&L for the bank", "hideLogo": false, "hideRunTime": false}
            }
        })
    }

    #[test]
    fn the_wire_shape_is_pinned() {
        let def: StandardDefinition = serde_json::from_value(fixture()).expect("parse");
        def.check().expect("valid");
        let o = &def.options;
        assert_eq!(o.dates, DateChoice::Preset(DatePreset::LastMonth));
        assert_eq!(o.basis, Some(Basis::Accrual));
        assert_eq!(o.columns, ColumnBy::Month);
        assert!(o.extras.contains(&Extra::PctOfIncome) && o.extras.contains(&Extra::Ytd));
        assert_eq!(o.filters.account_ids[0].to_string(), A);
        assert_eq!(
            o.filters.amount,
            Some(AmountFilter {
                min: Some("100.00".into()),
                max: None
            })
        );
        let ts = o.tag_split.as_ref().expect("split");
        assert_eq!(ts.tag_group_id.to_string(), G);
        assert_eq!(ts.label, TagLabel::Prepend);
        assert_eq!(o.collapsed, vec![RowPath::of(["income"])]);
        // And back to exactly the same JSON.
        assert_eq!(serde_json::to_value(&def).expect("json"), fixture());
    }

    #[test]
    fn a_round_trip_keeps_everything() {
        let spec = crate::reports::find("balance_sheet").expect("spec");
        let mut o = crate::reports::ReportOptions::for_spec(spec);
        o.dates = DateChoice::AsOf(cl_core::CalendarDate::from_ymd(2026, 10, 31).expect("d"));
        o.tag_split = Some(TagSplit {
            tag_group_id: G.parse().expect("id"),
            show_account_total: false,
            label: TagLabel::None,
            separator: "/".into(),
        });
        let def = StandardDefinition::new("balance_sheet", o);
        let text = serde_json::to_string(&def).expect("json");
        assert!(text.len() < MAX_BYTES);
        let back: StandardDefinition = serde_json::from_str(&text).expect("back");
        assert_eq!(back, def);
    }

    #[test]
    fn a_newer_schema_or_another_kind_is_refused() {
        let mut v = fixture();
        v["schemaVersion"] = serde_json::json!(2);
        let def: StandardDefinition = serde_json::from_value(v).expect("parse");
        assert_eq!(def.check(), Err(DefinitionError::Newer));
        let mut v = fixture();
        v["kind"] = serde_json::json!("builder");
        let def: StandardDefinition = serde_json::from_value(v).expect("parse");
        assert_eq!(def.check(), Err(DefinitionError::NotStandard));
        let mut v = fixture();
        v["reportKey"] = serde_json::json!("Profit-Loss");
        let def: StandardDefinition = serde_json::from_value(v).expect("parse");
        assert_eq!(def.check(), Err(DefinitionError::UnknownReport));
    }

    #[test]
    fn memorized_bodies_route_on_their_kind() {
        assert!(matches!(
            read_memorized(&fixture()),
            Ok(Memorized::Standard(_))
        ));
        let builder: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/report_def/ar_aging_summary.json"
        ))
        .expect("json");
        assert!(matches!(
            read_memorized(&builder),
            Ok(Memorized::Builder(_))
        ));
        let mut newer = builder.clone();
        newer["schemaVersion"] = serde_json::json!(9);
        assert!(read_memorized(&newer).expect_err("newer").is_newer());
        let mut newer = fixture();
        newer["schemaVersion"] = serde_json::json!(9);
        assert!(read_memorized(&newer).expect_err("newer").is_newer());
        assert_eq!(
            read_memorized(&serde_json::json!({"kind": "chart"})),
            Err(MemorizedError::Unreadable)
        );
    }

    #[test]
    fn keys_follow_the_engine_pattern() {
        assert!(valid_key("profit_loss"));
        assert!(!valid_key(""));
        assert!(!valid_key("P&L"));
        assert!(!valid_key(&"a".repeat(65)));
    }
}
