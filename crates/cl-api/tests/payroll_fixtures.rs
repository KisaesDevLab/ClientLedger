//! P30 WP0: the payroll fixtures and their goldens agree.
//!
//! Every golden in `fixtures/payroll/expected/` must equal an expectation
//! computed independently from the raw CSV with `rust_decimal`
//! (`support/payroll_expect.rs`). The bank file's withdrawals must equal the
//! net pay those expectations give, and no worker name may reach a golden.

#[path = "support/payroll_expect.rs"]
mod payroll_expect;

use payroll_expect::{
    FIXTURES, Side, column_cells, expect, fixture_bytes, golden, money, read_csv,
};
use rust_decimal::Decimal;

#[test]
fn every_golden_equals_the_independent_expectation() {
    for stem in FIXTURES {
        let g = golden(stem);
        let want = expect(stem, &g.check_date);
        assert_eq!(want.len(), g.entries.len(), "{stem}: entry count");
        for (w, e) in want.iter().zip(&g.entries) {
            assert_eq!(w.date, e.date, "{stem}: date");
            assert_eq!(
                w.sorted_lines(),
                e.sorted_lines(),
                "{stem} {}: lines",
                w.date
            );
            assert_eq!(w.total_debits, e.total_debits, "{stem} {}: debits", w.date);
            assert_eq!(
                w.total_credits, e.total_credits,
                "{stem} {}: credits",
                w.date
            );
            assert_eq!(
                e.total_debits, e.total_credits,
                "{stem} {}: balanced",
                w.date
            );
            assert!(
                w.lines.iter().all(|l| l.amount > Decimal::ZERO),
                "{stem}: no zero lines"
            );
        }
    }
}

#[test]
fn entries_follow_pay_dates_and_net_pay_stays_one_line() {
    let g = golden("gusto");
    let dates: Vec<&str> = g.entries.iter().map(|e| e.date.as_str()).collect();
    assert_eq!(dates, ["2025-01-10", "2025-01-24"]);
    for e in &g.entries {
        let clearing: Vec<_> = e
            .lines
            .iter()
            .filter(|l| l.line_type == "payroll_clearing")
            .collect();
        assert_eq!(clearing.len(), 1, "one net-pay line per date (U-246)");
        assert!(clearing[0].department.is_none());
        assert!(
            e.lines
                .iter()
                .filter(|l| l.side == Side::Credit)
                .all(|l| l.department.is_none()),
            "liability lines are never split"
        );
        let depts: Vec<_> = e
            .lines
            .iter()
            .filter(|l| l.line_type == "gross_wages_expense")
            .map(|l| l.department.clone())
            .collect();
        assert_eq!(depts.len(), 3, "Kitchen, Front and blank");
    }
    for stem in ["square", "quickbooks_payroll"] {
        let g = golden(stem);
        assert_eq!(g.entries.len(), 1);
        assert_eq!(
            g.entries[0].date, g.check_date,
            "{stem} takes the wizard's date (U-242)"
        );
    }
    assert!(
        golden("square").entries[0]
            .lines
            .iter()
            .any(|l| l.line_type == "other_tax_payable")
    );
}

#[test]
fn bank_file_withdrawals_equal_the_expected_net_pay() {
    let rows = read_csv(&fixture_bytes("bank_payroll.csv"));
    assert_eq!(rows[0], ["Date", "Description", "Amount"]);
    let bank: Vec<(String, Decimal)> = rows[1..]
        .iter()
        .map(|r| (r[0].clone(), money(&r[2])))
        .collect();
    assert!(
        bank.iter().all(|(_, a)| *a < Decimal::ZERO),
        "money out is negative in the file"
    );
    let mut expected: Vec<(String, Decimal)> = Vec::new();
    for e in expect("gusto", "2025-01-24") {
        expected.push((e.date.clone(), e.net_pay().expect("net pay")));
    }
    let adp = expect("adp_run_gli", "2025-02-14");
    let net = adp[0]
        .lines
        .iter()
        .find(|l| l.line_type == "Net Pay")
        .expect("ADP net pay line")
        .amount;
    expected.push((adp[0].date.clone(), net));
    for (date, amount) in &expected {
        assert!(
            bank.iter().any(|(d, a)| d == date && -*a == *amount),
            "a withdrawal of {amount} on {date}"
        );
    }
    assert_eq!(
        bank.len(),
        expected.len() + 1,
        "plus the one tax withdrawal"
    );
    let tax = bank
        .iter()
        .find(|(d, _)| d == "2025-01-12")
        .expect("tax withdrawal");
    assert!(
        expected.iter().all(|(_, a)| *a != -tax.1),
        "the tax withdrawal has no expected entry"
    );
}

