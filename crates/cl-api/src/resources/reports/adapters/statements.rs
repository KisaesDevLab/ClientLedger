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
    Basis, Cell, ColumnHead, ColumnKind, DateRange, Drill, ReportMeta, ReportRequest, ReportRow,
    ReportTable, RowKind, RowPath, Source, TagScope,
};
use serde_json::Value;

use super::exact::{
    account_caption, amount, amount_opt, amount_or_null, cell, id, list, meta_basis, require_exact,
    text,
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
        contact: cl_core::report::ContactScope::Any,
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
        ..ReportMeta::default()
    }
}

/// The engine's name of the current year's net income row on the sheet.
const BS_NET_INCOME: &str = "Net Income (Current Year)";
/// The engine's name of the prior years' row without a Retained Earnings
/// account.
const BS_RETAINED: &str = "Retained Earnings (Prior Years)";

/// The row flag of a Retained Earnings line that includes the prior
/// years' income (the column layout's; the classic layout's with U-213).
pub const PRIOR_INCOME_FLAG: &str = "includesPriorYearsIncome";

/// The fiscal year start the engine used (U-213 `fiscalYearStart`).
fn engine_year_start(v: &Value) -> Option<CalendarDate> {
    v.get("fiscalYearStart")
        .and_then(Value::as_str)
        .and_then(|s| CalendarDate::parse_wire(s).ok())
}

/// The day the engine starts the fiscal year containing `to`. The engine
/// knows only the fiscal start month and splits balance reports at its
/// first day (S1, U-25): on a 4-4-5 or 52-53 week year, which starts a few
/// days off a month, that is the latest first of the start month on or
/// before `to`, not the desktop's retail start.
fn year_start(req: &ReportRequest, to: CalendarDate) -> CalendarDate {
    let Some(start) = req.fiscal_year_start else {
        return CalendarDate::from_ymd(to.year(), 1, 1).unwrap_or(to);
    };
    if start.day() == 1 {
        return start;
    }
    // The month a retail year stands for: the nearest first of a month.
    let month = if start.day() <= 15 {
        start.month()
    } else {
        start.month() % 12 + 1
    };
    let this = CalendarDate::from_ymd(to.year(), month, 1).unwrap_or(to);
    if this <= to {
        this
    } else {
        CalendarDate::from_ymd(to.year() - 1, month, 1).unwrap_or(start)
    }
}

/// Balance Sheet as of `as_of`, one balance column.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
#[allow(
    clippy::too_many_lines,
    reason = "the three sections, then the equity rules, in order"
)]
pub fn balance_sheet(
    v: &Value,
    req: &ReportRequest,
    as_of: CalendarDate,
) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    // U-213 says where it split the year; else the engine's month rule.
    let fy = engine_year_start(v).unwrap_or_else(|| year_start(req, as_of));
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
        // The current year's net income opens the fiscal year-to-date P&L
        // with the sheet's basis, lens and tags (S3); the prior years' row
        // has no drill.
        for r in &mut s.rows {
            if r.caption == "@reports.bs.net_income" {
                r.drill = Some(Drill::ProfitLoss {
                    from: fy,
                    to: as_of,
                    basis: req.basis,
                    adjusting: req.filters.adjusting,
                    tag_ids: req.filters.tag_ids.clone(),
                });
            }
        }
        // B2: the Retained Earnings account's line also carries the prior
        // years' income, which no ledger line holds (U-213 flags it).
        let folded: Vec<String> = entries
            .iter()
            .filter(|e| e.get(PRIOR_INCOME_FLAG) == Some(&Value::Bool(true)))
            .filter_map(|e| text(e, "accountId").map(str::to_string))
            .collect();
        for r in &mut s.rows {
            if r.path.0.last().is_some_and(|k| folded.contains(k)) {
                r.drill = None;
            }
        }
        // A section total drills only when its ledger sums to it: equity
        // with a calculated row (net income, prior years) or a folded
        // Retained Earnings line has no drill.
        // U-213 says how much of the total no ledger line holds.
        let calculated = if key == "equity" && v.get("calculatedEquityExact").is_some() {
            amount_or_null(v, "calculatedEquityExact")?.is_none_or(|x| !x.is_zero())
        } else {
            entries.iter().any(|e| {
                e.get("accountId").is_none_or(Value::is_null)
                    || e.get("calculated") == Some(&Value::Bool(true))
            })
        };
        rows.push(header(key, title));
        rows.append(&mut s.rows);
        let drill = if s.ids.len() <= MAX_DRILL_ACCOUNTS && !calculated && folded.is_empty() {
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

/// The accounts a balance report needs to know by role: the designated
/// Retained Earnings account (B2) and A/R and A/P (S5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountRoles {
    /// The Retained Earnings account (`system_tag = retained_earnings`).
    pub retained_earnings: Option<AccountId>,
    /// Accounts receivable and payable accounts.
    pub receivable_payable: Vec<AccountId>,
}

