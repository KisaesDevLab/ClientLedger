//! Profit & Loss, Balance Sheet and Trial Balance from the exact routes
//! (U-14, `reports-exact-v1`). Account rows keep the engine's tree
//! (`parentAccountId`, `depth`): a parent row carries its own amount, its
//! sub-accounts sit under it (so collapse hides them, Q-P28j) and a
//! "Total" row beside it carries the engine's `rollupExact`. Section
//! totals, gross profit, net income and the sheet's totals are the engine's
//! figures; nothing here adds them up.

use cl_core::CalendarDate;
use cl_core::ids::{AccountId, TagId};
use cl_core::report::{
    Cell, ColumnHead, ColumnKind, DateRange, Drill, ReportMeta, ReportRequest, ReportRow,
    ReportTable, RowKind, RowPath, TagScope,
};
use serde_json::Value;

use super::exact::{
    account_caption, amount, amount_opt, amount_or_null, cell, id, list, meta_basis,
    meta_cash_rules, require_exact, text,
};
use super::row;
use crate::errors::ApiError;

/// The tag scope a drill carries from the report's own tag filter (one tag
/// at most: a drill can't name several).
pub(crate) fn drill_tag(req: &ReportRequest) -> Option<TagScope> {
    match req.filters.tag_ids.as_slice() {
        [] => Some(TagScope::Any),
        [t] => Some(TagScope::Tag(*t)),
        _ => None,
    }
}

/// The lines behind `ids` over `from..=to` with the report's basis, lens
/// and tag (`None` when the report's filters can't be carried).
pub(crate) fn lines(
    req: &ReportRequest,
    ids: Vec<AccountId>,
    from: Option<CalendarDate>,
    to: CalendarDate,
) -> Option<Drill> {
    if ids.is_empty() {
        return None;
    }
    Some(Drill::Lines {
        account_ids: ids,
        from,
        to,
        contact: None,
        tag: drill_tag(req)?,
        basis: req.basis,
        adjusting: req.filters.adjusting,
    })
}

/// A section of account rows (P&L, Balance Sheet): the engine's tree in
/// its order, parent totals beside their parents.
struct Section<'r> {
    req: &'r ReportRequest,
    key: &'static str,
    amount_key: &'static str,
    from: Option<CalendarDate>,
    to: CalendarDate,
    rows: Vec<ReportRow>,
    /// Every account id in the section (the subtotal's drill).
    ids: Vec<AccountId>,
}

/// One open parent: its depth, id, caption, rollup and its subtree's ids.
struct Open {
    depth: usize,
    id: String,
    caption: String,
    rollup: Option<rust_decimal::Decimal>,
    ids: Vec<AccountId>,
}

impl Section<'_> {
    fn path(&self, stack: &[Open], last: &str) -> RowPath {
        let mut p = RowPath::of([self.key]);
        for o in stack {
            p = p.child(o.id.clone());
        }
        p.child(last)
    }

    /// Closes open parents deeper than or as deep as `depth`, writing each
    /// one's total row.
    fn close(&mut self, stack: &mut Vec<Open>, depth: usize) {
        while stack.last().is_some_and(|o| o.depth >= depth) {
            let Some(o) = stack.pop() else { break };
            let Some(rollup) = o.rollup else { continue };
            let mut r = row(
                RowKind::AccountTotal,
                u8::try_from(o.depth + 1).unwrap_or(u8::MAX),
                self.path(stack, &format!("{}~total", o.id)),
                // `@key argument`: the window fills the account's name in.
                format!("@reports.row.total_of {}", o.caption),
                vec![Cell::Amount(rollup)],
            );
            r.drill = lines(self.req, o.ids.clone(), self.from, self.to);
            if let Some(parent) = stack.last_mut() {
                parent.ids.extend(o.ids);
            }
            self.rows.push(r);
        }
    }

    fn entries(
        &mut self,
        entries: &[Value],
        named: &dyn Fn(&Value) -> String,
    ) -> Result<(), ApiError> {
        let mut stack: Vec<Open> = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            let depth = e.get("depth").and_then(Value::as_u64).unwrap_or(0);
            let depth = usize::try_from(depth).unwrap_or(0);
            self.close(&mut stack, depth);
            let account = id(e, "accountId", AccountId::parse)?;
            let key = account.map_or_else(|| format!("row{i}"), |a| a.to_string());
            let caption = named(e);
            let rollup = amount_opt(e, "rollupExact")?;
            let mut r = row(
                RowKind::Account,
                u8::try_from(depth + 1).unwrap_or(u8::MAX),
                self.path(&stack, &key),
                caption.clone(),
                vec![cell(e, self.amount_key)?],
            );
            r.expandable = rollup.is_some();
            r.drill = account.and_then(|a| lines(self.req, vec![a], self.from, self.to));
            self.rows.push(r);
            let own: Vec<AccountId> = account.into_iter().collect();
            self.ids.extend(own.iter().copied());
            if rollup.is_some() {
                stack.push(Open {
                    depth,
                    id: key,
                    caption,
                    rollup,
                    ids: own,
                });
            } else if let Some(parent) = stack.last_mut() {
                parent.ids.extend(own);
            }
        }
        self.close(&mut stack, 0);
        Ok(())
    }
}

