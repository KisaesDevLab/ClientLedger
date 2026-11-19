use super::*;
use proptest::prelude::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn d(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn account(id: &str, name: &str, ty: &str, detail: &str) -> SourceAccount {
    SourceAccount {
        id: id.into(),
        name: name.into(),
        number: None,
        parent_id: None,
        account_type: ty.into(),
        detail_type: Some(detail.into()),
        system_tag: None,
        description: None,
    }
}

fn contact(id: &str, name: &str, kind: ContactKind) -> SourceContact {
    SourceContact {
        id: id.into(),
        name: name.into(),
        kind,
    }
}

fn line(id: &str, account: &str, amount: &str) -> SourceLine {
    SourceLine {
        line_id: id.into(),
        account_id: account.into(),
        contact_id: None,
        amount: m(amount),
        memo: None,
    }
}

fn txn(id: &str, date: &str, ty: &str, contact: Option<&str>, lines: Vec<SourceLine>) -> SourceTxn {
    SourceTxn {
        id: id.into(),
        date: d(date),
        txn_type: ty.into(),
        number: Some(format!("N{id}")),
        memo: None,
        contact_id: contact.map(str::to_string),
        lines,
    }
}

fn books() -> Books {
    Books {
        accounts: vec![
            account("bank", "Checking", "asset", "bank"),
            SourceAccount {
                system_tag: Some("accounts_receivable".into()),
                ..account("ar", "Accounts Receivable", "asset", "accounts_receivable")
            },
            account("ap", "Accounts Payable", "liability", "accounts_payable"),
            account("inc", "Sales", "revenue", "service"),
            account("exp", "Supplies", "expense", "office_supplies"),
        ],
        contacts: vec![
            contact("c1", "Acme", ContactKind::Client),
            contact("v1", "Paper Co", ContactKind::Vendor),
            contact("b1", "Both Ways", ContactKind::Both),
        ],
        items: Vec::new(),
        txns: Vec::new(),
    }
}

fn text(p: &Plan) -> String {
    p.to_bytes().iter().map(|&b| char::from(b)).collect()
}

#[test]
fn every_engine_detail_type_maps_without_a_fallback() {
    for l in DETAIL_TYPES.lines().filter(|l| !l.trim().is_empty()) {
        let mut p = l.split_whitespace();
        let (ty, detail) = (p.next().expect("type"), p.next().expect("detail"));
        let (_, fell_back) = account_iif_type(ty, Some(detail), None).expect(l);
        assert!(!fell_back, "{l}");
    }
    assert_eq!(
        account_iif_type("asset", Some("tenant_custom"), None),
        Some((AccountKind::OtherCurrentAsset, true))
    );
    assert_eq!(
        account_iif_type("expense", Some("tenant_custom"), None),
        Some((AccountKind::Expense, true))
    );
    assert_eq!(
        account_iif_type(
            "asset",
            Some("other_current_asset"),
            Some("accounts_receivable")
        ),
        Some((AccountKind::Ar, false))
    );
    assert_eq!(account_iif_type("mystery", None, None), None);
    assert_eq!(
        account_iif_type("asset", Some("checking"), None),
        Some((AccountKind::Bank, false))
    );
}

/// The checked-in list matches the engine's `DETAIL_TYPES` (drift check;
/// skipped when the engine submodule isn't checked out).
#[test]
fn detail_types_match_the_engine() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../engine/packages/shared/src/types/accounts.ts");
    let Ok(src) = std::fs::read_to_string(&path) else {
        eprintln!("SKIPPED: engine sources absent");
        return;
    };
    let start = src
        .find("export const DETAIL_TYPES")
        .expect("DETAIL_TYPES in the engine");
    let block = &src[start..];
    let block = &block[..block.find("};").expect("end")];
    let mut engine = Vec::new();
    let mut ty = String::new();
    for l in block.lines().skip(1) {
        let l = l.trim();
        let values = match l.split_once(':') {
            Some((t, rest)) if !t.contains('\'') => {
                ty = t.trim().to_string();
                rest
            }
            _ => l,
        };
        for v in values.split('\'').skip(1).step_by(2) {
            engine.push(format!("{ty} {v}"));
        }
    }
    let ours: Vec<String> = DETAIL_TYPES
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    assert_eq!(ours, engine, "update fixtures/iif/detail_types.txt");
}

