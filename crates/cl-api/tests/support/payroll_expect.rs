//! Independent expectations for the payroll fixtures (P30 WP0).
//!
//! Reads a fixture CSV byte for byte and computes, with `rust_decimal`
//! only, the summary entries the engine must post for it. Nothing here comes
//! from the engine or the demo: the column meanings are written out per
//! provider below, and the line rules follow the plan (§A U-242, U-243,
//! U-246). The goldens in `fixtures/payroll/expected/` must equal this.
//!
//! Used by `tests/payroll_fixtures.rs` and the payroll contract test.
#![allow(dead_code, clippy::struct_field_names)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use rust_decimal::Decimal;
use serde_json::Value;

/// The repository's `fixtures/payroll` folder.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/payroll")
}

/// A fixture's bytes.
pub fn fixture_bytes(file: &str) -> Vec<u8> {
    std::fs::read(fixtures_dir().join(file)).unwrap_or_else(|e| panic!("{file}: {e}"))
}

/// A fixture's golden.
pub fn golden(stem: &str) -> Golden {
    let path = fixtures_dir().join("expected").join(format!("{stem}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{stem}: {e}"));
    let v: Value = serde_json::from_str(&text).expect("golden json");
    Golden::from_json(&v)
}

/// A minimal RFC 4180 reader: quoted cells, doubled quotes, CRLF or LF.
pub fn read_csv(bytes: &[u8]) -> Vec<Vec<String>> {
    let text = std::str::from_utf8(bytes).expect("utf-8 fixture");
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    cell.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => cell.push(c),
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            _ => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// A money cell as the engine reads it (U-250): `$`, `,`, spaces, `(x)` and
/// a trailing `-`; blank is zero; more than 2 decimals is refused.
pub fn money(cell: &str) -> Decimal {
    let mut s: String = cell
        .chars()
        .filter(|c| !matches!(c, '$' | ',' | ' '))
        .collect();
    if s.is_empty() {
        return Decimal::ZERO;
    }
    let mut negative = false;
    if s.starts_with('(') && s.ends_with(')') {
        negative = true;
        s = s[1..s.len() - 1].to_string();
    }
    if let Some(rest) = s.strip_suffix('-') {
        negative = true;
        s = rest.to_string();
    }
    let d: Decimal = s.parse().unwrap_or_else(|_| panic!("not money: {cell:?}"));
    assert!(d.normalize().scale() <= 2, "over 2 decimals: {cell:?}");
    if negative { -d } else { d }
}

/// `MM/DD/YYYY` or `YYYY-MM-DD` to ISO.
pub fn iso_date(cell: &str) -> String {
    let c = cell.trim();
    if let Some((m, rest)) = c.split_once('/') {
        let (d, y) = rest.split_once('/').expect("MM/DD/YYYY");
        return format!("{y}-{m:0>2}-{d:0>2}");
    }
    c.to_string()
}

/// Debit or credit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    Debit,
    Credit,
}

/// One expected line.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Line {
    pub line_type: String,
    pub department: Option<String>,
    pub side: Side,
    pub amount: Decimal,
}

/// One expected entry (one pay date).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub date: String,
    pub lines: Vec<Line>,
    pub total_debits: Decimal,
    pub total_credits: Decimal,
}

impl Entry {
    fn new(date: String, lines: Vec<Line>) -> Entry {
        let sum = |side| {
            lines
                .iter()
                .filter(|l| l.side == side)
                .map(|l| l.amount)
                .sum::<Decimal>()
        };
        Entry {
            total_debits: sum(Side::Debit),
            total_credits: sum(Side::Credit),
            date,
            lines,
        }
    }

    /// The lines sorted, for order-free comparison.
    pub fn sorted_lines(&self) -> Vec<Line> {
        let mut v = self.lines.clone();
        v.sort();
        v
    }

    /// The net-pay line (Mode A `payroll_clearing`).
    pub fn net_pay(&self) -> Option<Decimal> {
        self.lines
            .iter()
            .find(|l| l.line_type == "payroll_clearing")
            .map(|l| l.amount)
    }
}

/// A golden file.
#[derive(Clone, Debug)]
pub struct Golden {
    pub provider: String,
    pub mode: String,
    pub period_start: String,
    pub period_end: String,
    pub check_date: String,
    pub entries: Vec<Entry>,
}

