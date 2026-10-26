use super::*;

fn id(n: u32) -> AccountId {
    AccountId::parse(&format!("a0000000-0000-4000-8000-{n:012x}")).expect("id")
}

fn entry(n: u32, kind: AccountKind, number: Option<&str>, name: &str) -> ChartEntry {
    ChartEntry {
        id: id(n),
        parent: None,
        number: number.map(str::to_string),
        name: name.to_string(),
        kind,
        detail: None,
        description: None,
        active: true,
        system: false,
        sensitive: false,
        balance: None,
        card_last4: CardEnding::default(),
    }
}

fn under(mut e: ChartEntry, parent: u32) -> ChartEntry {
    e.parent = Some(id(parent));
    e
}

fn names(chart: &Chart, rows: &[usize]) -> Vec<String> {
    rows.iter()
        .map(|r| {
            let row = &chart.rows()[*r];
            format!("{}{}", "  ".repeat(row.depth), row.full_name)
        })
        .collect()
}

fn all(chart: &Chart) -> Vec<String> {
    let rows: Vec<usize> = (0..chart.rows().len()).collect();
    names(chart, &rows)
}

fn sample() -> Vec<ChartEntry> {
    vec![
        entry(1, AccountKind::Expense, Some("6200"), "Fuel"),
        entry(2, AccountKind::Asset, Some("1010"), "Checking"),
        under(
            entry(3, AccountKind::Cogs, Some("5110"), "Mulch & stone"),
            4,
        ),
        entry(4, AccountKind::Cogs, Some("5100"), "Materials"),
        under(entry(5, AccountKind::Cogs, Some("5105"), "Plants"), 4),
        entry(6, AccountKind::Asset, Some("900"), "Petty cash"),
        entry(7, AccountKind::Asset, None, "Undeposited funds"),
        entry(8, AccountKind::Income, Some("4000"), "Landscaping income"),
    ]
}

#[test]
fn wire_names_round_trip_and_kinds_keep_chart_order() {
    for k in AccountKind::ALL {
        assert_eq!(AccountKind::from_wire(k.wire()), Some(k));
        assert_ne!(k.detail_types().len(), 0);
        for d in k.detail_types() {
            assert!(detail_label(d).is_some(), "{d} has a label");
        }
    }
    assert_eq!(AccountKind::from_wire("revenue"), Some(AccountKind::Income));
    assert_eq!(AccountKind::from_wire("bogus"), None);
    assert_eq!(detail_label("my_own_type"), None);
    assert!(AccountKind::Asset < AccountKind::OtherExpense);
}

#[test]
fn types_then_numbers_then_children_under_parents() {
    let chart = Chart::new(sample(), true);
    assert_eq!(
        all(&chart),
        [
            "Petty cash",
            "Checking",
            "Undeposited funds",
            "Landscaping income",
            "Materials",
            "  Materials:Plants",
            "  Materials:Mulch & stone",
            "Fuel",
        ],
        "900 before 1010 (numeric), unnumbered last, children by number"
    );
    assert_eq!(chart.rows()[5].depth, 1);
    assert!(!chart.rows().iter().any(|r| r.detached));
}

#[test]
fn without_numbers_siblings_sort_by_name() {
    let chart = Chart::new(sample(), false);
    assert_eq!(
        all(&chart),
        [
            "Checking",
            "Petty cash",
            "Undeposited funds",
            "Landscaping income",
            "Materials",
            "  Materials:Mulch & stone",
            "  Materials:Plants",
            "Fuel",
        ]
    );
    assert_eq!(chart.display_name(id(2)).as_deref(), Some("Checking"));
    let on = Chart::new(sample(), true);
    assert_eq!(
        on.display_name(id(2)).as_deref(),
        Some("1010 \u{b7} Checking")
    );
    assert_eq!(
        on.display_name(id(3)).as_deref(),
        Some("5110 \u{b7} Materials:Mulch & stone")
    );
    assert_eq!(on.display_name(id(7)).as_deref(), Some("Undeposited funds"));
}

#[test]
fn missing_parents_and_loops_are_listed_at_the_top_and_marked() {
    let entries = vec![
        under(entry(1, AccountKind::Expense, None, "Orphan"), 99),
        under(entry(2, AccountKind::Expense, None, "Loop A"), 3),
        under(entry(3, AccountKind::Expense, None, "Loop B"), 2),
        under(entry(4, AccountKind::Expense, None, "Self"), 4),
        under(entry(5, AccountKind::Expense, None, "Below B"), 3),
    ];
    let chart = Chart::new(entries, false);
    assert_eq!(chart.rows().len(), 5, "every account is listed once");
    assert_eq!(
        all(&chart),
        [
            "Orphan",
            "Self",
            "Loop A",
            "  Loop A:Loop B",
            "    Loop A:Loop B:Below B",
        ]
    );
    let detached: Vec<bool> = chart.rows().iter().map(|r| r.detached).collect();
    assert_eq!(detached, [true, true, true, false, false]);
    assert!(chart.is_within(id(5), id(2)));
    assert!(
        chart.is_within(id(2), id(3)),
        "the loop never spins forever"
    );
    assert!(!chart.is_within(id(1), id(2)));
}

