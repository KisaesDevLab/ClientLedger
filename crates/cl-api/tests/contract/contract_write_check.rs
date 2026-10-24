//! Write Check against the real engine (P13 WP4, plan §6.2).
//!
//! - `gl_effect`: the mockup check 4418 = 842.15 + 444.25 posts exactly a
//!   bank credit of 1,286.40 and two debits, moves the three balances by
//!   exactly those amounts, shows as one register row and one checks-list
//!   row, and advances the engine's next number past it, the first line
//!   billable to its customer (`line-billable-v1`; an older engine runs
//!   the same check without a line customer).
//! - `edges`: a 0.01 mismatch refused locally with nothing sent (the
//!   engine's float tolerance is never relied on), a duplicate number 409,
//!   Print later unnumbered and leaving the counter alone, a lost create
//!   reply reconciled to one check, and the check edit (`check-edit-v1`;
//!   refused locally with nothing sent on an older engine).
//! - `printed_check_edits`: printed checks edited as the screen does
//!   (`CHECK_PRINTED`, Keep number, Reprint, header tags replaced).
//! - `closed_period`: a check dated on or before the closing date asks for
//!   the closing-date password; a wrong one is refused, the right one saves
//!   and is audited (on the second company, cleaned up afterwards).

use cl_api::{
    AccountType, ApiError, AuditAction, AuditEntity, BankPosting, Capabilities, CheckDraft,
    CheckEditOptions, CheckRecord, ClosedPeriodAuthorization, ConflictReason, ContactFields,
    ContactType, EditableFields, LocalValidation, Method, NewContact, PageRequest, PostingOptions,
    PrintStatus, PrintedCheckChange, RecordVersion, RegisterQuery, Session, SplitLine, TagFields,
    TxnType, is_check_printed,
};
use cl_core::ids::{AccountId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use crate::common::{account, date, key, m, wire_lines};
use crate::contract_bank_postings::detail_account;
use crate::support::{Harness, unique};

struct Books {
    bank: AccountId,
    plants: AccountId,
    mulch: AccountId,
    vendor: ContactId,
    job: ContactId,
}

fn contact(h: &Harness, s: &Session, t: ContactType, name: &str) -> ContactId {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(t, unique(name)),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("create contact: {e}"))
    .record
    .id
}

fn books(h: &Harness, s: &Session) -> Books {
    Books {
        bank: detail_account(h, s, AccountType::Asset, "bank"),
        plants: account(h, s, AccountType::Expense, "Materials:Plants").id,
        mulch: account(h, s, AccountType::Expense, "Materials:Mulch & stone").id,
        vendor: contact(h, s, ContactType::Vendor, "Feed & Seed"),
        job: contact(h, s, ContactType::Customer, "Family Trust:Fall bed install"),
    }
}

fn split(account: AccountId, amount: &str) -> SplitLine {
    SplitLine {
        account,
        amount: m(amount),
        memo: None,
        contact: None,
        billable: false,
        tag: None,
    }
}

/// The mockup check; the first line is billable to the job when the engine
/// takes line customers.
fn story(b: &Books, on: CalendarDate, number: Option<u32>, billable: bool) -> CheckDraft {
    let memo = unique("Acct 7712 · perennials and mulch");
    CheckDraft {
        bank: b.bank,
        date: on,
        payee: Some(b.vendor),
        payee_name: "Feed & Seed".into(),
        check_number: number,
        print_later: false,
        payee_address: Some("1200 Commerce Dr\nAurora, MO 65605".into()),
        memo: Some(memo.clone()),
        printed_memo: Some(memo),
        tag_ids: Vec::new(),
        draft_attachment: None,
        amount: m("1286.40"),
        lines: vec![
            SplitLine {
                contact: billable.then_some(b.job),
                billable,
                ..split(b.plants, "842.15")
            },
            split(b.mulch, "444.25"),
        ],
    }
}

fn balance(h: &Harness, s: &Session, a: AccountId) -> Money {
    h.run(s.accounts().get(a))
        .expect("account")
        .record
        .balance
        .unwrap_or(Money::ZERO)
}