#[test]
fn cents_round_half_away_from_zero_and_never_print_minus_zero() {
    assert_eq!(cents(m("1.005")).to_string(), "1.01");
    assert_eq!(cents(m("-1.005")).to_string(), "-1.01");
    assert_eq!(cents(m("1.0049")).to_string(), "1.00");
    assert_eq!(amount_text(cents(m("-0.004"))), "0.00");
    assert_eq!(amount_text(Decimal::new(-0, 2)), "0.00");
    assert_eq!(amount_text(cents(m("12"))), "12.00");
    assert_eq!(amount_text(cents(m("-12.5"))), "-12.50");
}

#[test]
fn a_transaction_that_does_not_foot_at_cents_stops_everything_q_p32c() {
    let mut b = books();
    b.txns = vec![
        txn(
            "t1",
            "2026-01-05",
            "journal_entry",
            None,
            vec![line("1", "exp", "10.00"), line("2", "bank", "-10.00")],
        ),
        txn(
            "t2",
            "2026-01-06",
            "journal_entry",
            None,
            vec![
                line("3", "exp", "0.005"),
                line("4", "exp", "0.005"),
                line("5", "bank", "-0.01"),
            ],
        ),
    ];
    match plan(&b, Options::default()) {
        Err(IifError::Stopped(stops)) => {
            assert_eq!(stops.len(), 1);
            assert_eq!(stops[0].txn_id, "t2");
            assert_eq!(stops[0].number.as_deref(), Some("Nt2"));
            assert_eq!(
                stops[0].reason,
                StopReason::NotFooting {
                    off: Decimal::new(1, 2)
                }
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn ar_and_ap_lines_need_their_names() {
    let mut b = books();
    b.txns = vec![
        txn(
            "t1",
            "2026-01-05",
            "journal_entry",
            None,
            vec![line("1", "ar", "5.00"), line("2", "inc", "-5.00")],
        ),
        txn(
            "t2",
            "2026-01-05",
            "journal_entry",
            Some("c1"),
            vec![line("3", "exp", "5.00"), line("4", "ap", "-5.00")],
        ),
    ];
    match plan(&b, Options::default()) {
        Err(IifError::Stopped(stops)) => {
            assert_eq!(stops[0].reason, StopReason::ArWithoutClient);
            assert_eq!(
                stops[1].reason,
                StopReason::ApWithoutVendor,
                "a client on an A/P line"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_client_and_vendor_contact_is_written_twice_and_used_by_side() {
    let mut b = books();
    b.txns = vec![
        txn(
            "t1",
            "2026-01-05",
            "invoice",
            Some("b1"),
            vec![line("1", "ar", "5.00"), line("2", "inc", "-5.00")],
        ),
        txn(
            "t2",
            "2026-01-06",
            "bill",
            Some("b1"),
            vec![line("3", "exp", "7.00"), line("4", "ap", "-7.00")],
        ),
    ];
    let p = plan(&b, Options::default()).expect("plan");
    let t = text(&p);
    assert!(t.contains("CUST\tBoth Ways\r\n"), "{t}");
    assert!(t.contains("VEND\tBoth Ways (V)\r\n"), "{t}");
    assert!(
        t.contains("\tAccounts Receivable\tBoth Ways\t5.00\t"),
        "{t}"
    );
    assert!(t.contains("\tSales\tBoth Ways\t-5.00\t"), "{t}");
    assert!(
        t.contains("\tAccounts Payable\tBoth Ways (V)\t-7.00\t"),
        "{t}"
    );
    assert!(t.contains("\tSupplies\tBoth Ways (V)\t7.00\t"), "{t}");
}

#[test]
fn future_dated_entries_are_written_and_blocks_foot() {
    let mut b = books();
    b.txns = vec![txn(
        "t1",
        "2031-12-31",
        "journal_entry",
        None,
        vec![
            line("1", "exp", "2.50"),
            line("2", "exp", "2.5049"),
            line("3", "exp", "4.9951"),
            line("4", "bank", "-10.00"),
            line("5", "exp", "0.0001"),
            line("6", "exp", "-0.0001"),
        ],
    )];
    let p = plan(&b, Options::default()).expect("plan");
    assert_eq!(p.counts.transactions, 1);
    assert_eq!(p.counts.lines, 4, "lines rounding to 0.00 are left out");
    let t = text(&p);
    assert!(
        t.contains("TRNS\t\tGENERAL JOURNAL\t12/31/2031\tSupplies\t\t2.50\tNt1\t\r\n"),
        "{t}"
    );
    assert!(
        t.contains("SPL\t\tGENERAL JOURNAL\t12/31/2031\tChecking\t\t-10.00\tNt1\t\r\nENDTRNS\r\n"),
        "{t}"
    );
}

#[test]
fn hostile_names_are_cleaned_and_kept_unique() {
    let mut b = books();
    b.accounts.push(SourceAccount {
        parent_id: Some("exp".into()),
        ..account("sub", "Paper:\tand \"ink\"", "expense", "office_supplies")
    });
    b.contacts = vec![
        contact("c1", "Smith\r\nCo", ContactKind::Client),
        contact("v1", "SMITH CO", ContactKind::Vendor),
        contact("c2", "Caf\u{e9} \u{1F600}", ContactKind::Client),
        contact("c3", "Caf\u{e9} \u{3A9}", ContactKind::Client),
        contact("c4", "N\u{0085}x", ContactKind::Client),
        contact("c5", &"L".repeat(60), ContactKind::Client),
    ];
    let p = plan(&b, Options::default()).expect("plan");
    let t = text(&p);
    assert!(t.contains("ACCNT\tSupplies:Paper- and  ink\tEXP"), "{t}");
    assert!(t.contains("CUST\tSmith  Co\r\n"), "{t}");
    assert!(
        t.contains("VEND\tSMITH CO\r\n"),
        "different after the cut: {t}"
    );
    assert!(t.contains("CUST\tCaf\u{e9} ?\r\n"), "{t}");
    assert!(
        t.contains("CUST\tCaf\u{e9} ? 2\r\n"),
        "collide after `?`: {t}"
    );
    assert!(t.contains("CUST\tN?x\r\n"), "{t}");
    assert!(t.contains(&format!("CUST\t{}\r\n", "L".repeat(41))), "{t}");
    assert!(
        p.limitations.contains(&Limitation::Unmappable(3)),
        "{:?}",
        p.limitations
    );
    assert!(
        p.limitations.contains(&Limitation::Renamed(1)),
        "{:?}",
        p.limitations
    );
    for l in t.split("\r\n") {
        assert!(!l.contains('"') && !l.contains('\n') && !l.contains('\r'));
    }
}

#[test]
fn same_name_vendor_and_client_share_one_list() {
    let mut b = books();
    b.contacts = vec![
        contact("c1", "Acme", ContactKind::Client),
        contact("v1", "acme", ContactKind::Vendor),
    ];
    let t = text(&plan(&b, Options::default()).expect("plan"));
    assert!(t.contains("CUST\tAcme\r\n"));
    assert!(t.contains("VEND\tacme 2\r\n"), "{t}");
}

#[test]
fn lines_are_grouped_by_transaction_not_by_runs() {
    let head = |id: &str| txn(id, "2026-01-05", "journal_entry", None, Vec::new());
    let mut c = TxnCollector::default();
    // Two entries with the same date and time come back interleaved.
    c.add_window(vec![
        (head("a"), line("1", "exp", "1.00")),
        (head("b"), line("2", "exp", "2.00")),
        (head("a"), line("3", "bank", "-1.00")),
        (head("b"), line("4", "bank", "-2.00")),
    ])
    .expect("window");
    let txns = c.finish();
    assert_eq!(txns.len(), 2);
    assert_eq!(txns[0].lines.len(), 2);
    assert_eq!(txns[0].lines[1].line_id, "3");
    let mut b = books();
    b.txns = txns;
    assert!(plan(&b, Options::default()).is_ok());

    let mut c = TxnCollector::default();
    c.add_window(vec![(head("a"), line("1", "exp", "1.00"))])
        .expect("window");
    assert!(matches!(
        c.add_window(vec![(head("a"), line("9", "exp", "1.00"))]),
        Err(IifError::Duplicate)
    ));
    let mut c = TxnCollector::default();
    assert!(matches!(
        c.add_window(vec![
            (head("a"), line("1", "exp", "1.00")),
            (head("b"), line("1", "exp", "1.00")),
        ]),
        Err(IifError::Duplicate)
    ));
}

#[test]
fn a_line_on_an_unknown_account_or_contact_aborts() {
    let mut b = books();
    b.txns = vec![txn(
        "t",
        "2026-01-05",
        "journal_entry",
        None,
        vec![line("1", "gone", "1.00"), line("2", "bank", "-1.00")],
    )];
    assert!(matches!(
        plan(&b, Options::default()),
        Err(IifError::Inconsistent { what: "account" })
    ));
    b.txns[0].lines[0].account_id = "exp".into();
    b.txns[0].contact_id = Some("nobody".into());
    assert!(matches!(
        plan(&b, Options::default()),
        Err(IifError::Inconsistent { what: "contact" })
    ));
}

#[test]
fn lists_only_or_transactions_only() {
    let mut b = books();
    b.txns = vec![txn(
        "t",
        "2026-01-05",
        "journal_entry",
        None,
        vec![line("1", "exp", "1.00"), line("2", "bank", "-1.00")],
    )];
    let lists = plan(
        &b,
        Options {
            include_lists: true,
            include_transactions: false,
        },
    )
    .expect("plan");
    let t = text(&lists);
    assert!(t.contains("!ACCNT") && !t.contains("!TRNS"));
    let txns = plan(
        &b,
        Options {
            include_lists: false,
            include_transactions: true,
        },
    )
    .expect("plan");
    let t = text(&txns);
    assert!(!t.contains("!ACCNT") && t.contains("!TRNS"));
    assert_eq!(txns.counts.accounts, 0);
}

#[test]
fn items_follow_their_kinds() {
    let mut b = books();
    let item = |id: &str, ty: &str, income: Option<&str>| SourceItem {
        id: id.into(),
        name: format!("Item {id}"),
        item_type: ty.into(),
        description: None,
        income_account_id: income.map(str::to_string),
    };
    b.items = vec![
        item("1", "service", Some("inc")),
        item("2", "inventory_part", Some("inc")),
        item("3", "discount", Some("inc")),
        item("4", "non_inventory", None),
        item("5", "other_charge", Some("inc")),
    ];
    let p = plan(&b, Options::default()).expect("plan");
    let t = text(&p);
    assert!(t.contains("INVITEM\tItem 1\tSERV\t\tSales\r\n"), "{t}");
    assert!(t.contains("INVITEM\tItem 2\tPART\t\tSales\r\n"), "{t}");
    assert!(t.contains("INVITEM\tItem 5\tOTHC\t\tSales\r\n"), "{t}");
    assert_eq!(p.counts.items, 3);
    assert!(p.limitations.contains(&Limitation::ItemsLeftOut(2)));
    assert!(
        p.limitations
            .contains(&Limitation::InventoryAsNonInventory(1))
    );
}

/// T9 (pure part): on 2-decimal books the per-account sums of the file's
/// lines equal the books' own per-account sums.
#[test]
fn per_account_sums_tie_to_the_books_on_two_decimal_data() {
    let mut b = books();
    b.txns = (0..40)
        .map(|i| {
            let amt = format!("{}.{:02}", i * 3 + 1, i % 100);
            txn(
                &format!("t{i:02}"),
                &format!("2026-{:02}-10", i % 12 + 1),
                if i % 2 == 0 { "invoice" } else { "bill" },
                Some(if i % 2 == 0 { "c1" } else { "v1" }),
                if i % 2 == 0 {
                    vec![
                        line(&format!("{i}a"), "ar", &amt),
                        line(&format!("{i}b"), "inc", &format!("-{amt}")),
                    ]
                } else {
                    vec![
                        line(&format!("{i}a"), "exp", &amt),
                        line(&format!("{i}b"), "ap", &format!("-{amt}")),
                    ]
                },
            )
        })
        .collect();
    let p = plan(&b, Options::default()).expect("plan");
    let mut file: BTreeMap<String, Decimal> = BTreeMap::new();
    for t in &p.txns {
        for l in &t.lines {
            *file.entry(l.account.clone()).or_default() += l.amount;
        }
    }
    let names: BTreeMap<&str, &str> = [
        ("ar", "Accounts Receivable"),
        ("ap", "Accounts Payable"),
        ("inc", "Sales"),
        ("exp", "Supplies"),
    ]
    .into();
    let mut books_sum: BTreeMap<String, Decimal> = BTreeMap::new();
    for t in &b.txns {
        for l in &t.lines {
            *books_sum
                .entry(names[l.account_id.as_str()].to_string())
                .or_default() += l.amount.as_decimal();
        }
    }
    assert_eq!(file, books_sum);
}

fn arb_txn() -> impl Strategy<Value = Vec<i64>> {
    // 1–4 lines of up to ±1,000.0000 in ten-thousandths; a balancing line
    // is added, so every transaction sums to exactly zero at 4 decimals.
    proptest::collection::vec(-10_000_000i64..10_000_000, 1..4)
}

proptest! {
    #[test]
    fn stops_are_exactly_the_transactions_that_do_not_foot(txns in proptest::collection::vec(arb_txn(), 1..12)) {
        let mut b = books();
        let mut expect_stop = BTreeSet::new();
        for (i, units) in txns.iter().enumerate() {
            let mut amounts: Vec<Money> = units
                .iter()
                .map(|u| Money::from_ten_thousandths(i128::from(*u)).expect("money"))
                .collect();
            let total: i128 = units.iter().map(|u| i128::from(*u)).sum();
            amounts.push(Money::from_ten_thousandths(-total).expect("money"));
            let rounded: Decimal = amounts.iter().map(|a| cents(*a)).sum();
            let id = format!("t{i:02}");
            if !rounded.is_zero() {
                expect_stop.insert(id.clone());
            }
            let lines = amounts
                .iter()
                .enumerate()
                .map(|(j, a)| SourceLine {
                    line_id: format!("{i}-{j}"),
                    account_id: if j % 2 == 0 { "exp".into() } else { "bank".into() },
                    contact_id: None,
                    amount: *a,
                    memo: None,
                })
                .collect();
            b.txns.push(txn(&id, "2026-03-01", "journal_entry", None, lines));
        }
        match plan(&b, Options::default()) {
            Ok(p) => {
                prop_assert!(expect_stop.is_empty());
                for t in &p.txns {
                    let s: Decimal = t.lines.iter().map(|l| l.amount).sum();
                    prop_assert!(s.is_zero());
                }
            }
            Err(IifError::Stopped(stops)) => {
                let got: BTreeSet<String> = stops.into_iter().map(|s| s.txn_id).collect();
                prop_assert_eq!(got, expect_stop);
            }
            Err(e) => prop_assert!(false, "{e:?}"),
        }
    }
}