#[test]
fn deep_chains_need_no_recursion_and_names_stay_short() {
    let depth: u32 = 5_000;
    let entries: Vec<ChartEntry> = (0..depth)
        .map(|i| {
            let e = entry(i + 1, AccountKind::Asset, None, &format!("L{i}"));
            if i == 0 { e } else { under(e, i) }
        })
        .collect();
    let chart = Chart::new(entries, false);
    let last = chart.rows().last().expect("rows");
    assert_eq!(last.depth, usize::try_from(depth).unwrap() - 1);
    assert!(last.full_name.starts_with('\u{2026}'), "{}", last.full_name);
    assert!(last.full_name.len() < 64);
}

#[test]
fn filter_hides_inactive_and_searches_names_and_numbers() {
    let mut entries = sample();
    entries[0].active = false;
    let chart = Chart::new(entries, true);
    let mut f = ChartFilter::default();
    assert!(
        !names(&chart, &chart.visible(&f))
            .iter()
            .any(|n| n.contains("Fuel"))
    );
    f.show_inactive = true;
    assert_eq!(chart.visible(&f).len(), 8);
    f.search = "  MULCH ".into();
    assert_eq!(
        names(&chart, &chart.visible(&f)),
        ["  Materials:Mulch & stone"]
    );
    f.search = "materials".into();
    assert_eq!(
        chart.visible(&f).len(),
        3,
        "the parent's name is in each full name"
    );
    f.search = "510".into();
    assert_eq!(
        chart.visible(&f).len(),
        2,
        "numbers match while shown (5100, 5105)"
    );
    let off = Chart::new(sample(), false);
    assert_eq!(off.visible(&f).len(), 0, "hidden numbers do not match");
}

#[test]
fn twenty_thousand_accounts_filter_quickly() {
    let entries: Vec<ChartEntry> = (0..20_000u32)
        .map(|i| {
            let kind = AccountKind::ALL[usize::try_from(i % 8).unwrap()];
            let e = entry(
                i + 1,
                kind,
                Some(&format!("{}", 10_000 + i)),
                &format!("Account {i}"),
            );
            if i % 5 == 0 || i < 8 {
                e
            } else {
                under(e, (i / 5) * 5 + 1)
            }
        })
        .collect();
    let chart = Chart::new(entries, true);
    assert_eq!(chart.rows().len(), 20_000);
    let f = ChartFilter {
        show_inactive: false,
        search: "account 1999".into(),
    };
    let (hits, took) = crate::lists::fastest_of(5, || chart.visible(&f));
    assert_ne!(hits.len(), 0);
    assert!(took.as_millis() < 100, "filter took {took:?}");
}

#[test]
fn parent_choices_are_active_same_type_and_never_below_the_account() {
    let mut entries = sample();
    entries.push(entry(9, AccountKind::Cogs, Some("5900"), "Old materials"));
    entries[8].active = false;
    let chart = Chart::new(entries, true);
    let picks = |editing| -> Vec<String> {
        chart
            .parent_choices(AccountKind::Cogs, editing)
            .iter()
            .map(|r| chart.rows()[*r].full_name.clone())
            .collect()
    };
    assert_eq!(
        picks(None),
        ["Materials", "Materials:Plants", "Materials:Mulch & stone"]
    );
    assert_eq!(
        picks(Some(id(4))),
        Vec::<String>::new(),
        "not itself or below"
    );
    assert_eq!(picks(Some(id(3))), ["Materials", "Materials:Plants"]);
    assert!(chart.has_children(id(4)));
    assert!(!chart.has_children(id(3)));
}

#[test]
fn number_owner_ignores_case_space_and_the_edited_account() {
    let chart = Chart::new(sample(), true);
    assert_eq!(chart.number_owner(" 6200 ", None), Some(id(1)));
    assert_eq!(chart.number_owner("6200", Some(id(1))), None);
    assert_eq!(chart.number_owner("", None), None);
    assert_eq!(chart.number_owner("7777", None), None);
}

