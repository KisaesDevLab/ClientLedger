//! Bulk Entry paste and mapping helpers, and the money round-trip
//! properties.

#![allow(clippy::assert_is_empty, reason = "plain emptiness checks")]

use cl_core::{MoneyStyle, NegativeStyle};
use proptest::prelude::*;

use super::*;
use crate::date_entry::DateOrder;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 21).expect("date")
}

fn cells(date: &str, number: &str, contact: &str, account: &str, amount: &str) -> ParsedCells {
    let s = |t: &str| (!t.is_empty()).then(|| t.to_string());
    ParsedCells {
        date: s(date),
        number: s(number),
        contact: s(contact),
        account: s(account),
        amount: s(amount),
        ..ParsedCells::default()
    }
}

#[test]
fn pasted_and_imported_amounts_are_literal_never_calculated() {
    // Typed cells keep the calculator.
    assert_eq!(parse_amount("100/4"), Ok(Some(m("25.00"))));
    assert_eq!(parse_amount("400+44.25"), Ok(Some(m("444.25"))));
    // Pasted or imported cells do not.
    for calc in ["100/4", "400+44.25", "12 x 3", "5*2", "10-2", "100/4="] {
        assert!(parse_amount_literal(calc).is_err(), "{calc}");
    }
    for (text, want) in [
        ("$1,234.56", "1234.56"),
        ("(12.50)", "-12.50"),
        ("-7", "-7.00"),
        ("12.345", "12.35"),
        ("\u{2212}3.00", "-3.00"),
        ("  42 ", "42.00"),
    ] {
        assert_eq!(parse_amount_literal(text), Ok(Some(m(want))), "{text}");
    }
    assert_eq!(parse_amount_literal("  "), Ok(None));
    // A row from a file with a calculation: the amount is unreadable, so
    // the row shows it as missing.
    let mut keys = RowKeyGen::new();
    let rows = rows_from_parsed(
        SectionKind::Expenses,
        &[cells("2026-10-01", "", "", "6100", "100/4")],
        DateOrder::Mdy,
        today(),
        &mut keys,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].row.amount, None);
    assert_eq!(rows[0].unreadable, vec![Field::Amount]);
    // A header row with a calculation-looking cell is still a header.
    assert!(looks_like_header(&["Date", "100/4"]));
}

#[test]
fn header_detection() {
    assert!(looks_like_header(&["Date", "Payee", "Amount"]));
    assert!(looks_like_header(&["Date", "", "Memo"]));
    assert!(!looks_like_header(&["3/2/2026", "Acme", "12.50"]));
    assert!(
        !looks_like_header(&["Acme", "12.50"]),
        "an amount means data"
    );
    assert!(
        !looks_like_header(&["2026-03-02", "Acme"]),
        "a date means data"
    );
    assert!(!looks_like_header(&["2/29", "x"]), "a leap day is a date");
    assert!(
        !looks_like_header(&["", " "]),
        "an empty row is not a header"
    );
}

#[test]
fn default_mapping_uses_aliases_per_kind() {
    let headers = [
        "Txn Date", "Check #", "Pay to", "Category", "Amount", "Memo", "Debit",
    ];
    let map = default_mapping(SectionKind::Checks, &headers);
    assert_eq!(map[&Field::Date], 0);
    assert_eq!(map[&Field::Number], 1);
    assert_eq!(map[&Field::Contact], 2);
    assert_eq!(map[&Field::Account], 3);
    assert_eq!(map[&Field::Amount], 4);
    assert_eq!(map[&Field::Memo], 5);
    assert!(
        !map.contains_key(&Field::Debit),
        "checks have no debit column"
    );

    let je = default_mapping(
        SectionKind::JournalEntries,
        &[
            "Date",
            "Entry No.",
            "Account",
            "Debit",
            "Credit",
            "Amount",
            "Description",
        ],
    );
    assert_eq!(je[&Field::Number], 1);
    assert_eq!(je[&Field::Debit], 3);
    assert_eq!(je[&Field::Credit], 4);
    assert_eq!(je[&Field::Memo], 6);
    assert!(!je.contains_key(&Field::Amount));

    // The first matching column wins; unknown headers are skipped.
    let map = default_mapping(
        SectionKind::Deposits,
        &["Notes", "Memo", "Whatever", "Ref. No."],
    );
    assert_eq!(map[&Field::Memo], 0);
    assert_eq!(map[&Field::Number], 3);
    assert_eq!(map.len(), 2);
    assert_eq!(
        mappable_fields(SectionKind::Checks),
        [
            Field::Date,
            Field::Number,
            Field::Contact,
            Field::Account,
            Field::Amount,
            Field::Memo
        ]
    );
}

