//! Independent readers for the P31 import fixtures (`fixtures/qbd/`,
//! `fixtures/imports/`). Nothing here uses cl-api or the engine: each file is
//! read with a small parser of its own and every amount is summed in
//! `rust_decimal`, so the expectations stay independent of the code under
//! test. Shared by `tests/imports_fixtures.rs` (offline) and the contract
//! tests.

#![allow(dead_code, reason = "each test binary uses part of it")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use rust_decimal::Decimal;

/// `fixtures/` at the repository root.
pub fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// A fixture set's folder (`qbd/shoal-creek`, `imports/generic`, …).
pub fn set_dir(set: &str) -> PathBuf {
    fixtures_root().join(set)
}

/// The raw bytes of a fixture file.
pub fn bytes(set: &str, file: &str) -> Vec<u8> {
    let p = set_dir(set).join(file);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Latin-1 text: every byte is the code point of the same value.
pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// The text of a file: Latin-1 for `QuickBooks` Desktop files, UTF-8 for the
/// rest.
pub fn text(set: &str, file: &str) -> String {
    let b = bytes(set, file);
    if set.starts_with("qbd") || set.ends_with("qbd-gl") {
        latin1(&b)
    } else {
        String::from_utf8(b).unwrap_or_else(|_| panic!("{set}/{file} is not UTF-8"))
    }
}

/// A small CSV reader: quoted cells, doubled quotes, LF or CRLF.
pub fn read_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut cell)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            (_, c) => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// An amount as the fixtures write it: `$`, thousands commas, `(x)` or a
/// leading `-` for negatives; blank is zero.
pub fn amount(s: &str) -> Decimal {
    let t = s.trim().replace(['$', ','], "");
    if t.is_empty() {
        return Decimal::ZERO;
    }
    let (neg, body) = match t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        Some(inner) => (true, inner.to_string()),
        None => (false, t),
    };
    let v = Decimal::from_str(&body).unwrap_or_else(|_| panic!("amount {s:?}"));
    if neg { -v } else { v }
}

/// An exact amount from `EXPECTED.toml`.
pub fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap_or_else(|_| panic!("decimal {s:?}"))
}

/// `EXPECTED.toml`, read as sections of `key = "value"` (the subset these
/// files use: comments, `[section]`, bare or quoted keys, quoted values).
pub type Expected = BTreeMap<String, BTreeMap<String, String>>;

/// Reads a set's `EXPECTED.toml`.
pub fn expected(set: &str) -> Expected {
    let path = set_dir(set).join("EXPECTED.toml");
    let body = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out = Expected::new();
    let mut current = String::new();
    for (line_no, raw) in body.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current = name.to_string();
            out.entry(current.clone()).or_default();
            continue;
        }
        let (key, val) = line
            .split_once(" = ")
            .unwrap_or_else(|| panic!("{}:{}: {line}", path.display(), line_no + 1));
        let unquote = |s: &str| s.trim().trim_matches('"').to_string();
        out.entry(current.clone())
            .or_default()
            .insert(unquote(key), unquote(val));
    }
    out
}
/// One section of an expectation.
pub fn section<'a>(e: &'a Expected, name: &str) -> &'a BTreeMap<String, String> {
    e.get(name)
        .unwrap_or_else(|| panic!("EXPECTED.toml has no [{name}]"))
}

/// A section as account → exact amount.
pub fn amounts(e: &Expected, name: &str) -> BTreeMap<String, Decimal> {
    section(e, name)
        .iter()
        .map(|(k, v)| (k.clone(), dec(v)))
        .collect()
}

/// Balances by account key, signed (debit positive).
pub type Balances = BTreeMap<String, Decimal>;

/// Adds `v` to `key`, dropping keys that come back to zero.
pub fn add(b: &mut Balances, key: &str, v: Decimal) {
    let e = b.entry(key.to_string()).or_insert(Decimal::ZERO);
    *e += v;
    if e.is_zero() {
        b.remove(key);
    }
}

/// The non-zero entries of `b` minus `a` (books minus source).
pub fn diffs(books: &Balances, source: &Balances) -> Balances {
    let mut d = books.clone();
    for (k, v) in source {
        add(&mut d, k, -*v);
    }
    d
}