fn line_billable(h: &Harness) -> bool {
    h.require_cap(Capabilities::LINE_BILLABLE, "line-billable-v1")
}

/// AC1: the GL effect of the mockup check, exactly.
pub fn gl_effect(h: &Harness) {
    let s = &h.s1;
    let b = books(h, s);
    let billable = line_billable(h);
    let before = [b.bank, b.plants, b.mulch].map(|a| balance(h, s, a));
    let on = date(2026, 11, 4);
    let draft = story(&b, on, Some(4418), billable);
    let p = BankPosting::check_with(draft, billable).expect("check");
    let made = h
        .run(
            s.transactions()
                .create_bank_posting(&p, &PostingOptions::default(), key()),
        )
        .unwrap_or_else(|e| panic!("write check: {e}"));

    // The ledger record: two debits, the bank credit last, exact strings.
    let got = h.run(s.transactions().get(made.record.id)).expect("get");
    assert_eq!(got.record.txn_type, TxnType::Expense);
    assert_eq!(got.record.check_number, Some(4418));
    assert_eq!(
        got.record.total.map(Money::to_wire).as_deref(),
        Some("1286.4000")
    );
    let accounts: Vec<AccountId> = {
        let mut l: Vec<_> = got.record.lines.iter().collect();
        l.sort_by_key(|l| l.line_order.unwrap_or(0));
        l.iter().map(|l| l.account_id).collect()
    };
    assert_eq!(accounts, vec![b.plants, b.mulch, b.bank]);
    assert_eq!(
        wire_lines(&got.record),
        [
            ("842.1500", "0.0000"),
            ("444.2500", "0.0000"),
            ("0.0000", "1286.4000"),
        ]
        .map(|(d, c)| (d.to_string(), c.to_string()))
    );
    assert_eq!(
        got.record.fingerprint(),
        p.created_fingerprint(),
        "the stored check is what was sent"
    );
    assert_eq!(got.version, made.version);
    if billable {
        let first = &got.record.lines[0];
        assert_eq!(first.contact_id, Some(b.job));
        assert!(first.is_billable);
        assert_eq!(first.billed_status, Some(cl_api::BilledStatus::Unbilled));
        assert!(!got.record.lines[2].is_billable, "the bank line");
    }

    // The check fields, read as a check.
    let check = h.run(s.checks().get(made.record.id)).expect("check");
    assert_eq!(check.record.check_number, Some(4418));
    assert_eq!(check.record.print_status, PrintStatus::HandWritten);
    assert_eq!(check.record.payee_name, "Feed & Seed");
    assert_eq!(
        check.record.payee_address.as_deref(),
        Some("1200 Commerce Dr\nAurora, MO 65605")
    );
    assert_eq!(check.record.printed_memo, check.record.txn.memo);

    // Balances move by exactly the check (debit-positive: the bank down).
    let after = [b.bank, b.plants, b.mulch].map(|a| balance(h, s, a));
    let moved: Vec<String> = after
        .iter()
        .zip(before)
        .map(|(a, b)| a.checked_sub(b).expect("difference").to_wire())
        .collect();
    assert_eq!(moved, ["-1286.4000", "842.1500", "444.2500"]);

    // One register row: check 4418, payment 1,286.40.
    let q = RegisterQuery::new(date(1900, 1, 1), date(2199, 12, 31));
    let page = h
        .run(s.register().page(b.bank, &q, 1, 200))
        .expect("register");
    let rows: Vec<_> = page
        .lines
        .iter()
        .filter(|l| l.txn_id == made.record.id)
        .collect();
    assert_eq!(rows.len(), 1, "one register row");
    assert_eq!(rows[0].check_number, Some(4418));
    assert_eq!(
        rows[0].payment.as_ref().and_then(|a| a.display(false)),
        Some("1286.40".into())
    );
    assert!(rows[0].deposit.is_none());

    // The checks list: one row, the engine's amount as a decimal string.
    let listed = h
        .run(s.checks().list(
            Some(b.bank),
            None,
            None,
            PageRequest::new(50, 0).expect("page"),
        ))
        .expect("checks");
    let row = listed
        .iter()
        .find(|r| r.id == made.record.id)
        .expect("listed");
    assert_eq!(row.number, Some(4418));
    assert_eq!(row.date, Some(on), "the list date, nearest day");
    assert_eq!(
        row.amount.as_ref().and_then(cl_api::RegisterAmount::exact),
        Some(m("1286.40"))
    );
    let raw = h
        .run(s.raw(
            Method::Get,
            "/api/v1/checks",
            &[("bank_account_id", &b.bank.to_string())],
            None,
        ))
        .expect("raw checks");
    assert!(raw.body["data"][0]["amount"].is_string(), "{}", raw.body);

    // A typed number moves the engine's counter past it.
    let settings = h.run(s.checks().settings()).expect("settings");
    assert_eq!(settings.next_for(b.bank), Some(4419));
}

