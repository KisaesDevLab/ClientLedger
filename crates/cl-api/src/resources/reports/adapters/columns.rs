//! The column layout of the Profit & Loss and the Balance Sheet (U-201,
//! `reports-columns-v1`, `layout: "columns"`): period, YTD, prior, $ and %
//! change, client and tag columns, % of income, and the tag split (each
//! account's lines without a tag of the group, one row per group tag and
//! the account total). Every value is the engine's; a cell drills to the
//! ledger through its row's drill plus its column's (the engine's
//! `drillTarget`).

use cl_core::CalendarDate;
use cl_core::ids::{AccountId, ContactId, TagGroupId, TagId};
use cl_core::report::{
    Cell, ColumnDrill, ColumnHead, ColumnKind, DateRange, Drill, ReportMeta, ReportRequest,
    ReportRow, ReportTable, RowKind, RowPath, TagScope,
};
use rust_decimal::Decimal;
use serde_json::Value;

use super::exact::{account_caption, date, decimal, list, meta_basis, require_exact, shape, text};
use super::row;
use super::statements::{MAX_DRILL_ACCOUNTS, drill_tag};
use crate::errors::ApiError;

/// True when an exact answer is in the column layout.
#[must_use]
pub fn is_columns(v: &Value) -> bool {
    v.get("layout").and_then(Value::as_str) == Some("columns")
}

/// One engine column and where its values sit in the table's cells.
struct Col {
    /// Left out of the table (the YTD column's day-count prior, Q-PR6a).
    skip: bool,
    percent: bool,
    /// The % of income column after it.
    with_pct: bool,
}

fn opt_dec(v: &Value, what: &'static str) -> Result<Option<Decimal>, ApiError> {
    match v {
        Value::String(s) => decimal(s, what).map(Some),
        Value::Null => Ok(None),
        _ => Err(shape(what)),
    }
}

fn tag_scope(d: &Value) -> Result<Option<TagScope>, ApiError> {
    if let Some(t) = text(d, "tag_id") {
        if t == "untagged" {
            return Ok(Some(TagScope::Untagged));
        }
        return TagId::parse(t)
            .map(|t| Some(TagScope::Tag(t)))
            .map_err(|_| shape("drill.tag_id"));
    }
    if text(d, "tag_scope") == Some("untagged_in_group") {
        let g = text(d, "tag_group_id")
            .and_then(|g| TagGroupId::parse(g).ok())
            .ok_or(shape("drill.tag_group_id"))?;
        return Ok(Some(TagScope::UntaggedInGroup(g)));
    }
    Ok(None)
}

/// A column's part of a cell's drill.
fn column_drill(d: &Value) -> Result<Option<ColumnDrill>, ApiError> {
    if d.is_null() {
        return Ok(Some(ColumnDrill::None));
    }
    if let Some(c) = text(d, "contact_id") {
        // "No client" and "All others" select lines a drill can't name yet.
        return Ok(Some(
            ContactId::parse(c).map_or(ColumnDrill::None, ColumnDrill::Contact),
        ));
    }
    Ok(tag_scope(d)?.map(ColumnDrill::Tag))
}

fn columns(
    v: &Value,
    req: &ReportRequest,
    balance: bool,
    pct: bool,
) -> Result<(Vec<ColumnHead>, Vec<Col>), ApiError> {
    use cl_core::report::Extra;
    // The pinned engine compares the YTD column with the same number of
    // days before it, whatever prior ranges the periods send: on "Prior
    // period" the YTD column stands alone (Q-PR6a).
    let ytd_prior_is_days =
        req.extras.contains(&Extra::PriorPeriod) && !req.extras.contains(&Extra::PriorYear);
    let mut heads = Vec::new();
    let mut cols = Vec::new();
    for c in list(v, "columns")? {
        let key = text(c, "key").ok_or(shape("columns.key"))?.to_string();
        let percent = text(c, "valueType") == Some("percent");
        let kind = text(c, "kind").unwrap_or("");
        if ytd_prior_is_days
            && text(c, "baseKey") == Some("ytd")
            && matches!(kind, "prior" | "change" | "pct_change")
        {
            cols.push(Col {
                skip: true,
                percent,
                with_pct: false,
            });
            continue;
        }
        let range = if balance {
            match c
                .get("asOf")
                .and_then(Value::as_str)
                .or(c.get("to").and_then(Value::as_str))
            {
                Some(s) => CalendarDate::parse_wire(s)
                    .ok()
                    .map(|d| DateRange { from: d, to: d }),
                None => None,
            }
        } else {
            match (c.get("from"), c.get("to")) {
                (Some(Value::String(_)), Some(Value::String(_))) => Some(DateRange {
                    from: date(c, "from")?,
                    to: date(c, "to")?,
                }),
                _ => None,
            }
        };
        heads.push(ColumnHead {
            key: key.clone(),
            caption: text(c, "caption").unwrap_or("").to_string(),
            kind: if percent {
                ColumnKind::Percent
            } else {
                ColumnKind::Amount
            },
            range,
            drill: column_drill(c.get("drill").unwrap_or(&Value::Null))?,
        });
        let with_pct = pct && !percent && kind != "change";
        if with_pct {
            heads.push(ColumnHead {
                key: format!("{key}_pct_income"),
                caption: "@reports.col.pct_of_income".into(),
                kind: ColumnKind::Percent,
                range: None,
                drill: Some(ColumnDrill::None),
            });
        }
        cols.push(Col {
            skip: false,
            percent,
            with_pct,
        });
    }
    Ok((heads, cols))
}