/// Sum of the debit side.
pub fn debits(b: &Balances) -> Decimal {
    b.values().filter(|v| v.is_sign_positive()).copied().sum()
}

/// The number at the front of a `QuickBooks` Desktop account cell
/// (`1000 · Checking` → `1000`).
pub fn qbd_number(cell: &str) -> String {
    let c = cell.trim();
    match c.split_once('\u{b7}') {
        Some((n, _)) => n.trim().to_string(),
        None => c.to_string(),
    }
}

fn col(header: &[String], name: &str) -> usize {
    header
        .iter()
        .position(|h| h.trim().eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("no {name:?} column in {header:?}"))
}

fn header_row(rows: &[Vec<String>], needles: &[&str]) -> usize {
    rows.iter()
        .position(|r| {
            needles
                .iter()
                .all(|n| r.iter().any(|c| c.trim().eq_ignore_ascii_case(n)))
        })
        .unwrap_or_else(|| panic!("no header with {needles:?}"))
}

fn is_total(c: &str) -> bool {
    c.trim().to_ascii_uppercase().starts_with("TOTAL")
}

/// The "As of" date in a report's heading (`As of October 27, 2026`), as
/// `YYYY-MM-DD`.
pub fn as_of(text: &str) -> String {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let line = text
        .lines()
        .find(|l| l.contains("As of"))
        .expect("an As of heading");
    let rest = line
        .split("As of ")
        .nth(1)
        .expect("date")
        .trim_matches(|c: char| c == '"' || c == ',' || c.is_whitespace());
    let rest = rest.split('"').next().unwrap_or(rest);
    let mut parts = rest.split_whitespace();
    let month = parts.next().expect("month").to_ascii_lowercase();
    let day: u32 = parts
        .next()
        .expect("day")
        .trim_end_matches(',')
        .parse()
        .expect("day number");
    let year: u32 = parts.next().expect("year").parse().expect("year number");
    let m = MONTHS.iter().position(|x| *x == month).expect("month name") + 1;
    format!("{year:04}-{m:02}-{day:02}")
}

/// `MM/DD/YYYY` or `YYYY-MM-DD` as `YYYY-MM-DD`.
pub fn iso(d: &str) -> String {
    let d = d.trim();
    if d.len() == 10 && d.as_bytes()[4] == b'-' {
        return d.to_string();
    }
    let p: Vec<&str> = d.split('/').collect();
    assert_eq!(p.len(), 3, "date {d:?}");
    format!("{}-{:0>2}-{:0>2}", p[2], p[0], p[1])
}

// ── QuickBooks Desktop ──

/// Account number → QBD account type (`Bank`, `Income`, …).
pub fn qbd_account_types(text: &str) -> BTreeMap<String, String> {
    let rows = read_csv(text);
    let h = header_row(&rows, &["Account", "Type"]);
    let (ia, it) = (col(&rows[h], "Account"), col(&rows[h], "Type"));
    rows[h + 1..]
        .iter()
        .filter(|r| r.len() > it && !r[ia].trim().is_empty())
        .map(|r| (qbd_number(&r[ia]), r[it].trim().to_string()))
        .collect()
}

/// Income statement accounts close to retained earnings at year end.
pub fn is_income_statement(qbd_type: &str) -> bool {
    matches!(
        qbd_type.to_ascii_lowercase().as_str(),
        "income" | "other income" | "cost of goods sold" | "expense" | "other expense"
    )
}

/// A QBD trial balance by account number (signed).
pub fn qbd_tb(text: &str) -> Balances {
    let rows = read_csv(text);
    let h = header_row(&rows, &["Debit", "Credit"]);
    let (id, ic) = (col(&rows[h], "Debit"), col(&rows[h], "Credit"));
    let mut b = Balances::new();
    for r in &rows[h + 1..] {
        let acct = r.first().map_or("", |s| s.trim());
        if acct.is_empty() || is_total(acct) {
            continue;
        }
        let get = |i: usize| r.get(i).map_or(Decimal::ZERO, |s| amount(s));
        add(&mut b, &qbd_number(acct), get(id) - get(ic));
    }
    b
}