/// Edge cases: local mismatch, duplicate number, Print later, lost reply,
/// the check edit.
pub fn edges(h: &Harness) {
    let s = &h.s1;
    let b = books(h, s);
    let on = date(2026, 11, 5);
    let opts = PostingOptions::default();

    // AC2: one cent off is refused before anything is sent.
    let before = h.observer.total();
    let mut off = story(&b, on, Some(4418), false);
    off.amount = m("1286.41");
    assert_eq!(
        BankPosting::check(off).expect_err("mismatch"),
        LocalValidation::TotalMismatch
    );
    assert_eq!(h.observer.total(), before, "nothing may be sent");

    let first = h
        .run(s.transactions().create_bank_posting(
            &BankPosting::check(story(&b, on, Some(4418), false)).expect("check"),
            &opts,
            key(),
        ))
        .expect("4418");

    // The same number on the same account: 409, nothing posted.
    let sent = h.observer.sent(Method::Post, "/api/v1/checks");
    let e = h
        .run(s.transactions().create_bank_posting(
            &BankPosting::check(story(&b, on, Some(4418), false)).expect("check"),
            &opts,
            key(),
        ))
        .expect_err("duplicate");
    assert!(matches!(e, ApiError::Conflict { .. }), "{e:?}");
    assert_eq!(
        h.observer.sent(Method::Post, "/api/v1/checks") - sent,
        1,
        "a 409 is a final answer, never retried"
    );

    // Print later: no number, queued; the counter stays where it was.
    let next = h
        .run(s.checks().settings())
        .expect("settings")
        .next_for(b.bank);
    let mut queued = story(&b, on, None, false);
    queued.print_later = true;
    let p = BankPosting::check(queued).expect("print later");
    let made = h
        .run(s.transactions().create_bank_posting(&p, &opts, key()))
        .expect("queued");
    let q = h.run(s.checks().get(made.record.id)).expect("queued check");
    assert_eq!(q.record.check_number, None);
    assert_eq!(q.record.print_status, PrintStatus::Queue);
    assert_eq!(q.record.txn.fingerprint(), p.created_fingerprint());
    assert_eq!(
        h.run(s.checks().settings())
            .expect("settings")
            .next_for(b.bank),
        next,
        "a queued check takes no number"
    );

    // A lost create reply is reconciled to exactly one check.
    let lost = BankPosting::check(story(&b, on, Some(4420), false)).expect("check");
    let sent = h.observer.sent(Method::Post, "/api/v1/checks");
    h.observer.drop_next_reply(Method::Post, "/api/v1/checks");
    let found = h
        .run(s.transactions().create_bank_posting(&lost, &opts, key()))
        .unwrap_or_else(|e| panic!("reconciled: {e}"));
    assert_eq!(h.observer.sent(Method::Post, "/api/v1/checks") - sent, 1);
    assert_eq!(found.record.fingerprint(), lost.created_fingerprint());
    let numbered: Vec<_> = h
        .run(s.checks().list(
            Some(b.bank),
            None,
            None,
            PageRequest::new(50, 0).expect("page"),
        ))
        .expect("checks")
        .into_iter()
        .filter(|r| r.number == Some(4420))
        .collect();
    assert_eq!(numbered.len(), 1, "entered once");

    // The check edit (`check-edit-v1`).
    let read = h.run(s.checks().get(first.record.id)).expect("check");
    let mut edited = story(&b, on, Some(4421), false);
    edited.payee_address = Some("PO Box 12\nAurora, MO 65605".into());
    let edit = BankPosting::check(edited).expect("edit");
    if h.require_cap(Capabilities::CHECK_EDIT, "check-edit-v1") {
        let saved = h
            .run(
                s.checks()
                    .update_check(first.record.id, &read.version, &edit, &opts, key()),
            )
            .unwrap_or_else(|e| panic!("edit: {e}"));
        let after = h.run(s.checks().get(first.record.id)).expect("check");
        assert_eq!(after.record.check_number, Some(4421));
        assert_eq!(after.record.print_status, PrintStatus::HandWritten);
        assert_eq!(after.version, saved.version, "answer and read agree");
        // The old version is stale now: refused, nothing sent.
        let puts = h.observer.sent(Method::Put, "/api/v1/checks/{id}");
        let e = h
            .run(
                s.checks()
                    .update_check(first.record.id, &read.version, &edit, &opts, key()),
            )
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
        assert_eq!(h.observer.sent(Method::Put, "/api/v1/checks/{id}"), puts);
        // Print later on an edit: unnumbered, queued.
        let mut to_print = story(&b, on, None, false);
        to_print.print_later = true;
        let to_print = BankPosting::check(to_print).expect("print later");
        // Its reply lost: reconciled by reading the check again.
        h.observer
            .drop_next_reply(Method::Put, "/api/v1/checks/{id}");
        let q = h
            .run(
                s.checks()
                    .update_check(first.record.id, &after.version, &to_print, &opts, key()),
            )
            .unwrap_or_else(|e| panic!("print later edit: {e}"));
        assert_eq!(q.record.check_number, None);
        assert_eq!(q.record.print_status, PrintStatus::Queue);
        // The number taken by another check: 409.
        let e = h
            .run(s.checks().update_check(
                first.record.id,
                &q.version,
                &BankPosting::check(story(&b, on, Some(4420), false)).expect("check"),
                &opts,
                key(),
            ))
            .expect_err("duplicate on edit");
        assert!(matches!(e, ApiError::Conflict { .. }), "{e:?}");
    } else {
        let before = h.observer.total();
        assert_eq!(
            h.run(
                s.checks()
                    .update_check(first.record.id, &read.version, &edit, &opts, key(),)
            )
            .expect_err("gated"),
            ApiError::Local(LocalValidation::CheckEditUnavailable)
        );
        assert_eq!(h.observer.total(), before, "nothing may be sent");
    }
}

