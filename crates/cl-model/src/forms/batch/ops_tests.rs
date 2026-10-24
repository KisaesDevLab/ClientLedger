//! Bulk Entry row operations and column choice (P16 WP5a): Copy down per
//! column kind, bulk Set, Delete rows, one undo step per action, changed
//! rows re-checked, money never touched, and the required-columns property.

#![allow(
    clippy::assert_is_empty,
    clippy::many_single_char_names,
    reason = "short test bindings and plain emptiness checks"
)]

use std::collections::BTreeSet;

use proptest::prelude::*;

use super::ops::{self, CopyScope, FieldValue};
use super::*;
use crate::forms::FormState;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn acct(n: u8) -> AccountId {
    AccountId::parse(&format!("4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02x}")).expect("id")
}

fn contact(n: u8) -> ContactId {
    ContactId::parse(&format!("5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02x}")).expect("id")
}

fn tag(n: u8) -> TagId {
    TagId::parse(&format!("6c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02x}")).expect("id")
}

fn day(d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, 3, d).expect("date")
}

const BANK: u8 = 1;
const RENT: u8 = 10;
const SUPPLIES: u8 = 11;

/// A filled check row: date, number, payee, account, amount, memo, tag.
fn check(key: u64, date: u32, number: &str, amount: &str) -> RowDraft {
    RowDraft {
        key: RowKey(key),
        date: Some(day(date)),
        number: number.into(),
        contact: Some(contact(u8::try_from(key).expect("small"))),
        contact_text: format!("Payee {key}"),
        account: Some(acct(RENT)),
        account_text: "Rent".into(),
        amount: Some(m(amount)),
        memo: format!("memo {key}"),
        tag: Some(tag(1)),
        ..RowDraft::default()
    }
}

/// Four check rows (keys 1-4), a blank row (5) in the middle and the blank
/// entry row (6) at the end.
fn checks() -> Section {
    let mut rows = vec![
        check(1, 2, "1001", "100.00"),
        check(2, 3, "1002", "25.50"),
        RowDraft::new(RowKey(5)),
        check(3, 4, "1003", "7.25"),
        check(4, 5, "1004", "12.00"),
        RowDraft::new(RowKey(6)),
    ];
    rows[1].contact = None;
    rows[1].contact_text = "Typed payee".into();
    rows[1].account = None;
    rows[1].account_text = "Typed account".into();
    rows[1].tag = None;
    rows[1].print_later = true;
    rows[1].number.clear();
    Section {
        kind: SectionKind::Checks,
        context: Some(acct(BANK)),
        rows,
    }
}

fn row(s: &Section, key: u64) -> &RowDraft {
    s.rows.iter().find(|r| r.key == RowKey(key)).expect("row")
}

/// A row's key, number, amount, debit and credit.
type Protected = (RowKey, String, Option<Money>, Option<Money>, Option<Money>);

/// The money and number cells of every row, which no operation may change.
fn protected(s: &Section) -> Vec<Protected> {
    s.rows
        .iter()
        .map(|r| (r.key, r.number.clone(), r.amount, r.debit, r.credit))
        .collect()
}

fn blanks(s: &Section) -> Vec<RowKey> {
    s.rows
        .iter()
        .filter(|r| r.is_blank())
        .map(|r| r.key)
        .collect()
}

// ------------------------------------------------------------ copy down

#[test]
fn copy_down_offers_every_column_but_money_and_numbers() {
    for kind in SectionKind::ALL {
        for f in kind.columns() {
            let money_or_number = matches!(
                f,
                Field::Amount | Field::Debit | Field::Credit | Field::Number
            );
            assert_eq!(ops::copyable(kind, *f), !money_or_number, "{kind:?} {f:?}");
            assert_eq!(ops::settable(kind, *f), !money_or_number, "{kind:?} {f:?}");
        }
        // A column the kind does not have is never offered.
        if !kind.columns().contains(&Field::PrintLater) {
            assert!(!ops::copyable(kind, Field::PrintLater));
            assert!(!ops::settable(kind, Field::PrintLater));
        }
    }
}

