use super::*;
use proptest::prelude::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn input() -> WhyInput {
    WhyInput {
        bucket: Some(Bucket::Ready),
        source: Source::None,
        has_rule: false,
        confidence_bp: None,
        possible_duplicate: false,
        match_candidates: 0,
        transfer: false,
        has_account: true,
        entry_voided: false,
        has_receipt: false,
    }
}

#[test]
fn badges_follow_the_engine_and_ai_hides_when_off() {
    let ai = |bp| WhyInput {
        source: Source::Ai,
        confidence_bp: Some(bp),
        ..input()
    };
    assert_eq!(why(ai(8_000), true), Why::Ai { high: true });
    assert_eq!(why(ai(7_999), true), Why::Ai { high: false });
    // AI off or offline: never an AI badge.
    assert_eq!(why(ai(9_500), false), Why::History);
    assert!(!why(ai(9_500), false).is_ai());
    let rule = WhyInput {
        has_rule: true,
        source: Source::Rule,
        ..input()
    };
    assert_eq!(why(rule, false), Why::Rule);
    let matched = WhyInput {
        bucket: Some(Bucket::Matches),
        match_candidates: 1,
        ..input()
    };
    assert_eq!(why(matched, true), Why::Match);
    let transfer = WhyInput {
        transfer: true,
        ..matched
    };
    assert_eq!(why(transfer, true), Why::Transfer);
    let none = WhyInput {
        bucket: Some(Bucket::Review),
        has_account: false,
        ..input()
    };
    assert_eq!(why(none, true), Why::NeedsYou);
    let dup = WhyInput {
        possible_duplicate: true,
        ..rule
    };
    assert_eq!(why(dup, true), Why::Duplicate);
    let voided = WhyInput {
        bucket: None,
        entry_voided: true,
        ..rule
    };
    assert_eq!(why(voided, true), Why::EntryVoided);
    // A05: a receipt shows over the proposal, never over "Needs you" (no
    // account yet), a match, a transfer, a duplicate or a voided entry.
    let receipt = WhyInput {
        has_receipt: true,
        ..rule
    };
    assert_eq!(why(receipt, true), Why::Receipt);
    assert_eq!(
        why(
            WhyInput {
                has_account: false,
                ..receipt
            },
            true
        ),
        Why::NeedsYou
    );
    assert_eq!(
        why(
            WhyInput {
                has_receipt: true,
                ..matched
            },
            true
        ),
        Why::Match
    );
    assert_eq!(
        why(
            WhyInput {
                has_receipt: true,
                ..dup
            },
            true
        ),
        Why::Duplicate
    );
    assert_eq!(Source::parse(Some("fuzzy")), Source::History);
    assert_eq!(Source::parse(None), Source::None);
}

#[test]
fn keys_move_select_and_ask() {
    let mut l: ReviewList<u32> = ReviewList::default();
    assert!(l.command("feed.next"));
    assert_eq!(l.current(), None, "nothing to move over");
    l.set_rows(vec![3, 1, 2]);
    assert_eq!(l.current(), Some(3));
    l.command("feed.next");
    l.command("feed.next_arrow");
    l.command("feed.next");
    assert_eq!(l.current(), Some(2), "stops at the last row");
    l.command("feed.select");
    l.command("feed.prev");
    l.command("feed.select");
    assert_eq!(l.selected(), vec![1, 2], "list order");
    l.command("feed.accept_selected");
    assert_eq!(
        l.action.take(),
        Some(FeedAction::AcceptSelected(vec![1, 2]))
    );
    l.command("feed.accept");
    assert_eq!(l.action.take(), Some(FeedAction::Accept(1)));
    l.command("feed.split");
    assert_eq!(l.action.take(), Some(FeedAction::Split(1)));
    l.command("feed.rule");
    assert_eq!(l.action.take(), Some(FeedAction::Rule(1)));
    assert!(!l.command("queue.next"));
    // A reload keeps the cursor's row and drops rows that left.
    l.set_rows(vec![1, 4]);
    assert_eq!(l.current(), Some(1));
    assert_eq!(l.selected(), vec![1]);
    // A row that changed elsewhere leaves the selection.
    l.drop_rows(&[1]);
    assert_eq!(l.selected(), Vec::<u32>::new());
    // Ctrl+Enter with nothing selected accepts the cursor's row.
    l.command("feed.accept_selected");
    assert_eq!(l.action.take(), Some(FeedAction::AcceptSelected(vec![1])));
    l.toggle_all();
    assert_eq!(l.selected(), vec![1, 4]);
    l.toggle_all();
    assert_eq!(l.selected(), Vec::<u32>::new());
    // The cursor's row left: the cursor stays in place.
    l.focus(4);
    l.set_rows(vec![1]);
    assert_eq!(l.current(), Some(1));
    l.set_rows(Vec::new());
    assert_eq!(l.current(), None);
}