#[test]
fn positional_mapping_runs_from_the_cursor_column() {
    // Checks: Date, No., Print later, Pay to, Account, Amount, Memo, Tag.
    let checks = SectionKind::Checks.columns();
    let map = positional_mapping(checks, 1, 6);
    assert_eq!(map[&Field::Number], 0);
    assert!(!map.contains_key(&Field::PrintLater));
    assert_eq!(map[&Field::Contact], 2);
    assert_eq!(map[&Field::Account], 3);
    assert_eq!(map[&Field::Amount], 4);
    assert_eq!(map[&Field::Memo], 5);
    let past_end = positional_mapping(checks, 6, 4);
    assert_eq!(past_end.len(), 1, "Memo only; Tag and beyond are dropped");
    assert_eq!(past_end[&Field::Memo], 0);
}

#[test]
fn positional_mapping_follows_the_shown_column_order() {
    // Q-273: Memo moved before Account and No. hidden.
    let mut choice = super::ColumnChoice::default_for(SectionKind::Checks);
    choice
        .set_hidden(SectionKind::Checks, Field::Number, true)
        .expect("No. may be hidden");
    choice.move_by(Field::Memo, -2);
    let shown = choice.visible();
    assert_eq!(
        shown,
        [
            Field::Date,
            Field::PrintLater,
            Field::Contact,
            Field::Memo,
            Field::Account,
            Field::Amount,
            Field::Tag
        ]
    );
    let map = positional_mapping(&shown, 0, 6);
    assert_eq!(map[&Field::Date], 0);
    assert_eq!(map[&Field::Contact], 2);
    assert_eq!(map[&Field::Memo], 3);
    assert_eq!(map[&Field::Account], 4);
    assert_eq!(map[&Field::Amount], 5);
    assert!(
        !map.contains_key(&Field::Number),
        "a hidden column is skipped"
    );
}

#[test]
fn memory_survives_reordered_columns() {
    let mut mem = MappingMemory::default();
    let headers = ["Date", "Who", "GL", "Amt"];
    let mut map = Mapping::new();
    map.insert(Field::Date, 0);
    map.insert(Field::Contact, 1);
    map.insert(Field::Account, 2);
    map.insert(Field::Amount, 3);
    mem.remember(SectionKind::Expenses, &headers, &map);

    let reordered = ["amt", "GL ", "Date", "WHO"];
    let back = mem
        .recall(SectionKind::Expenses, &reordered)
        .expect("remembered");
    assert_eq!(back[&Field::Amount], 0);
    assert_eq!(back[&Field::Account], 1);
    assert_eq!(back[&Field::Date], 2);
    assert_eq!(back[&Field::Contact], 3);
    assert_eq!(
        mem.recall(SectionKind::Deposits, &reordered),
        None,
        "per type"
    );
    assert_eq!(mem.recall(SectionKind::Expenses, &["Date", "Who"]), None);

    // Remembering again for the same set replaces the entry.
    map.remove(&Field::Contact);
    mem.remember(SectionKind::Expenses, &reordered, &map);
    assert_eq!(mem.entries.len(), 1);
    assert!(
        !mem.recall(SectionKind::Expenses, &headers)
            .expect("kept")
            .contains_key(&Field::Contact)
    );

    // Bounded, oldest dropped; serializable; Debug shows a count.
    for i in 0..(MAX_REMEMBERED + 5) {
        let h = format!("col{i}");
        mem.remember(SectionKind::Checks, &[h.as_str()], &Mapping::new());
    }
    assert_eq!(mem.entries.len(), MAX_REMEMBERED);
    assert_eq!(mem.recall(SectionKind::Expenses, &headers), None);
    let json = serde_json::to_string(&mem).expect("json");
    let again: MappingMemory = serde_json::from_str(&json).expect("parse");
    assert_eq!(again, mem);
    assert!(!format!("{mem:?}").contains("col1"));
}