#[test]
fn copy_down_each_column_kind() {
    type Same = fn(&RowDraft, &RowDraft) -> bool;
    let cases: [(Field, Same); 6] = [
        (Field::Date, |a, b| a.date == b.date),
        (Field::Contact, |a, b| {
            a.contact == b.contact && a.contact_text == b.contact_text
        }),
        (Field::Account, |a, b| {
            a.account == b.account && a.account_text == b.account_text
        }),
        (Field::Memo, |a, b| a.memo == b.memo),
        (Field::Tag, |a, b| a.tag == b.tag),
        (Field::PrintLater, |a, b| a.print_later == b.print_later),
    ];
    for (f, same) in cases {
        // From a picked row (1) and from a typed one (2).
        for from in [1, 2] {
            let mut s = checks();
            let money = protected(&s);
            let blank = blanks(&s);
            let before = s.rows.len();
            let changed = ops::copy_down(&mut s, RowKey(from), f, CopyScope::AllDisplayed);
            assert!(changed >= 1, "{f:?} from {from}");
            let src = row(&s, from).clone();
            for k in [1, 2, 3, 4] {
                assert!(same(row(&s, k), &src), "{f:?} from {from} onto {k}");
            }
            assert_eq!(s.rows.len(), before, "no rows added");
            assert_eq!(blanks(&s), blank, "blank rows stay blank");
            assert_eq!(protected(&s), money, "money and numbers untouched");
        }
    }
}

#[test]
fn copy_down_copies_the_name_and_account_ids_with_their_text() {
    let mut s = checks();
    ops::copy_down(&mut s, RowKey(2), Field::Account, CopyScope::AllDisplayed);
    let r = row(&s, 4);
    assert_eq!(r.account, None, "a typed account stays typed");
    assert_eq!(r.account_text, "Typed account");
    ops::copy_down(&mut s, RowKey(1), Field::Contact, CopyScope::AllDisplayed);
    let r = row(&s, 2);
    assert_eq!(r.contact, Some(contact(1)));
    assert_eq!(r.contact_text, "Payee 1");
}

#[test]
fn copy_down_below_limits_it_to_later_rows() {
    let mut s = checks();
    let changed = ops::copy_down(&mut s, RowKey(2), Field::Memo, CopyScope::Below);
    assert_eq!(changed, 2, "rows 3 and 4");
    assert_eq!(row(&s, 1).memo, "memo 1", "an earlier row keeps its memo");
    assert_eq!(row(&s, 3).memo, "memo 2");
    assert_eq!(row(&s, 4).memo, "memo 2");
    // From the last filled row nothing is below.
    assert!(!ops::can_copy_down(
        &s,
        RowKey(4),
        Field::Memo,
        CopyScope::Below
    ));
    assert_eq!(
        ops::copy_down(&mut s, RowKey(4), Field::Memo, CopyScope::Below),
        0
    );
    assert!(ops::can_copy_down(
        &s,
        RowKey(4),
        Field::Memo,
        CopyScope::AllDisplayed
    ));
}