/// The TOTAL row's debit of a QBD report.
pub fn qbd_total_debit(text: &str) -> Decimal {
    let rows = read_csv(text);
    let h = header_row(&rows, &["Debit", "Credit"]);
    let id = col(&rows[h], "Debit");
    let total = rows[h + 1..]
        .iter()
        .find(|r| r.iter().any(|c| is_total(c)))
        .expect("TOTAL row");
    amount(&total[id])
}

/// One posting leg of a journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leg {
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Account key.
    pub account: String,
    /// Debit minus credit.
    pub amount: Decimal,
}

/// The legs of a QBD Journal export.
pub fn qbd_journal(text: &str) -> Vec<Leg> {
    let rows = read_csv(text);
    let h = header_row(&rows, &["Trans #", "Type", "Date", "Account"]);
    let hd = &rows[h];
    let (itr, idt, iac, ide, icr) = (
        col(hd, "Trans #"),
        col(hd, "Date"),
        col(hd, "Account"),
        col(hd, "Debit"),
        col(hd, "Credit"),
    );
    let mut legs = Vec::new();
    let mut date = String::new();
    for r in &rows[h + 1..] {
        let cell = |i: usize| r.get(i).map_or("", |s| s.trim());
        if is_total(cell(itr)) {
            continue;
        }
        if !cell(itr).is_empty() {
            date = iso(cell(idt));
        }
        if cell(iac).is_empty() {
            continue;
        }
        legs.push(Leg {
            date: date.clone(),
            account: qbd_number(cell(iac)),
            amount: amount(cell(ide)) - amount(cell(icr)),
        });
    }
    legs
}

/// The TOTAL row's debit of a QBD Journal.
pub fn qbd_journal_total(text: &str) -> Decimal {
    let rows = read_csv(text);
    let h = header_row(&rows, &["Trans #", "Debit"]);
    let (itr, ide) = (col(&rows[h], "Trans #"), col(&rows[h], "Debit"));
    let total = rows[h + 1..]
        .iter()
        .find(|r| r.get(itr).is_some_and(|c| is_total(c)))
        .expect("TOTAL row");
    amount(&total[ide])
}

/// One open item (`Open Invoices`, `Unpaid Bills Detail`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenItem {
    /// Type (`Invoice`, `Credit Memo`, `Payment`, `Bill`, `Credit`).
    pub kind: String,
    /// Num, or `payment:<name>` when blank.
    pub key: String,
    /// Customer or vendor.
    pub name: String,
    /// Open balance.
    pub open: Decimal,
}

/// The rows of an open-items report.
pub fn qbd_open_items(text: &str) -> Vec<OpenItem> {
    let rows = read_csv(text);
    let h = header_row(&rows, &["Type", "Num", "Name", "Open Balance"]);
    let hd = &rows[h];
    let (it, inum, iname, iopen) = (
        col(hd, "Type"),
        col(hd, "Num"),
        col(hd, "Name"),
        col(hd, "Open Balance"),
    );
    rows[h + 1..]
        .iter()
        .filter(|r| {
            r.get(it)
                .is_some_and(|t| !t.trim().is_empty() && !is_total(t))
        })
        .map(|r| {
            let num = r[inum].trim();
            let name = r[iname].trim().to_string();
            OpenItem {
                kind: r[it].trim().to_string(),
                key: if num.is_empty() {
                    format!("payment:{name}")
                } else {
                    num.to_string()
                },
                name,
                open: amount(&r[iopen]),
            }
        })
        .collect()
}

/// Item → asset value of an Inventory Valuation Summary.
pub fn qbd_valuation(text: &str) -> BTreeMap<String, Decimal> {
    let rows = read_csv(text);
    let h = header_row(&rows, &["Item", "On Hand", "Asset Value"]);
    let (ii, iv) = (col(&rows[h], "Item"), col(&rows[h], "Asset Value"));
    rows[h + 1..]
        .iter()
        .filter(|r| {
            r.get(ii)
                .is_some_and(|t| !t.trim().is_empty() && !is_total(t))
        })
        .map(|r| (r[ii].trim().to_string(), amount(&r[iv])))
        .collect()
}