/// The edit path without `check-edit-v1` (`PUT /transactions/{id}`), as
/// the screen takes it: the check is read with `checks().get`, and the
/// update is guarded by the ledger record's version (the check's own
/// version also covers the check fields, which the plain transaction
/// re-read never has). It succeeds whatever the engine advertises, and
/// leaves the check's number and print status alone.
pub fn edit_without_check_edit(h: &Harness) {
    let s = &h.s1;
    let b = books(h, s);
    let on = date(2026, 11, 6);
    let opts = PostingOptions::default();
    let made = h
        .run(s.transactions().create_bank_posting(
            &BankPosting::check(story(&b, on, Some(5101), false)).expect("check"),
            &opts,
            key(),
        ))
        .expect("5101");
    let read = h.run(s.checks().get(made.record.id)).expect("check");
    let mut changed = story(&b, on, Some(5101), false);
    changed.memo = Some(unique("changed through the ledger"));
    changed.lines = vec![split(b.plants, "1000.00"), split(b.mulch, "286.40")];
    let edit = BankPosting::check(changed).expect("edit");
    if matches!(read.version, RecordVersion::Fingerprint(_)) {
        // The check's own fingerprint never matches the transaction
        // re-read: refused as stale with nothing sent (the P13 bug).
        let puts = h.observer.sent(Method::Put, "/api/v1/transactions/{id}");
        let e = h
            .run(s.transactions().update_bank_posting(
                made.record.id,
                &read.version,
                &edit,
                &opts,
                key(),
            ))
            .expect_err("the check's version is not the transaction's");
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
            puts
        );
    }
    let saved = h
        .run(s.transactions().update_bank_posting(
            made.record.id,
            &read.transaction_version(),
            &edit,
            &opts,
            key(),
        ))
        .unwrap_or_else(|e| panic!("edit without check-edit-v1: {e}"));
    assert_eq!(saved.record.fingerprint(), edit.updated_fingerprint());
    let after = h.run(s.checks().get(made.record.id)).expect("check");
    assert_eq!(after.record.check_number, Some(5101), "the number stays");
    // No line customer unless one was sent (the screen shows a check with
    // one read-only on an engine without `line-billable-v1`).
    assert!(
        after
            .record
            .txn
            .lines
            .iter()
            .all(|l| l.contact_id.is_none() && !l.is_billable),
        "{:?}",
        after
            .record
            .txn
            .lines
            .iter()
            .map(|l| (l.contact_id.is_some(), l.is_billable))
            .collect::<Vec<_>>()
    );
    assert_eq!(after.record.print_status, PrintStatus::HandWritten);
    assert_eq!(after.transaction_version(), saved.version);
    // The version it left guards the next edit.
    let again = BankPosting::check(story(&b, on, Some(5101), false)).expect("again");
    h.run(s.transactions().update_bank_posting(
        made.record.id,
        &saved.version,
        &again,
        &opts,
        key(),
    ))
    .unwrap_or_else(|e| panic!("second edit: {e}"));
}