#[test]
fn parsed_rows_round_to_the_cent_and_keep_text() {
    let mut keys = RowKeyGen::after([RowKey(10)]);
    let rows = rows_from_parsed(
        SectionKind::Checks,
        &[
            cells("2026-03-02", "1001", "Acme", "Rent", "12.345"),
            cells("3/4/2026", "", " Bolt ", "6100", "(1,234.50)"),
            cells("not a date", "", "", "Rent", "abc"),
            ParsedCells::default(),
            cells("3/5", "", "", "", ""),
        ],
        DateOrder::Mdy,
        today(),
        &mut keys,
    );
    assert_eq!(rows.len(), 4, "the empty record is dropped");
    let r0 = &rows[0].row;
    assert_eq!(r0.key, RowKey(11));
    assert_eq!(r0.date, CalendarDate::from_ymd(2026, 3, 2).ok());
    assert_eq!(r0.amount, Some(m("12.35")), "rounded half away from zero");
    assert_eq!(
        (
            r0.number.as_str(),
            r0.contact_text.as_str(),
            r0.account_text.as_str()
        ),
        ("1001", "Acme", "Rent")
    );
    assert!(rows[0].unreadable.is_empty());
    assert_eq!(
        rows[1].row.amount,
        Some(m("-1234.50")),
        "kept negative: a row problem"
    );
    assert_eq!(rows[1].row.contact_text, "Bolt");
    assert_eq!(rows[2].unreadable, [Field::Date, Field::Amount]);
    assert_eq!(rows[2].row.amount, None);
    assert_eq!(
        rows[3].row.date,
        CalendarDate::from_ymd(2026, 3, 5).ok(),
        "M/D takes today's year"
    );

    let s = Section {
        kind: SectionKind::Checks,
        context: None,
        rows: rows.into_iter().map(|r| r.row).collect(),
    };
    let p = local_problems(&s, None);
    assert!(p.contains(&(RowKey(12), Problem::NotPositive)));

    // Journal entries read Debit and Credit, not Amount.
    let je = rows_from_parsed(
        SectionKind::JournalEntries,
        &[ParsedCells {
            date: Some("2026-03-02".into()),
            number: Some("J1".into()),
            account: Some("Rent".into()),
            amount: Some("5".into()),
            debit: Some("10.005".into()),
            credit: Some("x".into()),
            ..ParsedCells::default()
        }],
        DateOrder::Mdy,
        today(),
        &mut keys,
    );
    assert_eq!(je[0].row.amount, None);
    assert_eq!(je[0].row.debit, Some(m("10.01")));
    assert_eq!(je[0].unreadable, [Field::Credit]);
    assert_eq!(
        format!("{:?}", ParsedCells::default()),
        "ParsedCells { .. }"
    );
}

fn round_half_away(units: i128) -> i128 {
    let q = units / 100;
    let r = units % 100;
    let q = if r.abs() >= 50 { q + units.signum() } else { q };
    q * 100
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Any amount with up to four decimals parses to the cent, rounded half
    /// away from zero, exactly as the engine would be sent it.
    #[test]
    fn pasted_amounts_round_to_the_cent(units in -10_000_000_000_000_i128..10_000_000_000_000) {
        let text = Money::from_ten_thousandths(units).expect("money").to_wire();
        let got = parse_amount(&text).expect("parses").expect("some");
        prop_assert_eq!(got.to_ten_thousandths(), round_half_away(units));
    }

    /// What the grid shows parses back to the same amount in every style.
    #[test]
    fn parse_display_parse_is_stable(
        cents in -100_000_000_000_i64..100_000_000_000,
        parens in any::<bool>(),
        symbol in any::<bool>(),
    ) {
        let style = MoneyStyle {
            negative: if parens { NegativeStyle::Parens } else { NegativeStyle::Minus },
            symbol,
        };
        let first = Money::from_cents(cents).expect("cents");
        let shown = first.format(style);
        let again = parse_amount(&shown).expect("parses").expect("some");
        prop_assert_eq!(again, first);
        prop_assert_eq!(parse_amount(&again.format(style)).expect("parses"), Some(first));
    }
}