/// The cells of one row: its values and, where shown, its % of income.
fn cells(cols: &[Col], values: &Value, pct: Option<&Value>) -> Result<Vec<Cell>, ApiError> {
    let values = values.as_array().ok_or(shape("valuesExact"))?;
    if values.len() != cols.len() {
        return Err(shape("valuesExact"));
    }
    let pct = match pct {
        Some(Value::Array(p)) if p.len() == cols.len() => Some(p),
        Some(_) => return Err(shape("pctOfIncomeExact")),
        None => None,
    };
    let mut out = Vec::new();
    for (i, c) in cols.iter().enumerate() {
        if c.skip {
            continue;
        }
        let x = opt_dec(&values[i], "valuesExact")?;
        out.push(match (c.percent, x) {
            (true, x) => Cell::Percent(x),
            (false, Some(x)) => Cell::Amount(x),
            (false, None) => Cell::Empty,
        });
        if c.with_pct {
            let p = pct.map_or(Ok(None), |p| opt_dec(&p[i], "pctOfIncomeExact"))?;
            out.push(Cell::Percent(p));
        }
    }
    Ok(out)
}

struct Builder<'r> {
    req: &'r ReportRequest,
    cols: &'r [Col],
    balance: bool,
    to: CalendarDate,
    from: Option<CalendarDate>,
}

/// One open parent: depth, path key, caption, its roll-up and its ids.
struct Open {
    depth: u64,
    key: String,
    caption: String,
    rollup: Vec<Cell>,
    ids: Vec<AccountId>,
}

