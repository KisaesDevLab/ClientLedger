//! Checks, deposits, transfers and card charges against the real engine
//! (P12 WP2): each kind created, read back exactly (strings), updated
//! through the fingerprint guard (a stale version is refused with nothing
//! sent), voided; and a lost reply on each create reconciled to exactly one
//! posting, including two identical checks on the same day.

use cl_api::{
    AccountFields, AccountType, ApiError, BankPosting, CardChargeDraft, CheckDraft, ConflictReason,
    DepositDraft, EditableFields, Method, PostingOptions, SplitLine, TransactionFilter,
    TransferDraft, TxnStatus, TxnType,
};
use cl_core::CalendarDate;
use cl_core::ids::AccountId;

use crate::common::{account, date, key, m, wire_lines};
use crate::support::{Harness, unique};

pub fn detail_account(h: &Harness, s: &cl_api::Session, t: AccountType, detail: &str) -> AccountId {
    let fields = AccountFields {
        name: unique(detail),
        account_number: None,
        account_type: t,
        detail_type: Some(detail.into()),
        description: None,
        parent_id: None,
    };
    h.run(s.accounts().create(&fields, key()))
        .unwrap_or_else(|e| panic!("create {detail}: {e}"))
        .record
        .id
}

fn split(account: AccountId, amount: &str, memo: Option<&str>) -> SplitLine {
    SplitLine {
        account,
        amount: m(amount),
        memo: memo.map(str::to_string),
        contact: None,
        billable: false,
        tag: None,
    }
}

pub struct Accounts {
    pub bank: AccountId,
    pub savings: AccountId,
    pub card: AccountId,
    pub supplies: AccountId,
    pub fuel: AccountId,
    pub sales: AccountId,
}

pub fn accounts(h: &Harness, s: &cl_api::Session) -> Accounts {
    Accounts {
        bank: detail_account(h, s, AccountType::Asset, "bank"),
        savings: detail_account(h, s, AccountType::Asset, "bank"),
        card: detail_account(h, s, AccountType::Liability, "credit_card"),
        supplies: account(h, s, AccountType::Expense, "Postings supplies").id,
        fuel: account(h, s, AccountType::Expense, "Postings fuel").id,
        sales: account(h, s, AccountType::Revenue, "Postings sales").id,
    }
}

pub fn check(a: &Accounts, on: CalendarDate, memo: &str, amounts: (&str, &str)) -> BankPosting {
    let total = m(amounts.0).checked_add(m(amounts.1)).expect("total");
    BankPosting::check(CheckDraft {
        bank: a.bank,
        date: on,
        payee: None,
        payee_name: "Sample Payee".into(),
        check_number: None,
        print_later: false,
        payee_address: None,
        memo: Some(memo.to_string()),
        printed_memo: None,
        tag_ids: Vec::new(),
        draft_attachment: None,
        amount: total,
        lines: vec![
            split(a.supplies, amounts.0, Some("Paper")),
            split(a.fuel, amounts.1, None),
        ],
    })
    .expect("check")
}

fn kinds(a: &Accounts, on: CalendarDate, memo: &str) -> Vec<BankPosting> {
    vec![
        check(a, on, memo, ("1000.0012", "84.6600")),
        BankPosting::deposit(DepositDraft::plain(
            a.bank,
            on,
            Some(memo.to_string()),
            vec![
                split(a.sales, "1100.0000", Some("Invoice 7")),
                split(a.sales, "0.0001", None),
            ],
        ))
        .expect("deposit"),
        BankPosting::transfer(TransferDraft {
            from: a.bank,
            to: a.savings,
            date: on,
            amount: m("250.5000"),
            memo: Some(memo.to_string()),
        })
        .expect("transfer"),
        BankPosting::card_charge(CardChargeDraft {
            pay_from: a.card,
            date: on,
            number: Some("R-77".into()),
            payee: None,
            memo: Some(memo.to_string()),
            lines: vec![
                split(a.fuel, "40.0000", None),
                split(a.supplies, "2.5000", Some("Snacks")),
            ],
        })
        .expect("card"),
    ]
}

