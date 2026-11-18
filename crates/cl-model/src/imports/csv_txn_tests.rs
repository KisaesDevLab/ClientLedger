use super::*;
use proptest::prelude::*;

fn dec(s: &str) -> Decimal {
    s.parse().expect("decimal")
}

fn rows(text: &[&[&str]]) -> Vec<Vec<String>> {
    text.iter()
        .map(|r| r.iter().map(|c| (*c).to_string()).collect())
        .collect()
}

const HEADERS: [&str; 9] = [
    "Group", "Type", "Date", "Num", "Name", "Account", "Amount", "Debit", "Credit",
];

fn mapping() -> TxnMapping {
    guess_mapping(&HEADERS)
}

/// The engine's E0 table (plan §2a lines 69–70), kept equal to it here.
#[test]
fn csv_txn_amount_table_matches_e0() {
    let ok: [(&str, bool, &str); 18] = [
        ("1234.50", false, "1234.50"),
        ("$1,234.50", false, "1234.50"),
        ("(1,234.00)", false, "-1234.00"),
        ("-5", false, "-5"),
        ("5-", false, "-5"),
        ("$-5", false, "-5"),
        ("-$5.25", false, "-5.25"),
        ("\u{2212}7.10", false, "-7.10"),
        ("1\u{a0}000.00", false, "1000.00"),
        (" 12 ", false, "12"),
        ("100.00CR", true, "-100.00"),
        ("100.00 DR", true, "100.00"),
        ("(100.00)CR", true, "100.00"),
        ("0.0040", false, "0.0040"),
        (".5", false, "0.5"),
        ("-0.00", false, "0.00"),
        ("1,000,000", false, "1000000"),
        ("999999999999999.9999", false, "999999999999999.9999"),
    ];
    for (text, signed, want) in ok {
        let got = parse_amount(text, signed)
            .unwrap_or_else(|e| panic!("{text:?}: {e:?}"))
            .expect("not blank");
        assert_eq!(canonical_amount(got), want, "{text:?}");
    }
    let bad: [(&str, bool, AmountProblem); 13] = [
        ("1e5", false, AmountProblem::Invalid),
        ("1E-2", false, AmountProblem::Invalid),
        ("1.23456", false, AmountProblem::TooManyPlaces),
        ("-(5)", false, AmountProblem::Invalid),
        ("(-5)", false, AmountProblem::Invalid),
        ("1.234,56", false, AmountProblem::Invalid),
        ("1.234,5", false, AmountProblem::Invalid),
        ("--5", false, AmountProblem::Invalid),
        ("5CR", false, AmountProblem::Invalid),
        ("abc", false, AmountProblem::Invalid),
        ("$", false, AmountProblem::Invalid),
        ("1,,000", false, AmountProblem::Invalid),
        ("1000000000000000", false, AmountProblem::TooLarge),
    ];
    for (text, signed, want) in bad {
        assert_eq!(parse_amount(text, signed), Err(want), "{text:?}");
    }
    assert_eq!(parse_amount("", false), Ok(None));
    assert_eq!(parse_amount(" \u{a0} ", true), Ok(None));
}

#[test]
fn csv_txn_dates_are_full_and_never_depend_on_today() {
    let d = |y, m, dd| CalendarDate::from_ymd(y, m, dd).ok();
    assert_eq!(parse_date("2026-03-31"), d(2026, 3, 31));
    assert_eq!(parse_date("3/31/2026"), d(2026, 3, 31));
    assert_eq!(parse_date("03/01/2026"), d(2026, 3, 1));
    for bad in [
        "3/31/26",
        "3/31",
        "T",
        "+",
        "2026-02-30",
        "31/3/2026",
        "",
        "March 3",
    ] {
        assert_eq!(parse_date(bad), None, "{bad:?}");
    }
}

