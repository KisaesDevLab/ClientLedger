use cl_api::{AccountRoles, Basis, ContactType, JeLine, JeOptions};

use super::*;
use crate::company::demo::account_id;
use crate::company::demo::register::DEMO_CLOSING_PASSWORD;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn day(mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, mo, d).expect("date")
}

fn line(n: u32, side: Side, contact: Option<ContactId>) -> JeLine {
    JeLine {
        account: account_id(n),
        side,
        description: None,
        tag: None,
        contact,
    }
}

fn contact_of(state: &mut DemoState, kind: ContactType) -> ContactId {
    state
        .contacts_mut()
        .iter()
        .find(|c| c.fields.contact_type == kind)
        .map(|c| c.id)
        .expect("a demo contact of that kind")
}

/// An entry built without the chart's roles, so the twin's own A/R-A/P
/// refusal is what answers.
fn entry(
    date: CalendarDate,
    lines: Vec<JeLine>,
    number: Option<&str>,
    reverse_on: Option<CalendarDate>,
) -> NewJournalEntry {
    NewJournalEntry::new_with(
        date,
        Some("accrual".into()),
        Basis::Both,
        lines,
        JeOptions {
            number: number.map(str::to_string),
            adjusting: true,
            reverse_on,
        },
        &AccountRoles::default(),
    )
    .expect("a balanced entry")
}

#[test]
fn an_entry_saves_with_its_names_number_and_adjusting_flag() {
    let mut s = DemoState::new();
    let vendor = contact_of(&mut s, ContactType::Vendor);
    let first = next_number(&s);
    let e = entry(
        day(11, 2),
        vec![
            line(6100, Side::Debit(m("1200.00")), None),
            line(2000, Side::Credit(m("1200.00")), Some(vendor)),
        ],
        Some(&first),
        None,
    );
    let (id, version) = save(&mut s, &e, None, None).expect("saved");
    let v = get(&mut s, id).expect("read");
    assert_eq!(v.version, version);
    assert_eq!(v.record.txn_number.as_deref(), Some(first.as_str()));
    assert!(v.record.is_adjusting);
    assert_eq!(v.record.lines[1].contact_id, Some(vendor));
    assert!(v.record.lines[1].contact_name.is_some());
    assert_eq!(v.record.lines[0].debit, m("1200.00"));
    assert_eq!(v.record.lines[1].credit, m("1200.00"));
    // The read-back is the entry sent (the engine's version rule).
    assert_eq!(v.record.fingerprint(), e.fingerprint());
    let n: u64 = first.parse().expect("digits");
    assert_eq!(next_number(&s), (n + 1).to_string());
    assert_eq!(list(&s).first().map(|r| r.id), Some(id));
}

#[test]
fn the_ar_ap_rule_mirrors_the_contract() {
    let mut s = DemoState::new();
    let customer = contact_of(&mut s, ContactType::Customer);
    let unnamed = entry(
        day(11, 2),
        vec![
            line(1200, Side::Debit(m("50")), None),
            line(4000, Side::Credit(m("50")), None),
        ],
        None,
        None,
    );
    assert_eq!(
        save(&mut s, &unnamed, None, None),
        Err(ApiError::Local(LocalValidation::ArApNameMissing))
    );
    let two = entry(
        day(11, 2),
        vec![
            line(1200, Side::Debit(m("50")), Some(customer)),
            line(2000, Side::Credit(m("50")), Some(customer)),
        ],
        None,
        None,
    );
    assert_eq!(
        save(&mut s, &two, None, None),
        Err(ApiError::Local(LocalValidation::ArApTwoAccounts))
    );
    assert!(
        list(&s).iter().all(|r| r.date != day(11, 2)),
        "nothing posted"
    );
    let named = entry(
        day(11, 2),
        vec![
            line(1200, Side::Debit(m("50")), Some(customer)),
            line(4000, Side::Credit(m("50")), None),
        ],
        None,
        None,
    );
    assert!(save(&mut s, &named, None, None).is_ok());
}

#[test]
fn reverse_on_posts_a_linked_reversal_with_the_lines_swapped() {
    let mut s = DemoState::new();
    let e = entry(
        day(10, 31),
        vec![
            line(6100, Side::Debit(m("1234.56")), None),
            line(1010, Side::Credit(m("1234.56")), None),
        ],
        Some("77"),
        Some(day(11, 1)),
    );
    let (id, _) = save(&mut s, &e, None, None).expect("saved");
    let v = get(&mut s, id).expect("read");
    assert_eq!(v.record.reverse_on, Some(day(11, 1)));
    let r = v.record.reversal_transaction_id.expect("linked");
    let rv = get(&mut s, r).expect("reversal");
    assert_eq!(rv.record.txn_date, day(11, 1));
    assert_eq!(rv.record.reverses_transaction_id, Some(id));
    assert_eq!(rv.record.txn_number, None);
    assert_eq!(rv.record.memo.as_deref(), Some("Reverses JE 77"));
    assert_eq!(rv.record.lines[0].credit, m("1234.56"));
    assert_eq!(rv.record.lines[1].debit, m("1234.56"));
    assert!(rv.record.is_adjusting, "the original's flag");
    // An edit replaces the reversal; without Reverse on there is none.
    let changed = entry(
        day(10, 31),
        vec![
            line(6100, Side::Debit(m("10.00")), None),
            line(1010, Side::Credit(m("10.00")), None),
        ],
        Some("77"),
        Some(day(11, 2)),
    );
    let (_, v2) = save(&mut s, &changed, Some((id, v.version)), None).expect("edit");
    let after = get(&mut s, id).expect("read");
    assert_eq!(after.version, v2);
    let r2 = after
        .record
        .reversal_transaction_id
        .expect("a new reversal");
    assert_ne!(r2, r);
    assert!(get(&mut s, r).is_err(), "the old reversal is gone");
    let plain = entry(day(10, 31), changed.lines().to_vec(), Some("77"), None);
    save(&mut s, &plain, Some((id, after.version)), None).expect("edit");
    assert_eq!(
        get(&mut s, id)
            .expect("read")
            .record
            .reversal_transaction_id,
        None
    );
    assert!(get(&mut s, r2).is_err());
}

