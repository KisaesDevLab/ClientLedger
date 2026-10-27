use proptest::prelude::*;

use super::*;
use crate::commands::find;

#[test]
fn every_kind_opens_a_real_form_or_the_chooser() {
    for kind in DocKind::ALL {
        match kind.form() {
            Some(form) => {
                assert_eq!(form.kind(), kind, "{kind:?}");
                let c = find(form.command()).expect("a command");
                assert!(c.opens && c.arrives.is_none(), "{kind:?}");
            }
            None => assert_eq!(kind, DocKind::Unknown),
        }
    }
    // Cash expenses use Record Card Charge's "Expense paid from" mode.
    assert_eq!(DraftForm::CashExpense.command(), "vendors.card_charge");
    assert_eq!(
        DocKind::ALL.map(DocKind::name),
        ["bill", "card_charge", "check", "cash_expense", "unknown"]
    );
    // The chooser offers every form once and no default (Q-A04n).
    let mut seen = std::collections::HashSet::new();
    assert!(DraftForm::ALL.iter().all(|f| seen.insert(*f)));
    assert_eq!(seen.len(), 4);
}

#[test]
fn status_classification() {
    use DuplicateCheck as D;
    use QueueStatus as Q;
    let cases = [
        (DraftState::Failed, false, D::Possible, Q::CouldntRead),
        (DraftState::Failed, true, D::Clear, Q::CouldntRead),
        (DraftState::New, true, D::Possible, Q::WaitingToBeRead),
        (DraftState::New, false, D::NotChecked, Q::WaitingToBeRead),
        (DraftState::Ready, true, D::Possible, Q::PossibleDuplicate),
        (DraftState::Ready, false, D::Possible, Q::PossibleDuplicate),
        (DraftState::Ready, true, D::Clear, Q::CheckFields),
        (DraftState::Ready, true, D::Checking, Q::CheckFields),
        (DraftState::Ready, false, D::Clear, Q::Ready),
        (DraftState::Ready, false, D::NotChecked, Q::Ready),
        (DraftState::Ready, false, D::Failed, Q::Ready),
    ];
    for (state, check, dup, want) in cases {
        assert_eq!(
            classify(state, check, dup),
            want,
            "{state:?} {check} {dup:?}"
        );
    }
    let row = QueueRow {
        id: 7_u32,
        updated_at: "t1".into(),
        kind: DocKind::Bill,
        state: DraftState::Ready,
        needs_check: false,
        duplicate: DuplicateCheck::default(),
    };
    assert_eq!(row.status(), Q::Ready);
    let names: std::collections::HashSet<_> = [
        Q::WaitingToBeRead,
        Q::Ready,
        Q::CheckFields,
        Q::PossibleDuplicate,
        Q::CouldntRead,
    ]
    .iter()
    .map(|s| (s.name(), s.key().0))
    .collect();
    assert_eq!(names.len(), 5);
    assert!(needs_check(Confidence::Medium) && needs_check(Confidence::Unrated));
    assert!(!needs_check(Confidence::High));
}

#[test]
fn j_and_k_stay_on_the_same_draft_across_a_refresh() {
    let mut c = QueueCursor::new();
    assert_eq!(c.selected(), None::<&&str>);
    let ids = ["a", "b", "c", "d"];
    // J with nothing selected selects the first row.
    assert_eq!(c.step(&ids, Step::Next), Some(&"a"));
    assert_eq!(c.step(&ids, Step::Next), Some(&"b"));
    assert_eq!(c.step(&ids, Step::Next), Some(&"c"));
    assert_eq!(c.step(&ids, Step::Prev), Some(&"b"));
    // A new draft arrives on top: still "b".
    c.refresh(&["z", "a", "b", "c", "d"]);
    assert_eq!(c.selected(), Some(&"b"));
    assert_eq!(c.position(&["z", "a", "b", "c", "d"]), Some(2));
    // "b" was entered: the row that took its place is selected.
    let after = ["z", "a", "c", "d"];
    c.refresh(&after);
    assert_eq!(c.selected(), Some(&"c"));
    // The last row leaves: the new last row.
    let mut c = QueueCursor::new();
    assert!(c.select(&after, &"d"));
    c.refresh(&["z", "a", "c"]);
    assert_eq!(c.selected(), Some(&"c"));
    // An empty list selects nothing; the next load selects the first row.
    c.refresh(&[]);
    assert_eq!(c.selected(), None);
    c.refresh(&["q", "r"]);
    assert_eq!(c.selected(), Some(&"q"));
    assert!(!c.select(&["q", "r"], &"x"));
    assert_eq!(c.selected(), Some(&"q"));
}