fn header(key: &str, caption: &str) -> ReportRow {
    let mut r = row(
        RowKind::Header,
        0,
        RowPath::of([key]),
        caption.to_string(),
        vec![Cell::Empty],
    );
    r.expandable = true;
    r
}

fn total(kind: RowKind, key: &str, caption: &str, value: Cell, drill: Option<Drill>) -> ReportRow {
    let mut r = row(
        kind,
        0,
        RowPath::of([key]),
        caption.to_string(),
        vec![value],
    );
    r.drill = drill;
    r
}

fn named(e: &Value) -> String {
    account_caption(text(e, "accountNumber"), text(e, "name"))
}

/// The P&L sections: engine key, total key, row key, captions.
const PL_SECTIONS: [(&str, &str, &str, &str, &str); 5] = [
    (
        "revenue",
        "totalRevenueExact",
        "income",
        "@reports.pl.income",
        "@reports.pl.total_income",
    ),
    (
        "cogs",
        "totalCogsExact",
        "cogs",
        "@reports.pl.cogs",
        "@reports.pl.total_cogs",
    ),
    (
        "expenses",
        "totalExpensesExact",
        "expenses",
        "@reports.pl.expenses",
        "@reports.pl.total_expenses",
    ),
    (
        "otherRevenue",
        "totalOtherRevenueExact",
        "other_income",
        "@reports.pl.other_income",
        "@reports.pl.total_other_income",
    ),
    (
        "otherExpenses",
        "totalOtherExpensesExact",
        "other_expenses",
        "@reports.pl.other_expenses",
        "@reports.pl.total_other_expenses",
    ),
];

fn section_key(engine: &str) -> &'static str {
    PL_SECTIONS
        .iter()
        .find(|s| s.0 == engine)
        .map_or("section", |s| s.2)
}