#[test]
fn check_trims_and_reports_each_problem() {
    let chart = Chart::new(sample(), true);
    let mut d = AccountDraft::new(AccountKind::Cogs);
    assert_eq!(d.detail, "cost_of_goods_sold");
    d.name = "  Sod  ".into();
    d.number = " 5120 ".into();
    d.parent = Some(id(4));
    d.description = "  ".into();
    let ok = check(&d, &chart, None).expect("valid");
    assert_eq!(ok.name, "Sod");
    assert_eq!(ok.number.as_deref(), Some("5120"));
    assert_eq!(ok.description, None);
    assert_eq!(ok.parent, Some(id(4)));

    let mut bad = d.clone();
    bad.name = "Plants:Trees".into();
    bad.number = "5100".into();
    bad.parent = Some(id(1));
    let p = check(&bad, &chart, None).expect_err("problems");
    assert_eq!(
        p,
        [
            DraftProblem::NameColon,
            DraftProblem::NumberTaken,
            DraftProblem::ParentOtherKind
        ]
    );
    assert_eq!(p[1].field(), DraftField::Number);
    assert_eq!(p[1].key().as_str(), "lists.accounts.problem.number_taken");

    bad = d.clone();
    bad.name = " ".into();
    bad.number = "1".repeat(NUMBER_MAX + 1);
    bad.detail = "x".repeat(DETAIL_MAX + 1);
    bad.parent = Some(id(99));
    assert_eq!(
        check(&bad, &chart, None).expect_err("problems"),
        [
            DraftProblem::NameMissing,
            DraftProblem::NumberTooLong,
            DraftProblem::DetailTooLong,
            DraftProblem::ParentMissing
        ]
    );
    bad = d.clone();
    bad.name = "n".repeat(NAME_MAX + 1);
    assert_eq!(
        check(&bad, &chart, None).expect_err("long"),
        [DraftProblem::NameTooLong]
    );
}

/// A new credit card account (Liability, detail `credit_card`) with
/// `digits` typed in "Card ends in".
fn card_draft(digits: &str) -> AccountDraft {
    let mut d = AccountDraft::new(AccountKind::Liability);
    d.detail = CREDIT_CARD_DETAIL.into();
    d.name = "Fuel Card".into();
    d.card_last4 = CardEnding(digits.into());
    d
}

#[test]
fn card_digits_are_four_digits_or_empty_on_credit_cards_only() {
    let chart = Chart::new(sample(), true);
    // Four digits, trimmed.
    let ok = check(&card_draft(" 4821 "), &chart, None).expect("valid");
    assert_eq!(ok.card_last4, Some(CardEnding("4821".into())));
    // Empty clears.
    let ok = check(&card_draft("  "), &chart, None).expect("empty");
    assert_eq!(ok.card_last4, None);
    // Anything else is refused on the card field.
    for bad in ["482", "48211", "48a1", "4 21", "\u{0664}821", "-482"] {
        let p = check(&card_draft(bad), &chart, None).expect_err("bad digits");
        assert_eq!(p, [DraftProblem::CardDigits], "{bad:?}");
        assert_eq!(p[0].field(), DraftField::Card);
        assert_eq!(p[0].key().as_str(), "lists.accounts.problem.card_digits");
    }
    // Not a credit card: the digits are dropped, never sent or refused.
    let mut bank = card_draft("48a1");
    bank.kind = AccountKind::Asset;
    bank.detail = "bank".into();
    assert_eq!(
        check(&bank, &chart, None).expect("dropped").card_last4,
        None
    );
    assert!(takes_card_digits("credit_card"));
    assert!(!takes_card_digits("bank"));
    assert!(!takes_card_digits(""));
}

#[test]
fn card_digits_follow_the_entry_clear_on_type_change_and_never_debug_print() {
    let mut e = entry(9, AccountKind::Liability, Some("2050"), "Card");
    e.detail = Some(CREDIT_CARD_DETAIL.into());
    e.card_last4 = CardEnding("4821".into());
    let mut d = AccountDraft::from_entry(&e);
    assert_eq!(d.card_last4.as_str(), "4821");
    let shown = format!("{d:?} {e:?}");
    assert!(!shown.contains("4821"), "{shown}");
    d.set_kind(AccountKind::Asset);
    assert!(d.card_last4.is_empty());
    assert!(CardEnding("0007".into()).is_valid());
    assert!(!CardEnding(String::new()).is_valid());
}

#[test]
fn editing_keeps_its_own_number_and_refuses_loops_and_type_changes() {
    let chart = Chart::new(sample(), true);
    let parent = chart.entry(id(4)).expect("materials").clone();
    let mut d = AccountDraft::from_entry(&parent);
    assert_eq!(d.number, "5100");
    assert!(
        check(&d, &chart, Some(id(4))).is_ok(),
        "its own number is fine"
    );
    d.parent = Some(id(5));
    assert_eq!(
        check(&d, &chart, Some(id(4))).expect_err("loop"),
        [DraftProblem::ParentLoop]
    );
    let mut d = AccountDraft::from_entry(&parent);
    d.kind = AccountKind::Expense;
    assert_eq!(
        check(&d, &chart, Some(id(4))).expect_err("kind"),
        [DraftProblem::KindHasSubaccounts]
    );
    let mut inactive = sample();
    inactive[3].active = false;
    let chart = Chart::new(inactive, true);
    let mut d = AccountDraft::new(AccountKind::Cogs);
    d.name = "Sod".into();
    d.parent = Some(id(4));
    assert_eq!(
        check(&d, &chart, None).expect_err("inactive"),
        [DraftProblem::ParentInactive]
    );
}