impl Golden {
    fn from_json(v: &Value) -> Golden {
        let s = |k: &str| v[k].as_str().unwrap_or_else(|| panic!("{k}")).to_string();
        let dec = |x: &Value| -> Decimal {
            x.as_str()
                .expect("decimal string")
                .parse()
                .expect("decimal")
        };
        let entries = v["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|e| {
                let lines = e["lines"]
                    .as_array()
                    .expect("lines")
                    .iter()
                    .map(|l| Line {
                        line_type: l["lineType"].as_str().expect("lineType").to_string(),
                        department: l["department"].as_str().map(str::to_string),
                        side: match l["side"].as_str() {
                            Some("debit") => Side::Debit,
                            Some("credit") => Side::Credit,
                            other => panic!("side {other:?}"),
                        },
                        amount: dec(&l["amount"]),
                    })
                    .collect();
                Entry {
                    date: e["date"].as_str().expect("date").to_string(),
                    lines,
                    total_debits: dec(&e["totalDebits"]),
                    total_credits: dec(&e["totalCredits"]),
                }
            })
            .collect();
        Golden {
            provider: s("provider"),
            mode: s("mode"),
            period_start: s("periodStart"),
            period_end: s("periodEnd"),
            check_date: s("checkDate"),
            entries,
        }
    }
}

/// Mode A (one row per worker): which header feeds which engine field.
pub struct ModeASpec {
    /// Rows before the header row.
    pub header_row: usize,
    /// Rows after the data to drop (total rows).
    pub skip_footer: usize,
    /// The worker column (rows where it is blank or "Total" are skipped).
    pub employee: &'static str,
    /// The pay-date column; `None` = the wizard's check date (U-242).
    pub check_date: Option<&'static str>,
    /// The department column (U-246 splits expenses by it).
    pub department: Option<&'static str>,
    /// `(engine field, header)`.
    pub money: &'static [(&'static str, &'static str)],
}

/// The fixtures, with their spec or Mode B column names.
pub enum Spec {
    A(ModeASpec),
    /// Mode B: date, description, then the amount convention.
    B {
        date: &'static str,
        description: &'static str,
        amounts: ModeBAmounts,
    },
}

/// How a Mode B file carries amounts.
pub enum ModeBAmounts {
    DebitCredit(&'static str, &'static str),
    /// Amount column plus a Category column (Expense/Cost = debit).
    Category(&'static str, &'static str),
}

/// Every payroll fixture stem.
pub const FIXTURES: [&str; 8] = [
    "gusto",
    "adp_run_gli",
    "quickbooks_payroll",
    "paychex_flex_gls",
    "square",
    "onpay_gl_summary",
    "generic_period1",
    "generic_period2",
];

/// The written-out column meanings per fixture.
pub fn spec(stem: &str) -> Spec {
    match stem {
        "gusto" => Spec::A(ModeASpec {
            header_row: 0,
            skip_footer: 0,
            employee: "Employee Name",
            check_date: Some("Check Date"),
            department: Some("Department"),
            money: &[
                ("gross_pay", "Gross Pay"),
                ("federal_income_tax", "Federal Income Tax"),
                ("state_income_tax", "State Income Tax"),
                ("social_security_ee", "Social Security (Employee)"),
                ("medicare_ee", "Medicare (Employee)"),
                ("health_insurance_ee", "Health Insurance"),
                ("retirement_401k_ee", "401(k)"),
                ("reimbursement_ee", "Reimbursements"),
                ("net_pay", "Net Pay"),
                ("social_security_er", "Social Security (Employer)"),
                ("medicare_er", "Medicare (Employer)"),
                ("futa_er", "FUTA"),
                ("suta_er", "SUTA"),
            ],
        }),
        "quickbooks_payroll" => Spec::A(ModeASpec {
            header_row: 3,
            skip_footer: 1,
            employee: "Employee",
            check_date: None,
            department: None,
            money: &[
                ("gross_pay", "Gross Pay"),
                ("federal_income_tax", "Federal Taxes"),
                ("state_income_tax", "State Taxes"),
                ("social_security_ee", "Social Security Employee"),
                ("medicare_ee", "Medicare Employee"),
                ("social_security_er", "Social Security Employer"),
                ("medicare_er", "Medicare Employer"),
                ("net_pay", "Net Pay"),
            ],
        }),
        "square" => Spec::A(ModeASpec {
            header_row: 0,
            skip_footer: 0,
            employee: "Name",
            check_date: None,
            department: None,
            money: &[
                ("gross_pay", "Gross Pay"),
                ("other_ee_tax", "Total Employee Taxes"),
                ("social_security_er", "Employer Social Security"),
                ("medicare_er", "Employer Medicare"),
                ("futa_er", "FUTA"),
                ("suta_er", "SUTA"),
                ("other_deduction_ee", "Total Deductions"),
                ("net_pay", "Net Pay"),
            ],
        }),
        "generic_period1" | "generic_period2" => Spec::A(ModeASpec {
            header_row: 0,
            skip_footer: 0,
            employee: "Worker",
            check_date: Some("Paid On"),
            department: None,
            money: &[
                ("gross_pay", "Wages"),
                ("federal_income_tax", "Fed WH"),
                ("state_income_tax", "State WH"),
                ("social_security_ee", "SS EE"),
                ("medicare_ee", "Medicare EE"),
                ("social_security_er", "SS ER"),
                ("medicare_er", "Medicare ER"),
                ("net_pay", "Take Home"),
            ],
        }),
        "adp_run_gli" => Spec::B {
            date: "Check Date",
            description: "GL Account Description",
            amounts: ModeBAmounts::DebitCredit("Debit Amount", "Credit Amount"),
        },
        // Synthetic Mode B correction pair (P30 review S5); no golden: the
        // contract test compares the engine to this expectation directly.
        "paychex_flex_gls" | "paychex_correction" => Spec::B {
            date: "Check Date",
            description: "Description",
            amounts: ModeBAmounts::DebitCredit("Debit", "Credit"),
        },
        "onpay_gl_summary" => Spec::B {
            date: "Pay Date",
            description: "Description",
            amounts: ModeBAmounts::Category("Amount", "Category"),
        },
        other => panic!("no spec for {other}"),
    }
}