#[test]
fn copy_down_refuses_money_numbers_and_unknown_rows() {
    for f in [Field::Amount, Field::Number] {
        let mut s = checks();
        let before = s.clone();
        assert!(!ops::can_copy_down(
            &s,
            RowKey(1),
            f,
            CopyScope::AllDisplayed
        ));
        assert_eq!(
            ops::copy_down(&mut s, RowKey(1), f, CopyScope::AllDisplayed),
            0
        );
        assert_eq!(s, before, "{f:?}");
    }
    let mut je = Section {
        kind: SectionKind::JournalEntries,
        context: None,
        rows: vec![
            RowDraft {
                key: RowKey(1),
                date: Some(day(1)),
                number: "J1".into(),
                account: Some(acct(RENT)),
                debit: Some(m("5.00")),
                ..RowDraft::default()
            },
            RowDraft {
                key: RowKey(2),
                date: Some(day(1)),
                number: "J1".into(),
                account: Some(acct(SUPPLIES)),
                credit: Some(m("5.00")),
                ..RowDraft::default()
            },
        ],
    };
    let before = je.clone();
    for f in [
        Field::Debit,
        Field::Credit,
        Field::Number,
        Field::PrintLater,
    ] {
        assert_eq!(
            ops::copy_down(&mut je, RowKey(1), f, CopyScope::AllDisplayed),
            0
        );
    }
    assert_eq!(je, before);
    // A row that is not in the section, and a source without a date.
    let mut s = checks();
    assert_eq!(
        ops::copy_down(&mut s, RowKey(99), Field::Memo, CopyScope::AllDisplayed),
        0
    );
    s.rows[0].date = None;
    assert!(!ops::can_copy_down(
        &s,
        RowKey(1),
        Field::Date,
        CopyScope::AllDisplayed
    ));
    assert_eq!(
        ops::copy_down(&mut s, RowKey(1), Field::Date, CopyScope::AllDisplayed),
        0
    );
}

#[test]
fn copy_down_in_a_section_without_other_rows_is_off() {
    let s = Section {
        kind: SectionKind::Expenses,
        context: Some(acct(BANK)),
        rows: vec![RowDraft::new(RowKey(1))],
    };
    for f in SectionKind::Expenses.columns() {
        for scope in [CopyScope::AllDisplayed, CopyScope::Below] {
            assert!(!ops::can_copy_down(&s, RowKey(1), *f, scope));
        }
    }
}

#[test]
fn print_later_copied_onto_a_numbered_check_flags_the_number() {
    let mut s = checks();
    ops::copy_down(
        &mut s,
        RowKey(2),
        Field::PrintLater,
        CopyScope::AllDisplayed,
    );
    assert_eq!(row(&s, 1).number, "1001", "numbers are never cleared");
    let problems = s.local_problems(None);
    for k in [1, 3, 4] {
        assert!(
            problems.contains(&(RowKey(k), Problem::PrintLaterNumber)),
            "row {k}"
        );
    }
}

// ------------------------------------------------------------- bulk set

#[test]
fn set_field_for_each_settable_field() {
    let picked: BTreeSet<RowKey> = [RowKey(1), RowKey(3), RowKey(5)].into();
    let values = [
        FieldValue::Date(day(20)),
        FieldValue::Name {
            id: Some(contact(9)),
            text: "Ozark Supply".into(),
        },
        FieldValue::Account {
            id: None,
            text: "Typed".into(),
        },
        FieldValue::Memo("bulk".into()),
        FieldValue::Tag(Some(tag(7))),
        FieldValue::Tag(None),
        FieldValue::PrintLater(true),
    ];
    for v in values {
        let mut s = checks();
        let money = protected(&s);
        let changed = ops::set_field(&mut s, &picked, &v);
        assert_eq!(changed, 2, "{v:?}: rows 1 and 3; blank row 5 is skipped");
        for k in [1, 3] {
            assert_eq!(
                FieldValue::of(row(&s, k), v.field()),
                Some(v.clone()),
                "{v:?}"
            );
        }
        assert_ne!(
            FieldValue::of(row(&s, 4), v.field()),
            Some(v.clone()),
            "an unselected row keeps its value"
        );
        assert!(row(&s, 5).is_blank());
        assert_eq!(protected(&s), money);
        // Setting the same value again changes nothing.
        assert_eq!(ops::set_field(&mut s, &picked, &v), 0);
    }
}

#[test]
fn set_field_refuses_a_column_the_section_does_not_have() {
    let mut s = checks();
    s.kind = SectionKind::Deposits;
    let before = s.clone();
    let all: BTreeSet<RowKey> = s.rows.iter().map(|r| r.key).collect();
    assert_eq!(
        ops::set_field(&mut s, &all, &FieldValue::PrintLater(true)),
        0
    );
    assert_eq!(s, before);
}