#[test]
fn changing_the_type_resets_detail_and_parent() {
    let mut d = AccountDraft::new(AccountKind::Asset);
    assert_eq!(d.detail, "bank");
    d.parent = Some(id(2));
    d.set_kind(AccountKind::Asset);
    assert_eq!(d.parent, Some(id(2)), "same type: nothing changes");
    d.set_kind(AccountKind::Liability);
    assert_eq!(d.detail, "accounts_payable");
    assert_eq!(d.parent, None);
    assert!(holds_bank_or_card_number(AccountKind::Asset, Some("bank")));
    assert!(holds_bank_or_card_number(
        AccountKind::Liability,
        Some("credit_card")
    ));
    assert!(!holds_bank_or_card_number(
        AccountKind::Asset,
        Some("fixed_asset")
    ));
    assert!(!holds_bank_or_card_number(AccountKind::Expense, None));
}

#[test]
fn balances_are_carried_not_combined() {
    let mut entries = sample();
    entries[3].balance = Money::parse_wire("10.0000").ok();
    entries[4].balance = Money::parse_wire("2.5000").ok();
    let chart = Chart::new(entries, true);
    assert_eq!(
        chart.entry(id(4)).and_then(|e| e.balance),
        Money::parse_wire("10.0000").ok(),
        "the parent keeps its own balance"
    );
    assert_eq!(chart.len(), 8);
    assert!(!chart.is_empty());
    assert!(chart.numbers_shown());
    assert_eq!(chart.entry_at(0).map(|e| e.id), Some(id(6)));
    assert_eq!(chart.full_name(id(5)), Some("Materials:Plants"));
}

/// Review P16-part1 #3: `9 < 10 < 1a < 9` used to be a cycle, and Rust's sort
/// may panic on a comparator that is not a total order.
#[test]
fn number_cmp_sorts_mixed_numbers_without_a_cycle() {
    use std::cmp::Ordering::{Greater, Less};
    assert_eq!(number_cmp("9", "10"), Less);
    assert_eq!(
        number_cmp("10", "1a"),
        Greater,
        "1a has the smaller leading number"
    );
    assert_eq!(number_cmp("1a", "9"), Less);
    let mut numbers = vec![
        "9", "10", "1a", "1A", "100", "1000", "1000-A", "1000-b", "900", "010", "0", "00", "A-1",
        "a-2", "a-10", "B-1", "2000", "20", "2", "2.1", "2.10", "2.9", "x", "X", "1 0", "CC",
        "cc-5", "5cc", "50", "05", "",
    ];
    numbers.sort_by(|a, b| number_cmp(a, b));
    assert_eq!(
        numbers,
        [
            "", "0", "00", "1 0", "1A", "1a", "2", "2.1", "2.9", "2.10", "5cc", "05", "9", "10",
            "010", "20", "50", "100", "900", "1000", "1000-A", "1000-b", "2000", "A-1", "a-2",
            "a-10", "B-1", "CC", "cc-5", "X", "x",
        ]
    );
}

mod number_order {
    use super::number_cmp;
    use proptest::prelude::*;
    use std::cmp::Ordering;

    fn short() -> impl Strategy<Value = String> {
        "[0-9aA. -]{0,5}"
    }

    proptest! {
        #[test]
        fn antisymmetric_and_equal_only_for_same_text(a in short(), b in short()) {
            prop_assert_eq!(number_cmp(&a, &b), number_cmp(&b, &a).reverse());
            prop_assert_eq!(number_cmp(&a, &b) == Ordering::Equal, a == b);
        }

        #[test]
        fn transitive(a in short(), b in short(), c in short()) {
            let (ab, bc) = (number_cmp(&a, &b), number_cmp(&b, &c));
            if ab != Ordering::Greater && bc != Ordering::Greater {
                prop_assert_ne!(number_cmp(&a, &c), Ordering::Greater);
            }
            if ab == Ordering::Less && bc == Ordering::Less {
                prop_assert_eq!(number_cmp(&a, &c), Ordering::Less);
            }
        }

        #[test]
        fn sorting_never_panics(mut v in proptest::collection::vec(short(), 0..60)) {
            v.sort_by(|a, b| number_cmp(a, b));
            for w in v.windows(2) {
                prop_assert_ne!(number_cmp(&w[0], &w[1]), Ordering::Greater);
            }
        }
    }
}