/// Expense line types that split by department (U-246).
const EXPENSES: [&str; 4] = [
    "gross_wages_expense",
    "employer_tax_expense",
    "employer_benefits_expense",
    "reimbursement_expense",
];

/// Liability lines in engine order, with the fields each one collects.
const CREDITS: [(&str, &[&str]); 11] = [
    ("fit_payable", &["federal_income_tax"]),
    ("sit_payable", &["state_income_tax", "local_income_tax"]),
    ("ss_payable", &["social_security_ee", "social_security_er"]),
    ("medicare_payable", &["medicare_ee", "medicare_er"]),
    ("futa_payable", &["futa_er"]),
    ("suta_payable", &["suta_er"]),
    (
        "health_ins_payable",
        &[
            "health_insurance_ee",
            "health_insurance_er",
            "dental_vision_ee",
        ],
    ),
    (
        "retirement_payable",
        &["retirement_401k_ee", "roth_401k_ee", "retirement_401k_er"],
    ),
    (
        "other_deduction_payable",
        &["hsa_ee", "other_deduction_ee", "other_benefit_er"],
    ),
    ("other_tax_payable", &["other_ee_tax", "other_er_tax"]),
    ("payroll_clearing", &["net_pay"]),
];

fn expense_amount(line_type: &str, f: &BTreeMap<&str, Decimal>) -> Decimal {
    let g = |k: &str| f.get(k).copied().unwrap_or_default();
    match line_type {
        "gross_wages_expense" => g("gross_pay") - g("reimbursement_ee"),
        "employer_tax_expense" => {
            g("social_security_er")
                + g("medicare_er")
                + g("futa_er")
                + g("suta_er")
                + g("other_er_tax")
        }
        "employer_benefits_expense" => {
            g("health_insurance_er") + g("retirement_401k_er") + g("other_benefit_er")
        }
        "reimbursement_expense" => g("reimbursement_ee"),
        _ => unreachable!(),
    }
}

/// The expectation for `stem`, given the check date the wizard sends.
pub fn expect(stem: &str, check_date: &str) -> Vec<Entry> {
    let rows = read_csv(&fixture_bytes(&format!("{stem}.csv")));
    match spec(stem) {
        Spec::A(s) => expect_a(&rows, &s, check_date),
        Spec::B {
            date,
            description,
            amounts,
        } => expect_b(&rows, date, description, &amounts),
    }
}

fn col(header: &[String], name: &str) -> usize {
    header
        .iter()
        .position(|h| h.trim() == name)
        .unwrap_or_else(|| panic!("no column {name}"))
}

/// Per worker: (date, department, field → amount).
type Worker<'a> = (String, Option<String>, BTreeMap<&'a str, Decimal>);