fn tag(h: &Harness, s: &Session, name: &str) -> TagId {
    h.run(s.tags().create(
        &TagFields {
            name: unique(name),
            color: None,
            group_id: None,
            description: None,
            tag_number: None,
        },
        key(),
    ))
    .expect("tag")
    .record
    .id
}

/// Prints a queued check through the engine (`POST /checks/print`).
fn print(h: &Harness, s: &Session, bank: AccountId, id: TransactionId, number: u32) {
    h.run(s.raw_mapped(
        Method::Post,
        "/api/v1/checks/print",
        Some(&json!({
            "bankAccountId": bank.to_string(),
            "checkIds": [id.to_string()],
            "startingCheckNumber": number,
            "format": "voucher",
        })),
    ))
    .unwrap_or_else(|e| panic!("print: {e}"));
}

/// The check as the screen sends it back: the read's memo, printed memo,
/// name and address, so only what the owner changed differs.
fn as_read(b: &Books, read: &CheckRecord, lines: Vec<SplitLine>, tags: Vec<TagId>) -> CheckDraft {
    let amount = lines
        .iter()
        .try_fold(Money::ZERO, |sum, l| sum.checked_add(l.amount))
        .expect("sum");
    CheckDraft {
        bank: b.bank,
        date: read.txn.txn_date,
        payee: Some(b.vendor),
        payee_name: read.payee_name.clone(),
        check_number: read.check_number,
        print_later: false,
        payee_address: read.payee_address.clone(),
        memo: read.txn.memo.clone(),
        printed_memo: read.printed_memo.clone(),
        tag_ids: tags,
        draft_attachment: None,
        amount,
        lines,
    }
}