impl Builder<'_> {
    fn lines(&self, ids: Vec<AccountId>, tag: Option<TagScope>) -> Option<Drill> {
        if ids.is_empty() || ids.len() > MAX_DRILL_ACCOUNTS {
            return None;
        }
        let report = drill_tag(self.req)?;
        let ((None, tag) | (Some(tag), TagScope::Any)) = (tag, report) else {
            return None;
        };
        Some(Drill::Lines {
            account_ids: ids,
            from: if self.balance { None } else { self.from },
            to: self.to,
            contact: None,
            tag,
            basis: self.req.basis,
            adjusting: self.req.filters.adjusting,
        })
    }

    fn row_drill(&self, r: &Value) -> Result<Option<Drill>, ApiError> {
        let d = match r.get("drill") {
            Some(d) if d.is_object() => d,
            _ => return Ok(None),
        };
        let ids = text(d, "account_ids")
            .unwrap_or("")
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| AccountId::parse(s).map_err(|_| shape("drill.account_ids")))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.lines(ids, tag_scope(d)?))
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
            let mut r = row(
                RowKind::AccountTotal,
                u8::try_from(o.depth + 1).unwrap_or(u8::MAX),
                Self::path(section, stack, &format!("{}~total", o.key)),
                format!("@reports.row.total_of {}", o.caption),
                o.rollup,
            );
            r.drill = self.lines(o.ids.clone(), None);
            if let Some(p) = stack.last_mut() {
                p.ids.extend(o.ids);
            }
            rows.push(r);
        }
    }

    /// A section's rows and every account id in it.
    fn section(
        &self,
        section: &str,
        entries: &[Value],
        named: &dyn Fn(&Value) -> String,
    ) -> Result<(Vec<ReportRow>, Vec<AccountId>), ApiError> {
        let mut rows = Vec::new();
        let mut ids = Vec::new();
        let mut stack: Vec<Open> = Vec::new();
        // The account a tag or account-total row belongs to.
        let mut current = String::new();
        for (i, e) in entries.iter().enumerate() {
            let kind = text(e, "rowKind").unwrap_or("account");
            let depth = e.get("depth").and_then(Value::as_u64).unwrap_or(0);
            let account = super::exact::id(e, "accountId", AccountId::parse)?;
            let level = u8::try_from(depth + 1).unwrap_or(u8::MAX);
            let pct = e.get("pctOfIncomeExact");
            let values = cells(self.cols, e.get("valuesExact").unwrap_or(&Value::Null), pct)?;
            match kind {
                "tag" => {
                    // `number · name`, like an account; the window writes the
                    // owner's label style (Q-271).
                    let label = account_caption(text(e, "tagNumber"), text(e, "tagName"));
                    let tag = text(e, "tagId").unwrap_or("");
                    let mut r = row(
                        RowKind::TagRow,
                        level.saturating_add(1),
                        Self::path(section, &stack, &current).child(format!("t:{tag}")),
                        label,
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
                        format!("@reports.row.total_of {}", named(e)),
                        values,
                    );
                    r.drill = self.row_drill(e)?;
                    rows.push(r);
                }
                _ => {
                    self.close(section, &mut stack, depth, &mut rows);
                    current = account.map_or_else(|| format!("row{i}"), |a| a.to_string());
                    let rollup = match e.get("rollupValuesExact") {
                        Some(r) if r.is_array() => Some(cells(self.cols, r, None)?),
                        _ => None,
                    };
                    let split = self.req.tag_split.is_some();
                    let mut r = row(
                        RowKind::Account,
                        level,
                        Self::path(section, &stack, &current),
                        named(e),
                        values,
                    );
                    r.expandable = rollup.is_some() || split;
                    let calculated = e.get("calculated") == Some(&Value::Bool(true));
                    r.drill = if calculated { None } else { self.row_drill(e)? };
                    rows.push(r);
                    let own: Vec<AccountId> = account.into_iter().collect();
                    ids.extend(own.iter().copied());
                    if let Some(mut rollup) = rollup {
                        // The roll-up row has no % of income of its own.
                        for c in &mut rollup {
                            if matches!(c, Cell::Percent(_)) {
                                *c = Cell::Percent(None);
                            }
                        }
                        stack.push(Open {
                            depth,
                            key: current.clone(),
                            caption: named(e),
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

fn header(key: &str, caption: &str, width: usize) -> ReportRow {
    let mut r = row(
        RowKind::Header,
        0,
        RowPath::of([key]),
        caption.to_string(),
        vec![Cell::Empty; width],
    );
    r.expandable = true;
    r
}

/// The table of a column-layout P&L or Balance Sheet.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
#[allow(
    clippy::too_many_lines,
    reason = "sections, then totals, in report order"
)]
pub fn table(v: &Value, req: &ReportRequest, balance: bool) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    let pct_totals = v.get("pctOfIncomeTotalsExact").filter(|p| p.is_object());
    let (heads, cols) = columns(v, req, balance, pct_totals.is_some())?;
    let span = req.dates.span().ok_or(shape("dates"))?;
    let b = Builder {
        req,
        cols: &cols,
        balance,
        to: span.to,
        from: Some(span.from),
    };
    let totals = v.get("totalsExact").ok_or(shape("totalsExact"))?;
    let total_cells = |key: &'static str| -> Result<Option<Vec<Cell>>, ApiError> {
        match totals.get(key) {
            Some(Value::Null) => Ok(None),
            Some(t) => cells(&cols, t, pct_totals.and_then(|p| p.get(key))).map(Some),
            None => Err(shape(key)),
        }
    };
    let named = |e: &Value| -> String {
        let calculated = e.get("calculated") == Some(&Value::Bool(true));
        match (calculated, text(e, "name")) {
            (true, Some("Net Income (Current Year)")) => "@reports.bs.net_income".into(),
            (true, Some("Retained Earnings (Prior Years)")) => "@reports.bs.retained_prior".into(),
            _ => account_caption(text(e, "accountNumber"), text(e, "name")),
        }
    };
    let width = heads.len();
    let mut rows = Vec::new();
    let sections: &[(&'static str, &'static str, &str, &str, &str, RowKind)] = if balance {
        &[
            (
                "assets",
                "totalAssets",
                "assets",
                "@reports.bs.assets",
                "@reports.bs.total_assets",
                RowKind::Total,
            ),
            (
                "liabilities",
                "totalLiabilities",
                "liabilities",
                "@reports.bs.liabilities",
                "@reports.bs.total_liabilities",
                RowKind::Subtotal,
            ),
            (
                "equity",
                "totalEquity",
                "equity",
                "@reports.bs.equity",
                "@reports.bs.total_equity",
                RowKind::Subtotal,
            ),
        ]
    } else {
        &[
            (
                "revenue",
                "totalRevenue",
                "income",
                "@reports.pl.income",
                "@reports.pl.total_income",
                RowKind::Subtotal,
            ),
            (
                "cogs",
                "totalCogs",
                "cogs",
                "@reports.pl.cogs",
                "@reports.pl.total_cogs",
                RowKind::Subtotal,
            ),
            (
                "expenses",
                "totalExpenses",
                "expenses",
                "@reports.pl.expenses",
                "@reports.pl.total_expenses",
                RowKind::Subtotal,
            ),
            (
                "otherRevenue",
                "totalOtherRevenue",
                "other_income",
                "@reports.pl.other_income",
                "@reports.pl.total_other_income",
                RowKind::Subtotal,
            ),
            (
                "otherExpenses",
                "totalOtherExpenses",
                "other_expenses",
                "@reports.pl.other_expenses",
                "@reports.pl.total_other_expenses",
                RowKind::Subtotal,
            ),
        ]
    };
    for (engine, total_key, key, title, total_title, kind) in sections {
        let entries = list(v, engine)?;
        let section_total = total_cells(total_key)?.ok_or(shape("totalsExact"))?;
        if !entries.is_empty() || balance {
            let (mut body, ids) = b.section(key, entries, &named)?;
            rows.push(header(key, title, width));
            rows.append(&mut body);
            let mut t = row(
                *kind,
                0,
                RowPath::of([format!("{key}~total")]),
                (*total_title).to_string(),
                section_total,
            );
            t.drill = b.lines(ids, None);
            rows.push(t);
        }
        if !balance
            && *engine == "cogs"
            && let Some(g) = total_cells("grossProfit")?
        {
            rows.push(row(
                RowKind::Total,
                0,
                RowPath::of(["gross_profit"]),
                "@reports.pl.gross_profit".into(),
                g,
            ));
        }
        if !balance
            && *engine == "expenses"
            && let Some(o) = total_cells("operatingIncome")?
        {
            rows.push(row(
                RowKind::Total,
                0,
                RowPath::of(["operating_income"]),
                "@reports.pl.operating_income".into(),
                o,
            ));
        }
    }
    let (grand_key, grand_path, grand_caption) = if balance {
        (
            "totalLiabilitiesAndEquity",
            "liabilities_and_equity",
            "@reports.bs.total_liabilities_and_equity",
        )
    } else {
        ("netIncome", "net_income", "@reports.pl.net_income")
    };
    let grand = total_cells(grand_key)?.ok_or(shape("totalsExact"))?;
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of([grand_path]),
        grand_caption.into(),
        grand,
    ));
    Ok(ReportTable {
        columns: heads,
        rows,
        meta: ReportMeta {
            basis: meta_basis(v).or(req.basis),
            dates: Some(req.dates.clone()),
            ..ReportMeta::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::report::{Basis, ReportDates, Source};

    fn fixture(name: &str) -> Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/reports")
            .join(format!("{name}.json"));
        serde_json::from_str(&std::fs::read_to_string(p).expect("fixture")).expect("json")
    }

    fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, dd).expect("date")
    }

    fn req(source: Source, dates: ReportDates) -> ReportRequest {
        let mut r = ReportRequest::new(source, dates);
        r.basis = Some(Basis::Accrual);
        r
    }

    fn amount(c: &Cell) -> Decimal {
        match c {
            Cell::Amount(x) => *x,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn periods_ytd_prior_and_pct_of_income() {
        let v = fixture("columns_profit_loss_periods");
        assert!(is_columns(&v));
        let r = req(
            Source::ProfitLoss,
            ReportDates::Ranges(vec![
                DateRange {
                    from: d(2033, 3, 1),
                    to: d(2033, 3, 15),
                },
                DateRange {
                    from: d(2033, 3, 16),
                    to: d(2033, 3, 31),
                },
            ]),
        );
        let t = table(&v, &r, false).expect("table");
        // 13 engine columns; % of income after each but the $ change ones.
        assert!(t.columns.iter().any(|c| c.key == "ytd"));
        assert!(t.columns.iter().any(|c| c.key == "total_pct_income"));
        let change = t
            .columns
            .iter()
            .find(|c| c.key == "p0_change")
            .expect("change");
        assert_eq!(change.drill, Some(ColumnDrill::None));
        let net = t.rows.last().expect("net");
        let total = t
            .columns
            .iter()
            .position(|c| c.key == "total")
            .expect("total");
        assert_eq!(amount(&net.cells[total]).to_string(), "-74.7000");
        assert!(net.cells.iter().all(|c| !matches!(c, Cell::Text(_))));
        // Every row has a cell per column.
        assert!(t.rows.iter().all(|r| r.cells.len() == t.columns.len()));
        // A $ change cell has no drill; a period cell does.
        let acct = t
            .rows
            .iter()
            .find(|r| r.kind == RowKind::Account)
            .expect("account");
        let d = acct.drill.as_ref().expect("drill");
        assert!(d.for_cell(change).is_none());
        assert!(d.for_cell(&t.columns[0]).is_some());
        // "Prior year" keeps the YTD comparison.
        let mut py = r.clone();
        py.extras.insert(cl_core::report::Extra::PriorYear);
        let t = table(&v, &py, false).expect("table");
        assert!(t.columns.iter().any(|c| c.key == "ytd_prior"));
    }

    /// Q-PR6a: on "Prior period" the engine compares the YTD column with
    /// the same number of days before it, so the YTD column stands alone.
    #[test]
    fn prior_period_leaves_out_the_ytd_day_count_comparison() {
        let v = fixture("columns_profit_loss_periods");
        let mut r = req(
            Source::ProfitLoss,
            ReportDates::Ranges(vec![
                DateRange {
                    from: d(2033, 3, 1),
                    to: d(2033, 3, 15),
                },
                DateRange {
                    from: d(2033, 3, 16),
                    to: d(2033, 3, 31),
                },
            ]),
        );
        r.extras.insert(cl_core::report::Extra::PriorPeriod);
        let t = table(&v, &r, false).expect("table");
        assert!(t.columns.iter().any(|c| c.key == "ytd"));
        assert!(t.columns.iter().any(|c| c.key == "p0_prior"));
        assert!(!t.columns.iter().any(|c| c.key.starts_with("ytd_prior")
            || c.key.starts_with("ytd_change")
            || c.key.starts_with("ytd_pct_change")));
        assert!(t.rows.iter().all(|r| r.cells.len() == t.columns.len()));
    }

    #[test]
    fn tag_split_rows_untagged_tags_and_total() {
        let v = fixture("columns_profit_loss_tag_split");
        let mut r = req(
            Source::ProfitLoss,
            ReportDates::Range(DateRange {
                from: d(2033, 3, 1),
                to: d(2033, 3, 31),
            }),
        );
        r.tag_split = Some(TagGroupId::parse("e2000773-7016-4df8-a1ee-46c854135039").expect("g"));
        let t = table(&v, &r, false).expect("table");
        let kinds: Vec<_> = t.rows.iter().take(4).map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            [
                RowKind::Header,
                RowKind::Account,
                RowKind::TagRow,
                RowKind::AccountTotal
            ]
        );
        // The untagged row drills to lines without a tag of the group.
        assert!(matches!(
            t.rows[1].drill,
            Some(Drill::Lines {
                tag: TagScope::UntaggedInGroup(_),
                ..
            })
        ));
        assert!(matches!(
            t.rows[2].drill,
            Some(Drill::Lines {
                tag: TagScope::Tag(_),
                ..
            })
        ));
        assert!(t.rows[2].path.is_under(&t.rows[1].path));
        assert!(
            !t.rows[3].path.is_under(&t.rows[1].path),
            "the total stays when collapsed"
        );
    }

    #[test]
    fn balance_sheet_columns_are_as_of_dates() {
        let v = fixture("columns_balance_sheet_periods");
        let r = req(
            Source::BalanceSheet,
            ReportDates::Ranges(vec![
                DateRange {
                    from: d(2033, 2, 1),
                    to: d(2033, 2, 28),
                },
                DateRange {
                    from: d(2033, 3, 1),
                    to: d(2033, 3, 31),
                },
            ]),
        );
        let t = table(&v, &r, true).expect("table");
        assert_eq!(
            t.columns[0].range,
            Some(DateRange {
                from: d(2033, 2, 28),
                to: d(2033, 2, 28)
            })
        );
        let ni = t
            .rows
            .iter()
            .find(|r| r.caption == "@reports.bs.net_income")
            .expect("ni");
        assert!(ni.drill.is_none(), "calculated rows have no drill");
    }

    #[test]
    fn client_columns_without_a_client_have_no_drill() {
        let v = fixture("columns_profit_loss_by_contact");
        let r = req(
            Source::ProfitLoss,
            ReportDates::Range(DateRange {
                from: d(2033, 3, 1),
                to: d(2033, 3, 31),
            }),
        );
        let t = table(&v, &r, false).expect("table");
        assert_eq!(t.columns[0].drill, Some(ColumnDrill::None));
        assert_eq!(t.columns[1].drill, None);
    }
}
