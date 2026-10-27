use super::*;
use crate::model::{
    CheckCode, DocKind, LeftOutKind, MAX_STATEMENT_LINES, PageWords, Reading, SignSource,
};
use crate::pipeline::testdoc::Doc;
use crate::pipeline::{ReadInput, read as pipeline_read};
use crate::protocol::JobHint;
use cl_core::intake::IntakeKind;
use cl_core::{CalendarDate, Money};
use proptest::prelude::*;

fn money(s: &str) -> Money {
    Money::parse_wire(s).unwrap()
}

fn day(m: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, m, d).unwrap()
}

fn today() -> CalendarDate {
    day(10, 27)
}

fn read_pages(pages: &[PageWords], hint: JobHint) -> Reading {
    let count = u16::try_from(pages.len()).unwrap();
    pipeline_read(&ReadInput {
        pages,
        page_count: count,
        unread_pages: &[],
        templates: &[],
        hint,
        today: today(),
    })
}

fn letterhead(d: &mut Doc) {
    d.text(500, 400, "Cedar Valley Parts");
    d.text(7000, 400, "STATEMENT");
    d.text(500, 600, "88 Quarry Road");
    d.text(500, 800, "Rogers, AR 72756");
    d.text(6000, 900, "Statement Date: 09/30/2026");
    d.text(6000, 1100, "Account #: AC-55120");
    d.text(6000, 1300, "Statement Period: 09/01/2026 - 09/30/2026");
}

/// One amount column and a running balance.
fn single_column() -> Doc {
    let mut d = Doc::new();
    letterhead(&mut d);
    d.text(500, 3000, "Date");
    d.text(2000, 3000, "Reference");
    d.text(3500, 3000, "Description");
    d.amount(7800, 3000, "Amount");
    d.amount(9500, 3000, "Balance");
    let rows: [(&str, &str, &str, &str, &str); 5] = [
        ("09/01/2026", "", "Balance Forward", "", "1,000.00"),
        (
            "09/03/2026",
            "INV-1001",
            "Hydraulic fittings",
            "250.00",
            "1,250.00",
        ),
        (
            "09/10/2026",
            "PMT-77",
            "Payment - thank you",
            "(400.00)",
            "850.00",
        ),
        ("09/15/2026", "CM-12", "Credit memo", "50.00CR", "800.00"),
        ("09/20/2026", "INV-1002", "Hose", "125.50", "925.50"),
    ];
    let mut y = 3200;
    for (date, r, desc, amt, bal) in rows {
        d.text(500, y, date);
        if !r.is_empty() {
            d.text(2000, y, r);
        }
        d.text(3500, y, desc);
        if !amt.is_empty() {
            d.amount(7800, y, amt);
        }
        d.amount(9500, y, bal);
        y += 200;
    }
    d.text(3500, y + 100, "Closing Balance");
    d.amount(9500, y + 100, "925.50");
    d
}

#[test]
fn a_single_column_statement_reads_signs_as_printed() {
    let pages = single_column().pages();
    let r = read_pages(&pages, JobHint::statement());
    assert_eq!(r.kind, DocKind::Statement);
    let st = r.statement.as_ref().unwrap();
    let amounts: Vec<Money> = st.lines.iter().map(|l| l.amount.value).collect();
    assert_eq!(
        amounts,
        [money("250"), money("-400"), money("-50"), money("125.50")]
    );
    assert!(
        st.lines
            .iter()
            .all(|l| l.sign_from == SignSource::SingleColumn)
    );
    let refs: Vec<&str> = st
        .lines
        .iter()
        .map(|l| l.reference.as_ref().unwrap().value.as_str())
        .collect();
    assert_eq!(refs, ["INV-1001", "PMT-77", "CM-12", "INV-1002"]);
    assert_eq!(st.lines[1].date.value, day(9, 10));
    assert_eq!(
        st.lines[0].description.as_ref().unwrap().value,
        "Hydraulic fittings"
    );
    assert_eq!(st.lines[3].running.as_ref().unwrap().value, money("925.50"));
    assert_eq!(st.opening.as_ref().unwrap().value, money("1000"));
    assert_eq!(st.closing.as_ref().unwrap().value, money("925.50"));
    assert_eq!(st.statement_date.as_ref().unwrap().value, day(9, 30));
    assert_eq!(st.period_from.as_ref().unwrap().value, day(9, 1));
    assert_eq!(st.period_to.as_ref().unwrap().value, day(9, 30));
    assert_eq!(st.vendor.as_ref().unwrap().value, "Cedar Valley Parts");
    assert_eq!(st.account_no.as_ref().unwrap().value, "AC-55120");
    let kinds: Vec<LeftOutKind> = st.left_out.iter().map(|l| l.what).collect();
    assert_eq!(kinds, [LeftOutKind::BalanceForward, LeftOutKind::Total]);
    assert!(r.checks.is_empty(), "{:?}", r.checks);
    assert!(r.lines.is_empty(), "no bill lines on a statement");
    r.validate(0, true).unwrap();
}

