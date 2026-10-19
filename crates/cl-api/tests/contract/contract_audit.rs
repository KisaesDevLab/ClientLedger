//! The audit trail of a transaction (P12 WP2, AC3): an update and a void
//! each leave a row with the record as it was. With `register-exact-v1`
//! (P12 R6) the before-image carries every original line with exact money;
//! PENDING(R6) until then. Delete (R7) is PENDING by `transaction-delete-v1`.

use cl_api::{AuditAction, AuditEntity, Capabilities, PostingOptions};

use crate::common::{date, key, wire_lines};
use crate::contract_bank_postings::{accounts, check};
use crate::support::{Harness, unique};

pub fn update_and_void_keep_the_before_image(h: &Harness) {
    let s = &h.s1;
    let a = accounts(h, s);
    let memo = unique("audit");
    let original = check(&a, date(2026, 6, 2), &memo, ("12.3456", "0.0001"));
    let made = h
        .run(
            s.transactions()
                .create_bank_posting(&original, &PostingOptions::default(), key()),
        )
        .expect("check");
    let got = h.run(s.transactions().get(made.record.id)).expect("get");
    let before_lines = wire_lines(&got.record);
    let changed = check(&a, date(2026, 6, 3), &memo, ("20.0000", "1.0000"));
    let upd = h
        .run(s.transactions().update_bank_posting(
            made.record.id,
            &got.version,
            &changed,
            &PostingOptions::default(),
            key(),
        ))
        .expect("update");
    h.run(s.transactions().void(
        made.record.id,
        "audit test",
        &PostingOptions::default(),
        key(),
    ))
    .expect("void");
    let _ = upd;

    let rows = h
        .run(
            s.audit()
                .for_record(AuditEntity::Transaction, *made.record.id.as_uuid()),
        )
        .expect("audit rows");
    let update = rows
        .iter()
        .find(|r| r.action == AuditAction::Update)
        .expect("an update row");
    let void = rows
        .iter()
        .find(|r| r.action == AuditAction::Void)
        .expect("a void row");
    for r in [update, void] {
        let before = r.before.as_ref().expect("before image");
        assert_eq!(
            before.raw()["id"].as_str(),
            Some(made.record.id.to_string().as_str()),
            "{:?}",
            r.action
        );
    }
    if h.require_cap(Capabilities::REGISTER_EXACT, "R6") {
        let lines = update
            .before
            .as_ref()
            .and_then(|b| b.lines().expect("exact lines"))
            .expect("update before-image has lines");
        let got: Vec<(String, String)> = lines
            .iter()
            .map(|l| (l.debit.to_wire(), l.credit.to_wire()))
            .collect();
        assert_eq!(got, before_lines, "every original line, exactly");
        let void_lines = void
            .before
            .as_ref()
            .and_then(|b| b.lines().expect("exact lines"))
            .expect("void before-image has lines");
        assert_eq!(void_lines.len(), 3, "the updated check's lines");
    }
    // R7: deleting a transaction (soft delete, audited with lines).
    let _ = h.require_cap(Capabilities::TXN_DELETE, "R7");
}
