//! Templates against the real engine (P15 lane T, U-62 `templates-v1`).
//! PENDING until the engine pin carries the capability.
//!
//! - `draft_round_trip`: per kind, a form's checked draft stored as a
//!   template comes back from `GET /{id}/draft` as the same create body
//!   (amounts exact, no date or numbers), and storing it posted nothing.
//! - `grouped_deposit_refused`: a raw draft with `sourceTransactionId` is
//!   refused with `TEMPLATE_DEPOSIT_GROUPING`.
//! - `stale_on_replace`: a change naming an old version is `Conflict
//!   { Stale }`; the template is unchanged.

use cl_api::{
    AccountType, ApiError, BankPosting, Basis, Capabilities, CardChargeDraft, CheckDraft,
    ConflictReason, DepositDraft, JeLine, Method, NewJournalEntry, NewTemplate, Side, SplitLine,
    TemplateChange, TemplateDraft, TransferDraft,
};
use cl_core::ids::AccountId;
use serde_json::json;

use crate::common::{account, date, m};
use crate::support::{Harness, unique};

fn split(account: AccountId, amount: &str) -> SplitLine {
    SplitLine {
        account,
        amount: m(amount),
        memo: Some("synthetic".into()),
        contact: None,
        billable: false,
        tag: None,
    }
}

fn new(draft: TemplateDraft) -> NewTemplate {
    NewTemplate {
        name: unique("Template"),
        group: None,
        client_form: None,
        draft,
        schedule: None,
    }
}

pub fn draft_round_trip(h: &Harness) {
    if !h.require_cap(Capabilities::TEMPLATES, "U-62 templates-v1") {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "Template bank").id;
    let card = account(h, s, AccountType::Liability, "Template card").id;
    let rent = account(h, s, AccountType::Expense, "Template rent").id;
    let income = account(h, s, AccountType::Revenue, "Template income").id;
    let on = date(2026, 11, 4);
    let before = h.run(s.accounts().get(bank)).expect("bank").record.balance;
    let drafts = [
        TemplateDraft::Check(CheckDraft {
            bank,
            date: on,
            payee: None,
            payee_name: "Synthetic Landlord".into(),
            check_number: None,
            print_later: true,
            payee_address: None,
            memo: None,
            printed_memo: None,
            tag_ids: Vec::new(),
            draft_attachment: None,
            amount: m("1200.00"),
            lines: vec![split(rent, "1200.00")],
        }),
        TemplateDraft::CardCharge(CardChargeDraft {
            pay_from: card,
            date: on,
            number: None,
            payee: None,
            memo: None,
            lines: vec![split(rent, "49.00")],
        }),
        TemplateDraft::Deposit(DepositDraft::plain(
            bank,
            on,
            None,
            vec![split(income, "500.1234")],
        )),
        TemplateDraft::Transfer(TransferDraft {
            from: bank,
            to: card,
            date: on,
            amount: m("250.00"),
            memo: None,
        }),
        TemplateDraft::Journal(
            NewJournalEntry::new(
                on,
                None,
                Basis::Both,
                vec![
                    JeLine {
                        account: rent,
                        side: Side::Debit(m("412.50")),
                        description: None,
                        tag: None,
                    },
                    JeLine {
                        account: bank,
                        side: Side::Credit(m("412.50")),
                        description: None,
                        tag: None,
                    },
                ],
            )
            .expect("balanced"),
        ),
    ];
    let caps = s.client().capabilities();
    for d in drafts {
        let made = h
            .run(s.templates().create(&new(d.clone())))
            .unwrap_or_else(|e| panic!("create {:?}: {e}", d.kind()));
        let used = h
            .run(s.templates().draft(made.id, on))
            .unwrap_or_else(|e| panic!("draft {:?}: {e}", d.kind()));
        assert!(used.warnings.is_empty(), "{:?}", used.warnings);
        assert_eq!(
            used.draft.to_value(&caps).expect("body"),
            d.to_value(&caps).expect("body"),
            "{:?}: the same create body, exactly",
            d.kind()
        );
        h.run(s.templates().delete(made.id, made.version))
            .expect("delete");
    }
    // Storing templates posts nothing.
    assert_eq!(
        h.run(s.accounts().get(bank)).expect("bank").record.balance,
        before
    );
    // A builder-valid draft still posts as the form would.
    assert!(
        BankPosting::transfer(TransferDraft {
            from: bank,
            to: card,
            date: on,
            amount: m("1"),
            memo: None,
        })
        .is_ok()
    );
}

pub fn grouped_deposit_refused(h: &Harness) {
    if !h.require_cap(Capabilities::TEMPLATES, "U-62 templates-v1") {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "Grouped bank").id;
    let other = account(h, s, AccountType::Revenue, "Grouped income").id;
    let e = h
        .run(s.raw_mapped(
            Method::Post,
            "/api/v1/templates",
            Some(&json!({
                "name": unique("Grouped"),
                "kind": "deposit",
                "draft": {
                    "depositToAccountId": bank,
                    "lines": [{
                        "accountId": other,
                        "amount": "10.0000",
                        "sourceTransactionId": "00000000-0000-4000-8000-000000000001",
                    }],
                },
            })),
        ))
        .expect_err("grouped");
    match e {
        ApiError::Validation(v) => assert_eq!(
            v.code().map(|c| c.as_str().to_string()),
            Some("TEMPLATE_DEPOSIT_GROUPING".into())
        ),
        other => panic!("expected TEMPLATE_DEPOSIT_GROUPING, got {other:?}"),
    }
}

pub fn stale_on_replace(h: &Harness) {
    if !h.require_cap(Capabilities::TEMPLATES, "U-62 templates-v1") {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "Stale bank").id;
    let card = account(h, s, AccountType::Liability, "Stale card").id;
    let made = h
        .run(
            s.templates()
                .create(&new(TemplateDraft::Transfer(TransferDraft {
                    from: bank,
                    to: card,
                    date: date(2026, 11, 4),
                    amount: m("10.00"),
                    memo: None,
                }))),
        )
        .expect("made");
    let renamed = h
        .run(s.templates().update(
            made.id,
            made.version,
            &TemplateChange {
                name: Some(unique("Renamed")),
                ..TemplateChange::default()
            },
        ))
        .expect("renamed");
    assert!(renamed.version > made.version);
    let e = h
        .run(s.templates().update(
            made.id,
            made.version,
            &TemplateChange {
                name: Some(unique("Old window")),
                ..TemplateChange::default()
            },
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
    let now = h.run(s.templates().get(made.id)).expect("get");
    assert_eq!(now.name, renamed.name);
    h.run(s.templates().delete(made.id, now.version))
        .expect("delete");
}
