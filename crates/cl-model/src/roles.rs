//! Roles on the Users and Roles screens (P29 WP3, plan §1): what each
//! user's role reads as, the right grid grouped the way the engine groups
//! its permission matrix, and the key map a template or override sends.
//!
//! The engine refuses a permission map with a key it does not know, so
//! [`wire_map`] sends only keys the running engine has: `financial_reports`
//! with `report-areas-v1` (U-227), `sensitive_accounts` with
//! `sensitive-accounts-v1` (U-33). (There is no `item_costs`: U-228 was cut
//! and inventory value follows `items`.)

use std::collections::BTreeMap;

use crate::permissions::{BuiltinRole, Level, Resource};

/// The engine key of the reserved "See sensitive accounts" row (U-33).
pub const SENSITIVE_KEY: &str = "sensitive_accounts";

/// The engine's permission groups (`PERMISSION_GROUPS`), in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// Journal entries, Bulk Entry, templates, imports.
    Transactions,
    /// Invoices, payments, estimates, items, sales tax.
    Sales,
    /// Bills, Pay Bills, vendor credits.
    Payables,
    /// Payroll import.
    Payroll,
    /// Checks.
    Checks,
    /// Bank feed, deposits, reconcile, daily sales.
    Banking,
    /// Reports, budgets, Home figures.
    Reporting,
    /// The accountant's trial balance tools.
    TrialBalance,
    /// Lists, settings, attachments, audit trail.
    Manage,
}

impl Group {
    /// Every group, in the engine's order.
    pub const ALL: &'static [Group] = &[
        Group::Transactions,
        Group::Sales,
        Group::Payables,
        Group::Payroll,
        Group::Checks,
        Group::Banking,
        Group::Reporting,
        Group::TrialBalance,
        Group::Manage,
    ];

    /// The i18n key of the group's heading.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Group::Transactions => "roles.group.transactions",
            Group::Sales => "roles.group.sales",
            Group::Payables => "roles.group.payables",
            Group::Payroll => "roles.group.payroll",
            Group::Checks => "roles.group.checks",
            Group::Banking => "roles.group.banking",
            Group::Reporting => "roles.group.reporting",
            Group::TrialBalance => "roles.group.trial_balance",
            Group::Manage => "roles.group.manage",
        }
    }
}

/// The group a right shows under (every right has one; `Option` kept for
/// callers).
#[must_use]
pub const fn group_of(r: Resource) -> Option<Group> {
    use Resource as R;
    Some(match r {
        R::Transactions | R::BatchEntry | R::Recurring | R::Duplicates | R::BulkImport => {
            Group::Transactions
        }
        R::Invoices | R::ReceivePayment | R::Estimates | R::Items | R::SalesTax => Group::Sales,
        R::Bills | R::PayBills | R::VendorCredits => Group::Payables,
        R::PayrollImport => Group::Payroll,
        R::Checks => Group::Checks,
        R::Banking | R::DailySales => Group::Banking,
        R::Reports | R::FinancialReports | R::Budgets | R::Dashboard => Group::Reporting,
        R::TrialBalance | R::TaxEstimator => Group::TrialBalance,
        R::Accounts
        | R::Contacts
        | R::Attachments
        | R::Tags
        | R::CompanySettings
        | R::AiChat
        | R::AuditLog => Group::Manage,
    })
}

/// The i18n key of a right's row label.
#[must_use]
pub fn right_key(r: Resource) -> String {
    format!("roles.right.{}", r.key())
}

/// Which optional keys the running engine knows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineKeys {
    /// `report-areas-v1`: `financial_reports` exists.
    pub report_areas: bool,
    /// `sensitive-accounts-v1`: `sensitive_accounts` exists.
    pub sensitive: bool,
    /// `sensitive-accounts-grant-v1`: a role may carry the right (owner
    /// 2026-11-17: the built-in Bookkeeper does).
    pub sensitive_grant: bool,
}

impl EngineKeys {
    /// True when the engine has this right's key (and the grid shows it).
    #[must_use]
    pub fn knows(self, r: Resource) -> bool {
        match r {
            Resource::FinancialReports => self.report_areas,
            _ => true,
        }
    }
}