#[test]
fn csv_txn_header_guess_and_saved_mapping_follow_the_headers() {
    let m = mapping();
    assert_eq!(m.get(&TxnField::GroupId), Some(&0));
    assert_eq!(m.get(&TxnField::Number), Some(&3));
    assert_eq!(m.get(&TxnField::LineAccount), Some(&5));
    assert_eq!(m.get(&TxnField::LineCredit), Some(&8));
    assert_eq!(mapping_gaps(&m), Vec::new());
    assert_eq!(
        mapping_gaps(&TxnMapping::new()),
        vec![
            MissingColumn::Type,
            MissingColumn::Date,
            MissingColumn::AccountOrItem,
            MissingColumn::Amount
        ]
    );
    // Saved by header name, so a reordered file maps the same.
    let saved = mapping_to_saved(&HEADERS, &m);
    let reordered = [
        "Credit", "Debit", "Amount", "Account", "Name", "Num", "Date", "Type", "Group",
    ];
    let again = mapping_from_saved(&reordered, &saved);
    assert_eq!(again.get(&TxnField::GroupId), Some(&8));
    assert_eq!(again.get(&TxnField::LineCredit), Some(&0));
    assert_eq!(header_signature(&HEADERS), header_signature(&reordered));
    assert_ne!(header_signature(&HEADERS), header_signature(&HEADERS[..5]));
}

#[test]
fn csv_txn_groups_rows_into_documents() {
    let r = rows(&[
        &[
            "1",
            "Invoice",
            "2026-03-02",
            "1001",
            "Pine Co",
            "Sales",
            "100.00",
            "",
            "",
        ],
        &["1", "", "", "", "", "Sales", "25.50", "", ""],
        &[
            "2",
            "Bill",
            "3/5/2026",
            "B-7",
            "Oak Supply",
            "Supplies",
            "$40",
            "",
            "",
        ],
        &[
            "3",
            "Journal Entry",
            "2026-03-31",
            "",
            "",
            "Rent",
            "",
            "500.00",
            "",
        ],
        &[
            "3",
            "Journal Entry",
            "2026-03-31",
            "",
            "",
            "Checking",
            "",
            "",
            "500.00",
        ],
        &["", "", "", "", "", "", "", "", ""],
        &[
            "1",
            "invoice",
            "03/02/2026",
            "",
            "",
            "Sales",
            "(5.00)",
            "",
            "",
        ],
    ]);
    let plan = build(&r, 2, &mapping());
    assert!(plan.can_import(), "{plan:?}");
    assert_eq!(plan.rows, 6);
    assert_eq!(
        plan.counts(),
        DocCounts {
            invoices: 1,
            bills: 1,
            journal_entries: 1
        }
    );
    let inv = &plan.documents[0];
    assert_eq!(inv.lines.len(), 3);
    assert_eq!(inv.total(), dec("120.50"));
    assert_eq!(inv.lines[2].row, 8, "the blank row still counts");
    assert_eq!(plan.documents[2].total(), dec("500.00"));
    assert_eq!(plan.documents[2].imbalance(), Decimal::ZERO);
}

#[test]
fn csv_txn_without_a_group_column_groups_by_type_date_number_and_name() {
    let headers = ["Type", "Date", "No", "Customer", "Item", "Amount"];
    let m = guess_mapping(&headers);
    let r = rows(&[
        &["Invoice", "2026-01-05", "7", "Ash", "Design", "10"],
        &["Invoice", "2026-01-05", "7", "Ash", "Hosting", "5"],
        &["Invoice", "2026-01-05", "8", "Ash", "Design", "1"],
    ]);
    let plan = build(&r, 2, &m);
    assert!(plan.can_import(), "{plan:?}");
    assert_eq!(plan.documents.len(), 2);
    assert_eq!(plan.documents[0].group, "G1");
    assert_eq!(plan.documents[0].total(), dec("15"));
}

#[test]
fn csv_txn_unbalanced_entry_is_blocked_with_its_difference() {
    let r = rows(&[
        &["9", "JE", "2026-03-31", "", "", "Rent", "", "500.00", ""],
        &[
            "9",
            "JE",
            "2026-03-31",
            "",
            "",
            "Checking",
            "",
            "",
            "499.99",
        ],
    ]);
    let plan = build(&r, 2, &mapping());
    assert!(!plan.can_import());
    assert_eq!(
        plan.problems,
        vec![RowProblem {
            row: 2,
            field: None,
            kind: ProblemKind::Unbalanced(dec("0.01")),
        }]
    );
    // The amount never reaches Debug.
    assert!(!format!("{plan:?}").contains("0.01"));
}