#[test]
fn ends_hold_and_neighbours_do_not_move() {
    let ids = [1, 2, 3];
    let mut c = QueueCursor::new();
    assert_eq!(c.neighbour(&ids, Step::Next), None, "nothing selected");
    c.select(&ids, &1);
    assert_eq!(c.step(&ids, Step::Prev), Some(&1));
    assert_eq!(c.neighbour(&ids, Step::Prev), None);
    assert_eq!(c.neighbour(&ids, Step::Next), Some(2));
    assert_eq!(c.selected(), Some(&1), "neighbour doesn't move");
    c.select(&ids, &3);
    assert_eq!(c.step(&ids, Step::Next), Some(&3));
    assert_eq!(c.neighbour(&ids, Step::Next), None);
    // A selection that left the list: J selects the first row again.
    assert_eq!(c.step(&[7, 8], Step::Next), Some(&7));
}

proptest! {
    /// A selected draft that is still listed stays selected after any
    /// reorder or insertion; one that left is replaced by a listed row.
    #[test]
    fn refresh_keeps_a_listed_selection(
        ids in proptest::collection::hash_set(0_u32..200, 1..40),
        pick in 0_usize..40,
        extra in proptest::collection::vec(200_u32..400, 0..10),
        drop_it in any::<bool>(),
    ) {
        let ids: Vec<u32> = ids.into_iter().collect();
        let chosen = ids[pick % ids.len()];
        let mut c = QueueCursor::new();
        prop_assert!(c.select(&ids, &chosen));
        let mut next: Vec<u32> = extra.into_iter().chain(ids.iter().rev().copied()).collect();
        if drop_it {
            next.retain(|i| *i != chosen);
        }
        c.refresh(&next);
        if drop_it {
            prop_assert_eq!(c.selected().is_some(), !next.is_empty());
            if let Some(s) = c.selected() {
                prop_assert!(next.contains(s));
            }
        } else {
            prop_assert_eq!(c.selected(), Some(&chosen));
        }
    }
}

#[test]
fn page_boxes() {
    assert_eq!(PageBox::new(0, 10, 10, 5, 20), None, "left past right");
    assert_eq!(PageBox::new(0, 0, 0, 10_001, 10), None, "off the page");
    let b = PageBox::new(1, 1000, 2000, 3000, 2500).expect("box");
    assert_eq!((b.width(), b.height()), (2000, 500));
    assert!(!b.too_small() && !b.whole_page());
    assert!(PageBox::new(0, 0, 0, 99, 5000).expect("thin").too_small());
    assert!(!PageBox::new(0, 0, 0, 100, 100).expect("1%").too_small());
    assert!(
        PageBox::new(0, 0, 0, 9_500, 9_500)
            .expect("big")
            .whole_page()
    );
    assert!(
        !PageBox::new(0, 0, 0, 9_500, 9_000)
            .expect("tall")
            .whole_page()
    );
    let word = PageBox::new(1, 1200, 2100, 1800, 2400).expect("word");
    assert!(b.contains(word));
    assert!(!b.contains(PageBox { page: 0, ..word }), "another page");
    assert!(!b.contains(PageBox { r: 3001, ..word }));
}

