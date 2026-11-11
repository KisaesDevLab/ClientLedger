//! Exact reports against the real engine (P28 WP2, U-14 `reports-exact-v1`).
//!
//! The postings live in 2033, a year no other contract test posts in, and
//! every line carries one fresh tag, so the tag-filtered statements show
//! only this test's lines. `CL_REPORTS_RECORD=<dir>` writes each raw answer
//! there (how the adapter fixtures in `tests/fixtures/reports/` were
//! recorded).

use cl_api::resources::reports::run::{COLUMNS, DRILL};
use cl_api::{
    AccountType, ApiError, Capabilities, JeLine, LocalValidation, Method, NewJournalEntry, Session,
    Side, TagFields, TagGroupFields,
};
use cl_core::ids::{AccountId, TagGroupId, TagId};
use cl_core::report::{
    Basis, Cell, DateRange, Drill, Extra, ReportDates, ReportRequest, ReportTable, RowKind, Source,
    TagScope,
};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::common::{account, date, key, m, post};
use crate::support::{Harness, unique};

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::REPORTS_EXACT, "U-14")
}

fn tagged(a: AccountId, side: Side, tag: TagId) -> JeLine {
    JeLine {
        account: a,
        side,
        description: None,
        tag: Some(tag),
        contact: None,
    }
}

fn entry(on: (u32, u32), lines: Vec<JeLine>) -> NewJournalEntry {
    NewJournalEntry::new(
        date(2033, on.0, on.1),
        Some(unique("exact")),
        cl_api::Basis::Both,
        lines,
    )
    .expect("balanced")
}

/// A raw exact GET; records it when `CL_REPORTS_RECORD` is set.
fn get(h: &Harness, s: &Session, name: &str, path: &str, q: &[(&str, &str)]) -> Value {
    let mut query = q.to_vec();
    query.push(("exact", "1"));
    let mut r = h
        .run(s.raw(Method::Get, path, &query, None))
        .unwrap_or_else(|e| panic!("{path}: {e}"));
    // The suite shares the engine's 30-a-minute report limit.
    for _ in 0..10 {
        if r.status != 429 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(15));
        r = h
            .run(s.raw(Method::Get, path, &query, None))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
    }
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    if let Ok(dir) = std::env::var("CL_REPORTS_RECORD") {
        let p = std::path::Path::new(&dir).join(format!("{name}.json"));
        std::fs::write(&p, serde_json::to_string_pretty(&r.body).expect("json")).expect("write");
    }
    r.body
}

/// The test's books: a revenue, a parent expense with a sub-account, a
/// bank and a liability; 0.10 + 0.20 of revenue (the float trap), 50.00 of
/// sub-account expense paid from the bank and 25.00 owed.
pub struct Books {
    pub group: TagGroupId,
    pub tag: TagId,
    pub revenue: AccountId,
    pub expense: AccountId,
    pub sub_expense: AccountId,
    pub bank: AccountId,
    pub owed: AccountId,
}

pub fn books(h: &Harness, s: &Session) -> Books {
    let group = h
        .run(s.tags().create_group(
            &TagGroupFields {
                name: unique("Exact group"),
                description: None,
                is_single_select: false,
            },
            key(),
        ))
        .expect("tag group")
        .id;
    let tag = h
        .run(s.tags().create(
            &TagFields {
                name: unique("Exact"),
                color: None,
                group_id: Some(group),
                description: None,
                tag_number: None,
            },
            key(),
        ))
        .expect("tag")
        .record
        .id;
    let revenue = account(h, s, AccountType::Revenue, "ExactRev").id;
    let expense = account(h, s, AccountType::Expense, "ExactExp").id;
    let mut sub = crate::common::account_fields(AccountType::Expense, "ExactSub");
    sub.parent_id = Some(expense);
    let sub_expense = h
        .run(s.accounts().create(&sub, key()))
        .expect("sub-account")
        .record
        .id;
    let bank = account(h, s, AccountType::Asset, "ExactBank").id;
    let owed = account(h, s, AccountType::Liability, "ExactOwed").id;
    for (on, amt) in [((3, 5), "0.10"), ((3, 6), "0.20")] {
        post(
            h,
            s,
            &entry(
                on,
                vec![
                    tagged(bank, Side::Debit(m(amt)), tag),
                    tagged(revenue, Side::Credit(m(amt)), tag),
                ],
            ),
        );
    }
    post(
        h,
        s,
        &entry(
            (3, 10),
            vec![
                tagged(sub_expense, Side::Debit(m("50.00")), tag),
                tagged(bank, Side::Credit(m("50.00")), tag),
            ],
        ),
    );
    post(
        h,
        s,
        &entry(
            (3, 12),
            vec![
                tagged(expense, Side::Debit(m("25.00")), tag),
                tagged(owed, Side::Credit(m("25.00")), tag),
            ],
        ),
    );
    Books {
        group,
        tag,
        revenue,
        expense,
        sub_expense,
        bank,
        owed,
    }
}