/// Editing printed checks through `update_check` (`check-edit-v1`) as the
/// screen does: a recategorized split keeps it printed and the sent header
/// tags replace the old ones; a new amount is refused with `CHECK_PRINTED`
/// (nothing changes) until the owner chooses Keep number (handwritten) or
/// Reprint (back to the queue, unnumbered). `LINE_BILLED` needs a line
/// invoiced to a customer, which no route makes yet; the client mapping is
/// covered by the stub tests.
pub fn printed_check_edits(h: &Harness) {
    if !h.require_cap(Capabilities::CHECK_EDIT, "check-edit-v1") {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let on = date(2026, 11, 7);
    let opts = PostingOptions::default();
    let install = tag(h, s, "Install");
    let spring = tag(h, s, "Spring");
    let next = h
        .run(s.checks().settings())
        .expect("settings")
        .next_for(b.bank)
        .unwrap_or(1);
    let number = next.max(6000);

    let queued = |tags: Vec<TagId>| {
        let mut d = story(&b, on, None, false);
        d.print_later = true;
        d.tag_ids = tags;
        h.run(s.transactions().create_bank_posting(
            &BankPosting::check(d).expect("queued"),
            &opts,
            key(),
        ))
        .expect("queued check")
        .record
        .id
    };
    let id = queued(vec![install]);
    print(h, s, b.bank, id, number);
    let read = h.run(s.checks().get(id)).expect("printed");
    assert_eq!(read.record.print_status, PrintStatus::Printed);
    assert_eq!(read.record.check_number, Some(number));
    assert_eq!(read.record.tag_ids, vec![install]);

    // Same face, lines recategorized, tags replaced: no question asked.
    let resplit = as_read(
        &b,
        &read.record,
        vec![split(b.plants, "1000.00"), split(b.mulch, "286.40")],
        vec![spring],
    );
    let saved = h
        .run(s.checks().update_check(
            id,
            &read.version,
            &BankPosting::check(resplit).expect("resplit"),
            &opts,
            key(),
        ))
        .unwrap_or_else(|e| panic!("face unchanged: {e}"));
    let after = h.run(s.checks().get(id)).expect("check");
    assert_eq!(after.record.print_status, PrintStatus::Printed);
    assert_eq!(after.record.check_number, Some(number));
    assert_eq!(after.record.tag_ids, vec![spring], "the sent tags replace");
    assert_eq!(after.version, saved.version);

    // A new amount on a printed check: refused, nothing changes.
    let more = || {
        BankPosting::check(as_read(
            &b,
            &after.record,
            vec![split(b.plants, "855.75"), split(b.mulch, "444.25")],
            Vec::new(),
        ))
        .expect("more")
    };
    let e = h
        .run(
            s.checks()
                .update_check(id, &after.version, &more(), &opts, key()),
        )
        .expect_err("printed face changed");
    assert!(is_check_printed(&e), "{e:?}");
    let same = h.run(s.checks().get(id)).expect("check");
    assert_eq!(same.version, after.version, "nothing changed");
    assert_eq!(
        same.record.txn.total.map(Money::to_wire).as_deref(),
        Some("1286.4000")
    );

    // Keep number: handwritten with the same number, new amount, tags
    // cleared (an empty set is sent).
    let keep = CheckEditOptions {
        posting: opts.clone(),
        printed_check_change: Some(PrintedCheckChange::KeepNumber),
    };
    let kept = h
        .run(
            s.checks()
                .update_check(id, &after.version, &more(), keep, key()),
        )
        .unwrap_or_else(|e| panic!("keep number: {e}"));
    assert_eq!(kept.record.print_status, PrintStatus::HandWritten);
    assert_eq!(kept.record.check_number, Some(number));
    let reread = h.run(s.checks().get(id)).expect("check");
    assert_eq!(
        reread.record.txn.total.map(Money::to_wire).as_deref(),
        Some("1300.0000")
    );
    assert!(reread.record.tag_ids.is_empty(), "[] clears the tags");
    assert_eq!(reread.version, kept.version);

    // Reprint: a second printed check, a new address, back to the queue.
    let other = queued(Vec::new());
    print(h, s, b.bank, other, number + 1);
    let read = h.run(s.checks().get(other)).expect("printed");
    let mut moved = as_read(
        &b,
        &read.record,
        vec![split(b.plants, "842.15"), split(b.mulch, "444.25")],
        Vec::new(),
    );
    moved.payee_address = Some("PO Box 12\nAurora, MO 65605".into());
    let reprint = CheckEditOptions {
        posting: opts.clone(),
        printed_check_change: Some(PrintedCheckChange::Reprint),
    };
    // The screen sends the same form again (its number still on it) with
    // the owner's choice; the reply is lost and reconciled by reading the
    // check again, so the client's prediction must match the engine.
    assert_eq!(moved.check_number, Some(number + 1));
    h.observer
        .drop_next_reply(Method::Put, "/api/v1/checks/{id}");
    let queued_again = h
        .run(s.checks().update_check(
            other,
            &read.version,
            &BankPosting::check(moved).expect("moved"),
            reprint,
            key(),
        ))
        .unwrap_or_else(|e| panic!("reprint: {e}"));
    assert_eq!(queued_again.record.print_status, PrintStatus::Queue);
    assert_eq!(queued_again.record.check_number, None);
    assert_eq!(
        h.run(s.checks().get(other)).expect("check").version,
        queued_again.version
    );
}

/// A closed period asks for the closing-date password (second company).
pub fn closed_period(h: &Harness) {
    let s = h.s3();
    let b = books(h, &s);
    let settings = |lock: serde_json::Value| {
        h.run(s.raw_mapped(
            Method::Put,
            "/api/v1/company/settings",
            Some(&json!({ "lockDate": lock })),
        ))
        .expect("closing date");
    };
    settings(json!("2026-09-30"));
    let check = || BankPosting::check(story(&b, date(2026, 9, 15), None, false)).expect("check");
    let e = h
        .run(
            s.transactions()
                .create_bank_posting(&check(), &PostingOptions::default(), key()),
        )
        .expect_err("closed");
    assert!(
        matches!(&e, ApiError::ClosedPeriod(c) if c.lock_date == Some(date(2026, 9, 30))),
        "{e:?}"
    );
    if h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15") {
        let pw = Secret::new(unique("closing-check"));
        assert!(
            h.run(s.set_closing_password(None, Some(&pw), key()))
                .expect("set password")
        );
        let with = |p: &Secret| PostingOptions {
            closed_period: Some(ClosedPeriodAuthorization::with_password(p.clone())),
        };
        let e = h
            .run(
                s.transactions()
                    .create_bank_posting(&check(), &PostingOptions::default(), key()),
            )
            .expect_err("password required");
        assert!(
            matches!(&e, ApiError::ClosedPeriod(c) if c.password_required == Some(true)),
            "{e:?}"
        );
        let e = h
            .run(s.transactions().create_bank_posting(
                &check(),
                &with(&Secret::new("not-the-password")),
                key(),
            ))
            .expect_err("wrong password");
        assert_eq!(e, ApiError::ClosingPasswordRejected);
        let saved = h
            .run(
                s.transactions()
                    .create_bank_posting(&check(), &with(&pw), key()),
            )
            .expect("right password");
        let rows = h
            .run(
                s.audit()
                    .for_record(AuditEntity::ClosingDateOverride, *saved.record.id.as_uuid()),
            )
            .expect("audit");
        assert!(
            rows.iter().any(|r| r.action == AuditAction::Override),
            "the closed-period check is audited"
        );
        // An edit of the closed check (`check-edit-v1`) asks the same way.
        if h.require_cap(Capabilities::CHECK_EDIT, "check-edit-v1") {
            let read = h.run(s.checks().get(saved.record.id)).expect("check");
            let mut moved = story(&b, date(2026, 9, 16), read.record.check_number, false);
            moved.memo.clone_from(&read.record.txn.memo);
            let moved = BankPosting::check(moved).expect("edit");
            let e = h
                .run(s.checks().update_check(
                    saved.record.id,
                    &read.version,
                    &moved,
                    &PostingOptions::default(),
                    key(),
                ))
                .expect_err("closed edit");
            assert!(
                matches!(&e, ApiError::ClosedPeriod(c) if c.password_required == Some(true)),
                "{e:?}"
            );
            let e = h
                .run(s.checks().update_check(
                    saved.record.id,
                    &read.version,
                    &moved,
                    &with(&Secret::new("not-the-password")),
                    key(),
                ))
                .expect_err("wrong password");
            assert_eq!(e, ApiError::ClosingPasswordRejected);
            h.run(s.checks().update_check(
                saved.record.id,
                &read.version,
                &moved,
                &with(&pw),
                key(),
            ))
            .expect("edit with the password");
        }
        assert!(
            !h.run(s.set_closing_password(Some(&pw), None, key()))
                .expect("clear password")
        );
    }
    settings(serde_json::Value::Null);
}