/// The highest level a right takes (view-only surfaces stop at view).
#[must_use]
pub fn cap(r: Resource, l: Level) -> Level {
    if r.writable() { l } else { l.min(Level::View) }
}

/// The key map sent for a template: every right the engine knows (an
/// absent key is `none` there), view-only rights capped at view, and the
/// sensitive row only when the engine has it (none or view).
#[must_use]
pub fn wire_map(
    levels: &[(Resource, Level)],
    sensitive: Level,
    keys: EngineKeys,
) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = levels
        .iter()
        .filter(|(r, _)| keys.knows(*r))
        .map(|(r, l)| (r.key().to_owned(), cap(*r, *l).key().to_owned()))
        .collect();
    if keys.sensitive {
        out.insert(
            SENSITIVE_KEY.to_owned(),
            sensitive.min(Level::View).key().to_owned(),
        );
    }
    out
}

/// Reads a template's map back into the grid: an absent key is `none`,
/// except a right with `inheritsFrom`, which takes that right's level (as
/// the engine resolves it). Returns the grid and the sensitive level.
#[must_use]
pub fn levels_from_wire(map: &BTreeMap<String, String>) -> (Vec<(Resource, Level)>, Level) {
    let get = |r: Resource| map.get(r.key()).map(|s| Level::parse(s));
    let levels = Resource::ALL
        .iter()
        .map(|r| {
            let own = get(*r).or_else(|| r.inherits_from().and_then(get));
            (*r, cap(*r, own.unwrap_or(Level::None)))
        })
        .collect();
    let sensitive = map
        .get(SENSITIVE_KEY)
        .map_or(Level::None, |s| Level::parse(s).min(Level::View));
    (levels, sensitive)
}

/// The built-in role a template name stands for (Q-P29v).
#[must_use]
pub fn builtin_by_name(name: &str) -> Option<BuiltinRole> {
    BuiltinRole::ALL
        .iter()
        .copied()
        .find(|b| b.template_name() == name.trim())
}

/// The map a built-in template must hold on this engine.
#[must_use]
pub fn builtin_wire(b: BuiltinRole, keys: EngineKeys) -> BTreeMap<String, String> {
    let sensitive = if keys.sensitive_grant {
        b.sensitive()
    } else {
        Level::None
    };
    wire_map(&b.levels(), sensitive, keys)
}

/// The put-back after an Admin signs in (owner 2026-11-17, Q-P29ac): the
/// map to send for built-in `b`'s stored template, or None to leave it.
///
/// Only on an engine with `sensitive-accounts-grant-v1`, and only when the
/// stored map is what the built-in table gives apart from the
/// `sensitive_accounts` key (an absent key reads as `none`, as the engine
/// resolves it) and that key differs. Any other difference (an older
/// engine's map, a hand edit) is left for Users and Roles; a custom role
/// or a copy never has a built-in's name, so it is never passed here.
#[must_use]
pub fn sensitive_putback(
    b: BuiltinRole,
    stored: &BTreeMap<String, String>,
    keys: EngineKeys,
) -> Option<BTreeMap<String, String>> {
    if !keys.sensitive_grant {
        return None;
    }
    let want = builtin_wire(b, keys);
    let level = |m: &BTreeMap<String, String>| {
        m.get(SENSITIVE_KEY)
            .map_or(Level::None, |s| Level::parse(s).min(Level::View))
    };
    let rest = |m: &BTreeMap<String, String>| {
        let mut m = m.clone();
        m.remove(SENSITIVE_KEY);
        m
    };
    (level(stored) != level(&want) && rest(stored) == rest(&want)).then_some(want)
}

/// What a user's role reads as on the Users list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleKind {
    /// Engine `owner` (or the super admin).
    Admin,
    /// Engine `readonly`.
    ViewOnly,
    /// Engine `accountant` (only a transfer from the online books brings one in, Q-P29k).
    FullNotAdmin,
    /// A `bookkeeper` with no permission row: full rights until a role is
    /// chosen (plan §1).
    NoRow,
    /// A `bookkeeper` on a built-in template.
    Builtin(BuiltinRole),
    /// A `bookkeeper` on a custom template (its name), or on overrides
    /// only (None).
    Custom(Option<String>),
    /// Any other engine word.
    Other(String),
}