fn expect_a(rows: &[Vec<String>], s: &ModeASpec, check_date: &str) -> Vec<Entry> {
    let header = &rows[s.header_row];
    let body = &rows[s.header_row + 1..rows.len() - s.skip_footer];
    let emp = col(header, s.employee);
    let mut workers: Vec<Worker<'_>> = Vec::new();
    for r in body {
        let name = r[emp].trim();
        if name.is_empty() || name.eq_ignore_ascii_case("total") {
            continue;
        }
        let date = s
            .check_date
            .map_or_else(|| check_date.to_string(), |c| iso_date(&r[col(header, c)]));
        let dept = s
            .department
            .map(|c| r[col(header, c)].trim().to_string())
            .filter(|d| !d.is_empty());
        let fields = s
            .money
            .iter()
            .map(|(f, h)| (*f, money(&r[col(header, h)])))
            .collect();
        workers.push((date, dept, fields));
    }
    let mut dates: Vec<String> = workers.iter().map(|w| w.0.clone()).collect();
    dates.sort();
    dates.dedup();
    let split = s.department.is_some();
    dates
        .into_iter()
        .map(|date| {
            let of_date: Vec<&Worker<'_>> = workers.iter().filter(|w| w.0 == date).collect();
            let mut depts: Vec<Option<String>> = if split {
                let mut named: Vec<Option<String>> = of_date
                    .iter()
                    .filter_map(|w| w.1.clone())
                    .map(Some)
                    .collect();
                named.sort();
                named.dedup();
                named.push(None);
                named
            } else {
                vec![None]
            };
            depts.dedup();
            let mut lines = Vec::new();
            for t in EXPENSES {
                for d in &depts {
                    let amount: Decimal = of_date
                        .iter()
                        .filter(|w| !split || &w.1 == d)
                        .map(|w| expense_amount(t, &w.2))
                        .sum();
                    if !amount.is_zero() {
                        lines.push(Line {
                            line_type: t.to_string(),
                            department: d.clone(),
                            side: Side::Debit,
                            amount,
                        });
                    }
                }
            }
            for (t, fields) in CREDITS {
                let amount: Decimal = of_date
                    .iter()
                    .flat_map(|w| {
                        fields
                            .iter()
                            .map(|f| w.2.get(f).copied().unwrap_or_default())
                    })
                    .sum();
                if !amount.is_zero() {
                    lines.push(Line {
                        line_type: t.to_string(),
                        department: None,
                        side: Side::Credit,
                        amount,
                    });
                }
            }
            Entry::new(date, lines)
        })
        .collect()
}

fn expect_b(
    rows: &[Vec<String>],
    date: &str,
    description: &str,
    amounts: &ModeBAmounts,
) -> Vec<Entry> {
    let header = &rows[0];
    let (dc, xc) = (col(header, date), col(header, description));
    let mut by_date: BTreeMap<String, Vec<Line>> = BTreeMap::new();
    for r in &rows[1..] {
        let desc = r[xc].trim().to_string();
        if desc.is_empty() {
            continue;
        }
        let (side, amount) = match amounts {
            ModeBAmounts::DebitCredit(d, c) => {
                // A negative debit is a credit and a negative credit a debit
                // (a correction pair; P30 review S5).
                let (d, c) = (money(&r[col(header, d)]), money(&r[col(header, c)]));
                if d > Decimal::ZERO {
                    (Side::Debit, d)
                } else if d < Decimal::ZERO {
                    (Side::Credit, -d)
                } else if c < Decimal::ZERO {
                    (Side::Debit, -c)
                } else {
                    (Side::Credit, c)
                }
            }
            ModeBAmounts::Category(a, c) => {
                // The category gives the side; a negative amount is the
                // other side (S5).
                let amount = money(&r[col(header, a)]);
                let cat = r[col(header, c)].trim().to_ascii_lowercase();
                let debit = cat.starts_with("expense") || cat.starts_with("cost");
                match (debit, amount < Decimal::ZERO) {
                    (true, false) | (false, true) => (Side::Debit, amount.abs()),
                    _ => (Side::Credit, amount.abs()),
                }
            }
        };
        if amount.is_zero() {
            continue;
        }
        by_date.entry(iso_date(&r[dc])).or_default().push(Line {
            line_type: desc,
            department: None,
            side,
            amount,
        });
    }
    by_date
        .into_iter()
        .map(|(d, lines)| Entry::new(d, lines))
        .collect()
}

/// The raw money cells of a Mode A fixture's column, for the float-drift
/// check.
pub fn column_cells(stem: &str, header_name: &str) -> Vec<String> {
    let rows = read_csv(&fixture_bytes(&format!("{stem}.csv")));
    let Spec::A(s) = spec(stem) else {
        panic!("{stem} is not Mode A")
    };
    let header = &rows[s.header_row];
    let (emp, c) = (col(header, s.employee), col(header, header_name));
    rows[s.header_row + 1..rows.len() - s.skip_footer]
        .iter()
        .filter(|r| {
            let n = r[emp].trim();
            !n.is_empty() && !n.eq_ignore_ascii_case("total")
        })
        .map(|r| r[c].clone())
        .collect()
}