/// Records every PR-A route WP2 reads (accrual and cash), and checks that
/// they answer with their exact fields.
pub fn record_statements(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let tag = b.tag.to_string();
    let ids = format!("{},{},{}", b.bank, b.expense, b.sub_expense);
    let range = [("start_date", "2033-03-01"), ("end_date", "2033-03-31")];
    for basis in ["accrual", "cash"] {
        let mut q = range.to_vec();
        q.push(("basis", basis));
        q.push(("tag_id", tag.as_str()));
        let pl = get(
            h,
            s,
            &format!("profit_loss_{basis}"),
            "/api/v1/reports/profit-loss",
            &q,
        );
        assert_eq!(pl["netIncomeExact"], "-74.7000", "{pl}");
        let tb = get(
            h,
            s,
            &format!("trial_balance_{basis}"),
            "/api/v1/reports/trial-balance",
            &q,
        );
        assert!(tb["totalDebitsExact"].is_string(), "{tb}");
        let bs = get(
            h,
            s,
            &format!("balance_sheet_{basis}"),
            "/api/v1/reports/balance-sheet",
            &[
                ("as_of_date", "2033-03-31"),
                ("basis", basis),
                ("tag_id", tag.as_str()),
            ],
        );
        assert!(bs["totalAssetsExact"].is_string(), "{bs}");
        let mut gq = range.to_vec();
        gq.push(("basis", basis));
        gq.push(("account_ids", ids.as_str()));
        let gl = get(
            h,
            s,
            &format!("general_ledger_{basis}"),
            "/api/v1/reports/general-ledger",
            &gq,
        );
        assert!(gl["totalDebitsExact"].is_string(), "{gl}");
    }
    let mut q = range.to_vec();
    q.push(("tag_id", tag.as_str()));
    let _ = get(
        h,
        s,
        "revenue_by_category",
        "/api/v1/reports/revenue-by-category",
        &q,
    );
    let _ = get(
        h,
        s,
        "expense_by_category",
        "/api/v1/reports/expense-by-category",
        &q,
    );
    let mut d = q.clone();
    d.push(("display", "detail"));
    let _ = get(
        h,
        s,
        "expense_by_category_detail",
        "/api/v1/reports/expense-by-category",
        &d,
    );
    let _ = get(
        h,
        s,
        "transaction_list",
        "/api/v1/reports/transaction-list",
        &q,
    );
    let _ = get(
        h,
        s,
        "journal_entry_report",
        "/api/v1/reports/journal-entry-report",
        &q,
    );
    let mut l = range.to_vec();
    l.push(("account_ids", ids.as_str()));
    l.push(("limit", "2"));
    let _ = get(
        h,
        s,
        "general_ledger_limit",
        "/api/v1/reports/general-ledger",
        &l,
    );
    if s.client().capabilities().has(COLUMNS) {
        record_columns(h, s, &b);
    }
    // Why the client refuses PR-B options itself: an engine without
    // `reports-columns-v1` answers `period_ranges` with the plain report.
    if s.client().capabilities().has(COLUMNS) {
        return;
    }
    let mut pr = q.clone();
    pr.push((
        "period_ranges",
        "2033-03-01..2033-03-15,2033-03-16..2033-03-31",
    ));
    let probe = h
        .run(s.raw(
            Method::Get,
            "/api/v1/reports/profit-loss",
            &[&pr[..], &[("exact", "1")]].concat(),
            None,
        ))
        .expect("probe");
    assert_eq!(probe.status, 200);
    assert!(probe.body.get("columns").is_none(), "{}", probe.body);
}