#[test]
fn delete_rows_drops_only_the_selected_rows() {
    let mut s = checks();
    let picked: BTreeSet<RowKey> = [RowKey(2), RowKey(4), RowKey(77)].into();
    assert_eq!(ops::delete_rows(&mut s, &picked), 2);
    let keys: Vec<u64> = s.rows.iter().map(|r| r.key.0).collect();
    assert_eq!(keys, [1, 5, 3, 6]);
    assert_eq!(ops::delete_rows(&mut s, &BTreeSet::new()), 0);
}

#[test]
fn field_value_debug_prints_no_values() {
    let v = FieldValue::Name {
        id: None,
        text: "Ozark 842".into(),
    };
    let d = format!("{v:?} {:?}", FieldValue::Memo("rent 2026".into()));
    assert!(
        !d.contains("Ozark") && !d.contains("rent") && !d.contains("842"),
        "{d}"
    );
}

// -------------------------------------------- undo and engine re-checks

fn ok_all(batch: &BulkBatch, checks: &mut EngineChecks) {
    let out = batch.outgoing(Some(&batch.rows_to_check(checks)), None);
    checks.begin(&out);
    let verdicts: Vec<_> = out
        .sent
        .keys()
        .map(|n| {
            (
                *n,
                EngineVerdict::Ok {
                    new_contact: false,
                    account: None,
                },
            )
        })
        .collect();
    checks.apply(batch, &out, verdicts);
}

#[test]
fn each_action_is_one_undo_step_and_changed_rows_are_rechecked() {
    type Action = fn(&mut BulkBatch);
    let actions: [(Action, &[u64]); 4] = [
        (
            |b| {
                ops::copy_down(
                    &mut b.sections[0],
                    RowKey(1),
                    Field::Memo,
                    CopyScope::AllDisplayed,
                );
            },
            &[2, 3, 4],
        ),
        (
            |b| {
                ops::copy_down(
                    &mut b.sections[0],
                    RowKey(2),
                    Field::Account,
                    CopyScope::Below,
                );
            },
            &[3, 4],
        ),
        (
            |b| {
                let rows = [RowKey(1), RowKey(4)].into();
                ops::set_field(&mut b.sections[0], &rows, &FieldValue::Date(day(28)));
            },
            &[1, 4],
        ),
        (
            |b| {
                let rows = [RowKey(3)].into();
                ops::delete_rows(&mut b.sections[0], &rows);
            },
            &[],
        ),
    ];
    for (act, changed) in actions {
        let original = BulkBatch::new(checks());
        let mut form = FormState::new(original.clone());
        let mut engine = EngineChecks::new();
        ok_all(form.current(), &mut engine);
        assert!(form.current().rows_to_check(&engine).is_empty());
        assert!(form.edit_step(act));
        let b = form.current();
        let recheck: Vec<u64> = b.rows_to_check(&engine).iter().map(|k| k.0).collect();
        assert_eq!(recheck, changed, "only the changed rows are re-checked");
        for k in [1u64, 2, 3, 4] {
            if b.find(RowKey(k)).is_none() {
                continue;
            }
            let fresh = matches!(engine.state(b, RowKey(k)), EngineState::Unchecked);
            assert_eq!(fresh, changed.contains(&k), "row {k}");
        }
        // One undo step brings the whole batch back.
        assert!(form.undo());
        assert_eq!(form.current(), &original);
        assert!(!form.can_undo(), "the action was a single step");
        assert!(form.redo());
        assert_ne!(form.current(), &original);
    }
}

// ------------------------------------------------------- column choice

#[test]
fn required_columns_are_columns_of_their_kind() {
    for kind in SectionKind::ALL {
        for f in required_fields(kind) {
            assert!(kind.columns().contains(f), "{kind:?} {f:?}");
        }
        assert!(required_fields(kind).contains(&Field::Date));
        assert!(required_fields(kind).contains(&Field::Account));
    }
}

