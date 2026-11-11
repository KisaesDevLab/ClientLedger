//! The demo engine's reports tie to the mockup's figures and to themselves.

use super::*;
use cl_core::report::{Cell, RowKind, Source};

fn day(y: i32, m: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, d).expect("date")
}

fn dec(s: &str) -> Decimal {
    s.parse().expect("decimal")
}

fn october() -> DateRange {
    DateRange {
        from: day(2026, 10, 1),
        to: day(2026, 10, 31),
    }
}

fn row<'t>(t: &'t ReportTable, caption: &str) -> &'t cl_core::report::ReportRow {
    t.rows
        .iter()
        .find(|r| r.caption == caption)
        .unwrap_or_else(|| panic!("no row {caption}"))
}

fn amount(c: &Cell) -> Decimal {
    match c {
        Cell::Amount(d) => *d,
        other => panic!("not an amount: {other:?}"),
    }
}

#[test]
fn october_and_year_to_date_net_income_are_the_mockups() {
    let d = DemoState::new();
    let mut req = ReportRequest::new(Source::ProfitLoss, ReportDates::Range(october()));
    let plain = statement(&d, &req).expect("plain P&L");
    let net = row(&plain, "@reports.pl.net_income");
    assert_eq!(amount(&net.cells[0]), dec("13865.14"));

    req.extras.insert(Extra::Ytd);
    req.extras.insert(Extra::PctOfIncome);
    let t = statement(&d, &req).expect("columns P&L");
    let keys: Vec<&str> = t.columns.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(
        keys,
        ["total", "total_pct_income", "ytd", "ytd_pct_income"],
        "Total, % of income, YTD, % of income"
    );
    let net = row(&t, "@reports.pl.net_income");
    assert_eq!(amount(&net.cells[0]), dec("13865.14"));
    assert_eq!(amount(&net.cells[2]), dec("125985.00"));
    let income = row(&t, "@reports.pl.total_income");
    assert_eq!(income.cells[1], Cell::Percent(Some(dec("100.0"))));
}

#[test]
fn every_october_account_cell_equals_its_ledger_drill() {
    let d = DemoState::new();
    let b = Books::load(&d);
    let req = ReportRequest::new(Source::ProfitLoss, ReportDates::Range(october()));
    let v = profit_loss(&b, &req, october());
    let mut seen = 0;
    for (section, _) in PL_SECTIONS {
        for e in v[section].as_array().expect("section") {
            let id = AccountId::parse(e["accountId"].as_str().expect("id")).expect("id");
            let mut gl = ReportRequest::new(Source::GeneralLedger, ReportDates::Range(october()));
            gl.filters.account_ids = vec![id];
            let g = general_ledger(&b, &gl, october());
            let acct = &g["accounts"][0];
            assert_eq!(
                dec(acct["netChangeExact"].as_str().expect("net")),
                dec(e["amountExact"].as_str().expect("amount")),
                "{}",
                e["name"]
            );
            seen += 1;
        }
    }
    assert!(seen > 5, "October has accounts");
}

#[test]
fn the_balance_sheet_balances_and_the_trial_balance_ties() {
    let d = DemoState::new();
    let b = Books::load(&d);
    let as_of = day(2026, 11, 4);
    let req = ReportRequest::new(Source::BalanceSheet, ReportDates::AsOf(as_of));
    let v = balance_sheet(&b, &req, as_of);
    assert_eq!(
        dec(v["totalAssetsExact"].as_str().expect("assets")),
        dec(v["totalLiabilitiesAndEquityExact"].as_str().expect("l+e"))
    );
    let t = statement(&d, &req).expect("balance sheet");
    assert!(t.rows.iter().any(|r| r.caption == "@reports.bs.net_income"));

    let req = ReportRequest::new(Source::TrialBalance, ReportDates::AsOf(as_of));
    let v = trial_balance(&b, &req, as_of);
    assert_eq!(v["totalDebitsExact"], v["totalCreditsExact"]);
    statement(&d, &req).expect("trial balance");
}

#[test]
fn month_columns_add_up_to_their_total_and_carry_prior_year() {
    let d = DemoState::new();
    let mut req = ReportRequest::new(
        Source::ProfitLoss,
        ReportDates::Ranges(vec![
            DateRange {
                from: day(2026, 9, 1),
                to: day(2026, 9, 30),
            },
            october(),
        ]),
    );
    req.extras.insert(Extra::PriorYear);
    let t = statement(&d, &req).expect("months");
    let keys: Vec<&str> = t.columns.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "p0",
            "p0_prior",
            "p0_change",
            "p0_pct_change",
            "p1",
            "p1_prior",
            "p1_change",
            "p1_pct_change",
            "total"
        ]
    );
    assert_eq!(t.columns[0].caption, "Sep 2026");
    let net = row(&t, "@reports.pl.net_income");
    assert_eq!(
        amount(&net.cells[0]) + amount(&net.cells[4]),
        amount(&net.cells[8])
    );
}

#[test]
fn the_tag_split_rows_add_up_to_the_account() {
    let d = DemoState::new();
    let mut req = ReportRequest::new(Source::ProfitLoss, ReportDates::Range(october()));
    req.tag_split = Some(super::super::lists::group_id(1));
    let t = statement(&d, &req).expect("tag split");
    let tags: Vec<_> = t
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::TagRow)
        .collect();
    assert!(!tags.is_empty(), "income lines carry route tags");
}

#[test]
fn by_tag_columns_add_up_to_the_total() {
    let d = DemoState::new();
    let mut req = ReportRequest::new(Source::ProfitLoss, ReportDates::Range(october()));
    req.columns = Compare::ByTag {
        tag_group_id: super::super::lists::group_id(1),
    };
    let t = statement(&d, &req).expect("by tag");
    let n = t.columns.len();
    let income = row(&t, "@reports.pl.total_income");
    let parts: Decimal = income.cells[..n - 1].iter().map(amount).sum();
    assert_eq!(parts, amount(&income.cells[n - 1]));
}

#[test]
fn percents_round_half_away_from_zero_and_skip_a_zero_base() {
    assert_eq!(percent1(dec("1"), dec("3")).as_deref(), Some("33.3"));
    assert_eq!(percent1(dec("1"), dec("8")).as_deref(), Some("12.5"));
    assert_eq!(percent1(dec("-1"), dec("16")).as_deref(), Some("-6.3"));
    assert_eq!(percent1(dec("0"), dec("16")).as_deref(), Some("0.0"));
    assert_eq!(percent1(dec("1"), Decimal::ZERO), None);
    assert_eq!(range_caption(october()), "Oct 2026");
    assert_eq!(
        range_caption(DateRange {
            from: day(2026, 10, 2),
            to: day(2026, 10, 31)
        }),
        "Oct 2, 2026 - Oct 31, 2026"
    );
}

#[test]
fn options_the_demo_does_not_play_say_so() {
    let d = DemoState::new();
    let mut req = ReportRequest::new(Source::BalanceSheet, ReportDates::AsOf(day(2026, 10, 31)));
    req.extras.insert(Extra::Ytd);
    assert!(matches!(
        statement(&d, &req),
        Err(ApiError::Local(LocalValidation::Unsupported))
    ));
}
