//! Bulk Entry model: rows, local checks, gating, totals and the exactness
//! properties.

#![allow(
    clippy::assert_is_empty,
    clippy::many_single_char_names,
    clippy::cloned_ref_to_slice_refs,
    reason = "short test bindings and plain emptiness checks"
)]

use super::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn acct(n: u8) -> AccountId {
    AccountId::parse(&format!("4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02x}")).expect("id")
}

fn day(d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, 3, d).expect("date")
}

const BANK: u8 = 1;
const CARD: u8 = 2;
const RENT: u8 = 10;
const SUPPLIES: u8 = 11;

fn row(key: u64, date: u32, account: u8, amount: &str) -> RowDraft {
    RowDraft {
        key: RowKey(key),
        date: Some(day(date)),
        account: Some(acct(account)),
        amount: Some(m(amount)),
        ..RowDraft::default()
    }
}

fn je_row(
    key: u64,
    date: u32,
    no: &str,
    account: u8,
    debit: Option<&str>,
    credit: Option<&str>,
) -> RowDraft {
    RowDraft {
        key: RowKey(key),
        date: Some(day(date)),
        number: no.into(),
        account: Some(acct(account)),
        debit: debit.map(m),
        credit: credit.map(m),
        ..RowDraft::default()
    }
}

fn section(kind: SectionKind, context: Option<u8>, rows: Vec<RowDraft>) -> Section {
    Section {
        kind,
        context: context.map(acct),
        rows,
    }
}

fn problems_of(list: &[(RowKey, Problem)], key: u64) -> Vec<Problem> {
    list.iter()
        .filter(|(k, _)| *k == RowKey(key))
        .map(|(_, p)| p.clone())
        .collect()
}

// ------------------------------------------------------------------ B1

#[test]
fn columns_and_contexts_per_kind() {
    assert_eq!(SectionKind::Checks.columns().len(), 8);
    assert!(SectionKind::Checks.columns().contains(&Field::PrintLater));
    assert!(!SectionKind::Deposits.columns().contains(&Field::PrintLater));
    let je = SectionKind::JournalEntries.columns();
    assert!(je.contains(&Field::Debit) && je.contains(&Field::Credit));
    assert!(!je.contains(&Field::Amount));
    assert_eq!(SectionKind::Checks.needs_context(), Some(ContextKind::Bank));
    assert_eq!(
        SectionKind::Expenses.needs_context(),
        Some(ContextKind::Bank)
    );
    assert_eq!(
        SectionKind::Deposits.needs_context(),
        Some(ContextKind::Bank)
    );
    assert_eq!(
        SectionKind::CardCharges.needs_context(),
        Some(ContextKind::Card)
    );
    assert_eq!(
        SectionKind::CardCredits.needs_context(),
        Some(ContextKind::Card)
    );
    assert_eq!(SectionKind::JournalEntries.needs_context(), None);
    let wires: Vec<_> = SectionKind::ALL.iter().map(|k| k.engine_kind()).collect();
    assert_eq!(
        wires,
        [
            "check",
            "expense",
            "credit_card_charge",
            "credit_card_credit",
            "deposit",
            "journal_entry"
        ]
    );
}

#[test]
fn typed_amounts_round_to_the_cent() {
    assert_eq!(parse_amount("12.345").expect("ok"), Some(m("12.35")));
    assert_eq!(parse_amount("-12.345").expect("ok"), Some(m("-12.35")));
    assert_eq!(parse_amount("2.505").expect("ok"), Some(m("2.51")));
    assert_eq!(parse_amount("$1,234.50").expect("ok"), Some(m("1234.50")));
    assert_eq!(parse_amount("(1,234.50)").expect("ok"), Some(m("-1234.50")));
    assert_eq!(parse_amount("100/3").expect("ok"), Some(m("33.33")));
    assert_eq!(parse_amount("  ").expect("ok"), None);
    assert!(parse_amount("abc").is_err());
}

#[test]
fn blank_rows_are_ignored() {
    let blank = RowDraft {
        key: RowKey(1),
        date: Some(day(1)),
        number: "1001".into(),
        print_later: true,
        ..RowDraft::default()
    };
    assert!(
        blank.is_blank(),
        "date, number and print later alone are blank"
    );
    let s = section(SectionKind::Checks, Some(BANK), vec![blank]);
    assert!(local_problems(&s, Some(day(31))).is_empty());
    let memo_only = RowDraft {
        memo: "x".into(),
        ..RowDraft::new(RowKey(2))
    };
    assert!(!memo_only.is_blank());
}