#[test]
fn every_read_target_has_its_own_key() {
    let targets = [
        ReadTarget::Vendor,
        ReadTarget::BillDate,
        ReadTarget::DueDate,
        ReadTarget::Reference,
        ReadTarget::AmountDue,
        ReadTarget::Line(RowKey(4)),
        ReadTarget::Payee,
        ReadTarget::Date,
        ReadTarget::Total,
        ReadTarget::CheckNumber,
        ReadTarget::PaidFrom,
    ];
    let keys: std::collections::HashSet<TargetKey> =
        targets.iter().map(|t| TargetKey::from(*t)).collect();
    assert_eq!(keys.len(), targets.len());
    assert_eq!(
        TargetKey::from(ReadTarget::Line(RowKey(4))),
        TargetKey::line("line_amount", RowKey(4))
    );
    assert_eq!(
        TargetKey::from(ReadTarget::AmountDue),
        TargetKey::field("amount_due")
    );
}

#[test]
fn target_levels() {
    let read = |level| Some(ReadField::Read { level });
    assert_eq!(
        TargetLevel::of(read(Confidence::High), true),
        TargetLevel::Sure
    );
    for l in [Confidence::Medium, Confidence::Low, Confidence::Unrated] {
        assert_eq!(TargetLevel::of(read(l), true), TargetLevel::CheckThis);
    }
    assert_eq!(
        TargetLevel::of(Some(ReadField::Accepted), true),
        TargetLevel::Sure
    );
    assert_eq!(
        TargetLevel::of(Some(ReadField::Edited), true),
        TargetLevel::Sure
    );
    assert_eq!(TargetLevel::of(None, true), TargetLevel::Sure);
    assert_eq!(
        TargetLevel::of(read(Confidence::High), false),
        TargetLevel::NotFound
    );
    assert_eq!(
        [
            TargetLevel::Sure,
            TargetLevel::CheckThis,
            TargetLevel::NotFound
        ]
        .map(TargetLevel::name),
        ["sure", "check_this", "not_found"]
    );
}

fn target(field: &'static str, fill: Option<FillKind>) -> CompareTarget {
    CompareTarget {
        key: TargetKey::field(field),
        label: field.to_string(),
        spot: None,
        level: TargetLevel::NotFound,
        fill,
    }
}

#[test]
fn the_fill_arm_survives_focus_moves() {
    let targets = vec![
        target("vendor", None),
        target("amount_due", Some(FillKind::Money)),
        target("bill_date", Some(FillKind::Date)),
    ];
    let mut arm = FillArm::new();
    assert_eq!(arm.armed(), None);
    // A list field never arms.
    arm.focus(Some(TargetKey::field("vendor")), &targets);
    assert_eq!(arm.armed(), None);
    arm.focus(Some(TargetKey::field("amount_due")), &targets);
    let want = Some((TargetKey::field("amount_due"), FillKind::Money));
    assert_eq!(arm.armed(), want);
    // To the document, to a button, to the vendor: still armed.
    arm.focus(None, &targets);
    arm.focus(Some(TargetKey::field("not_a_target")), &targets);
    arm.focus(Some(TargetKey::field("vendor")), &targets);
    assert_eq!(arm.armed(), want);
    // Another fillable field takes it.
    arm.focus(Some(TargetKey::field("bill_date")), &targets);
    assert_eq!(
        arm.armed(),
        Some((TargetKey::field("bill_date"), FillKind::Date))
    );
    arm.retain(&targets);
    assert!(arm.armed().is_some());
    // Its field is gone: disarmed.
    arm.retain(&targets[..2]);
    assert_eq!(arm.armed(), None);
    arm.focus(Some(TargetKey::field("amount_due")), &targets);
    arm.clear();
    assert_eq!(arm.armed(), None);
    assert_eq!(CompareFeed::default().generation, 0);
}