/// Charges, payments and credits columns: the column decides the sign.
#[test]
fn three_columns_take_the_sign_from_the_column() {
    let mut d = Doc::new();
    letterhead(&mut d);
    d.text(6000, 1500, "Previous Balance: 0.00");
    d.text(500, 3000, "Date");
    d.text(1800, 3000, "Invoice #");
    d.text(3200, 3000, "Description");
    d.amount(6200, 3000, "Charges");
    d.amount(7300, 3000, "Payments");
    d.amount(8400, 3000, "Credits");
    d.amount(9500, 3000, "Balance");
    d.text(500, 3200, "09/02/2026");
    d.text(1800, 3200, "10234");
    d.text(3200, 3200, "Widgets");
    d.amount(6200, 3200, "300.00");
    d.amount(9500, 3200, "300.00");
    d.text(500, 3400, "09/09/2026");
    d.text(1800, 3400, "4412");
    d.text(3200, 3400, "Check");
    d.amount(7300, 3400, "200.00");
    d.amount(9500, 3400, "100.00");
    d.text(500, 3600, "09/12/2026");
    d.text(1800, 3600, "CM-9");
    d.text(3200, 3600, "Return");
    // A minus printed in a credits column is still a credit.
    d.amount(8400, 3600, "-20.00");
    d.amount(9500, 3600, "80.00");
    d.text(3200, 3900, "Amount Due");
    d.amount(9500, 3900, "$80.00");
    let pages = d.pages();
    let r = read_pages(&pages, JobHint::statement());
    let st = r.statement.unwrap();
    let got: Vec<(Money, SignSource)> = st
        .lines
        .iter()
        .map(|l| (l.amount.value, l.sign_from))
        .collect();
    assert_eq!(
        got,
        [
            (money("300"), SignSource::ChargesColumn),
            (money("-200"), SignSource::PaymentsColumn),
            (money("-20"), SignSource::CreditsColumn),
        ]
    );
    assert_eq!(st.lines[0].reference.as_ref().unwrap().value, "10234");
    assert_eq!(st.opening.as_ref().unwrap().value, Money::ZERO);
    assert_eq!(st.closing.as_ref().unwrap().value, money("80"));
    assert!(r.checks.is_empty(), "{:?}", r.checks);
}

fn table_page(d: &mut Doc, header: bool, rows: &[(&str, &str, &str)], start_y: u16) -> u16 {
    if header {
        d.text(500, start_y, "Date");
        d.text(2000, start_y, "Ref");
        d.amount(7800, start_y, "Amount");
    }
    let mut y = start_y + 200;
    for (date, r, amt) in rows {
        d.text(500, y, date);
        d.text(2000, y, r);
        d.amount(7800, y, amt);
        y += 200;
    }
    y
}