#[test]
fn required_cells_per_kind() {
    let r = RowDraft {
        memo: "something".into(),
        ..RowDraft::new(RowKey(1))
    };
    let s = section(SectionKind::Expenses, Some(BANK), vec![r.clone()]);
    let p = problems_of(&local_problems(&s, None), 1);
    assert!(p.contains(&Problem::Missing(Field::Date)));
    assert!(p.contains(&Problem::Missing(Field::Account)));
    assert!(p.contains(&Problem::Missing(Field::Amount)));
    assert!(
        !p.contains(&Problem::Missing(Field::Contact)),
        "payee is optional"
    );
    assert!(
        !p.contains(&Problem::Missing(Field::Number)),
        "ref no. is optional"
    );

    let s = section(SectionKind::JournalEntries, None, vec![r]);
    let p = problems_of(&local_problems(&s, None), 1);
    assert!(
        p.contains(&Problem::Missing(Field::Number)),
        "Entry No. is required"
    );
    assert!(p.contains(&Problem::Missing(Field::Debit)));
    assert!(!p.contains(&Problem::Missing(Field::Amount)));

    // A typed (not picked) account counts.
    let typed = RowDraft {
        account: None,
        account_text: "Rent".into(),
        ..row(3, 2, RENT, "10.00")
    };
    let s = section(SectionKind::Deposits, Some(BANK), vec![typed]);
    assert!(local_problems(&s, None).is_empty());
}

#[test]
fn amounts_must_be_positive_whole_cents() {
    let s = section(
        SectionKind::Checks,
        Some(BANK),
        vec![
            row(1, 2, RENT, "0.00"),
            row(2, 2, RENT, "-5.00"),
            row(3, 2, RENT, "5.0050"),
            row(4, 2, RENT, "0.01"),
        ],
    );
    let p = local_problems(&s, None);
    assert_eq!(problems_of(&p, 1), [Problem::NotPositive]);
    assert_eq!(problems_of(&p, 2), [Problem::NotPositive]);
    assert_eq!(problems_of(&p, 3), [Problem::SubCent(Field::Amount)]);
    assert!(problems_of(&p, 4).is_empty());
}

#[test]
fn journal_rows_take_one_side() {
    let s = section(
        SectionKind::JournalEntries,
        None,
        vec![
            je_row(1, 2, "J1", RENT, Some("5.00"), Some("5.00")),
            je_row(2, 2, "J2", RENT, Some("-5.00"), None),
        ],
    );
    let p = local_problems(&s, None);
    assert!(problems_of(&p, 1).contains(&Problem::BothDebitCredit));
    assert!(problems_of(&p, 2).contains(&Problem::NotPositive));
}

#[test]
fn journal_groups_balance_exactly_by_date_and_entry_no() {
    let s = section(
        SectionKind::JournalEntries,
        None,
        vec![
            je_row(1, 2, "J1", RENT, Some("100.01"), None),
            je_row(2, 2, "J1", SUPPLIES, None, Some("100.00")),
            // Same Entry No., other date: its own group, balanced.
            je_row(3, 3, "J1", RENT, Some("7.00"), None),
            je_row(4, 3, "J1", SUPPLIES, None, Some("7.00")),
        ],
    );
    let p = local_problems(&s, None);
    let want = Problem::JeUnbalanced {
        group: JeGroupKey {
            date: day(2),
            number: "J1".into(),
        },
        difference: m("0.01"),
    };
    assert_eq!(problems_of(&p, 1), [want.clone()]);
    assert_eq!(problems_of(&p, 2), [want]);
    assert!(problems_of(&p, 3).is_empty());
    assert!(problems_of(&p, 4).is_empty());
    assert_eq!(s.je_groups().len(), 2);
    // Entry No. is trimmed for grouping.
    let s = section(
        SectionKind::JournalEntries,
        None,
        vec![
            je_row(1, 2, " J1", RENT, Some("1.00"), None),
            je_row(2, 2, "J1 ", SUPPLIES, None, Some("1.00")),
        ],
    );
    assert!(local_problems(&s, None).is_empty());
}

#[test]
fn duplicate_check_numbers_within_and_across_sections() {
    let numbered = |key, no: &str| RowDraft {
        number: no.into(),
        ..row(key, 2, RENT, "1.00")
    };
    let s = section(
        SectionKind::Checks,
        Some(BANK),
        vec![
            numbered(1, "1001"),
            numbered(2, "01001"),
            numbered(3, "1002"),
        ],
    );
    let p = local_problems(&s, None);
    assert_eq!(problems_of(&p, 1), [Problem::DuplicateNumber]);
    assert_eq!(problems_of(&p, 2), [Problem::DuplicateNumber]);
    assert!(problems_of(&p, 3).is_empty());

    // Across two Checks sections on the same account, not on another.
    let batch = BulkBatch {
        sections: vec![
            section(SectionKind::Checks, Some(BANK), vec![numbered(1, "1002")]),
            section(SectionKind::Checks, Some(BANK), vec![numbered(2, "1002")]),
            section(SectionKind::Checks, Some(3), vec![numbered(3, "1002")]),
            // Ref no. on an expense is not a check number.
            section(SectionKind::Expenses, Some(BANK), vec![numbered(4, "1002")]),
        ],
        add_new_names: false,
    };
    let p = batch.local_problems(None);
    assert_eq!(problems_of(&p, 1), [Problem::DuplicateNumber]);
    assert_eq!(problems_of(&p, 2), [Problem::DuplicateNumber]);
    assert!(problems_of(&p, 3).is_empty());
    assert!(problems_of(&p, 4).is_empty());
}