/// Profit & Loss over the request's range, one Total column.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
pub fn profit_loss(
    v: &Value,
    req: &ReportRequest,
    range: DateRange,
) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    let mut rows = Vec::new();
    for (engine, total_key, key, title, total_title) in PL_SECTIONS {
        let entries = list(v, engine)?;
        let total_value = amount(v, total_key)?;
        if !entries.is_empty() {
            let mut s = Section {
                req,
                key: section_key(engine),
                amount_key: "amountExact",
                from: Some(range.from),
                to: range.to,
                rows: Vec::new(),
                ids: Vec::new(),
            };
            s.entries(entries, &named)?;
            rows.push(header(key, title));
            rows.append(&mut s.rows);
            let drill = if s.ids.len() <= MAX_DRILL_ACCOUNTS {
                lines(req, s.ids, Some(range.from), range.to)
            } else {
                None
            };
            rows.push(total(
                RowKind::Subtotal,
                &format!("{key}~total"),
                total_title,
                Cell::Amount(total_value),
                drill,
            ));
        }
        match engine {
            "cogs" => {
                if let Some(g) = amount_or_null(v, "grossProfitExact")? {
                    rows.push(total(
                        RowKind::Total,
                        "gross_profit",
                        "@reports.pl.gross_profit",
                        Cell::Amount(g),
                        None,
                    ));
                }
            }
            "expenses" => {
                if let Some(o) = amount_or_null(v, "operatingIncomeExact")? {
                    rows.push(total(
                        RowKind::Total,
                        "operating_income",
                        "@reports.pl.operating_income",
                        Cell::Amount(o),
                        None,
                    ));
                }
            }
            _ => {}
        }
    }
    rows.push(total(
        RowKind::Grand,
        "net_income",
        "@reports.pl.net_income",
        cell(v, "netIncomeExact")?,
        None,
    ));
    Ok(ReportTable {
        columns: vec![amount_col("total", "@reports.col.total", range)],
        rows,
        meta: meta(v, req),
    })
}

/// A subtotal drills when it names at most this many accounts (plan §4).
pub const MAX_DRILL_ACCOUNTS: usize = 500;

fn amount_col(key: &str, caption: &str, range: DateRange) -> ColumnHead {
    ColumnHead {
        key: key.to_string(),
        caption: caption.to_string(),
        kind: ColumnKind::Amount,
        range: Some(range),
        drill: None,
    }
}

fn meta(v: &Value, req: &ReportRequest) -> ReportMeta {
    ReportMeta {
        basis: meta_basis(v).or(req.basis),
        dates: Some(req.dates.clone()),
        cash_basis_rules: meta_cash_rules(v),
        ..ReportMeta::default()
    }
}

/// The engine's name of the current year's net income row on the sheet.
const BS_NET_INCOME: &str = "Net Income (Current Year)";
/// The engine's name of the prior years' row without a Retained Earnings
/// account.
const BS_RETAINED: &str = "Retained Earnings (Prior Years)";

/// The fiscal year's first day for `to` (the request's, or January 1).
fn year_start(req: &ReportRequest, to: CalendarDate) -> CalendarDate {
    req.fiscal_year_start
        .or_else(|| CalendarDate::from_ymd(to.year(), 1, 1).ok())
        .unwrap_or(to)
}

/// Balance Sheet as of `as_of`, one balance column.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
pub fn balance_sheet(
    v: &Value,
    req: &ReportRequest,
    as_of: CalendarDate,
) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    let fy = year_start(req, as_of);
    let bs_named = |e: &Value| -> String {
        match (e.get("accountId"), text(e, "name")) {
            (Some(Value::Null) | None, Some(BS_NET_INCOME)) => "@reports.bs.net_income".into(),
            (Some(Value::Null) | None, Some(BS_RETAINED)) => "@reports.bs.retained_prior".into(),
            _ => named(e),
        }
    };
    let mut rows = Vec::new();
    let sections: [(&'static str, &'static str, &str, &str, RowKind); 3] = [
        (
            "assets",
            "totalAssetsExact",
            "@reports.bs.assets",
            "@reports.bs.total_assets",
            RowKind::Total,
        ),
        (
            "liabilities",
            "totalLiabilitiesExact",
            "@reports.bs.liabilities",
            "@reports.bs.total_liabilities",
            RowKind::Subtotal,
        ),
        (
            "equity",
            "totalEquityExact",
            "@reports.bs.equity",
            "@reports.bs.total_equity",
            RowKind::Subtotal,
        ),
    ];
    for (key, total_key, title, total_title, kind) in sections {
        let entries = list(v, key)?;
        let total_value = amount(v, total_key)?;
        let mut s = Section {
            req,
            key,
            amount_key: "balanceExact",
            from: None,
            to: as_of,
            rows: Vec::new(),
            ids: Vec::new(),
        };
        s.entries(entries, &bs_named)?;
        // The current year's net income opens the fiscal year-to-date P&L;
        // the prior years' row has no drill.
        for r in &mut s.rows {
            if r.caption == "@reports.bs.net_income" {
                r.drill = Some(Drill::ProfitLoss {
                    from: fy,
                    to: as_of,
                    basis: req.basis,
                });
            }
        }
        rows.push(header(key, title));
        rows.append(&mut s.rows);
        let drill = if s.ids.len() <= MAX_DRILL_ACCOUNTS {
            lines(req, s.ids, None, as_of)
        } else {
            None
        };
        rows.push(total(
            kind,
            &format!("{key}~total"),
            total_title,
            Cell::Amount(total_value),
            drill,
        ));
    }
    rows.push(total(
        RowKind::Grand,
        "liabilities_and_equity",
        "@reports.bs.total_liabilities_and_equity",
        cell(v, "totalLiabilitiesAndEquityExact")?,
        None,
    ));
    Ok(ReportTable {
        columns: vec![amount_col(
            "balance",
            "@reports.col.balance",
            DateRange {
                from: as_of,
                to: as_of,
            },
        )],
        rows,
        meta: meta(v, req),
    })
}