pub fn each_kind_round_trip_update_and_void(h: &Harness) {
    let s = &h.s1;
    let a = accounts(h, s);
    let memo = unique("bank posting");
    let on = date(2026, 5, 11);
    for p in kinds(&a, on, &memo) {
        let made = h
            .run(
                s.transactions()
                    .create_bank_posting(&p, &PostingOptions::default(), key()),
            )
            .unwrap_or_else(|e| panic!("{:?}: {e}", p.kind()));
        let got = h.run(s.transactions().get(made.record.id)).expect("get");
        assert_eq!(got.record.status, TxnStatus::Posted);
        assert_eq!(
            got.record.fingerprint(),
            p.created_fingerprint(),
            "{:?}: the stored record is what was sent",
            p.kind()
        );
        assert_eq!(got.version, made.version, "{:?}", p.kind());
        let expected_type = match p.kind() {
            cl_api::BankPostingKind::Deposit => TxnType::Deposit,
            cl_api::BankPostingKind::Transfer => TxnType::Transfer,
            _ => TxnType::Expense,
        };
        assert_eq!(got.record.txn_type, expected_type);
        // Exact strings, never rounded.
        let lines = wire_lines(&got.record);
        assert!(
            lines
                .iter()
                .all(|(d, c)| d.len() > 5 && c.len() > 5 && d.contains('.')),
            "{lines:?}"
        );
        if p.kind() == cl_api::BankPostingKind::Check {
            assert!(got.record.check_number.is_some(), "a check has a number");
            assert!(lines.contains(&("1000.0012".into(), "0.0000".into())));
            assert!(lines.contains(&("0.0000".into(), "1084.6612".into())));
        }

        // Update through the guard: a new date.
        let moved = match p.kind() {
            cl_api::BankPostingKind::Check => {
                check(&a, date(2026, 5, 12), &memo, ("1000.0012", "84.6600"))
            }
            _ => kinds(&a, date(2026, 5, 12), &memo)
                .into_iter()
                .find(|k| k.kind() == p.kind())
                .expect("kind"),
        };
        let upd = h
            .run(s.transactions().update_bank_posting(
                made.record.id,
                &got.version,
                &moved,
                &PostingOptions::default(),
                key(),
            ))
            .unwrap_or_else(|e| panic!("update {:?}: {e}", p.kind()));
        assert_eq!(upd.record.txn_date, date(2026, 5, 12));
        let after = h.run(s.transactions().get(made.record.id)).expect("get");
        assert_eq!(
            after.record.fingerprint(),
            moved.updated_fingerprint(),
            "{:?}: updated record",
            p.kind()
        );

        // The old version is stale now: refused, nothing sent.
        let puts = h.observer.sent(Method::Put, "/api/v1/transactions/{id}");
        let e = h
            .run(s.transactions().update_bank_posting(
                made.record.id,
                &got.version,
                &p,
                &PostingOptions::default(),
                key(),
            ))
            .expect_err("stale");
        assert!(
            matches!(
                e,
                ApiError::Conflict {
                    reason: ConflictReason::Stale,
                    ..
                }
            ),
            "{e:?}"
        );
        assert_eq!(
            h.observer.sent(Method::Put, "/api/v1/transactions/{id}"),
            puts,
            "a stale update is never sent"
        );

        let voided = h
            .run(s.transactions().void(
                made.record.id,
                "entered by mistake",
                &PostingOptions::default(),
                key(),
            ))
            .expect("void");
        assert_eq!(voided.record.status, TxnStatus::Void);
    }
}

fn count(h: &Harness, kind: TxnType, on: CalendarDate, memo: &str) -> usize {
    let filter = TransactionFilter {
        txn_type: Some(kind),
        start_date: Some(on),
        end_date: Some(on),
        search: Some(memo.to_string()),
        ..TransactionFilter::default()
    };
    h.run(
        h.s1.transactions()
            .list(&filter, cl_api::PageRequest::first_max()),
    )
    .expect("list")
    .data
    .len()
}

pub fn lost_replies_are_reconciled_to_one_posting(h: &Harness) {
    let s = &h.s1;
    let a = accounts(h, s);
    let memo = unique("lost bank reply");
    let on = date(2026, 5, 20);
    for p in kinds(&a, on, &memo) {
        let route = if p.kind() == cl_api::BankPostingKind::Check {
            "/api/v1/checks"
        } else {
            "/api/v1/transactions"
        };
        let sent = h.observer.sent(Method::Post, route);
        let dropped = h.observer.dropped();
        h.observer.drop_next_reply(Method::Post, route);
        let made = h
            .run(
                s.transactions()
                    .create_bank_posting(&p, &PostingOptions::default(), key()),
            )
            .unwrap_or_else(|e| panic!("{:?} reconciled: {e}", p.kind()));
        assert_eq!(
            h.observer.dropped(),
            dropped + 1,
            "the reply was not dropped"
        );
        assert_eq!(
            h.observer.sent(Method::Post, route) - sent,
            1,
            "{:?}: the create must not be re-sent",
            p.kind()
        );
        assert_eq!(made.record.fingerprint(), p.created_fingerprint());
    }
    assert_eq!(
        count(h, TxnType::Expense, on, &memo),
        2,
        "one check, one charge"
    );
    assert_eq!(count(h, TxnType::Deposit, on, &memo), 1);
    assert_eq!(count(h, TxnType::Transfer, on, &memo), 1);

    // A second identical check on the same day, its reply lost too: the
    // probe must not adopt the first one (ours), and must find this one.
    let twin = check(&a, on, &memo, ("1000.0012", "84.6600"));
    let first = h
        .run(
            s.transactions()
                .create_bank_posting(&twin, &PostingOptions::default(), key()),
        )
        .expect("first twin");
    h.observer.drop_next_reply(Method::Post, "/api/v1/checks");
    let second = h
        .run(
            s.transactions()
                .create_bank_posting(&twin, &PostingOptions::default(), key()),
        )
        .expect("second twin reconciled");
    assert_ne!(
        first.record.id, second.record.id,
        "the earlier check was adopted"
    );
    assert_eq!(count(h, TxnType::Expense, on, &memo), 4);
}