fn dec(c: &Cell) -> Decimal {
    match c {
        Cell::Amount(d) => *d,
        other => panic!("not an amount: {other:?}"),
    }
}

/// Runs `r`, waiting out the engine's 30-a-minute report limit
/// (`expensiveOpLimiter`), which the whole suite shares.
fn run(h: &Harness, s: &Session, r: &ReportRequest) -> ReportTable {
    for _ in 0..10 {
        match h.run(s.reports().run(r)) {
            Ok(t) => return t,
            Err(ApiError::RateLimited { retry_after }) => {
                std::thread::sleep(retry_after.unwrap_or(std::time::Duration::from_secs(15)));
            }
            Err(e) => panic!("{:?}: {e}", r.source),
        }
    }
    panic!("{:?}: still rate limited", r.source)
}

/// The ledger's net (debits - credits) over a drill's lines.
fn drill_net(h: &Harness, s: &Session, d: &Drill) -> Decimal {
    let Drill::Lines {
        account_ids,
        from,
        to,
        tag,
        basis,
        adjusting,
        ..
    } = d
    else {
        panic!("not a lines drill: {d:?}")
    };
    let from = from.unwrap_or_else(|| date(1900, 1, 1));
    let mut r = ReportRequest::new(
        Source::GeneralLedger,
        ReportDates::Range(DateRange { from, to: *to }),
    );
    r.basis = *basis;
    r.filters.account_ids.clone_from(account_ids);
    r.filters.adjusting = *adjusting;
    if let TagScope::Tag(t) = tag {
        r.filters.tag_ids = vec![*t];
    }
    let gl = run(h, s, &r);
    // Each account's totals row: debits - credits.
    gl.rows
        .iter()
        .filter(|r| r.kind == RowKind::AccountTotal)
        .map(|r| dec(&r.cells[5]) - dec(&r.cells[6]))
        .sum()
}