#[test]
fn a_page_without_a_header_row_carries_the_columns_on() {
    for repeat in [false, true] {
        let mut d = Doc::new();
        letterhead(&mut d);
        let y = table_page(
            &mut d,
            true,
            &[
                ("09/02/2026", "A-1", "10.00"),
                ("09/03/2026", "A-2", "20.00"),
            ],
            3000,
        );
        d.text(3500, y, "Page Total");
        d.amount(7800, y, "30.00");
        d.page();
        d.text(500, 300, "Cedar Valley Parts - continued");
        d.text(8000, 300, "Page 2");
        let y = table_page(
            &mut d,
            repeat,
            &[
                ("09/04/2026", "A-3", "5.00"),
                ("09/05/2026", "A-4", "(2.50)"),
            ],
            800,
        );
        d.text(3500, y + 100, "Total Due");
        d.amount(7800, y + 100, "32.50");
        let pages = d.pages();
        let r = read_pages(&pages, JobHint::statement());
        let st = r.statement.unwrap();
        let refs: Vec<&str> = st
            .lines
            .iter()
            .map(|l| l.reference.as_ref().unwrap().value.as_str())
            .collect();
        assert_eq!(refs, ["A-1", "A-2", "A-3", "A-4"], "repeat {repeat}");
        assert_eq!(st.lines[3].amount.spot.page, 1);
        assert_eq!(st.lines[3].amount.value, money("-2.50"));
        let kinds: Vec<LeftOutKind> = st.left_out.iter().map(|l| l.what).collect();
        assert_eq!(kinds, [LeftOutKind::Subtotal, LeftOutKind::Total]);
        assert_eq!(st.closing.as_ref().unwrap().value, money("32.50"));
        assert!(r.checks.is_empty(), "{:?}", r.checks);
    }
}

#[test]
fn aging_rows_are_left_out() {
    let mut d = single_column();
    d.text(500, 5000, "Current");
    d.text(2000, 5000, "1-30 Days");
    d.text(3500, 5000, "31-60 Days");
    d.text(5000, 5000, "61-90 Days");
    d.text(6500, 5000, "Over 90");
    d.amount(1200, 5200, "125.50");
    d.amount(2700, 5200, "800.00");
    d.amount(4200, 5200, "0.00");
    d.amount(5700, 5200, "0.00");
    d.amount(7200, 5200, "0.00");
    let pages = d.pages();
    let r = read_pages(&pages, JobHint::statement());
    let st = r.statement.unwrap();
    assert_eq!(st.lines.len(), 4);
    assert!(st.left_out.iter().any(|l| l.what == LeftOutKind::Aging));
    assert!(r.checks.is_empty(), "{:?}", r.checks);
}

/// A boxed summary: labels on one line, values under them.
#[test]
fn summary_values_under_their_labels() {
    let mut d = Doc::new();
    d.text(500, 400, "Cedar Valley Parts");
    d.text(7000, 400, "Statement of Account");
    d.text(500, 1400, "Previous Balance");
    d.text(3000, 1400, "Payments");
    d.text(5500, 1400, "New Balance");
    d.amount(2000, 1600, "100.00");
    d.amount(4300, 1600, "100.00");
    d.amount(6900, 1600, "40.00");
    d.text(500, 2000, "Billing Period: Sep 1 - Sep 30, 2026");
    let y = table_page(
        &mut d,
        true,
        &[
            ("09/05/2026", "P-1", "(100.00)"),
            ("09/18/2026", "I-7", "40.00"),
        ],
        3000,
    );
    let _ = y;
    let pages = d.pages();
    let r = read_pages(&pages, JobHint::statement());
    let st = r.statement.unwrap();
    assert_eq!(st.opening.as_ref().unwrap().value, money("100"));
    assert_eq!(st.closing.as_ref().unwrap().value, money("40"));
    assert_eq!(st.period_from.as_ref().unwrap().value, day(9, 1));
    assert_eq!(st.period_to.as_ref().unwrap().value, day(9, 30));
    assert!(r.checks.is_empty(), "{:?}", r.checks);
}