/// Account types whose balance restarts each fiscal year.
fn is_pl_type(t: Option<&str>) -> bool {
    matches!(
        t,
        Some("revenue" | "other_revenue" | "cogs" | "expense" | "other_expense")
    )
}

/// Trial Balance as of `as_of`: Debit and Credit, every account flat
/// (indented by depth; no roll-ups, which would need a side), and the
/// engine's totals.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
pub fn trial_balance(
    v: &Value,
    req: &ReportRequest,
    as_of: CalendarDate,
) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    let fy = year_start(req, as_of);
    let range = DateRange {
        from: fy,
        to: as_of,
    };
    let mut rows = Vec::new();
    // Under the tag split (U-201) each account is its untagged row, its tag
    // rows and its total (the unsplit row), in the engine's order.
    let group = req.tag_split;
    let mut current = String::new();
    for (i, e) in list(v, "data")?.iter().enumerate() {
        let account = id(e, "accountId", AccountId::parse)?;
        let depth = e.get("depth").and_then(Value::as_u64).unwrap_or(0);
        let level = u8::try_from(depth + 1).unwrap_or(u8::MAX);
        let name = account_caption(text(e, "account_number"), text(e, "name"));
        let cells = vec![cell(e, "debitExact")?, cell(e, "creditExact")?];
        // Income and expense accounts count from the fiscal year's start;
        // balance accounts from the first entry.
        let from = is_pl_type(text(e, "account_type")).then_some(fy);
        let drill = |tag: TagScope| {
            account
                .and_then(|a| lines(req, vec![a], from, as_of))
                .map(|mut d| {
                    if let Drill::Lines { tag: t, .. } = &mut d {
                        *t = tag;
                    }
                    d
                })
        };
        match (text(e, "rowKind"), group) {
            (Some("tag"), Some(_)) => {
                let tag = id(e, "tagId", TagId::parse)?;
                let mut r = row(
                    RowKind::TagRow,
                    level.saturating_add(1),
                    RowPath::of(["accounts", current.as_str()]).child(format!(
                        "t:{}",
                        tag.map(|t| t.to_string()).unwrap_or_default()
                    )),
                    account_caption(text(e, "tagNumber"), text(e, "tagName")),
                    cells,
                );
                r.drill = tag.and_then(|t| drill(TagScope::Tag(t)));
                rows.push(r);
            }
            (Some("account_total"), Some(_)) => {
                let mut r = row(
                    RowKind::AccountTotal,
                    level,
                    RowPath::of(["accounts", format!("{current}~split").as_str()]),
                    format!("@reports.row.total_of {name}"),
                    cells,
                );
                r.drill = drill(TagScope::Any);
                rows.push(r);
            }
            _ => {
                current = account.map_or_else(|| format!("row{i}"), |a| a.to_string());
                let mut r = row(
                    RowKind::Account,
                    level,
                    RowPath::of(["accounts", current.as_str()]),
                    name,
                    cells,
                );
                r.expandable = group.is_some();
                r.drill = match group {
                    Some(g) => drill(TagScope::UntaggedInGroup(g)),
                    None => account.and_then(|a| lines(req, vec![a], from, as_of)),
                };
                rows.push(r);
            }
        }
    }
    let mut grand = row(
        RowKind::Grand,
        0,
        RowPath::of(["total"]),
        "@reports.row.total".into(),
        vec![cell(v, "totalDebitsExact")?, cell(v, "totalCreditsExact")?],
    );
    grand.drill = None;
    rows.push(grand);
    Ok(ReportTable {
        columns: vec![
            amount_col("debit", "@reports.col.debit", range),
            amount_col("credit", "@reports.col.credit", range),
        ],
        rows,
        meta: meta(v, req),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::ids::TagId;
    use cl_core::report::{Basis, ReportDates, Source};

    fn tag(s: &str) -> TagId {
        TagId::parse(s).expect("tag")
    }

    fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, dd).expect("date")
    }

    fn fixture(name: &str) -> Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/reports")
            .join(format!("{name}.json"));
        serde_json::from_str(&std::fs::read_to_string(p).expect("fixture")).expect("json")
    }

    fn march() -> DateRange {
        DateRange {
            from: d(2033, 3, 1),
            to: d(2033, 3, 31),
        }
    }

    fn req(source: Source, dates: ReportDates) -> ReportRequest {
        let mut r = ReportRequest::new(source, dates);
        r.basis = Some(Basis::Accrual);
        r
    }

    #[test]
    fn profit_and_loss_keeps_the_engine_s_tree_and_totals() {
        let r = req(Source::ProfitLoss, ReportDates::Range(march()));
        let t = profit_loss(&fixture("exact_profit_loss_accrual"), &r, march()).expect("p&l");
        let kinds: Vec<_> = t
            .rows
            .iter()
            .map(|r| (r.kind, r.level, r.caption.as_str()))
            .collect();
        assert_eq!(kinds[0], (RowKind::Header, 0, "@reports.pl.income"));
        // The 0.10 + 0.20 trap stays exact.
        assert_eq!(
            t.rows[1].cells,
            vec![Cell::Amount("0.3000".parse().expect("d"))]
        );
        assert_eq!(t.rows[2].kind, RowKind::Subtotal);
        // The parent expense, its sub-account under it, and its total
        // beside it with the engine's roll-up.
        let parent = t
            .rows
            .iter()
            .position(|r| r.expandable && r.kind == RowKind::Account)
            .expect("parent");
        let child = &t.rows[parent + 1];
        assert!(child.path.is_under(&t.rows[parent].path));
        assert_eq!(child.level, 2);
        let tot = &t.rows[parent + 2];
        assert_eq!(tot.kind, RowKind::AccountTotal);
        assert!(
            !tot.path.is_under(&t.rows[parent].path),
            "a collapsed parent keeps its total"
        );
        assert_eq!(tot.cells, vec![Cell::Amount("75.0000".parse().expect("d"))]);
        match &tot.drill {
            Some(Drill::Lines { account_ids, .. }) => assert_eq!(account_ids.len(), 2),
            other => panic!("{other:?}"),
        }
        let last = t.rows.last().expect("net");
        assert_eq!(last.kind, RowKind::Grand);
        assert_eq!(
            last.cells,
            vec![Cell::Amount("-74.7000".parse().expect("d"))]
        );
        // No cost of goods sold: no gross profit line.
        assert!(
            !t.rows
                .iter()
                .any(|r| r.caption == "@reports.pl.gross_profit")
        );
    }

    #[test]
    fn cash_basis_carries_the_rules_note() {
        let mut r = req(Source::ProfitLoss, ReportDates::Range(march()));
        r.basis = Some(Basis::Cash);
        let t = profit_loss(&fixture("exact_profit_loss_cash"), &r, march()).expect("p&l");
        assert_eq!(t.meta.basis, Some(Basis::Cash));
        assert!(t.meta.cash_basis_rules.is_some());
    }

    #[test]
    fn a_drill_carries_the_report_s_tag_and_basis() {
        let mut r = req(Source::ProfitLoss, ReportDates::Range(march()));
        r.filters.tag_ids = vec![tag("232a46f2-ddb1-4e0c-93bc-37fae91ec7cd")];
        let t = profit_loss(&fixture("exact_profit_loss_accrual"), &r, march()).expect("p&l");
        match &t.rows[1].drill {
            Some(Drill::Lines {
                from,
                to,
                tag,
                basis,
                ..
            }) => {
                assert_eq!(*from, Some(d(2033, 3, 1)));
                assert_eq!(*to, d(2033, 3, 31));
                assert!(matches!(tag, TagScope::Tag(_)));
                assert_eq!(*basis, Some(Basis::Accrual));
            }
            other => panic!("{other:?}"),
        }
        // Two tags can't be carried: no drill rather than a wrong one.
        r.filters
            .tag_ids
            .push(tag("332a46f2-ddb1-4e0c-93bc-37fae91ec7cd"));
        let t = profit_loss(&fixture("exact_profit_loss_accrual"), &r, march()).expect("p&l");
        assert!(t.rows[1].drill.is_none());
    }

    #[test]
    fn a_missing_exact_total_is_a_shape_error() {
        let mut v = fixture("exact_profit_loss_accrual");
        v.as_object_mut().expect("obj").remove("netIncomeExact");
        let r = req(Source::ProfitLoss, ReportDates::Range(march()));
        assert!(matches!(
            profit_loss(&v, &r, march()),
            Err(ApiError::ReportShape { .. })
        ));
    }

    #[test]
    fn balance_sheet_net_income_opens_the_year_to_date_pl() {
        let as_of = d(2033, 3, 31);
        let mut r = req(Source::BalanceSheet, ReportDates::AsOf(as_of));
        r.fiscal_year_start = Some(d(2033, 1, 1));
        let t = balance_sheet(&fixture("exact_balance_sheet_accrual"), &r, as_of).expect("bs");
        let ni = t
            .rows
            .iter()
            .find(|r| r.caption == "@reports.bs.net_income")
            .expect("net income");
        assert_eq!(
            ni.drill,
            Some(Drill::ProfitLoss {
                from: d(2033, 1, 1),
                to: as_of,
                basis: Some(Basis::Accrual)
            })
        );
        let bank = t
            .rows
            .iter()
            .find(|r| r.kind == RowKind::Account)
            .expect("bank");
        match &bank.drill {
            Some(Drill::Lines { from, to, .. }) => {
                assert_eq!(*from, None, "a balance runs from the first entry");
                assert_eq!(*to, as_of);
            }
            other => panic!("{other:?}"),
        }
        let grand = t.rows.last().expect("grand");
        assert_eq!(
            grand.cells,
            vec![Cell::Amount("-49.7000".parse().expect("d"))]
        );
    }

    #[test]
    fn trial_balance_is_flat_with_the_engine_s_totals() {
        let as_of = d(2033, 3, 31);
        let mut r = req(Source::TrialBalance, ReportDates::AsOf(as_of));
        r.fiscal_year_start = Some(d(2033, 1, 1));
        let t = trial_balance(&fixture("exact_trial_balance_accrual"), &r, as_of).expect("tb");
        assert_eq!(t.columns.len(), 2);
        assert!(t.rows.iter().all(|r| !r.expandable));
        let grand = t.rows.last().expect("grand");
        assert_eq!(
            grand.cells,
            vec![
                Cell::Amount("75.0000".parse().expect("d")),
                Cell::Amount("75.0000".parse().expect("d"))
            ]
        );
        // An expense drills from the fiscal year's start, the bank from the
        // first entry.
        let froms: Vec<_> = t
            .rows
            .iter()
            .filter_map(|r| match &r.drill {
                Some(Drill::Lines { from, .. }) => Some(*from),
                _ => None,
            })
            .collect();
        assert!(froms.contains(&None) && froms.contains(&Some(d(2033, 1, 1))));
    }
}