/// The statements through `Reports::run`: the P&L's 0.10 + 0.20 stays
/// 0.3000, every P&L amount, parent total and subtotal drills to ledger
/// lines that sum to it exactly (AC2 for account and subtotal cells), the
/// Trial Balance and the Balance Sheet balance and a sheet balance drills
/// to the GL's ending balance; options that need `reports-columns-v1` or
/// `reports-drill-v1` are refused locally, never sent to an engine that
/// would ignore them (Q-220).
pub fn statements_drill_and_tie(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let march = DateRange {
        from: date(2033, 3, 1),
        to: date(2033, 3, 31),
    };
    for basis in [Basis::Accrual, Basis::Cash] {
        let mut r = ReportRequest::new(Source::ProfitLoss, ReportDates::Range(march));
        r.basis = Some(basis);
        r.filters.tag_ids = vec![b.tag];
        let pl = run(h, s, &r);
        let net = pl.rows.last().expect("net income");
        assert_eq!(net.kind, RowKind::Grand);
        assert_eq!(dec(&net.cells[0]).to_string(), "-74.7000");
        let revenue = pl
            .rows
            .iter()
            .find(|x| x.kind == RowKind::Account && x.path.0.contains(&b.revenue.to_string()))
            .expect("revenue row");
        assert_eq!(dec(&revenue.cells[0]).to_string(), "0.3000");
        let mut drilled = 0;
        for row in &pl.rows {
            let Some(d) = &row.drill else { continue };
            let d = d.at(&pl.columns[0]);
            let net = drill_net(h, s, &d);
            // Income is credit-positive on the P&L.
            let cell = dec(&row.cells[0]);
            let income = row
                .path
                .0
                .first()
                .is_some_and(|p| p.starts_with("income") || p.starts_with("other_income"));
            let want = if income { -cell } else { cell };
            assert_eq!(net, want, "{basis:?} drill of {}", row.caption);
            drilled += 1;
        }
        assert!(
            drilled >= 5,
            "accounts, the parent total and subtotals drill"
        );

        let as_of = date(2033, 3, 31);
        let mut t = ReportRequest::new(Source::TrialBalance, ReportDates::AsOf(as_of));
        t.basis = Some(basis);
        t.fiscal_year_start = Some(date(2033, 1, 1));
        t.filters.tag_ids = vec![b.tag];
        let tb = run(h, s, &t);
        let grand = tb.rows.last().expect("total");
        assert_eq!(dec(&grand.cells[0]), dec(&grand.cells[1]), "TB balances");

        let mut bs = ReportRequest::new(Source::BalanceSheet, ReportDates::AsOf(as_of));
        bs.basis = Some(basis);
        bs.fiscal_year_start = Some(date(2033, 1, 1));
        bs.filters.tag_ids = vec![b.tag];
        let sheet = run(h, s, &bs);
        let assets = sheet
            .rows
            .iter()
            .find(|x| x.path.0 == ["assets~total"])
            .expect("total assets");
        let grand = sheet.rows.last().expect("L&E");
        assert_eq!(
            dec(&assets.cells[0]),
            dec(&grand.cells[0]),
            "sheet balances"
        );
        let bank = sheet
            .rows
            .iter()
            .find(|x| x.path.0.contains(&b.bank.to_string()))
            .expect("bank");
        let d = bank.drill.as_ref().expect("drill").at(&sheet.columns[0]);
        assert_eq!(drill_net(h, s, &d), dec(&bank.cells[0]), "bank balance");
        let owed = sheet
            .rows
            .iter()
            .find(|x| x.path.0.contains(&b.owed.to_string()))
            .expect("owed");
        // A liability is credit-positive on the sheet.
        let d = owed.drill.as_ref().expect("drill").at(&sheet.columns[0]);
        assert_eq!(drill_net(h, s, &d), -dec(&owed.cells[0]), "owed balance");
        let ni = sheet
            .rows
            .iter()
            .find(|x| x.caption == "@reports.bs.net_income")
            .expect("net income");
        assert!(matches!(ni.drill, Some(Drill::ProfitLoss { .. })));
    }

    let mut e = ReportRequest::new(Source::ExpenseByCategory, ReportDates::Range(march));
    e.basis = Some(Basis::Accrual);
    e.filters.tag_ids = vec![b.tag];
    let cat = run(h, s, &e);
    assert_eq!(
        dec(&cat.rows.last().expect("total").cells[0]).to_string(),
        "75.0000"
    );

    let mut g = ReportRequest::new(Source::GeneralLedger, ReportDates::Range(march));
    g.filters.account_ids = vec![b.bank, b.expense, b.sub_expense];
    let gl = run(h, s, &g);
    assert!(gl.meta.truncated.is_none());
    assert_eq!(
        gl.rows.iter().filter(|r| r.kind == RowKind::Detail).count(),
        5
    );

    // PR-B options are refused here, not sent.
    let caps = s.client().capabilities();
    if !caps.has(COLUMNS) {
        let mut p = ReportRequest::new(Source::ProfitLoss, ReportDates::Range(march));
        p.extras.insert(Extra::PctOfIncome);
        assert!(matches!(
            h.run(s.reports().run(&p)),
            Err(ApiError::Local(LocalValidation::Unsupported))
        ));
        h.pending("U-201", "period, YTD, prior, compare columns and tag split");
    }
    if !caps.has(DRILL) {
        let mut f = ReportRequest::new(Source::GeneralLedger, ReportDates::Range(march));
        f.filters.cleared = Some(cl_core::report::Cleared::Cleared);
        assert!(matches!(
            h.run(s.reports().run(&f)),
            Err(ApiError::Local(LocalValidation::Unsupported))
        ));
        h.pending(
            "U-202",
            "GL drill filters: client, untagged, amount, cleared, types",
        );
    }
}