#[test]
fn hiding_a_required_column_is_refused() {
    for kind in SectionKind::ALL {
        let mut c = ColumnChoice::default_for(kind);
        for f in kind.columns() {
            let r = c.set_hidden(kind, *f, true);
            if is_required(kind, *f) {
                assert_eq!(r, Err(ColumnError::Required(*f)));
                assert!(!c.is_hidden(*f));
            } else {
                assert_eq!(r, Ok(()));
                assert!(c.is_hidden(*f));
            }
        }
        let shown: BTreeSet<Field> = c.visible().into_iter().collect();
        let needed: BTreeSet<Field> = required_fields(kind).iter().copied().collect();
        assert_eq!(shown, needed, "{kind:?}: only required columns stay");
        assert_eq!(c.all(), kind.columns(), "hiding keeps the order");
    }
    let mut c = ColumnChoice::default_for(SectionKind::Deposits);
    assert_eq!(
        c.set_hidden(SectionKind::Deposits, Field::PrintLater, true),
        Err(ColumnError::NotInSection(Field::PrintLater))
    );
    assert_eq!(
        c.set_hidden(SectionKind::Deposits, Field::Memo, true),
        Ok(())
    );
    assert_eq!(
        c.set_hidden(SectionKind::Deposits, Field::Memo, false),
        Ok(())
    );
    assert_eq!(c, ColumnChoice::default_for(SectionKind::Deposits));
}

#[test]
fn columns_move_and_stop_at_the_ends() {
    let kind = SectionKind::Expenses;
    let mut c = ColumnChoice::default_for(kind);
    assert!(c.move_by(Field::Memo, -2));
    assert_eq!(
        c.all(),
        [
            Field::Date,
            Field::Number,
            Field::Contact,
            Field::Memo,
            Field::Account,
            Field::Amount,
            Field::Tag,
        ]
    );
    assert!(!c.move_by(Field::Date, -1), "already first");
    assert!(!c.move_by(Field::Tag, 1), "already last");
    assert!(c.move_by(Field::Date, 100), "clamped to the end");
    assert_eq!(c.all().last(), Some(&Field::Date));
    assert!(!c.move_by(Field::PrintLater, 1), "not a column here");
    assert!(c.move_by(Field::Date, i8::MIN));
    assert_eq!(c.all().first(), Some(&Field::Date));
    c.set_hidden(kind, Field::Tag, true).expect("hide");
    assert!(!c.visible().contains(&Field::Tag));
}

#[test]
fn normalize_fixes_old_prefs() {
    let kind = SectionKind::Checks;
    // Old prefs: a column the kind lacks, a repeat, a missing column, and a
    // hidden required column.
    let json =
        r#"{"order":["memo","debit","date","memo","amount"],"hidden":["amount","tag","credit"]}"#;
    let c: ColumnChoice = serde_json::from_str(json).expect("json");
    let c = c.normalized(kind);
    assert_eq!(
        c.all(),
        [
            Field::Memo,
            Field::Date,
            Field::Amount,
            Field::Number,
            Field::PrintLater,
            Field::Contact,
            Field::Account,
            Field::Tag,
        ]
    );
    assert!(
        !c.is_hidden(Field::Amount),
        "required columns are shown again"
    );
    assert!(
        c.is_hidden(Field::Tag),
        "an optional hidden column stays hidden"
    );
    assert!(!c.is_hidden(Field::Credit));
    // A normalized choice is stable and round-trips.
    let again = c.clone().normalized(kind);
    assert_eq!(again, c);
    let back: ColumnChoice =
        serde_json::from_str(&serde_json::to_string(&c).expect("ser")).expect("de");
    assert_eq!(back, c);
    // `hidden` may be missing in very old prefs.
    let c: ColumnChoice = serde_json::from_str(r#"{"order":[]}"#).expect("json");
    assert_eq!(c.normalized(kind), ColumnChoice::default_for(kind));
}

// ------------------------------------------- required-columns property

fn number_text() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("J1".to_string()),
        Just("1001".to_string()),
        Just("0".to_string()),
        Just("12a".to_string()),
        Just("x".repeat(60)),
    ]
}

