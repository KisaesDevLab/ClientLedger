//! Budget vs. Actual (U-203, `budgets-exact-v1`, `exact=1`): every cell is
//! the engine's. Each engine column (a budget period, then Total) becomes
//! four table columns: Actual, Budget, Over budget (actual - budget) and %
//! of budget (1 dp; "—" on a zero budget). Only the Actual columns drill:
//! an actual cell opens the General Ledger of its row's lines over its
//! column's dates (the engine's `drillTarget`, with the budget's tag), and
//! its `netChangeExact` is the cell.
//!
//! Under a tag split the budget lives on the account, so the account
//! (untagged) and tag rows carry the actual only and the account total row
//! carries the budget; the window always shows that row on this report.

use cl_core::CalendarDate;
use cl_core::ids::{AccountId, TagGroupId, TagId};
use cl_core::report::{
    Cell, ColumnDrill, ColumnHead, ColumnKind, DateRange, Drill, ReportDates, ReportMeta,
    ReportRequest, ReportRow, ReportTable, RowKind, RowPath, TagScope,
};
use rust_decimal::Decimal;
use serde_json::Value;

use super::exact::{account_caption, date, decimal, list, meta_basis, require_exact, shape, text};
use super::row;
use super::statements::MAX_DRILL_ACCOUNTS;
use crate::errors::ApiError;

/// The table columns of one engine column.
pub const PER_COLUMN: usize = 4;

/// True when the answer is the exact Budget vs. Actual.
#[must_use]
pub fn is_bva(v: &Value) -> bool {
    v.get("layout").and_then(Value::as_str) == Some("budget_vs_actual")
}

fn opt(v: &Value, key: &'static str) -> Result<Option<Decimal>, ApiError> {
    match v.get(key) {
        Some(Value::String(s)) => decimal(s, key).map(Some),
        Some(Value::Null) => Ok(None),
        _ => Err(shape(key)),
    }
}

/// The four cells of one engine cell. A null budget (an untagged or tag
/// row of a split) leaves Budget, Over and % empty.
fn quad(c: &Value) -> Result<[Cell; PER_COLUMN], ApiError> {
    let actual = match c.get("actualExact") {
        Some(Value::String(s)) => decimal(s, "actualExact")?,
        _ => return Err(shape("actualExact")),
    };
    let budget = opt(c, "budgetExact")?;
    let over = opt(c, "overExact")?;
    let pct = opt(c, "pctOfBudgetExact")?;
    Ok(match budget {
        None => [Cell::Amount(actual), Cell::Empty, Cell::Empty, Cell::Empty],
        Some(b) => [
            Cell::Amount(actual),
            Cell::Amount(b),
            over.map_or(Cell::Empty, Cell::Amount),
            Cell::Percent(pct),
        ],
    })
}

fn cells(v: &Value, key: &'static str, width: usize) -> Result<Vec<Cell>, ApiError> {
    let arr = v.get(key).and_then(Value::as_array).ok_or(shape(key))?;
    if arr.len() * PER_COLUMN != width {
        return Err(shape(key));
    }
    let mut out = Vec::with_capacity(width);
    for c in arr {
        out.extend(quad(c)?);
    }
    Ok(out)
}

/// The columns, and the dates of the whole run.
fn columns(v: &Value) -> Result<(Vec<ColumnHead>, Vec<DateRange>), ApiError> {
    let mut heads = Vec::new();
    let mut ranges = Vec::new();
    for c in list(v, "columns")? {
        let key = text(c, "key").ok_or(shape("columns.key"))?.to_string();
        let total = text(c, "kind") == Some("total");
        let range = DateRange {
            from: date(c, "from")?,
            to: date(c, "to")?,
        };
        if !total {
            ranges.push(range);
        }
        // A total over periods with gaps has no one ledger behind it.
        let drill_ok = c.get("drill").is_some_and(Value::is_object);
        let name = if total {
            String::new()
        } else {
            format!(" {}", text(c, "caption").unwrap_or(""))
        };
        let suffix = if total { "_total" } else { "" };
        let head = |part: &str, kind: ColumnKind, drill: Option<ColumnDrill>, range| ColumnHead {
            key: format!("{key}_{part}"),
            caption: format!("@reports.col.bva_{part}{suffix}{name}"),
            kind,
            range,
            drill,
        };
        heads.push(head(
            "actual",
            ColumnKind::Amount,
            (!drill_ok).then_some(ColumnDrill::None),
            Some(range),
        ));
        heads.push(head(
            "budget",
            ColumnKind::Amount,
            Some(ColumnDrill::None),
            None,
        ));
        heads.push(head(
            "over",
            ColumnKind::Amount,
            Some(ColumnDrill::None),
            None,
        ));
        heads.push(head(
            "pct",
            ColumnKind::Percent,
            Some(ColumnDrill::None),
            None,
        ));
    }
    if ranges.is_empty() {
        return Err(shape("columns"));
    }
    Ok((heads, ranges))
}

