//! Bulk Entry batch state: outgoing bodies, engine results by hash, the
//! Save all gate, totals, and the exactness and gate-soundness properties.

#![allow(
    clippy::assert_is_empty,
    clippy::many_single_char_names,
    reason = "short test bindings and plain emptiness checks"
)]

use proptest::prelude::*;

use super::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn money(c: i64) -> Money {
    Money::from_cents(c).expect("cents")
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

fn row(key: u64, date: u32, account: u8, amount: Money) -> RowDraft {
    RowDraft {
        key: RowKey(key),
        date: Some(day(date)),
        account: Some(acct(account)),
        amount: Some(amount),
        ..RowDraft::default()
    }
}

fn je_row(
    key: u64,
    date: u32,
    account: u8,
    debit: Option<Money>,
    credit: Option<Money>,
) -> RowDraft {
    RowDraft {
        key: RowKey(key),
        date: Some(day(date)),
        number: "J1".into(),
        account: Some(acct(account)),
        debit,
        credit,
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

/// Sends every unchecked row and answers "ok" for all of them.
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

fn mixed() -> BulkBatch {
    BulkBatch {
        sections: vec![
            section(
                SectionKind::Checks,
                Some(BANK),
                vec![
                    row(1, 2, RENT, m("100.00")),
                    RowDraft::new(RowKey(2)),
                    row(3, 3, SUPPLIES, m("12.35")),
                ],
            ),
            section(
                SectionKind::Deposits,
                Some(BANK),
                vec![row(4, 4, RENT, m("50.00"))],
            ),
            section(
                SectionKind::JournalEntries,
                None,
                vec![
                    je_row(5, 5, RENT, Some(m("10.00")), None),
                    je_row(6, 5, SUPPLIES, None, Some(m("10.00"))),
                ],
            ),
        ],
        add_new_names: false,
    }
}

fn engine_problem() -> EngineProblem {
    EngineProblem {
        field: Some(Field::Account),
        kind: EngineProblemKind::NotInLists,
        message: "Not in your lists".into(),
    }
}

#[test]
fn touched_accounts_use_the_engines_match_for_typed_accounts() {
    // A journal entry whose lines are typed by name, and an expense with
    // a picked account.
    let typed = |key, debit, credit| RowDraft {
        account: None,
        account_text: "Supplies".into(),
        ..je_row(key, 5, RENT, debit, credit)
    };
    let mut b = BulkBatch::new(section(
        SectionKind::Expenses,
        Some(CARD),
        vec![row(1, 2, RENT, m("5.00"))],
    ));
    b.sections.push(section(
        SectionKind::JournalEntries,
        None,
        vec![
            typed(2, Some(m("10.00")), None),
            je_row(3, 5, BANK, None, Some(m("10.00"))),
        ],
    ));
    let mut checks = EngineChecks::new();
    let out = b.outgoing(None, None);
    assert_eq!(
        out.touched_accounts(|k| checks.resolved_account(&b, k)),
        None,
        "a typed account not matched yet: any account may have changed"
    );
    checks.begin(&out);
    let verdicts: Vec<_> = out
        .sent
        .iter()
        .map(|(n, s)| {
            let account = (s.key == RowKey(2)).then(|| acct(SUPPLIES));
            (
                *n,
                EngineVerdict::Ok {
                    new_contact: false,
                    account,
                },
            )
        })
        .collect();
    checks.apply(&b, &out, verdicts);
    assert_eq!(checks.resolved_account(&b, RowKey(2)), Some(acct(SUPPLIES)));
    assert_eq!(
        out.touched_accounts(|k| checks.resolved_account(&b, k)),
        Some(
            [acct(BANK), acct(CARD), acct(RENT), acct(SUPPLIES)]
                .into_iter()
                .collect()
        )
    );
    // A change to the entry makes the match unknown again.
    b.sections[1].rows[0].memo = "changed".into();
    assert_eq!(checks.resolved_account(&b, RowKey(2)), None);
}

#[test]
fn outgoing_numbers_rows_across_the_call_and_skips_blanks() {
    let b = mixed();
    let out = b.outgoing(None, Some(day(2)));
    assert_eq!(out.groups.len(), 3);
    assert_eq!(out.groups[0].rows.len(), 2, "the blank row is not sent");
    let numbers: Vec<u32> = out
        .groups
        .iter()
        .flat_map(|g| g.rows.iter().map(|r| r.row_number))
        .collect();
    assert_eq!(numbers, [1, 2, 3, 4, 5]);
    assert_eq!(out.key_of(3), Some(RowKey(4)));
    assert_eq!(out.sent[&3].group, 1);
    assert_eq!(out.sent[&3].section, 1);
    assert_eq!(out.section_of_group(2), Some(2));
    assert_eq!(out.section_of_group(9), None);
    // A section with nothing sent has no group: groups shift past it.
    let mut gap = mixed();
    gap.sections[1].rows.clear();
    let o = gap.outgoing(None, None);
    assert_eq!(o.section_of_group(1), Some(2));
    assert!(out.groups[0].rows[0].locked);
    assert!(!out.groups[0].rows[1].locked);
    let je = &out.groups[2].rows[0];
    assert_eq!(
        (je.amount, je.debit, je.number.as_deref()),
        (None, Some(m("10.00")), Some("J1"))
    );
    assert_eq!(out.groups[1].rows[0].amount, Some(m("50.00")));
    assert_eq!(
        out.touched_accounts(|_| None),
        Some(
            [acct(BANK), acct(RENT), acct(SUPPLIES)]
                .into_iter()
                .collect()
        )
    );
    assert!(!out.is_empty());
    assert!(BulkBatch::default().outgoing(None, None).is_empty());

    // A picked id wins over the typed text; typed text goes as a name.
    let typed = RowDraft {
        account: None,
        account_text: " Rent ".into(),
        contact: None,
        contact_text: "Acme".into(),
        memo: "  ".into(),
        ..row(9, 2, RENT, m("1.00"))
    };
    let b = BulkBatch::new(section(SectionKind::Expenses, Some(BANK), vec![typed]));
    let r = &b.outgoing(None, None).groups[0].rows[0];
    assert_eq!(r.account_name.as_deref(), Some("Rent"));
    assert_eq!(r.contact_name.as_deref(), Some("Acme"));
    assert_eq!(r.memo, None);
    assert!(!format!("{r:?}").contains("Acme"));
    assert!(!format!("{:?}", b.outgoing(None, None).groups[0]).contains("Acme"));

    // Only the asked rows; an undated row is never sent.
    let mut b = mixed();
    b.sections[0].rows[2].date = None;
    let only = [RowKey(3), RowKey(4)].into_iter().collect();
    let out = b.outgoing(Some(&only), None);
    assert_eq!(out.sent.len(), 1);
    assert_eq!(out.key_of(1), Some(RowKey(4)));
    assert_eq!(out.sent[&1].group, 0);
    assert!(!b.all_sendable().contains(&RowKey(3)));
}

#[test]
fn gate_walks_the_blockers_in_order() {
    let mut b = mixed();
    let mut checks = EngineChecks::new();
    let caps = SaveCaps::ALL;
    assert_eq!(
        b.can_save(&checks, SaveCaps::default(), None),
        Err(Blocker::EngineTooOld)
    );
    assert_eq!(
        BulkBatch::default().can_save(&checks, caps, None),
        Err(Blocker::Empty)
    );
    assert_eq!(
        b.can_save(
            &checks,
            SaveCaps {
                check: false,
                ..caps
            },
            None
        ),
        Err(Blocker::KindUnsupported(SectionKind::Checks))
    );
    // The save always sends groups: without `batch-mixed-v1` it is off,
    // even for one section (as `cl_api::Batch::save` refuses).
    let no_mixed = SaveCaps {
        mixed: false,
        ..caps
    };
    assert!(!no_mixed.can_post());
    assert_eq!(
        b.can_save(&checks, no_mixed, None),
        Err(Blocker::EngineTooOld)
    );
    let one = BulkBatch::new(b.sections[0].clone());
    assert_eq!(
        one.can_save(&checks, no_mixed, None),
        Err(Blocker::EngineTooOld)
    );
    for missing in [
        SaveCaps {
            atomic: false,
            ..caps
        },
        SaveCaps {
            exact: false,
            ..caps
        },
        SaveCaps {
            idempotent: false,
            ..caps
        },
    ] {
        assert!(!missing.can_post());
    }
    assert!(caps.can_post());
    assert_eq!(b.can_save(&checks, caps, None), Err(Blocker::Unchecked(5)));
    ok_all(&b, &mut checks);
    assert_eq!(b.can_save(&checks, caps, None), Ok(()));

    // Closed rows: fine with the password capability, blocked without.
    assert_eq!(b.can_save(&checks, caps, Some(day(2))), Ok(()));
    assert_eq!(
        b.can_save(
            &checks,
            SaveCaps {
                closing: false,
                ..caps
            },
            Some(day(2))
        ),
        Err(Blocker::ClosedUnsupported(1))
    );

    // An edit makes the row unchecked again; a local problem comes first.
    b.sections[1].rows[0].amount = Some(m("-1.00"));
    assert_eq!(
        b.can_save(&checks, caps, None),
        Err(Blocker::LocalProblems(1))
    );
    b.sections[1].rows[0].amount = Some(m("51.00"));
    assert_eq!(b.can_save(&checks, caps, None), Err(Blocker::Unchecked(1)));
    b.sections[1].rows[0].amount = Some(m("50.00"));
    assert_eq!(
        b.can_save(&checks, caps, None),
        Ok(()),
        "back to the checked content"
    );
    // Changing one JE row re-checks its whole entry.
    b.sections[2].rows[0].memo = "note".into();
    assert_eq!(b.can_save(&checks, caps, None), Err(Blocker::Unchecked(2)));
    assert_eq!(
        b.rows_to_check(&checks),
        [RowKey(5), RowKey(6)].into_iter().collect()
    );

    // Missing context account.
    let mut c = mixed();
    c.sections[1].context = None;
    assert_eq!(
        c.can_save(&checks, caps, None),
        Err(Blocker::MissingAccount(1))
    );

    // Too many rows.
    let many: Vec<RowDraft> = (1..=MAX_ROWS as u64 + 1)
        .map(|k| row(k, 2, RENT, m("1.00")))
        .collect();
    let big = BulkBatch::new(section(SectionKind::Expenses, Some(BANK), many));
    assert_eq!(
        big.can_save(&checks, caps, None),
        Err(Blocker::TooManyRows(MAX_ROWS + 1))
    );
}

#[test]
fn engine_results_land_only_on_unchanged_rows() {
    let mut b = mixed();
    let mut checks = EngineChecks::new();
    let out = b.outgoing(None, None);
    checks.begin(&out);
    assert_eq!(
        checks.state(&b, RowKey(1)),
        &EngineState::Checking(out.sent[&1].hash)
    );
    // The owner edits row 1 while the check is in flight.
    b.sections[0].rows[0].memo = "changed".into();
    let landed = checks.apply(
        &b,
        &out,
        vec![
            (
                1,
                EngineVerdict::Ok {
                    new_contact: false,
                    account: None,
                },
            ),
            (2, EngineVerdict::Problems(vec![engine_problem()])),
            (
                3,
                EngineVerdict::Ok {
                    new_contact: true,
                    account: None,
                },
            ),
            (
                99,
                EngineVerdict::Ok {
                    new_contact: false,
                    account: None,
                },
            ),
        ],
    );
    assert_eq!(landed, 2);
    assert_eq!(checks.state(&b, RowKey(1)), &EngineState::Unchecked);
    assert!(matches!(
        checks.state(&b, RowKey(3)),
        EngineState::Problems(_, l) if l.len() == 1
    ));
    assert!(matches!(
        checks.state(&b, RowKey(4)),
        EngineState::Ok {
            new_contact: true,
            ..
        }
    ));
    assert_eq!(
        checks.state(&b, RowKey(5)),
        &EngineState::Unchecked,
        "no verdict: unchecked"
    );

    // New names block until "Add new names" is ticked.
    let caps = SaveCaps::ALL;
    let mut checks = EngineChecks::new();
    let b = mixed();
    let out = b.outgoing(None, None);
    checks.begin(&out);
    checks.apply(
        &b,
        &out,
        out.sent.keys().map(|n| {
            (
                *n,
                EngineVerdict::Ok {
                    new_contact: *n == 2,
                    account: None,
                },
            )
        }),
    );
    assert_eq!(b.can_save(&checks, caps, None), Err(Blocker::NotInLists(1)));
    let mut ticked = b.clone();
    ticked.add_new_names = true;
    assert_eq!(ticked.can_save(&checks, caps, None), Ok(()));

    // A refused save marks the named rows.
    checks.refuse(
        &b,
        &out,
        vec![(4, engine_problem()), (99, engine_problem())],
    );
    assert_eq!(
        b.can_save(&checks, caps, None),
        Err(Blocker::EngineProblems(1))
    );

    // A failed call puts in-flight rows back.
    let mut checks = EngineChecks::new();
    checks.begin(&out);
    checks.fail(&out);
    assert_eq!(checks, EngineChecks::new());

    // Pruning forgets removed rows.
    let mut checks = EngineChecks::new();
    ok_all(&b, &mut checks);
    let mut smaller = b.clone();
    smaller.sections.remove(2);
    checks.prune(&smaller);
    assert_eq!(checks.state(&b, RowKey(5)), &EngineState::Unchecked);
    checks.clear();
    assert_eq!(checks.state(&b, RowKey(1)), &EngineState::Unchecked);
}

#[test]
fn row_hash_depends_on_section_and_entry() {
    let mut b = mixed();
    let h1 = b.row_hash(RowKey(1)).expect("row");
    b.sections[0].context = Some(acct(CARD));
    assert_ne!(b.row_hash(RowKey(1)), Some(h1), "the account is part of it");
    let h5 = b.row_hash(RowKey(5)).expect("row");
    b.sections[2].rows[1].credit = Some(m("10.01"));
    assert_ne!(
        b.row_hash(RowKey(5)),
        Some(h5),
        "a sibling's change re-checks"
    );
    assert_eq!(b.row_hash(RowKey(77)), None);
}

#[test]
fn content_hash_ignores_blanks_and_empty_sections() {
    let a = mixed();
    let mut b = a.clone();
    b.sections[0].rows.push(RowDraft::new(RowKey(50)));
    b.sections
        .push(Section::new(SectionKind::Deposits, Some(acct(BANK))));
    assert_eq!(a.content_hash(), b.content_hash());
    b.add_new_names = true;
    assert_ne!(a.content_hash(), b.content_hash());
    let mut c = a.clone();
    c.sections[1].rows[0].amount = Some(m("50.01"));
    assert_ne!(a.content_hash(), c.content_hash());
}

#[test]
fn totals_and_counts() {
    let b = mixed();
    let t = b.totals().expect("totals");
    assert_eq!(t[&(acct(BANK), SectionKind::Checks)], m("112.35"));
    assert_eq!(t[&(acct(BANK), SectionKind::Deposits)], m("50.00"));
    assert_eq!(t.len(), 2, "journal entries have no context account");
    let je = b.section_totals(2).expect("je");
    assert_eq!(
        (je.rows, je.debits, je.credits),
        (2, m("10.00"), m("10.00"))
    );
    assert_eq!(b.section_totals(0).expect("checks").rows, 2);
    assert_eq!(b.section_totals(9).expect("none").rows, 0);
    let counts = b.count_by_kind();
    assert_eq!(counts[&SectionKind::Checks], 2);
    assert_eq!(counts[&SectionKind::JournalEntries], 2);
    assert!(!counts.contains_key(&SectionKind::CardCharges));
    let by_row = problems_by_row(b.local_problems(Some(day(5))));
    assert_eq!(by_row.len(), 5, "every row is in the closed period");
    assert!(
        b.problem_rows(Some(day(5))).is_empty(),
        "lock marks are not problems"
    );
}

#[test]
fn engine_problem_debug_hides_the_message() {
    let text = format!("{:?}", engine_problem());
    assert!(!text.contains("Not in your lists"));
}

// ------------------------------------------------------------ properties

/// Checks and deposits from `cents` (split in two), plus one journal entry
/// of `je` debits balanced by one credit.
fn batch_from(cents: &[i64], je: &[i64]) -> BulkBatch {
    let mut key = 0;
    let mut next = || {
        key += 1;
        key
    };
    let (a, b) = cents.split_at(cents.len() / 2);
    let checks: Vec<RowDraft> = a.iter().map(|c| row(next(), 2, RENT, money(*c))).collect();
    let deposits: Vec<RowDraft> = b.iter().map(|c| row(next(), 2, RENT, money(*c))).collect();
    let mut je_rows: Vec<RowDraft> = je
        .iter()
        .map(|c| je_row(next(), 3, RENT, Some(money(*c)), None))
        .collect();
    let total: i64 = je.iter().sum();
    if total > 0 {
        je_rows.push(je_row(next(), 3, SUPPLIES, None, Some(money(total))));
    }
    BulkBatch {
        sections: vec![
            section(SectionKind::Checks, Some(BANK), checks),
            section(SectionKind::Deposits, Some(BANK), deposits),
            section(SectionKind::JournalEntries, None, je_rows),
        ],
        add_new_names: false,
    }
}

fn shift(m: Money, units: i128) -> Money {
    Money::from_ten_thousandths(m.to_ten_thousandths() + units).expect("money")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The totals are exactly the sum of the rows, however they split.
    #[test]
    fn totals_equal_the_sum_of_rows(cents in proptest::collection::vec(1_i64..100_000_000_000, 0..60)) {
        let b = batch_from(&cents, &[]);
        let t = b.totals().expect("totals");
        let total: i128 = t.values().map(|m| m.to_ten_thousandths()).sum();
        let want: i128 = cents.iter().map(|c| i128::from(*c) * 100).sum();
        prop_assert_eq!(total, want);
        let sections: i128 = (0..2)
            .map(|i| b.section_totals(i).expect("totals").amount.to_ten_thousandths())
            .sum();
        prop_assert_eq!(sections, want);
    }

    /// A JE group balances exactly for any cents; a cent off is reported on
    /// every row with the exact difference.
    #[test]
    fn je_balance_is_exact(
        je in proptest::collection::vec(1_i64..10_000_000_000, 1..20),
        off in -50_i64..50,
    ) {
        let mut b = batch_from(&[], &je);
        prop_assert!(b.local_problems(None).is_empty());
        if off != 0 {
            let last = b.sections[2].rows.last_mut().expect("credit row");
            let credit = last.credit.expect("credit");
            prop_assume!(credit.to_ten_thousandths() + i128::from(off) * 100 > 0);
            last.credit = Some(shift(credit, i128::from(off) * 100));
            let diff = money(-off);
            let ps = b.local_problems(None);
            prop_assert_eq!(ps.len(), b.sections[2].rows.len());
            let all = ps.iter().all(|(_, p)| {
                matches!(p, Problem::JeUnbalanced { difference, .. } if *difference == diff)
            });
            prop_assert!(all);
        }
    }

    /// Gate soundness: whenever Save all is allowed, every non-blank row is
    /// sent and engine-approved for its current content, every amount sent
    /// is a positive whole number of cents, and every journal entry sent
    /// balances exactly.
    #[test]
    fn gate_is_sound(
        cents in proptest::collection::vec(-200_i64..2_000, 0..30),
        sub_cent in proptest::collection::vec(0_u8..4, 0..30),
        je in proptest::collection::vec(1_i64..5_000, 0..6),
        je_off in 0_u8..3,
        blank_every in 2_usize..9,
        check_all in any::<bool>(),
        edit_after in any::<bool>(),
    ) {
        let mut b = batch_from(&cents, &je);
        for (i, r) in b.sections[0].rows.iter_mut().enumerate() {
            if sub_cent.get(i).copied() == Some(1) {
                r.amount = r.amount.map(|a| shift(a, 5));
            }
            if i % blank_every == 0 {
                *r = RowDraft::new(r.key);
            }
        }
        if je_off == 1
            && let Some(last) = b.sections[2].rows.last_mut()
        {
            last.credit = last.credit.map(|c| shift(c, 100));
        }
        let mut checks = EngineChecks::new();
        if check_all {
            ok_all(&b, &mut checks);
        }
        if edit_after && let Some(r) = b.sections[1].rows.first_mut() {
            r.memo.push('x');
        }
        if b.can_save(&checks, SaveCaps::ALL, None).is_ok() {
            let out = b.outgoing(None, None);
            prop_assert_eq!(out.sent.len(), b.filled_count());
            for g in &out.groups {
                for r in &g.rows {
                    for a in [r.amount, r.debit, r.credit].into_iter().flatten() {
                        prop_assert!(a.is_positive());
                        prop_assert_eq!(a.to_ten_thousandths() % 100, 0);
                    }
                }
                if g.kind.is_journal() {
                    let d: i128 = g.rows.iter().filter_map(|r| r.debit).map(Money::to_ten_thousandths).sum();
                    let c: i128 = g.rows.iter().filter_map(|r| r.credit).map(Money::to_ten_thousandths).sum();
                    prop_assert_eq!(d, c);
                }
            }
            for (_, r) in b.filled() {
                let ok = matches!(checks.state(&b, r.key), EngineState::Ok { .. });
                prop_assert!(ok);
            }
        }
    }
}