/// Records the U-201 column layouts (`reports-columns-v1`).
fn record_columns(h: &Harness, s: &Session, b: &Books) {
    let g = b.group.to_string();
    let base = [("start_date", "2033-03-01"), ("end_date", "2033-03-31")];
    let pl = "/api/v1/reports/profit-loss";
    let with = |extra: &[(&'static str, &'static str)]| -> Vec<(&'static str, &'static str)> {
        let mut q = base.to_vec();
        q.extend_from_slice(extra);
        q
    };
    let periods = with(&[
        (
            "period_ranges",
            "2033-03-01..2033-03-15,2033-03-16..2033-03-31",
        ),
        ("ytd", "1"),
        ("fiscal_year_start", "2033-01-01"),
        ("pct_of_income", "1"),
        ("prior", "year"),
    ]);
    let v = get(h, s, "columns_profit_loss_periods", pl, &periods);
    assert_eq!(v["layout"], "columns", "{v}");
    let mut split = with(&[("tag_split", "1")]);
    split.push(("tag_group_id", Box::leak(g.clone().into_boxed_str())));
    let _ = get(h, s, "columns_profit_loss_tag_split", pl, &split);
    let mut by_tag = with(&[("compare", "by_tag")]);
    by_tag.push(("tag_group_id", Box::leak(g.into_boxed_str())));
    let _ = get(h, s, "columns_profit_loss_by_tag", pl, &by_tag);
    let _ = get(
        h,
        s,
        "columns_profit_loss_by_contact",
        pl,
        &with(&[("compare", "by_contact")]),
    );
    let _ = get(
        h,
        s,
        "columns_balance_sheet_periods",
        "/api/v1/reports/balance-sheet",
        &[
            ("as_of_date", "2033-03-31"),
            (
                "period_ranges",
                "2033-02-01..2033-02-28,2033-03-01..2033-03-31",
            ),
            ("prior", "year"),
        ],
    );
}

/// The ledger's net behind a drill with any U-202 scope (client, untagged
/// in group), read through `Reports::run` on the General Ledger.
fn scoped_net(h: &Harness, s: &Session, d: &Drill) -> Decimal {
    let Drill::Lines {
        account_ids,
        from,
        to,
        contact,
        tag,
        basis,
        adjusting,
    } = d
    else {
        panic!("not a lines drill: {d:?}")
    };
    let mut r = ReportRequest::new(
        Source::GeneralLedger,
        ReportDates::Range(DateRange {
            from: from.unwrap_or_else(|| date(1900, 1, 1)),
            to: *to,
        }),
    );
    r.basis = *basis;
    r.filters.account_ids.clone_from(account_ids);
    r.filters.adjusting = *adjusting;
    match contact {
        cl_core::report::ContactScope::Any => {}
        cl_core::report::ContactScope::One(c) => r.filters.contact_ids = vec![*c],
        cl_core::report::ContactScope::NoContact => r.filters.no_contact = true,
        cl_core::report::ContactScope::Others(v) => r.filters.other_contacts_than = Some(v.clone()),
    }
    match tag {
        TagScope::Tag(t) => r.filters.tag_ids = vec![*t],
        TagScope::UntaggedInGroup(g) => r.filters.untagged_in_group = Some(*g),
        TagScope::Any => {}
        TagScope::Untagged => r.filters.untagged = true,
    }
    run(h, s, &r)
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::AccountTotal)
        .map(|r| dec(&r.cells[5]) - dec(&r.cells[6]))
        .sum()
}