/// S5: the Mode B correction pair reads as written by hand — the negative
/// debit is a credit, the negative credit a debit — and still balances.
#[test]
fn a_mode_b_correction_pair_swaps_sides() {
    let want = expect("paychex_correction", "2025-08-22");
    assert_eq!(want.len(), 1);
    let e = &want[0];
    let d = |s: &str| s.parse::<Decimal>().expect("decimal");
    let line = |desc: &str| {
        let l = e
            .lines
            .iter()
            .find(|l| l.line_type == desc)
            .unwrap_or_else(|| panic!("{desc}"));
        (l.side, l.amount)
    };
    assert_eq!(e.lines.len(), 8, "every row, the correction pair included");
    assert_eq!(line("Wage Correction"), (Side::Credit, d("50.05")));
    assert_eq!(line("Fed W/H Correction"), (Side::Debit, d("50.05")));
    assert_eq!(line("Gross Wages"), (Side::Debit, d("2500.15")));
    assert_eq!(line("Net Check"), (Side::Credit, d("2095.12")));
    assert_eq!(e.total_debits, d("2705.21"));
    assert_eq!(e.total_credits, d("2705.21"));
}

#[test]
fn no_worker_name_reaches_a_golden() {
    let dir = payroll_expect::fixtures_dir().join("expected");
    for entry in std::fs::read_dir(dir).expect("expected dir") {
        let text = std::fs::read_to_string(entry.expect("entry").path()).expect("golden");
        assert!(!text.contains("Test Worker"), "worker name in a golden");
    }
}

#[test]
fn fixture_dates_are_in_the_past_and_cents_only() {
    for stem in FIXTURES {
        let g = golden(stem);
        for e in &g.entries {
            assert!(e.date.as_str() < "2026-01-01", "{stem}: past dates only");
            assert!(e.lines.iter().all(|l| l.amount.scale() <= 2));
        }
    }
}

/// Proves the fixtures are awkward enough: for every Mode A file at least one
/// money column's `f64` sum differs from its exact sum. This is the only
/// floating point in the payroll work, and it exists to show why none is
/// used anywhere else.
#[test]
#[allow(clippy::float_arithmetic, clippy::float_cmp)]
fn float_sums_drift_on_every_employee_level_fixture() {
    let columns: [(&str, &[&str]); 5] = [
        (
            "gusto",
            &[
                "Gross Pay",
                "Net Pay",
                "Federal Income Tax",
                "Medicare (Employee)",
                "SUTA",
            ],
        ),
        (
            "quickbooks_payroll",
            &["Gross Pay", "Net Pay", "Federal Taxes", "Medicare Employee"],
        ),
        (
            "square",
            &[
                "Gross Pay",
                "Net Pay",
                "Total Employee Taxes",
                "SUTA",
                "FUTA",
            ],
        ),
        (
            "generic_period1",
            &["Wages", "Take Home", "Fed WH", "Medicare EE"],
        ),
        (
            "generic_period2",
            &[
                "Wages",
                "Take Home",
                "Fed WH",
                "State WH",
                "SS EE",
                "Medicare EE",
                "SS ER",
                "Medicare ER",
            ],
        ),
    ];
    for (stem, cols) in columns {
        let drifts = cols.iter().any(|c| {
            let cells = column_cells(stem, c);
            let exact: Decimal = cells.iter().map(|s| money(s)).sum();
            let float: f64 = cells
                .iter()
                .map(|s| money(s).to_string().parse::<f64>().expect("f64"))
                .sum();
            float != exact.to_string().parse::<f64>().expect("f64")
        });
        assert!(drifts, "{stem}: no column drifts in f64");
    }
}