/// Name → "Balance Total" of a QBD customer or vendor list (top-level names
/// only; `Customer:Job` rows are already in their parent's total).
pub fn qbd_list_balances(text: &str, name_col: &str) -> BTreeMap<String, Decimal> {
    let rows = read_csv(text);
    let h = header_row(&rows, &[name_col, "Balance Total"]);
    let (inm, ib) = (col(&rows[h], name_col), col(&rows[h], "Balance Total"));
    rows[h + 1..]
        .iter()
        .filter(|r| {
            r.get(inm)
                .is_some_and(|n| !n.contains(':') && !n.trim().is_empty())
        })
        .map(|r| (r[inm].trim().to_string(), amount(&r[ib])))
        .collect()
}

/// The books' trial balance as of `as_of` from opening balances and journal
/// legs, with income statement accounts closed into `retained` at each
/// fiscal year end (`fy_start_month` 1-12). The opening balances are dated
/// `opening_date`.
pub fn roll_forward(
    opening: &Balances,
    opening_date: &str,
    legs: &[Leg],
    as_of: &str,
    types: &BTreeMap<String, String>,
    retained: &str,
    fy_start_month: u32,
) -> Balances {
    let fy_start = fiscal_year_start(as_of, fy_start_month);
    let mut b = Balances::new();
    let mut post = |account: &str, date: &str, v: Decimal| {
        if date > as_of {
            return;
        }
        let pl = types.get(account).is_some_and(|t| is_income_statement(t));
        if pl && date < fy_start.as_str() {
            add(&mut b, retained, v);
        } else {
            add(&mut b, account, v);
        }
    };
    for (k, v) in opening {
        post(k, opening_date, *v);
    }
    for l in legs {
        post(&l.account, &l.date, l.amount);
    }
    b
}

/// The first day of the fiscal year `date` falls in.
pub fn fiscal_year_start(date: &str, fy_start_month: u32) -> String {
    let y: u32 = date[..4].parse().expect("year");
    let m: u32 = date[5..7].parse().expect("month");
    let start_year = if m >= fy_start_month { y } else { y - 1 };
    format!("{start_year:04}-{fy_start_month:02}-01")
}

/// Net income (credits minus debits on income statement accounts) of the
/// legs dated within `[from, to]`, plus `opening` P&L when it is in range.
pub fn net_income(legs: &[Leg], types: &BTreeMap<String, String>, from: &str, to: &str) -> Decimal {
    -legs
        .iter()
        .filter(|l| l.date.as_str() >= from && l.date.as_str() <= to)
        .filter(|l| {
            types
                .get(&l.account)
                .is_some_and(|t| is_income_statement(t))
        })
        .map(|l| l.amount)
        .sum::<Decimal>()
}

// ── Accounting Power ──

/// An Accounting Power trial balance's signed column by account code.
pub fn ap_tb(text: &str, column: &str) -> Balances {
    let rows = read_csv(text);
    let (ic, iv) = (col(&rows[0], "Account Code"), col(&rows[0], column));
    let mut b = Balances::new();
    for r in &rows[1..] {
        if r.get(ic).is_some_and(|c| !c.trim().is_empty()) {
            add(&mut b, r[ic].trim(), amount(&r[iv]));
        }
    }
    b
}

/// The legs of an Accounting Power GL export.
pub fn ap_gl(text: &str) -> Vec<Leg> {
    let rows = read_csv(text);
    let hd = &rows[0];
    let (idt, iac, ide, icr) = (
        col(hd, "Date"),
        col(hd, "Account"),
        col(hd, "Debit Amount"),
        col(hd, "Credit Amount"),
    );
    rows[1..]
        .iter()
        .filter(|r| r.get(iac).is_some_and(|a| !a.trim().is_empty()))
        .map(|r| Leg {
            date: iso(&r[idt]),
            account: r[iac].trim().to_string(),
            amount: amount(&r[ide]) - amount(&r[icr]),
        })
        .collect()
}

// ── QuickBooks Online ──