#[test]
fn check_number_and_print_later_rules() {
    let with = |key, no: &str, later: bool, payee: &str| RowDraft {
        number: no.into(),
        print_later: later,
        contact_text: payee.into(),
        ..row(key, 2, RENT, "1.00")
    };
    let s = section(
        SectionKind::Checks,
        Some(BANK),
        vec![
            with(1, "12a", false, ""),
            with(2, "0", false, ""),
            with(3, "1234567890", false, ""),
            with(4, "", false, ""),
            with(5, "77", true, "Acme"),
            with(6, "", true, ""),
        ],
    );
    let p = local_problems(&s, None);
    assert_eq!(problems_of(&p, 1), [Problem::BadCheckNumber]);
    assert_eq!(problems_of(&p, 2), [Problem::BadCheckNumber]);
    assert_eq!(problems_of(&p, 3), [Problem::BadCheckNumber]);
    assert!(problems_of(&p, 4).is_empty(), "empty means the next number");
    assert_eq!(problems_of(&p, 5), [Problem::PrintLaterNumber]);
    assert_eq!(problems_of(&p, 6), [Problem::Missing(Field::Contact)]);
}

#[test]
fn closed_period_marks_and_other_checks() {
    let s = section(
        SectionKind::Deposits,
        Some(BANK),
        vec![
            row(1, 10, RENT, "1.00"),
            row(2, 11, RENT, "1.00"),
            row(3, 12, BANK, "1.00"),
            RowDraft {
                memo: "m".repeat(MAX_MEMO_CHARS + 1),
                number: "r".repeat(MAX_REF_CHARS + 1),
                ..row(4, 12, RENT, "1.00")
            },
        ],
    );
    let p = local_problems(&s, Some(day(10)));
    assert_eq!(problems_of(&p, 1), [Problem::Closed]);
    assert!(Problem::Closed.is_lock_mark());
    assert!(problems_of(&p, 2).is_empty());
    assert_eq!(problems_of(&p, 3), [Problem::SameAsContext]);
    assert_eq!(
        problems_of(&p, 4),
        [
            Problem::TooLong(Field::Number),
            Problem::TooLong(Field::Memo)
        ]
    );
    let batch = BulkBatch::new(s);
    assert_eq!(batch.locked_rows(Some(day(10))), [RowKey(1)]);
    assert!(batch.locked_rows(None).is_empty());
}

#[test]
fn problem_fields() {
    assert_eq!(Problem::DuplicateNumber.field(), Some(Field::Number));
    assert_eq!(Problem::Closed.field(), Some(Field::Date));
    assert_eq!(Problem::SameAsContext.field(), Some(Field::Account));
    assert_eq!(Problem::Missing(Field::Memo).field(), Some(Field::Memo));
    assert_eq!(Problem::NotPositive.field(), None);
}

#[test]
fn row_hash_tracks_content_not_key() {
    let a = row(1, 2, RENT, "1.00");
    let mut b = a.clone();
    b.key = RowKey(99);
    assert_eq!(a.hash(), b.hash(), "the key is not content");
    b.amount = Some(m("1.01"));
    assert_ne!(a.hash(), b.hash());
    let mut c = a.clone();
    c.memo = "x".into();
    assert_ne!(a.hash(), c.hash());
    // Field boundaries are length-prefixed.
    let x = RowDraft {
        contact_text: "ab".into(),
        account_text: "c".into(),
        ..RowDraft::new(RowKey(1))
    };
    let y = RowDraft {
        contact_text: "a".into(),
        account_text: "bc".into(),
        ..RowDraft::new(RowKey(1))
    };
    assert_ne!(x.hash(), y.hash());
}

#[test]
fn debug_hides_owner_data() {
    let r = RowDraft {
        contact_text: "Secret Vendor".into(),
        memo: "private memo".into(),
        ..row(1, 2, RENT, "4321.09")
    };
    let text = format!("{r:?}");
    assert!(!text.contains("Secret") && !text.contains("private") && !text.contains("4321"));
}

#[test]
fn next_check_number_counts_books_and_batch() {
    let numbered = |key, no: &str| RowDraft {
        number: no.into(),
        ..row(key, 2, RENT, "1.00")
    };
    let batch = BulkBatch::new(section(
        SectionKind::Checks,
        Some(BANK),
        vec![numbered(1, "1005"), numbered(2, "x")],
    ));
    assert_eq!(
        batch.next_check_number(acct(BANK), Some(1000)).as_deref(),
        Some("1006")
    );
    assert_eq!(
        batch.next_check_number(acct(BANK), Some(2000)).as_deref(),
        Some("2001")
    );
    assert_eq!(batch.next_check_number(acct(CARD), None), None);
    assert_eq!(
        batch.next_check_number(acct(CARD), Some(7)).as_deref(),
        Some("8")
    );
    assert_eq!(batch.next_check_number(acct(CARD), Some(999_999_999)), None);
}