impl AccountRoles {
    /// The roles in a chart of accounts.
    #[must_use]
    pub fn of(accounts: &[crate::resources::accounts::Account]) -> AccountRoles {
        let role = |a: &crate::resources::accounts::Account, r: &str| {
            a.system_tag.as_deref() == Some(r) || a.fields.detail_type.as_deref() == Some(r)
        };
        AccountRoles {
            retained_earnings: accounts
                .iter()
                .find(|a| a.system_tag.as_deref() == Some("retained_earnings"))
                .map(|a| a.id),
            receivable_payable: accounts
                .iter()
                .filter(|a| role(a, "accounts_receivable") || role(a, "accounts_payable"))
                .map(|a| a.id)
                .collect(),
        }
    }
}

/// True when [`finish_balances`] needs the chart's roles: a classic
/// Balance Sheet from an engine that doesn't flag the folded Retained
/// Earnings line, or a cash-basis Balance Sheet or Trial Balance.
#[must_use]
pub fn wants_roles(req: &ReportRequest, v: &Value) -> bool {
    let unflagged_sheet =
        req.source == Source::BalanceSheet && !super::columns::is_columns(v) && !flags_equity(v);
    let cash_balances = req.basis == Some(Basis::Cash)
        && matches!(req.source, Source::BalanceSheet | Source::TrialBalance)
        && v.pointer("/meta/cashArApResidue").is_none();
    unflagged_sheet || cash_balances
}

/// True when the sheet says itself which equity lines no ledger holds
/// (U-213: `calculatedEquityExact`, or a flagged Retained Earnings line).
fn flags_equity(v: &Value) -> bool {
    v.get("calculatedEquityExact").is_some()
        || v.get("equity")
            .and_then(Value::as_array)
            .is_some_and(|e| e.iter().any(|e| e.get(PRIOR_INCOME_FLAG).is_some()))
}