/// Account name → number from a QBO chart of accounts (`Account #`).
pub fn qbo_numbers(text: &str) -> BTreeMap<String, String> {
    let rows = read_csv(text);
    let h = header_row(&rows, &["Full name", "Type"]);
    let (inum, iname) = (col(&rows[h], "Account #"), col(&rows[h], "Full name"));
    rows[h + 1..]
        .iter()
        .filter(|r| r.len() > iname)
        .map(|r| (r[iname].trim().to_string(), r[inum].trim().to_string()))
        .collect()
}

/// A QBO trial balance by account number.
pub fn qbo_tb(text: &str, numbers: &BTreeMap<String, String>) -> Balances {
    let rows = read_csv(text);
    let h = header_row(&rows, &["Debit", "Credit"]);
    let (id, ic) = (col(&rows[h], "Debit"), col(&rows[h], "Credit"));
    let mut b = Balances::new();
    for r in &rows[h + 1..] {
        let name = r.first().map_or("", |s| s.trim());
        if name.is_empty() || is_total(name) {
            continue;
        }
        let n = numbers
            .get(name)
            .unwrap_or_else(|| panic!("QBO account {name:?} not in the chart"));
        add(&mut b, n, amount(&r[id]) - amount(&r[ic]));
    }
    b
}

/// The legs of a QBO Journal report.
pub fn qbo_gl(text: &str, numbers: &BTreeMap<String, String>) -> Vec<Leg> {
    let rows = read_csv(text);
    let h = header_row(&rows, &["Date", "Transaction Type", "Account"]);
    let hd = &rows[h];
    let (idt, iac, ide, icr) = (
        col(hd, "Date"),
        col(hd, "Account"),
        col(hd, "Debit"),
        col(hd, "Credit"),
    );
    rows[h + 1..]
        .iter()
        .filter(|r| !r[idt].trim().is_empty() && !is_total(&r[idt]))
        .map(|r| Leg {
            date: iso(&r[idt]),
            account: numbers
                .get(r[iac].trim())
                .unwrap_or_else(|| panic!("QBO account {:?}", r[iac]))
                .clone(),
            amount: amount(&r[ide]) - amount(&r[icr]),
        })
        .collect()
}

// ── Generic CSV ──

/// A Generic trial balance by account number.
pub fn generic_tb(text: &str) -> Balances {
    let rows = read_csv(text);
    let hd = &rows[0];
    let (inum, id, ic) = (
        col(hd, "Account Number"),
        col(hd, "Debit"),
        col(hd, "Credit"),
    );
    let mut b = Balances::new();
    for r in &rows[1..] {
        if r.get(inum).is_some_and(|c| !c.trim().is_empty()) {
            add(&mut b, r[inum].trim(), amount(&r[id]) - amount(&r[ic]));
        }
    }
    b
}

/// The legs of a Generic GL file: a positive amount debits Account and
/// credits Offset Account.
pub fn generic_gl(text: &str) -> Vec<Leg> {
    let rows = read_csv(text);
    let hd = &rows[0];
    let (idt, iac, iam, iof) = (
        col(hd, "Date"),
        col(hd, "Account"),
        col(hd, "Amount"),
        col(hd, "Offset Account"),
    );
    let mut legs = Vec::new();
    for r in &rows[1..] {
        if r.get(iac).is_none_or(|a| a.trim().is_empty()) {
            continue;
        }
        let v = amount(&r[iam]);
        let date = iso(&r[idt]);
        legs.push(Leg {
            date: date.clone(),
            account: r[iac].trim().to_string(),
            amount: v,
        });
        legs.push(Leg {
            date,
            account: r[iof].trim().to_string(),
            amount: -v,
        });
    }
    legs
}

/// Sums legs into balances.
pub fn sum_legs(legs: &[Leg]) -> Balances {
    let mut b = Balances::new();
    for l in legs {
        add(&mut b, &l.account, l.amount);
    }
    b
}

/// Sum of the debit side of legs.
pub fn leg_debits(legs: &[Leg]) -> Decimal {
    legs.iter()
        .map(|l| l.amount)
        .filter(Decimal::is_sign_positive)
        .sum()
}

/// Debits minus credits of all legs (zero when every entry balances).
pub fn net(legs: &[Leg]) -> Decimal {
    legs.iter().map(|l| l.amount).sum()
}