struct Builder<'r> {
    req: &'r ReportRequest,
    width: usize,
    from: CalendarDate,
    to: CalendarDate,
    /// The budget's own tag (a budget for one tag): every drill keeps it.
    budget_tag: Option<TagId>,
}

/// One open parent account: its roll-up shows after its children.
struct Open {
    depth: u64,
    key: String,
    caption: String,
    rollup: Option<Vec<Cell>>,
    ids: Vec<AccountId>,
}

impl Builder<'_> {
    fn lines(&self, ids: Vec<AccountId>, tag: TagScope) -> Option<Drill> {
        if ids.is_empty() || ids.len() > MAX_DRILL_ACCOUNTS {
            return None;
        }
        let tag = match (self.budget_tag, tag) {
            (Some(t), TagScope::Any) => TagScope::Tag(t),
            (Some(_), _) => return None,
            (None, t) => t,
        };
        Some(Drill::Lines {
            account_ids: ids,
            from: Some(self.from),
            to: self.to,
            contact: cl_core::report::ContactScope::Any,
            tag,
            basis: self.req.basis,
            adjusting: self.req.filters.adjusting,
        })
    }

    fn row_drill(&self, e: &Value) -> Result<Option<Drill>, ApiError> {
        let Some(d) = e.get("drill").filter(|d| d.is_object()) else {
            return Ok(None);
        };
        let ids = text(d, "account_ids")
            .unwrap_or("")
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| AccountId::parse(s).map_err(|_| shape("drill.account_ids")))
            .collect::<Result<Vec<_>, _>>()?;
        let tag = if let Some(t) = text(d, "tag_id") {
            TagScope::Tag(TagId::parse(t).map_err(|_| shape("drill.tag_id"))?)
        } else if text(d, "tag_scope") == Some("untagged_in_group") {
            TagScope::UntaggedInGroup(
                text(d, "tag_group_id")
                    .and_then(|g| TagGroupId::parse(g).ok())
                    .ok_or(shape("drill.tag_group_id"))?,
            )
        } else {
            TagScope::Any
        };
        Ok(self.lines(ids, tag))
    }

    fn path(section: &str, stack: &[Open], last: &str) -> RowPath {
        let mut p = RowPath::of([section]);
        for o in stack {
            p = p.child(o.key.clone());
        }
        p.child(last)
    }

    fn close(&self, section: &str, stack: &mut Vec<Open>, depth: u64, rows: &mut Vec<ReportRow>) {
        while stack.last().is_some_and(|o| o.depth >= depth) {
            let Some(o) = stack.pop() else { break };
            if let Some(rollup) = o.rollup {
                let mut r = row(
                    RowKind::AccountTotal,
                    u8::try_from(o.depth + 1).unwrap_or(u8::MAX),
                    Self::path(section, stack, &format!("{}~total", o.key)),
                    format!("@reports.row.total_of {}", o.caption),
                    rollup,
                );
                r.drill = self.lines(o.ids.clone(), TagScope::Any);
                rows.push(r);
            }
            if let Some(p) = stack.last_mut() {
                p.ids.extend(o.ids);
            }
        }
    }

    fn section(
        &self,
        section: &str,
        entries: &[Value],
    ) -> Result<(Vec<ReportRow>, Vec<AccountId>), ApiError> {
        let mut rows = Vec::new();
        let mut ids = Vec::new();
        let mut stack: Vec<Open> = Vec::new();
        let mut current = String::new();
        for (i, e) in entries.iter().enumerate() {
            let kind = text(e, "rowKind").unwrap_or("account");
            let depth = e.get("depth").and_then(Value::as_u64).unwrap_or(0);
            let level = u8::try_from(depth + 1).unwrap_or(u8::MAX);
            let account = super::exact::id(e, "accountId", AccountId::parse)?;
            let named = account_caption(text(e, "accountNumber"), text(e, "name"));
            let values = cells(e, "cells", self.width)?;
            match kind {
                "tag" => {
                    let tag = text(e, "tagId").unwrap_or("");
                    let mut r = row(
                        RowKind::TagRow,
                        level.saturating_add(1),
                        Self::path(section, &stack, &current).child(format!("t:{tag}")),
                        account_caption(text(e, "tagNumber"), text(e, "tagName")),
                        values,
                    );
                    r.drill = self.row_drill(e)?;
                    rows.push(r);
                }
                "account_total" => {
                    let mut r = row(
                        RowKind::AccountTotal,
                        level,
                        Self::path(section, &stack, &format!("{current}~split")),
                        format!("@reports.row.total_of {named}"),
                        values,
                    );
                    r.drill = self.row_drill(e)?;
                    rows.push(r);
                }
                _ => {
                    self.close(section, &mut stack, depth, &mut rows);
                    current = account.map_or_else(|| format!("row{i}"), |a| a.to_string());
                    let rollup = match e.get("rollupCells") {
                        Some(r) if r.is_array() => Some(cells(e, "rollupCells", self.width)?),
                        _ => None,
                    };
                    let mut r = row(
                        RowKind::Account,
                        level,
                        Self::path(section, &stack, &current),
                        named.clone(),
                        values,
                    );
                    r.expandable = rollup.is_some() || self.req.tag_split.is_some();
                    r.drill = self.row_drill(e)?;
                    rows.push(r);
                    let own: Vec<AccountId> = account.into_iter().collect();
                    ids.extend(own.iter().copied());
                    if rollup.is_some() {
                        stack.push(Open {
                            depth,
                            key: current.clone(),
                            caption: named,
                            rollup,
                            ids: own,
                        });
                    } else if let Some(p) = stack.last_mut() {
                        p.ids.extend(own);
                    }
                }
            }
        }
        self.close(section, &mut stack, 0, &mut rows);
        Ok((rows, ids))
    }
}