#[test]
fn accept_selected_takes_mutual_single_matches_and_never_transfers() {
    let row = |id, bucket, has_account, n, mutual, transfer| PlanRow {
        id,
        bucket,
        has_account,
        match_candidates: n,
        mutual,
        transfer,
        possible_duplicate: false,
    };
    let rows = [
        row(9, Some(Bucket::Ready), true, 0, false, false),
        row(2, Some(Bucket::Ready), true, 0, false, false),
        row(5, Some(Bucket::Matches), true, 1, true, false),
        row(4, Some(Bucket::Matches), true, 2, false, false),
        row(7, Some(Bucket::Matches), true, 1, false, false),
        row(6, Some(Bucket::Matches), true, 1, true, true),
        row(8, Some(Bucket::Review), false, 0, false, false),
        row(1, None, true, 0, false, false),
        PlanRow {
            possible_duplicate: true,
            ..row(3, Some(Bucket::Review), true, 0, false, false)
        },
    ];
    let p = plan_accept(&rows);
    assert_eq!(p.categorize, vec![2, 9], "sorted by id");
    assert_eq!(p.matches, vec![5]);
    assert_eq!(p.len(), 3);
    assert_eq!(
        p.skipped,
        vec![
            (4, Skip::AmbiguousMatch),
            (7, Skip::AmbiguousMatch),
            (6, Skip::Transfer),
            (8, Skip::NeedsReview),
            (1, Skip::NotWaiting),
            (3, Skip::Duplicate),
        ]
    );
    assert!(plan_accept::<u32>(&[]).is_empty());
}

#[test]
fn split_remainder_and_cents() {
    let mut d: SplitDraft<u8> = SplitDraft::new(m("84.17")).expect("whole cents");
    assert_eq!(d.remainder(), Some(m("84.17")));
    d.lines[0] = (Some(1), Some(m("50.00")));
    d.lines[1] = (Some(2), Some(m("34.16")));
    assert_eq!(d.remainder(), Some(m("0.01")));
    assert!(!d.is_complete());
    d.lines[1].1 = Some(m("34.17"));
    assert!(d.is_complete());
    assert!(SplitDraft::<u8>::new(m("84.1750")).is_none(), "3+ decimals");
    // Money in splits its absolute amount.
    assert_eq!(
        SplitDraft::<u8>::new(m("-10.00")).map(|d| d.total),
        Some(m("10.00"))
    );
}

#[test]
fn deposit_difference_reaches_zero_with_the_fee() {
    // Plan test 24: 720.00 − 20.54 = 699.46.
    let mut d: DepositDraft<u8, u8> = DepositDraft {
        bank: m("699.46"),
        payments: vec![(1, m("720.00"))],
        fees: vec![(Some(9), Some(m("20.53")))],
    };
    assert_eq!(d.difference(), Some(m("-0.01")));
    assert!(!d.is_balanced());
    d.fees[0].1 = Some(m("20.54"));
    assert_eq!(d.difference(), Some(Money::ZERO));
    assert!(d.is_balanced());
    // Two payments, no fee (plan test 10): 413.27 + 286.19 = 699.46.
    let two: DepositDraft<u8, u8> = DepositDraft {
        bank: m("699.46"),
        payments: vec![(1, m("413.27")), (2, m("286.19"))],
        fees: Vec::new(),
    };
    assert!(two.is_balanced());
    let mut eleven = two.clone();
    eleven.fees = (0..11).map(|_| (Some(1), Some(m("0.00")))).collect();
    assert!(!eleven.is_balanced());
    // The largest amount stays exact.
    let big: DepositDraft<u8, u8> = DepositDraft {
        bank: m("999999999999.99"),
        payments: vec![(1, m("999999999999.99"))],
        fees: Vec::new(),
    };
    assert_eq!(big.difference(), Some(Money::ZERO));
}

#[test]
fn money_out_is_amount_sign_one() {
    assert_eq!(RuleDirection::MoneyOut.amount_sign(), Some(1));
    assert_eq!(RuleDirection::MoneyIn.amount_sign(), Some(-1));
    assert_eq!(RuleDirection::Either.amount_sign(), None);
    assert_eq!(RuleDirection::of(m("84.17")), RuleDirection::MoneyOut);
    assert_eq!(RuleDirection::of(m("-84.17")), RuleDirection::MoneyIn);
    assert_eq!(display_amount(m("84.17")), m("-84.17"));
}

proptest! {
    /// A split that places every cent leaves exactly nothing.
    #[test]
    fn split_remainder_is_exact(cents in 2i64..1_000_000_000, cut in 1i64..1_000_000) {
        let total = Money::from_cents(cents).expect("total");
        let first = Money::from_cents(cut.min(cents - 1)).expect("first");
        let mut d: SplitDraft<u8> = SplitDraft::new(total).expect("cents");
        d.lines[0] = (Some(1), Some(first));
        d.lines[1] = (Some(2), d.remainder());
        prop_assert_eq!(d.remainder(), Some(Money::ZERO));
        prop_assert!(d.is_complete());
    }
}