#[test]
fn csv_txn_row_problems_name_the_row_and_field() {
    let r = rows(&[
        &["1", "Invoice", "2026-03-02", "1", "", "Sales", "10", "", ""],
        &[
            "2",
            "Estimate",
            "2026-03-02",
            "2",
            "Pine",
            "Sales",
            "10",
            "",
            "",
        ],
        &[
            "3",
            "Bill",
            "31/3/2026",
            "3",
            "Oak",
            "Supplies",
            "10",
            "",
            "",
        ],
        &[
            "4",
            "Bill",
            "2026-03-02",
            "4",
            "Oak",
            "Supplies",
            "1.23456",
            "",
            "",
        ],
        &["5", "JE", "2026-03-02", "", "", "Rent", "", "5", "5"],
        &["6", "JE", "2026-03-02", "", "", "", "", "5", ""],
        &[
            "7",
            "Bill",
            "2026-03-02",
            "7",
            "Oak",
            "Supplies",
            "",
            "5",
            "",
        ],
        &[
            "8",
            "Bill",
            "2026-03-02",
            "8",
            "Oak",
            "Supplies",
            "",
            "",
            "",
        ],
        &[
            "",
            "Bill",
            "2026-03-02",
            "9",
            "Oak",
            "Supplies",
            "5",
            "",
            "",
        ],
        &[
            "10",
            "Bill",
            "2026-03-02",
            "10",
            "Oak",
            "Supplies",
            "5",
            "",
            "",
        ],
        &[
            "10",
            "Invoice",
            "2026-03-02",
            "10",
            "Oak",
            "Supplies",
            "5",
            "",
            "",
        ],
        &[
            "11",
            "Invoice",
            "2026-03-02",
            "11",
            "Ash",
            "Sales",
            "-5",
            "",
            "",
        ],
        &["12", "JE", "2026-03-02", "", "", "Rent", "", "-5", ""],
    ]);
    let plan = build(&r, 2, &mapping());
    let got: Vec<(usize, Option<TxnField>, &str)> = plan
        .problems
        .iter()
        .map(|p| (p.row, p.field, p.kind.code()))
        .collect();
    let want: Vec<(usize, Option<TxnField>, &str)> = vec![
        (2, Some(TxnField::Name), "missing"),
        (3, Some(TxnField::Type), "type_unknown"),
        (4, Some(TxnField::Date), "date_invalid"),
        (5, Some(TxnField::LineAmount), "amount_too_precise"),
        (6, Some(TxnField::LineDebit), "debit_and_credit"),
        (7, Some(TxnField::LineAccount), "missing"),
        (8, Some(TxnField::LineDebit), "debit_credit_on_document"),
        (9, Some(TxnField::LineAmount), "amount_missing"),
        (10, Some(TxnField::GroupId), "missing"),
        (12, Some(TxnField::Type), "mixed_group"),
        (13, Some(TxnField::LineAmount), "total_not_positive"),
        (14, Some(TxnField::LineDebit), "amount_negative"),
    ];
    assert_eq!(got, want);
    assert!(!plan.can_import());
}

#[test]
fn csv_txn_empty_file_and_due_date_checks() {
    let plan = build(&rows(&[&["", ""]]), 2, &mapping());
    assert_eq!(plan.problems[0].kind, ProblemKind::NoRows);
    assert!(!plan.can_import());
    let headers = ["Type", "Date", "Name", "Due Date", "Account", "Amount"];
    let r = rows(&[&["Bill", "2026-03-10", "Oak", "2026-03-01", "Supplies", "5"]]);
    let plan = build(&r, 2, &guess_mapping(&headers));
    assert_eq!(plan.problems[0].kind, ProblemKind::DueBeforeDate);
}