#[test]
fn the_checks_flag_and_never_adjust() {
    let mut d = Doc::new();
    letterhead(&mut d);
    d.text(500, 3000, "Date");
    d.text(2000, 3000, "Ref");
    d.amount(7800, 3000, "Amount");
    d.amount(9500, 3000, "Balance");
    let rows = [
        ("09/02/2026", "A-1", "10.00", "10.00"),
        ("09/03/2026", "A-2", "20.00", "31.00"),
        ("10/04/2026", "A-3", "5.00", "36.00"),
        ("10/04/2026", "A-3", "5.00", "41.00"),
    ];
    let mut y = 3200;
    for (date, r, amt, bal) in rows {
        d.text(500, y, date);
        d.text(2000, y, r);
        d.amount(7800, y, amt);
        d.amount(9500, y, bal);
        y += 200;
    }
    d.text(3500, y, "Total Due");
    d.amount(9500, y, "99.00");
    let r = read_pages(&d.pages(), JobHint::statement());
    let codes: Vec<(CheckCode, Option<u16>)> = r.checks.iter().map(|c| (c.code, c.line)).collect();
    assert_eq!(
        codes,
        [
            (CheckCode::StmtSum, None),
            (CheckCode::StmtRunning, Some(1)),
            (CheckCode::StmtDateOutsidePeriod, Some(2)),
            (CheckCode::StmtDateOutsidePeriod, Some(3)),
            (CheckCode::StmtDuplicateLine, Some(3)),
        ]
    );
    let st = r.statement.unwrap();
    assert_eq!(st.lines[1].amount.value, money("20"), "never adjusted");
    assert_eq!(st.lines[1].running.as_ref().unwrap().value, money("31"));
    assert_eq!(st.closing.as_ref().unwrap().value, money("99"));
    assert_eq!(r.checks[0].difference, Some(money("-59")));
}

/// Unhinted: plainly a statement only with a title, a period, a balance
/// column and three dated rows; a bills-folder hint keeps it a bill.
#[test]
fn unhinted_statements_need_every_sign() {
    let pages = single_column().pages();
    let r = read_pages(&pages, JobHint::default());
    assert_eq!(r.kind, DocKind::Statement);
    assert!(r.statement.is_some());
    let bills = JobHint {
        kind: Some(IntakeKind::Bills),
        statement: false,
    };
    let r = read_pages(&pages, bills);
    assert_eq!(r.kind, DocKind::Bill);
    assert!(r.statement.is_none());
    // No period printed: an A03 "statement" bill stays a bill.
    let mut d = Doc::new();
    d.text(500, 400, "Prairie Lumber");
    d.text(7000, 400, "STATEMENT");
    d.text(6000, 900, "Statement Date: 09/30/2026");
    table_page(
        &mut d,
        true,
        &[
            ("09/02/2026", "A-1", "10.00"),
            ("09/03/2026", "A-2", "20.00"),
            ("09/04/2026", "A-3", "20.00"),
        ],
        3000,
    );
    let r = read_pages(&d.pages(), JobHint::default());
    assert_ne!(r.kind, DocKind::Statement);
    assert!(r.statement.is_none());
    // An invoice is never a statement.
    let inv = crate::pipeline::testdoc::invoice(
        "INV-1",
        "09/02/2026",
        &[("Tarp", "1", "10.00", "10.00")],
        "10.00",
        "",
        "10.00",
    );
    let r = read_pages(&inv.pages(), JobHint::default());
    assert_eq!(r.kind, DocKind::Bill);
}

/// Hostile caps (step 7): twenty pages of 150 rows read 2,000 lines and
/// say so; the reading stays within the caps.
#[test]
fn twenty_pages_of_rows_are_capped_at_two_thousand_lines() {
    let mut d = Doc::new();
    letterhead(&mut d);
    let mut n = 0u32;
    for page in 0..20u16 {
        if page > 0 {
            d.page();
        }
        let top = if page == 0 { 3000 } else { 200 };
        if page == 0 {
            d.text(500, top, "Date");
            d.text(2000, top, "Ref");
            d.amount(7800, top, "Amount");
        }
        // Small print: 45 units a row.
        let mut y = top + 160;
        for _ in 0..150 {
            n += 1;
            d.word_box(500, y, 1300, y + 40, "09/02/2026");
            d.word_box(2000, y, 2600, y + 40, &format!("R{n}"));
            d.word_box(7480, y, 7800, y + 40, "1.00");
            y += 45;
            if y > 9_900 {
                break;
            }
        }
    }
    let pages = d.pages();
    let started = std::time::Instant::now();
    let r = read_pages(&pages, JobHint::statement());
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    let st = r.statement.as_ref().unwrap();
    assert!(n > 2_000, "{n} rows printed");
    assert_eq!(st.lines.len(), MAX_STATEMENT_LINES);
    assert_eq!(r.checks[0].code, CheckCode::StmtLinesCapped);
    r.validate(0, true).unwrap();
}