impl RoleKind {
    /// The i18n key of the role's name (Custom and Other add their own
    /// text).
    #[must_use]
    pub fn key(&self) -> &'static str {
        match self {
            RoleKind::Admin => "roles.kind.admin",
            RoleKind::ViewOnly => "roles.kind.view_only",
            RoleKind::FullNotAdmin => "roles.kind.full_not_admin",
            RoleKind::NoRow => "roles.kind.no_row",
            RoleKind::Builtin(BuiltinRole::Bookkeeper) => "roles.kind.bookkeeper",
            RoleKind::Builtin(BuiltinRole::Receivables) => "roles.kind.receivables",
            RoleKind::Builtin(BuiltinRole::Payables) => "roles.kind.payables",
            RoleKind::Custom(_) => "roles.kind.custom",
            RoleKind::Other(_) => "roles.kind.other",
        }
    }
}

/// Classifies a user: `role` is the engine word; `row` is None without a
/// permission row, else the name of its template (None: no template).
#[must_use]
pub fn classify(role: &str, row: Option<Option<&str>>) -> RoleKind {
    match role {
        "owner" | "super_admin" => RoleKind::Admin,
        "readonly" => RoleKind::ViewOnly,
        "accountant" => RoleKind::FullNotAdmin,
        "bookkeeper" => match row {
            None => RoleKind::NoRow,
            Some(Some(name)) => builtin_by_name(name).map_or_else(
                || RoleKind::Custom(Some(name.to_owned())),
                RoleKind::Builtin,
            ),
            Some(None) => RoleKind::Custom(None),
        },
        other => RoleKind::Other(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_KEYS: EngineKeys = EngineKeys {
        report_areas: true,
        sensitive: true,
        sensitive_grant: true,
    };

    #[test]
    fn every_right_has_a_group() {
        for r in Resource::ALL {
            assert!(group_of(*r).is_some(), "{r:?}");
            assert!(ALL_KEYS.knows(*r), "{r:?}");
        }
        assert_eq!(Group::ALL.len(), 9);
    }

    /// The engine refuses unknown keys: `item_costs` is never sent, the
    /// areas key only with U-227, the sensitive row only with U-33.
    /// Owner 2026-11-17: the Bookkeeper template carries
    /// `sensitive_accounts: view` on an engine with
    /// `sensitive-accounts-grant-v1`; A/R and A/P carry none; an engine
    /// without the grant capability gets none for every built-in.
    #[test]
    fn the_bookkeeper_template_sees_sensitive_accounts() {
        let bk = builtin_wire(BuiltinRole::Bookkeeper, ALL_KEYS);
        assert_eq!(bk[SENSITIVE_KEY], "view");
        for b in [BuiltinRole::Receivables, BuiltinRole::Payables] {
            assert_eq!(builtin_wire(b, ALL_KEYS)[SENSITIVE_KEY], "none", "{b:?}");
        }
        let older = EngineKeys {
            sensitive_grant: false,
            ..ALL_KEYS
        };
        assert_eq!(
            builtin_wire(BuiltinRole::Bookkeeper, older)[SENSITIVE_KEY],
            "none"
        );
        assert!(
            !builtin_wire(BuiltinRole::Bookkeeper, EngineKeys::default())
                .contains_key(SENSITIVE_KEY)
        );
    }

    /// Q-P29ac: the sign-in put-back touches only an old-shape built-in.
    #[test]
    fn the_sign_in_putback_adds_the_key_to_an_old_shape_template_only() {
        let old_keys = EngineKeys {
            sensitive_grant: false,
            ..ALL_KEYS
        };
        let want = builtin_wire(BuiltinRole::Bookkeeper, ALL_KEYS);
        // Old shape (`none`, or no key at all): gets the key.
        let old = builtin_wire(BuiltinRole::Bookkeeper, old_keys);
        assert_eq!(
            sensitive_putback(BuiltinRole::Bookkeeper, &old, ALL_KEYS),
            Some(want.clone())
        );
        let mut absent = old.clone();
        absent.remove(SENSITIVE_KEY);
        assert_eq!(
            sensitive_putback(BuiltinRole::Bookkeeper, &absent, ALL_KEYS),
            Some(want.clone())
        );
        // Already updated: left alone.
        assert_eq!(
            sensitive_putback(BuiltinRole::Bookkeeper, &want, ALL_KEYS),
            None
        );
        // Differs elsewhere (a hand edit): left alone.
        let mut edited = old.clone();
        edited.insert("bills".into(), "view".into());
        assert_eq!(
            sensitive_putback(BuiltinRole::Bookkeeper, &edited, ALL_KEYS),
            None
        );
        // An engine without the grant capability: nothing.
        assert_eq!(
            sensitive_putback(BuiltinRole::Bookkeeper, &old, old_keys),
            None
        );
        // A/R and A/P: an absent key already reads as none.
        for b in [BuiltinRole::Receivables, BuiltinRole::Payables] {
            let mut m = builtin_wire(b, ALL_KEYS);
            assert_eq!(sensitive_putback(b, &m, ALL_KEYS), None, "{b:?}");
            m.remove(SENSITIVE_KEY);
            assert_eq!(sensitive_putback(b, &m, ALL_KEYS), None, "{b:?}");
        }
    }

    #[test]
    fn the_wire_map_sends_only_keys_the_engine_knows() {
        let levels = BuiltinRole::Payables.levels();
        let old = wire_map(&levels, Level::View, EngineKeys::default());
        assert!(!old.contains_key("item_costs"));
        assert!(!old.contains_key("financial_reports"));
        assert!(!old.contains_key(SENSITIVE_KEY));
        assert_eq!(old["bills"], "full");
        assert_eq!(
            old["invoices"], "none",
            "absent would also be none; sent anyway"
        );
        let new = wire_map(&levels, Level::Full, ALL_KEYS);
        assert_eq!(
            new["financial_reports"], "none",
            "A/P has no financial reports"
        );
        assert_eq!(new[SENSITIVE_KEY], "view", "capped at view");
        assert!(!new.contains_key("item_costs"));
        // View-only rights never go above view.
        let full: Vec<_> = Resource::ALL.iter().map(|r| (*r, Level::Full)).collect();
        let m = wire_map(&full, Level::None, ALL_KEYS);
        assert_eq!(m["reports"], "view");
        assert_eq!(m["audit_log"], "view");
        assert_eq!(m["bills"], "full");
    }

    #[test]
    fn a_map_reads_back_with_the_engines_inheritance() {
        let mut map = BTreeMap::new();
        map.insert("reports".to_owned(), "view".to_owned());
        map.insert("bills".to_owned(), "full".to_owned());
        let (levels, sensitive) = levels_from_wire(&map);
        let level = |r| levels.iter().find(|(x, _)| *x == r).map(|(_, l)| *l);
        assert_eq!(level(Resource::Bills), Some(Level::Full));
        assert_eq!(level(Resource::Invoices), Some(Level::None));
        assert_eq!(
            level(Resource::FinancialReports),
            Some(Level::View),
            "inherits reports"
        );
        assert_eq!(sensitive, Level::None);
        // Round trip of every built-in.
        for b in BuiltinRole::ALL {
            let sent = builtin_wire(*b, ALL_KEYS);
            let (back, s) = levels_from_wire(&sent);
            assert_eq!(s, b.sensitive(), "{b:?}");
            assert_eq!(wire_map(&back, s, ALL_KEYS), sent, "{b:?}");
        }
    }

    #[test]
    fn roles_read_in_owner_words() {
        assert_eq!(classify("owner", None), RoleKind::Admin);
        assert_eq!(classify("readonly", Some(Some("x"))), RoleKind::ViewOnly);
        assert_eq!(classify("accountant", None), RoleKind::FullNotAdmin);
        assert_eq!(classify("bookkeeper", None), RoleKind::NoRow);
        assert_eq!(
            classify("bookkeeper", Some(Some("Accounts payable"))),
            RoleKind::Builtin(BuiltinRole::Payables)
        );
        assert_eq!(
            classify("bookkeeper", Some(Some("Front desk"))),
            RoleKind::Custom(Some("Front desk".into()))
        );
        assert_eq!(classify("bookkeeper", Some(None)), RoleKind::Custom(None));
        assert_eq!(
            builtin_by_name(" Bookkeeper "),
            Some(BuiltinRole::Bookkeeper)
        );
        assert_eq!(builtin_by_name("bookkeeper"), None, "names match exactly");
    }
}
