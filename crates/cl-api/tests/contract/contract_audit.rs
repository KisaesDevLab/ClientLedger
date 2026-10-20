//! The audit trail of a transaction (P12 WP2, AC3): an update and a void
//! each leave a row with the record as it was. With `register-exact-v1`
//! (P12 R6) the before-image carries every original line with exact money;
//! PENDING(R6) until then. Delete (R7) is PENDING by `txn-delete-v1`.

use cl_api::{
    ApiError, AuditAction, AuditEntity, Capabilities, ConflictReason, DeleteOptions,
    PostingOptions, TxnStatus,
};

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
}

/// R7 (AC3): a soft delete reverses the transaction, hides it, and leaves
/// an audit row with every original line, exactly. A second delete and a
/// void afterwards are refused. PENDING by `txn-delete-v1`.
pub fn delete_keeps_every_line_in_the_audit_trail(h: &Harness) {
    if !h.require_cap(Capabilities::TXN_DELETE, "R7") {
        return;
    }
    let s = &h.s1;
    let a = accounts(h, s);
    let memo = unique("audit delete");
    let made = h
        .run(s.transactions().create_bank_posting(
            &check(&a, date(2026, 6, 4), &memo, ("33.3333", "0.0067")),
            &PostingOptions::default(),
            key(),
        ))
        .expect("check");
    let got = h.run(s.transactions().get(made.record.id)).expect("get");
    let before_lines = wire_lines(&got.record);
    let opts = DeleteOptions {
        reason: Some("entered twice".into()),
        closing_password: None,
    };
    let gone = h
        .run(s.transactions().delete(made.record.id, &opts, key()))
        .expect("delete");
    assert_eq!(gone.record.status, TxnStatus::Deleted);

    let rows = h
        .run(
            s.audit()
                .for_record(AuditEntity::Transaction, *made.record.id.as_uuid()),
        )
        .expect("audit rows");
    let del = rows
        .iter()
        .find(|r| r.action == AuditAction::Delete)
        .expect("a delete row");
    let lines = del
        .before
        .as_ref()
        .and_then(|b| b.lines().expect("exact lines"))
        .expect("delete before-image has lines");
    let got: Vec<(String, String)> = lines
        .iter()
        .map(|l| (l.debit.to_wire(), l.credit.to_wire()))
        .collect();
    assert_eq!(got, before_lines, "every original line, exactly");

    let e = h
        .run(
            s.transactions()
                .delete(made.record.id, &DeleteOptions::default(), key()),
        )
        .expect_err("already deleted");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::AlreadyDeleted,
                ..
            } | ApiError::NotFound { .. }
        ),
        "{e:?}"
    );
    let e = h
        .run(s.transactions().void(
            made.record.id,
            "after delete",
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("void after delete");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
}