/// U-201 and U-202 (`reports-columns-v1`, `reports-drill-v1`): every amount
/// cell of a P&L with period, YTD and prior-year columns drills to ledger
/// lines that sum to it (AC2 for period, YTD and subtotal cells); under a
/// tag split, untagged + tags = the account total and each row drills to
/// its own lines (AC5); a Balance Sheet with as-of columns drills each
/// balance from the first entry.
pub fn columns_drill_and_tag_split(h: &Harness) {
    if !caps(h) || !h.require_cap(COLUMNS, "U-201") || !h.require_cap(DRILL, "U-202") {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let first = DateRange {
        from: date(2033, 3, 1),
        to: date(2033, 3, 15),
    };
    let second = DateRange {
        from: date(2033, 3, 16),
        to: date(2033, 3, 31),
    };
    let check = |t: &ReportTable, balance: bool| -> usize {
        let mut n = 0;
        for row in &t.rows {
            let Some(d) = &row.drill else { continue };
            for (i, c) in t.columns.iter().enumerate() {
                let Cell::Amount(cell) = row.cells[i] else {
                    continue;
                };
                let Some(cd) = d.for_cell(c) else { continue };
                let credit = row.path.0.first().is_some_and(|p| {
                    p.starts_with("income")
                        || p.starts_with("other_income")
                        || p.starts_with("liabilities")
                        || p.starts_with("equity")
                });
                let want = if credit { -cell } else { cell };
                assert_eq!(
                    scoped_net(h, s, &cd),
                    want,
                    "{} ({}) balance={balance}",
                    row.caption,
                    c.key
                );
                n += 1;
            }
        }
        n
    };

    // Period, YTD and prior-year columns with % of income, by tag.
    let mut p = ReportRequest::new(Source::ProfitLoss, ReportDates::Ranges(vec![first, second]));
    p.basis = Some(Basis::Accrual);
    p.extras.insert(Extra::Ytd);
    p.extras.insert(Extra::PriorYear);
    p.extras.insert(Extra::PctOfIncome);
    p.fiscal_year_start = Some(date(2033, 1, 1));
    p.filters.tag_ids = vec![b.tag];
    let pl = run(h, s, &p);
    let net = pl.rows.last().expect("net");
    let total = pl
        .columns
        .iter()
        .position(|c| c.key == "total")
        .expect("total");
    assert_eq!(dec(&net.cells[total]).to_string(), "-74.7000");
    assert!(
        check(&pl, false) > 10,
        "period, YTD and subtotal cells drill"
    );

    // Tag split by the books' group: untagged + tags = account total.
    let mut t = ReportRequest::new(
        Source::ProfitLoss,
        ReportDates::Range(DateRange {
            from: first.from,
            to: second.to,
        }),
    );
    t.basis = Some(Basis::Accrual);
    t.tag_split = Some(b.group);
    let split = run(h, s, &t);
    let mut accounts = 0;
    for (i, r) in split.rows.iter().enumerate() {
        if r.kind != RowKind::Account
            || !r
                .path
                .0
                .iter()
                .any(|p| *p == b.revenue.to_string() || *p == b.expense.to_string())
        {
            continue;
        }
        let mut sum = dec(&r.cells[0]);
        let mut j = i + 1;
        while split.rows[j].kind == RowKind::TagRow {
            sum += dec(&split.rows[j].cells[0]);
            j += 1;
        }
        assert_eq!(split.rows[j].kind, RowKind::AccountTotal);
        assert_eq!(sum, dec(&split.rows[j].cells[0]), "untagged + tags = total");
        accounts += 1;
    }
    assert!(accounts >= 2);
    check(&split, false);

    // Balance Sheet as-of columns, by tag.
    let mut bs = ReportRequest::new(
        Source::BalanceSheet,
        ReportDates::Ranges(vec![first, second]),
    );
    bs.basis = Some(Basis::Accrual);
    bs.filters.tag_ids = vec![b.tag];
    let sheet = run(h, s, &bs);
    assert!(check(&sheet, true) > 2, "balances drill");

    // The Journal: the books' four entries, each balanced, by date.
    let mut j = ReportRequest::new(
        Source::Journal,
        ReportDates::Range(DateRange {
            from: first.from,
            to: second.to,
        }),
    );
    j.filters.tag_ids = vec![b.tag];
    let journal = run(h, s, &j);
    let heads = journal
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::Header)
        .count();
    assert_eq!(heads, 4, "four entries");
    for r in journal
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::AccountTotal)
    {
        assert_eq!(dec(&r.cells[5]), dec(&r.cells[6]), "each entry balances");
    }
    let grand = journal.rows.last().expect("total");
    assert_eq!(dec(&grand.cells[5]).to_string(), "75.3000");
}