#[test]
fn csv_txn_canonical_csv_is_exact_and_maps_rows_back() {
    let r = rows(&[
        &[
            "A",
            "Invoice",
            "3/2/2026",
            "1001",
            "Pine, Co \"West\"",
            "Sales",
            "1,000.5",
            "",
            "",
        ],
        &["B", "JE", "2026-03-31", "", "", "Rent", "250", "", ""],
        &[
            "B",
            "JE",
            "2026-03-31",
            "",
            "",
            "Checking",
            "-250.00",
            "",
            "",
        ],
        &["C", "JE", "2026-03-31", "", "", "Rent", "", "0.0040", "0"],
        &["C", "JE", "2026-03-31", "", "", "Cash", "", "", "0.0040"],
    ]);
    let plan = build(&r, 2, &mapping());
    assert!(plan.can_import(), "{plan:?}");
    let csv = plan.canonical_csv();
    let lines: Vec<&str> = csv.split("\r\n").collect();
    assert_eq!(
        lines[0],
        "group_id,type,date,number,name,due_date,memo,line_account,line_item,line_description,line_amount,line_debit,line_credit,line_tag"
    );
    assert_eq!(
        lines[1],
        "A,invoice,2026-03-02,1001,\"Pine, Co \"\"West\"\"\",,,Sales,,,1000.5,,,"
    );
    assert_eq!(lines[2], "B,journal_entry,2026-03-31,,,,,Rent,,,,250,,");
    assert_eq!(
        lines[3],
        "B,journal_entry,2026-03-31,,,,,Checking,,,,,250.00,"
    );
    assert_eq!(lines[4], "C,journal_entry,2026-03-31,,,,,Rent,,,,0.0040,,");
    // Every line has one amount cell (E4 refuses a blank amount).
    for row in plan.canonical_rows().iter().skip(1) {
        assert_eq!(row[10..13].iter().filter(|c| !c.is_empty()).count(), 1);
    }
    assert_eq!(plan.source_row(2), Some(2));
    assert_eq!(plan.source_row(6), Some(6));
    assert_eq!(plan.source_row(1), None);
    assert_eq!(plan.source_row(0), None);
    assert_eq!(plan.source_row(99), None);
}

#[test]
fn csv_txn_debug_never_shows_cells() {
    let r = rows(&[&[
        "1",
        "Invoice",
        "2026-03-02",
        "1",
        "Secret Name",
        "Sales",
        "777.77",
        "",
        "",
    ]]);
    let plan = build(&r, 2, &mapping());
    let dbg = format!("{plan:?} {:?}", plan.documents);
    assert!(
        !dbg.contains("Secret Name") && !dbg.contains("777"),
        "{dbg}"
    );
}

/// No float types in the import models (plan §3; the xtask `nofloat` scan
/// covers `src/` too).
#[test]
fn csv_txn_has_no_floats() {
    let src = include_str!("csv_txn.rs");
    for word in [concat!("f", "32"), concat!("f", "64"), concat!("as ", "f")] {
        assert!(!src.contains(word), "{word}");
    }
}

proptest! {
    /// Any amount with up to four places reads back exactly, in every
    /// accepted spelling.
    #[test]
    fn csv_txn_amounts_round_trip(units in -999_999_999_999i64..999_999_999_999i64, places in 0u32..=4) {
        let d = Decimal::new(units, places);
        let plain = canonical_amount(d);
        prop_assert_eq!(parse_amount(&plain, false), Ok(Some(d)));
        let abs = canonical_amount(d.abs());
        let (int, frac) = abs.split_once('.').map_or((abs.as_str(), None), |(a, b)| (a, Some(b)));
        let mut grouped = String::new();
        for (i, c) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        if let Some(f) = frac {
            grouped = format!("{grouped}.{f}");
        }
        let spelled = if d.is_sign_negative() && !d.is_zero() {
            format!("(${grouped})")
        } else {
            format!("${grouped}")
        };
        prop_assert_eq!(parse_amount(&spelled, false), Ok(Some(d)));
        let cr = if d.is_sign_negative() && !d.is_zero() { format!("{abs}CR") } else { format!("{abs}DR") };
        prop_assert_eq!(parse_amount(&cr, true), Ok(Some(d)));
    }

    /// A balanced two-line entry of any amount imports, and its canonical
    /// debit equals its credit.
    #[test]
    fn csv_txn_balanced_entries_pass(units in 1i64..999_999_999_999i64, places in 0u32..=4) {
        let a = canonical_amount(Decimal::new(units, places));
        let r = vec![
            vec!["1".into(), "JE".into(), "2026-01-31".into(), String::new(), String::new(), "Rent".into(), String::new(), a.clone(), String::new()],
            vec!["1".into(), "JE".into(), "2026-01-31".into(), String::new(), String::new(), "Cash".into(), String::new(), String::new(), a.clone()],
        ];
        let plan = build(&r, 2, &mapping());
        prop_assert!(plan.can_import());
        let rows = plan.canonical_rows();
        prop_assert_eq!(&rows[1][11], &a);
        prop_assert_eq!(&rows[2][12], &a);
    }
}