fn amount() -> impl Strategy<Value = Option<Money>> {
    prop_oneof![
        Just(None),
        Just(Some(Money::from_cents(1_250).expect("m"))),
        Just(Some(Money::from_cents(0).expect("m"))),
        Just(Some(Money::from_cents(-500).expect("m"))),
        Just(Some(Money::from_ten_thousandths(12_345).expect("m"))),
    ]
}

prop_compose! {
    fn any_row(key: u64)(
        date in prop_oneof![Just(None), (1u32..28).prop_map(|d| Some(day(d)))],
        number in number_text(),
        print_later in any::<bool>(),
        contact_pick in any::<bool>(),
        contact_text in prop_oneof![Just(String::new()), Just("  ".to_string()), Just("Ozark".to_string())],
        account in prop_oneof![Just(None), Just(Some(acct(RENT))), Just(Some(acct(BANK)))],
        account_text in prop_oneof![Just(String::new()), Just("Rent".to_string())],
        amount in amount(),
        debit in amount(),
        credit in amount(),
        memo in prop_oneof![Just(String::new()), Just("m".to_string()), Just("z".repeat(2_001))],
        tagged in any::<bool>(),
    ) -> RowDraft {
        RowDraft {
            key: RowKey(key),
            date,
            number,
            print_later,
            contact: contact_pick.then(|| contact(1)),
            contact_text,
            account,
            account_text,
            amount,
            debit,
            credit,
            memo,
            tag: tagged.then(|| tag(1)),
        }
    }
}

fn any_section() -> impl Strategy<Value = Section> {
    (
        proptest::sample::select(SectionKind::ALL.to_vec()),
        any::<bool>(),
        1u64..8,
    )
        .prop_flat_map(|(kind, context, n)| {
            let rows: Vec<_> = (1..=n).map(any_row).collect();
            (Just(kind), Just(context), rows)
        })
        .prop_map(|(kind, context, rows)| Section {
            kind,
            context: (context && kind.needs_context().is_some()).then(|| acct(BANK)),
            rows,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// D8: every cell a local problem says is missing is a column the kind
    /// cannot hide, so the owner can always see and fill it.
    #[test]
    fn every_missing_field_is_a_required_column(
        s in any_section(),
        closing in prop_oneof![Just(None), Just(Some(day(10)))],
    ) {
        let needed = required_fields(s.kind);
        let mut all = s.local_problems(closing);
        all.extend(BulkBatch::new(s.clone()).local_problems(closing));
        for (_, p) in all {
            if let Problem::Missing(f) = p {
                prop_assert!(needed.contains(&f), "{:?} reports Missing({:?})", s.kind, f);
            }
        }
    }

    /// Copy down and Set never change an amount or a number, never add
    /// rows and never fill a blank row.
    #[test]
    fn copy_and_set_never_touch_money_or_blank_rows(
        s in any_section(),
        from in 1u64..8,
        field in proptest::sample::select(vec![
            Field::Date, Field::Number, Field::PrintLater, Field::Contact,
            Field::Account, Field::Amount, Field::Debit, Field::Credit,
            Field::Memo, Field::Tag,
        ]),
        below in any::<bool>(),
    ) {
        let mut s = s;
        let money = protected(&s);
        let blank: BTreeSet<RowKey> = blanks(&s).into_iter().collect();
        let n = s.rows.len();
        let scope = if below { CopyScope::Below } else { CopyScope::AllDisplayed };
        ops::copy_down(&mut s, RowKey(from), field, scope);
        if let Some(v) = s.rows.first().and_then(|r| FieldValue::of(r, field)) {
            let all: BTreeSet<RowKey> = s.rows.iter().map(|r| r.key).collect();
            ops::set_field(&mut s, &all, &v);
        }
        prop_assert_eq!(protected(&s), money);
        // A blank row is never filled in (copying an empty memo may empty
        // a row; that is the owner's choice).
        let after: BTreeSet<RowKey> = blanks(&s).into_iter().collect();
        prop_assert!(blank.is_subset(&after));
        prop_assert_eq!(s.rows.len(), n);
    }
}