/// The table of an exact Budget vs. Actual answer.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing or a row's
/// cells don't line up with the columns.
#[allow(
    clippy::too_many_lines,
    reason = "sections, then totals, in report order"
)]
pub fn table(v: &Value, req: &ReportRequest) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    if !is_bva(v) {
        return Err(shape("layout"));
    }
    let (heads, ranges) = columns(v)?;
    let width = heads.len();
    let budget_tag = v
        .pointer("/drillTarget/params/tag_id")
        .and_then(Value::as_str)
        .and_then(|t| TagId::parse(t).ok());
    let b = Builder {
        req,
        width,
        from: ranges.first().map(|r| r.from).ok_or(shape("columns"))?,
        to: ranges.last().map(|r| r.to).ok_or(shape("columns"))?,
        budget_tag,
    };
    let totals = v.get("totalsExact").ok_or(shape("totalsExact"))?;
    let mut rows = Vec::new();
    let sections: [(&str, &str, &str, &str, &str); 5] = [
        (
            "revenue",
            "totalRevenue",
            "income",
            "@reports.pl.income",
            "@reports.pl.total_income",
        ),
        (
            "cogs",
            "totalCogs",
            "cogs",
            "@reports.pl.cogs",
            "@reports.pl.total_cogs",
        ),
        (
            "expenses",
            "totalExpenses",
            "expenses",
            "@reports.pl.expenses",
            "@reports.pl.total_expenses",
        ),
        (
            "otherRevenue",
            "totalOtherRevenue",
            "other_income",
            "@reports.pl.other_income",
            "@reports.pl.total_other_income",
        ),
        (
            "otherExpenses",
            "totalOtherExpenses",
            "other_expenses",
            "@reports.pl.other_expenses",
            "@reports.pl.total_other_expenses",
        ),
    ];
    for (engine, total_key, key, title, total_title) in sections {
        let entries = list(v, engine)?;
        if !entries.is_empty() {
            let (mut body, ids) = b.section(key, entries)?;
            let mut h = row(
                RowKind::Header,
                0,
                RowPath::of([key]),
                title.to_string(),
                vec![Cell::Empty; width],
            );
            h.expandable = true;
            rows.push(h);
            rows.append(&mut body);
            let mut t = row(
                RowKind::Subtotal,
                0,
                RowPath::of([format!("{key}~total")]),
                total_title.to_string(),
                cells(totals, total_key, width)?,
            );
            t.drill = b.lines(ids, TagScope::Any);
            rows.push(t);
        }
        if engine == "cogs" && totals.get("grossProfit").is_some_and(Value::is_array) {
            rows.push(row(
                RowKind::Grand,
                0,
                RowPath::of(["gross_profit"]),
                "@reports.pl.gross_profit".into(),
                cells(totals, "grossProfit", width)?,
            ));
        }
    }
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of(["net_income"]),
        "@reports.pl.net_income".into(),
        cells(totals, "netIncome", width)?,
    ));
    Ok(ReportTable {
        columns: heads,
        rows,
        meta: ReportMeta {
            basis: meta_basis(v),
            dates: Some(ReportDates::Ranges(ranges)),
            truncated: None,
            note: None,
            hidden_notice: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::report::Source;
    use serde_json::json;

    const A4000: &str = "11111111-1111-4111-8111-111111111111";
    const A4010: &str = "22222222-2222-4222-8222-222222222222";
    const TAG: &str = "33333333-3333-4333-8333-333333333333";
    const GROUP: &str = "44444444-4444-4444-8444-444444444444";

    fn c(a: &str, b: Option<&str>, o: Option<&str>, p: Option<&str>) -> Value {
        json!({"actualExact": a, "budgetExact": b, "overExact": o, "pctOfBudgetExact": p})
    }

    fn answer() -> Value {
        let cell = |a: &str| c(a, Some("100.0000"), Some("0.0000"), Some("100.0"));
        json!({
            "title": "Budget vs. Actual", "layout": "budget_vs_actual",
            "budget": {"id": "b", "version": 2, "tagId": null},
            "columns": [
                {"key": "p0", "caption": "Jan 1 - 28, 2026", "kind": "period", "periodIndex": 1,
                 "from": "2026-01-01", "to": "2026-01-28", "drill": {"start_date": "2026-01-01", "end_date": "2026-01-28"}},
                {"key": "total", "caption": "Total", "kind": "total", "periodIndex": null,
                 "from": "2026-01-01", "to": "2026-01-28", "drill": null}
            ],
            "revenue": [
                {"rowKind": "account", "accountId": A4000, "name": "Sales", "accountNumber": "4000",
                 "parentAccountId": null, "depth": 0, "cells": [cell("100.0000"), cell("100.0000")],
                 "rollupCells": [cell("200.0000"), cell("200.0000")], "drill": {"account_ids": A4000}},
                {"rowKind": "account", "accountId": A4010, "name": "Online", "accountNumber": "4010",
                 "parentAccountId": A4000, "depth": 1,
                 "cells": [c("0.1000", Some("0.0000"), Some("0.1000"), None), c("0.1000", Some("0.0000"), Some("0.1000"), None)],
                 "drill": {"account_ids": A4010}}
            ],
            "cogs": [], "expenses": [], "otherRevenue": [], "otherExpenses": [],
            "totalsExact": {
                "totalRevenue": [cell("200.1000"), cell("200.1000")],
                "totalCogs": [cell("0"), cell("0")], "totalExpenses": [cell("0"), cell("0")],
                "totalOtherRevenue": [cell("0"), cell("0")], "totalOtherExpenses": [cell("0"), cell("0")],
                "grossProfit": null, "netIncome": [cell("200.1000"), cell("200.1000")]
            },
            "drillTarget": {"path": "/reports/general-ledger", "params": {"basis": "accrual", "adjusting": "include"}},
            "meta": {"exact": true, "basis": "accrual"}
        })
    }

    fn req() -> ReportRequest {
        ReportRequest::new(Source::BudgetVsActual, ReportDates::None)
    }

    fn dec(s: &str) -> Decimal {
        s.parse().expect("dec")
    }

    #[test]
    fn four_columns_per_period_with_the_engine_s_cells() {
        let t = table(&answer(), &req()).expect("table");
        assert_eq!(t.columns.len(), 8);
        assert_eq!(t.columns[0].key, "p0_actual");
        assert_eq!(
            t.columns[0].caption,
            "@reports.col.bva_actual Jan 1 - 28, 2026"
        );
        assert_eq!(t.columns[7].caption, "@reports.col.bva_pct_total");
        assert_eq!(t.columns[3].kind, ColumnKind::Percent);
        let sales = &t.rows[1];
        assert_eq!(sales.caption, "4000 \u{b7} Sales");
        assert_eq!(sales.cells[0], Cell::Amount(dec("100.0000")));
        assert_eq!(sales.cells[3], Cell::Percent(Some(dec("100.0"))));
        // The zero budget's percent is null ("—"), never computed here.
        assert_eq!(t.rows[2].cells[3], Cell::Percent(None));
        // The parent's roll-up follows its child.
        assert_eq!(t.rows[3].kind, RowKind::AccountTotal);
        assert_eq!(t.rows[3].cells[0], Cell::Amount(dec("200.0000")));
        assert_eq!(
            t.rows.last().map(|r| r.caption.as_str()),
            Some("@reports.pl.net_income")
        );
        assert!(
            !t.rows
                .iter()
                .any(|r| r.caption == "@reports.pl.gross_profit"),
            "no cost of goods sold, no gross profit"
        );
    }

    #[test]
    fn only_actual_cells_drill_and_a_gapped_total_has_none() {
        let t = table(&answer(), &req()).expect("table");
        let d = t.rows[1].drill.as_ref().expect("drill");
        assert!(d.for_cell(&t.columns[0]).is_some(), "period actual");
        for i in 1..4 {
            assert!(d.for_cell(&t.columns[i]).is_none(), "budget, over, %");
        }
        assert!(
            d.for_cell(&t.columns[4]).is_none(),
            "total without one ledger"
        );
        match d.for_cell(&t.columns[0]) {
            Some(Drill::Lines {
                from,
                to,
                account_ids,
                ..
            }) => {
                assert_eq!(account_ids.len(), 1);
                assert_eq!(from, CalendarDate::from_ymd(2026, 1, 1).ok());
                assert_eq!(to, CalendarDate::from_ymd(2026, 1, 28).expect("d"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_split_keeps_the_budget_on_the_account_total() {
        let mut v = answer();
        v["revenue"] = json!([
            {"rowKind": "account", "accountId": A4000, "name": "Sales", "accountNumber": "4000", "depth": 0,
             "cells": [c("40.0000", None, None, None), c("40.0000", None, None, None)],
             "drill": {"account_ids": A4000, "tag_scope": "untagged_in_group", "tag_group_id": GROUP}},
            {"rowKind": "tag", "accountId": A4000, "name": "Sales", "depth": 0, "tagId": TAG, "tagName": "East",
             "tagNumber": "10", "cells": [c("60.0000", None, None, None), c("60.0000", None, None, None)],
             "drill": {"account_ids": A4000, "tag_id": TAG}},
            {"rowKind": "account_total", "accountId": A4000, "name": "Sales", "accountNumber": "4000", "depth": 0,
             "cells": [c("100.0000", Some("90.0000"), Some("10.0000"), Some("111.1")),
                       c("100.0000", Some("90.0000"), Some("10.0000"), Some("111.1"))],
             "drill": {"account_ids": A4000}}
        ]);
        let mut r = req();
        r.tag_split = TagGroupId::parse(GROUP).ok();
        let t = table(&v, &r).expect("table");
        let kinds: Vec<RowKind> = t.rows.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds[1..4],
            [RowKind::Account, RowKind::TagRow, RowKind::AccountTotal]
        );
        assert_eq!(
            t.rows[1].cells[1],
            Cell::Empty,
            "no budget on the untagged row"
        );
        assert_eq!(t.rows[2].cells[3], Cell::Empty, "no % on a tag row");
        assert_eq!(t.rows[3].cells[1], Cell::Amount(dec("90.0000")));
        assert_eq!(t.rows[2].caption, "10 \u{b7} East");
        match &t.rows[1].drill {
            Some(Drill::Lines { tag, .. }) => assert!(matches!(tag, TagScope::UntaggedInGroup(_))),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_budget_for_one_tag_drills_to_its_tag() {
        let mut v = answer();
        v["drillTarget"]["params"]["tag_id"] = json!(TAG);
        let t = table(&v, &req()).expect("table");
        match &t.rows[1].drill {
            Some(Drill::Lines { tag, .. }) => {
                assert_eq!(*tag, TagScope::Tag(TagId::parse(TAG).expect("t")));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_missing_exact_field_is_a_shape_error() {
        let mut v = answer();
        v["revenue"][0]["cells"][0]
            .as_object_mut()
            .expect("o")
            .remove("actualExact");
        assert!(table(&v, &req()).is_err());
        let mut v = answer();
        v["meta"]["exact"] = json!(false);
        assert!(table(&v, &req()).is_err());
        let mut v = answer();
        v["totalsExact"]["netIncome"] = json!([c("1", None, None, None)]);
        assert!(
            table(&v, &req()).is_err(),
            "cells must line up with the columns"
        );
    }
}