/// The cash-basis A/R and A/P note from U-213's `meta.cashArApResidue`:
/// `Some(None)` when the engine says there is none, `None` before U-213.
#[allow(
    clippy::option_option,
    reason = "absent (before U-213) differs from null (no residue)"
)]
fn residue_note(v: &Value, req: &ReportRequest, to: CalendarDate) -> Option<Option<Drill>> {
    let r = v.pointer("/meta/cashArApResidue")?;
    if r.is_null() {
        return Some(None);
    }
    let strings = |k: &str| -> Vec<String> {
        r.get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(Some(Drill::EntriesOfTypes {
        account_ids: strings("accountIds")
            .iter()
            .filter_map(|s| AccountId::parse(s).ok())
            .collect(),
        txn_types: strings("txnTypes"),
        to,
        basis: req.basis,
    }))
}

/// The note on a cash-basis report that shows an A/R or A/P balance (S5,
/// Q-SPF7).
pub const CASH_AR_AP_NOTE: &str = "@reports.note.cash_ar_ap";

/// What only the chart's roles tell (`roles` is `None` when they couldn't
/// be read):
/// - B2: on an engine that doesn't flag it, the Retained Earnings line of
///   a classic Balance Sheet may carry the prior years' income, which no
///   ledger holds: it and the equity total have no drill (unknown roles:
///   no equity row drills).
/// - S5: a cash-basis Balance Sheet or Trial Balance with an A/R or A/P
///   balance says why in one line.
pub fn finish_balances(
    req: &ReportRequest,
    v: &Value,
    table: &mut ReportTable,
    roles: Option<&AccountRoles>,
) {
    if req.source == Source::BalanceSheet && !super::columns::is_columns(v) && !flags_equity(v) {
        let re = roles.map(|r| r.retained_earnings.map(|a| a.to_string()));
        let mut cleared = false;
        for r in &mut table.rows {
            let equity = r.path.0.first().is_some_and(|p| p == "equity");
            let hit = match &re {
                Some(Some(id)) => r.path.0.last() == Some(id),
                Some(None) => false,
                None => r.kind == RowKind::Account || r.kind == RowKind::AccountTotal,
            };
            if equity && hit && r.drill.is_some() {
                r.drill = None;
                cleared = true;
            }
        }
        if cleared {
            for r in &mut table.rows {
                if r.path.0.first().is_some_and(|p| p == "equity~total") {
                    r.drill = None;
                }
            }
        }
    }
    let cash = req.basis == Some(Basis::Cash)
        && matches!(req.source, Source::BalanceSheet | Source::TrialBalance);
    let residue = req.dates.span().and_then(|r| residue_note(v, req, r.to));
    if cash && let Some(note) = residue {
        // U-213: the engine's residue, with a link to its entries.
        if let Some(d) = note {
            table.meta.note = Some(CASH_AR_AP_NOTE.to_string());
            table.meta.note_drill = Some(d);
        }
    } else if cash && let Some(roles) = roles {
        let shown = table.rows.iter().any(|r| {
            r.kind == RowKind::Account
                && r.path
                    .0
                    .last()
                    .is_some_and(|k| roles.receivable_payable.iter().any(|a| a.to_string() == *k))
                && r.cells
                    .iter()
                    .any(|c| matches!(c, Cell::Amount(x) if !x.is_zero()))
        });
        if shown {
            table.meta.note = Some(CASH_AR_AP_NOTE.to_string());
        }
    }
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
    // The engine says where its income window starts (S1).
    let fy = engine_year_start(v)
        .or_else(|| {
            v.pointer("/drillTarget/startDate")
                .and_then(Value::as_str)
                .and_then(|s| CalendarDate::parse_wire(s).ok())
        })
        .filter(|d| *d <= as_of)
        .unwrap_or_else(|| year_start(req, as_of));
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
    fn cash_basis_shows_no_rules_note() {
        // Q-P28ai: the basis stays in the subtitle; the engine's general
        // cash-basis sentence is not shown or exported.
        let mut r = req(Source::ProfitLoss, ReportDates::Range(march()));
        r.basis = Some(Basis::Cash);
        let t = profit_loss(&fixture("exact_profit_loss_cash"), &r, march()).expect("p&l");
        assert_eq!(t.meta.basis, Some(Basis::Cash));
        assert!(t.meta.note.is_none());
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
                basis: Some(Basis::Accrual),
                adjusting: None,
                tag_ids: Vec::new(),
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

    const RE: &str = "4e1d0d2b-0000-4000-8000-000000000001";

    fn sheet_with_retained(net_income: bool, flag: Option<bool>) -> Value {
        let mut v = fixture("exact_balance_sheet_accrual");
        let mut re = serde_json::json!({"accountId": RE, "accountNumber": "30120",
            "balance": 10, "balanceExact": "10.0000", "depth": 0,
            "name": "Retained Earnings", "parentAccountId": null});
        if let Some(f) = flag {
            re[PRIOR_INCOME_FLAG] = Value::Bool(f);
        }
        let eq = v["equity"].as_array_mut().expect("equity");
        if !net_income {
            eq.clear();
        }
        eq.push(re);
        v
    }

    fn row<'t>(t: &'t ReportTable, last: &str) -> &'t ReportRow {
        t.rows
            .iter()
            .find(|r| r.path.0.last().is_some_and(|k| k == last))
            .unwrap_or_else(|| panic!("{last}"))
    }

    /// B2/T3: an equity amount drills only to a ledger that sums to it.
    #[test]
    fn equity_drills_only_where_the_ledger_sums() {
        let as_of = d(2033, 3, 31);
        let r = req(Source::BalanceSheet, ReportDates::AsOf(as_of));
        // A calculated net income row: the equity total has no drill.
        let v = sheet_with_retained(true, None);
        let mut t = balance_sheet(&v, &r, as_of).expect("bs");
        assert!(row(&t, "equity~total").drill.is_none());
        // The pinned engine doesn't flag the Retained Earnings line: the
        // chart's role decides.
        assert!(wants_roles(&r, &v));
        let roles = AccountRoles {
            retained_earnings: Some(AccountId::parse(RE).expect("id")),
            receivable_payable: Vec::new(),
        };
        finish_balances(&r, &v, &mut t, Some(&roles));
        assert!(row(&t, RE).drill.is_none(), "prior income has no ledger");
        // Unknown roles: no equity row drills.
        let v = sheet_with_retained(false, None);
        let mut t = balance_sheet(&v, &r, as_of).expect("bs");
        assert!(row(&t, "equity~total").drill.is_some(), "only ledger rows");
        finish_balances(&r, &v, &mut t, None);
        assert!(row(&t, RE).drill.is_none());
        assert!(row(&t, "equity~total").drill.is_none());
        // Another account is the Retained Earnings one: this line drills.
        let mut t = balance_sheet(&v, &r, as_of).expect("bs");
        let other = AccountRoles {
            retained_earnings: Some(
                AccountId::parse("4e1d0d2b-0000-4000-8000-000000000002").expect("id"),
            ),
            receivable_payable: Vec::new(),
        };
        finish_balances(&r, &v, &mut t, Some(&other));
        assert!(row(&t, RE).drill.is_some());
        assert!(row(&t, "equity~total").drill.is_some());
        // U-213's flag: the engine says so, no chart read.
        let v = sheet_with_retained(false, Some(true));
        let t = balance_sheet(&v, &r, as_of).expect("bs");
        assert!(!wants_roles(&r, &v));
        assert!(row(&t, RE).drill.is_none());
        assert!(row(&t, "equity~total").drill.is_none());
        let v = sheet_with_retained(false, Some(false));
        let t = balance_sheet(&v, &r, as_of).expect("bs");
        assert!(row(&t, RE).drill.is_some());
    }

    /// S5/T10: a cash-basis sheet with an A/R or A/P balance says why.
    #[test]
    fn a_cash_basis_ar_or_ap_balance_carries_a_note() {
        let as_of = d(2033, 3, 31);
        let mut r = req(Source::BalanceSheet, ReportDates::AsOf(as_of));
        r.basis = Some(Basis::Cash);
        let v = fixture("exact_balance_sheet_accrual");
        let owed = v["liabilities"][0]["accountId"]
            .as_str()
            .and_then(|s| AccountId::parse(s).ok())
            .expect("id");
        let mut t = balance_sheet(&v, &r, as_of).expect("bs");
        assert!(wants_roles(&r, &v));
        finish_balances(&r, &v, &mut t, Some(&AccountRoles::default()));
        assert_eq!(t.meta.note, None, "no A/R or A/P balance");
        let roles = AccountRoles {
            retained_earnings: None,
            receivable_payable: vec![owed],
        };
        finish_balances(&r, &v, &mut t, Some(&roles));
        assert_eq!(t.meta.note.as_deref(), Some(CASH_AR_AP_NOTE));
        // Accrual: never.
        r.basis = Some(Basis::Accrual);
        let mut t = balance_sheet(&v, &r, as_of).expect("bs");
        finish_balances(&r, &v, &mut t, Some(&roles));
        assert_eq!(t.meta.note, None);
    }

    /// S1: the Trial Balance drills from the engine's year start.
    #[test]
    fn trial_balance_drills_from_the_engine_s_year_start() {
        let as_of = d(2033, 3, 31);
        let mut r = req(Source::TrialBalance, ReportDates::AsOf(as_of));
        // A 4-4-5 year that starts on Dec 26 (the desktop's calendar).
        r.fiscal_year_start = Some(d(2032, 12, 26));
        let mut v = fixture("exact_trial_balance_accrual");
        let t = trial_balance(&v, &r, as_of).expect("tb");
        let froms: Vec<_> = t
            .rows
            .iter()
            .filter_map(|r| match &r.drill {
                Some(Drill::Lines { from: Some(f), .. }) => Some(*f),
                _ => None,
            })
            .collect();
        assert!(froms.iter().all(|f| *f == d(2033, 1, 1)), "{froms:?}");
        // Between the retail start and the 1st, the engine's year is the
        // one before.
        let early = d(2032, 12, 30);
        let mut r2 = req(Source::TrialBalance, ReportDates::AsOf(early));
        r2.fiscal_year_start = Some(d(2032, 12, 26));
        assert_eq!(year_start(&r2, early), d(2032, 1, 1));
        v["drillTarget"] = serde_json::json!({"startDate": "2033-02-01"});
        let t = trial_balance(&v, &r, as_of).expect("tb");
        assert!(t.rows.iter().any(|r| matches!(
            &r.drill,
            Some(Drill::Lines { from: Some(f), .. }) if *f == d(2033, 2, 1)
        )));
    }

    /// U-213: the sheet says which equity it can't drill, where its year
    /// starts and what A/R and A/P keep on cash basis.
    #[test]
    fn the_engine_s_equity_flags_and_residue_are_used() {
        let as_of = d(2033, 3, 31);
        let mut r = req(Source::BalanceSheet, ReportDates::AsOf(as_of));
        r.basis = Some(Basis::Cash);
        r.fiscal_year_start = Some(d(2032, 12, 26));
        let mut v = sheet_with_retained(false, Some(true));
        v["equity"][0]["priorYearsIncomeExact"] = Value::from("4.0000");
        v["fiscalYearStart"] = Value::from("2032-12-26");
        v["calculatedEquityExact"] = Value::from("0.0000");
        v["meta"]["cashArApResidue"] = serde_json::json!({
            "accountsReceivableExact": "12.0000", "accountsPayableExact": "0.0000",
            "accountIds": [RE], "txnTypes": ["journal_entry"]});
        assert!(!wants_roles(&r, &v), "no chart read");
        let mut t = balance_sheet(&v, &r, as_of).expect("bs");
        finish_balances(&r, &v, &mut t, None);
        assert!(row(&t, RE).drill.is_none(), "flagged");
        assert_eq!(t.meta.note.as_deref(), Some(CASH_AR_AP_NOTE));
        assert!(matches!(
            &t.meta.note_drill,
            Some(Drill::EntriesOfTypes { txn_types, to, .. })
                if txn_types == &["journal_entry".to_string()] && *to == as_of
        ));
        // Total equity drills only when nothing is calculated.
        let v2 = sheet_with_retained(false, Some(false));
        let mut v2 = v2;
        v2["calculatedEquityExact"] = Value::from("0.0000");
        let t = balance_sheet(&v2, &r, as_of).expect("bs");
        assert!(row(&t, "equity~total").drill.is_some());
        v2["calculatedEquityExact"] = Value::from("5.0000");
        let t = balance_sheet(&v2, &r, as_of).expect("bs");
        assert!(row(&t, "equity~total").drill.is_none());
        // The net income drill starts at the engine's year start.
        let mut v3 = fixture("exact_balance_sheet_accrual");
        v3["fiscalYearStart"] = Value::from("2032-12-26");
        v3["meta"]["cashArApResidue"] = Value::Null;
        let mut t = balance_sheet(&v3, &r, as_of).expect("bs");
        let ni = t
            .rows
            .iter()
            .find(|x| x.caption == "@reports.bs.net_income")
            .expect("ni");
        assert!(
            matches!(ni.drill, Some(Drill::ProfitLoss { from, .. }) if from == d(2032, 12, 26))
        );
        finish_balances(&r, &v3, &mut t, Some(&AccountRoles::default()));
        assert_eq!(t.meta.note, None, "no residue: no note");
    }
}