#[test]
fn money_words_with_cr_and_trailing_minus() {
    assert_eq!(table::money_word("50.00CR"), Some((money("50"), true)));
    assert_eq!(table::money_word("50.00-"), Some((money("50"), true)));
    assert_eq!(table::money_word("(50.00)"), Some((money("-50"), false)));
    assert_eq!(
        table::money_word("$1,204.66"),
        Some((money("1204.66"), false))
    );
    assert_eq!(table::money_word("CR"), None);
    assert_eq!(table::money_word("Total"), None);
}

// ---------------------------------------------------------------------------
// Injected errors (step 4): the checks fire and no value read changes.
// ---------------------------------------------------------------------------

fn wire_cents(c: i64) -> String {
    let neg = c < 0;
    let c = c.unsigned_abs();
    let whole = c / 100;
    let mut digits = whole.to_string();
    let mut grouped = String::new();
    while digits.len() > 3 {
        let tail = digits.split_off(digits.len() - 3);
        grouped = format!(",{tail}{grouped}");
    }
    let body = format!("{digits}{grouped}.{:02}", c % 100);
    if neg { format!("({body})") } else { body }
}

fn cents_money(c: i64) -> Money {
    Money::from_cents(c).unwrap()
}

#[derive(Debug, Clone, Copy)]
enum Fault {
    None,
    Amount(i64),
    Running(i64),
    OutOfPeriod,
    Duplicate,
    Closing(i64),
}