#[test]
fn an_edit_without_reverse_on_keeps_the_reversal_and_null_removes_it() {
    let mut s = DemoState::new();
    let lines = vec![
        line(6100, Side::Debit(m("40.00")), None),
        line(1010, Side::Credit(m("40.00")), None),
    ];
    let e = entry(day(10, 31), lines.clone(), Some("78"), Some(day(11, 3)));
    let (id, v1) = save(&mut s, &e, None, None).expect("saved");
    // Omitted: the stored date is kept, re-posted from the edited lines
    // (the entry's own Option is not on the wire).
    let edited = entry(
        day(10, 30),
        vec![
            line(6100, Side::Debit(m("41.00")), None),
            line(1010, Side::Credit(m("41.00")), None),
        ],
        Some("78"),
        None,
    );
    let (_, v2) = save_with(
        &mut s,
        &edited,
        Some((id, v1)),
        None,
        ReverseOnField::Omitted,
    )
    .expect("keep");
    let kept = get(&mut s, id).expect("read");
    assert_eq!(kept.version, v2);
    assert_eq!(kept.record.reverse_on, Some(day(11, 3)));
    let r = kept.record.reversal_transaction_id.expect("still reversed");
    assert_eq!(
        get(&mut s, r).expect("reversal").record.lines[0].credit,
        m("41.00")
    );
    // Omitted, but the kept date is no longer after the new entry date.
    let late = entry(day(11, 3), lines.clone(), Some("78"), None);
    assert_eq!(
        save_with(
            &mut s,
            &late,
            Some((id, v2.clone())),
            None,
            ReverseOnField::Omitted
        ),
        Err(ApiError::Local(LocalValidation::ReverseNotAfter))
    );
    // Sent null: removed.
    save_with(
        &mut s,
        &edited,
        Some((id, v2)),
        None,
        ReverseOnField::Sent(None),
    )
    .expect("remove");
    assert_eq!(get(&mut s, id).expect("read").record.reverse_on, None);
    assert!(get(&mut s, r).is_err(), "the reversal is gone");
}

#[test]
fn void_and_delete_take_the_reversal_with_them() {
    let mut s = DemoState::new();
    let e = entry(
        day(10, 31),
        vec![
            line(6100, Side::Debit(m("5")), None),
            line(1010, Side::Credit(m("5")), None),
        ],
        None,
        Some(day(11, 1)),
    );
    let (id, _) = save(&mut s, &e, None, None).expect("saved");
    let r = get(&mut s, id)
        .expect("read")
        .record
        .reversal_transaction_id
        .expect("reversal");
    void(&mut s, id, None).expect("void");
    assert!(is_void(&get(&mut s, id).expect("read")));
    assert!(is_void(&get(&mut s, r).expect("read")));
    let (id2, _) = save(&mut s, &e, None, None).expect("saved");
    let r2 = get(&mut s, id2)
        .expect("read")
        .record
        .reversal_transaction_id
        .expect("reversal");
    delete(&mut s, id2, None).expect("delete");
    assert!(get(&mut s, id2).is_err() && get(&mut s, r2).is_err());
}

#[test]
fn closed_dates_need_the_password_and_a_stale_edit_is_refused() {
    let mut s = DemoState::new();
    let lines = vec![
        line(6100, Side::Debit(m("5")), None),
        line(1010, Side::Credit(m("5")), None),
    ];
    let closed = entry(day(9, 15), lines.clone(), None, None);
    assert!(matches!(
        save(&mut s, &closed, None, None),
        Err(ApiError::ClosedPeriod(_))
    ));
    assert_eq!(
        save(&mut s, &closed, None, Some(&Secret::new("wrong"))),
        Err(ApiError::ClosingPasswordRejected)
    );
    let pw = Secret::new(DEMO_CLOSING_PASSWORD);
    let (id, v) = save(&mut s, &closed, None, Some(&pw)).expect("with the password");
    assert!(matches!(
        void(&mut s, id, None),
        Err(ApiError::ClosedPeriod(_))
    ));
    // An open entry reversing into... an open date is fine; one dated in the
    // closed period by its reversal is not (both dates are checked).
    let open = entry(day(10, 15), lines.clone(), None, Some(day(10, 16)));
    assert!(save(&mut s, &open, None, None).is_ok());
    // Moving the closed entry out of the period still needs the password.
    let moved = entry(day(10, 15), lines, None, None);
    assert!(matches!(
        save(&mut s, &moved, Some((id, v)), None),
        Err(ApiError::ClosedPeriod(_))
    ));
    assert!(matches!(
        save(
            &mut s,
            &moved,
            Some((id, RecordVersion::Fingerprint([0; 32]))),
            Some(&pw)
        ),
        Err(ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        })
    ));
    save(&mut s, &moved, Some((id, v)), Some(&pw)).expect("moved");
    delete(&mut s, id, None).expect("an open date deletes freely");
}