fn fault() -> impl Strategy<Value = Fault> {
    prop_oneof![
        Just(Fault::None),
        (1_i64..5_000).prop_map(Fault::Amount),
        (1_i64..5_000).prop_map(Fault::Running),
        Just(Fault::OutOfPeriod),
        Just(Fault::Duplicate),
        (1_i64..5_000).prop_map(Fault::Closing),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn injected_errors_are_flagged_and_values_kept(
        opening in 0_i64..1_000_000,
        amounts in prop::collection::vec(
            (-500_000_i64..500_000).prop_filter("non-zero", |c| *c != 0), 2..25),
        f in fault(),
        at in 0_usize..25,
    ) {
        let at = at % amounts.len();
        // What is printed: date, ref, amount, running.
        let mut printed: Vec<(CalendarDate, String, i64, i64)> = Vec::new();
        let mut running = opening;
        for (i, &a) in amounts.iter().enumerate() {
            running += a;
            let d = day(9, 1).add_days(i64::try_from(i).unwrap()).unwrap();
            printed.push((d, format!("R-{i}"), a, running));
        }
        let mut closing = running;
        match f {
            Fault::None => {}
            // The line's amount is misprinted; the balances are not.
            Fault::Amount(delta) => printed[at].2 += delta,
            Fault::Running(delta) => printed[at].3 += delta,
            Fault::OutOfPeriod => printed[at].0 = day(10, 5),
            Fault::Duplicate => {
                let copy = printed[at].clone();
                printed.insert(at + 1, (copy.0, copy.1, copy.2, copy.3 + copy.2));
                for row in printed.iter_mut().skip(at + 2) {
                    row.3 += copy.2;
                }
                closing += copy.2;
            }
            Fault::Closing(delta) => closing += delta,
        }
        let mut d = Doc::new();
        letterhead(&mut d);
        d.text(500, 2600, "Date");
        d.text(2000, 2600, "Ref");
        d.amount(7800, 2600, "Amount");
        d.amount(9500, 2600, "Balance");
        d.text(500, 2800, "09/01/2026");
        d.text(3500, 2800, "Previous Balance");
        d.amount(9500, 2800, &wire_cents(opening));
        let mut y = 3000;
        for (date, r, amt, bal) in &printed {
            d.text(500, y, &format!("{:02}/{:02}/{}", date.month(), date.day(), date.year()));
            d.text(2000, y, r);
            d.amount(7800, y, &wire_cents(*amt));
            d.amount(9500, y, &wire_cents(*bal));
            y += 200;
        }
        d.text(3500, y, "Ending Balance");
        d.amount(9500, y, &wire_cents(closing));
        let r = read_pages(&d.pages(), JobHint::statement());
        let st = r.statement.as_ref().unwrap();
        // Every value read is the value printed.
        prop_assert_eq!(st.lines.len(), printed.len());
        for (l, (date, rf, amt, bal)) in st.lines.iter().zip(&printed) {
            prop_assert_eq!(l.date.value, *date);
            prop_assert_eq!(&l.reference.as_ref().unwrap().value, rf);
            prop_assert_eq!(l.amount.value, cents_money(*amt));
            prop_assert_eq!(l.running.as_ref().unwrap().value, cents_money(*bal));
        }
        prop_assert_eq!(st.opening.as_ref().unwrap().value, cents_money(opening));
        prop_assert_eq!(st.closing.as_ref().unwrap().value, cents_money(closing));
        // The checks don't change the reading either.
        let before = serde_json::to_string(st).unwrap();
        let again = checks::run(st);
        prop_assert_eq!(serde_json::to_string(st).unwrap(), before);
        prop_assert_eq!(&again, &r.checks);
        let has = |c: CheckCode| r.checks.iter().any(|k| k.code == c);
        match f {
            Fault::None => prop_assert!(r.checks.is_empty(), "{:?}", r.checks),
            Fault::Amount(_) => {
                prop_assert!(has(CheckCode::StmtSum));
                prop_assert!(r.checks.iter().any(|c| c.code == CheckCode::StmtRunning
                    && c.line == u16::try_from(at).ok()));
            }
            Fault::Running(_) => prop_assert!(r.checks.iter().any(|c| c.code == CheckCode::StmtRunning
                && c.line == u16::try_from(at).ok())),
            Fault::OutOfPeriod => prop_assert!(r.checks.iter().any(|c| c.code == CheckCode::StmtDateOutsidePeriod
                && c.line == u16::try_from(at).ok())),
            Fault::Duplicate => prop_assert!(r.checks.iter().any(|c| c.code == CheckCode::StmtDuplicateLine
                && c.line == u16::try_from(at + 1).ok())),
            Fault::Closing(_) => {
                prop_assert!(has(CheckCode::StmtSum));
                prop_assert!(!has(CheckCode::StmtRunning));
            }
        }
    }
}

/// OCR lost the header row (a shaded band on a photo): the columns come
/// from the dated rows, and a split statement's columns from the balance.
#[test]
fn a_table_without_a_readable_header_row() {
    let mut d = Doc::new();
    letterhead(&mut d);
    let rows = [
        ("09/01/2026", "", "Previous Balance", "", "", "500.00"),
        ("09/02/2026", "INV-1", "Sales", "300.00", "", "800.00"),
        ("09/05/2026", "CK-77", "Payment", "", "200.00", "600.00"),
        ("09/08/2026", "INV-2", "Sales", "50.00", "", "650.00"),
        ("09/09/2026", "CM-3", "Return", "", "25.00", "625.00"),
    ];
    let mut y = 3200;
    for (date, r, desc, charge, pay, bal) in rows {
        d.text(500, y, date);
        d.text(2000, y, r);
        d.text(3500, y, desc);
        if !charge.is_empty() {
            d.amount(6600, y, charge);
        }
        if !pay.is_empty() {
            d.amount(7900, y, pay);
        }
        d.amount(9500, y, bal);
        y += 200;
    }
    let r = read_pages(&d.pages(), JobHint::statement());
    let st = r.statement.unwrap();
    let got: Vec<(Money, SignSource)> = st
        .lines
        .iter()
        .map(|l| (l.amount.value, l.sign_from))
        .collect();
    assert_eq!(
        got,
        [
            (money("300"), SignSource::ChargesColumn),
            (money("-200"), SignSource::PaymentsColumn),
            (money("50"), SignSource::ChargesColumn),
            (money("-25"), SignSource::PaymentsColumn),
        ]
    );
    assert_eq!(st.lines[1].reference.as_ref().unwrap().value, "CK-77");
    assert_eq!(st.opening.as_ref().unwrap().value, money("500"));
    assert!(r.checks.is_empty(), "{:?}", r.checks);
    // Not enough to go on without the header: unhinted, it isn't a statement.
    assert_eq!(
        read_pages(&d.pages(), JobHint::default()).kind,
        DocKind::Bill
    );
}
