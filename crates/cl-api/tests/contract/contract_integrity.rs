//! P34 (U-12, pin `f52b64ae`): Verify Data against the engine: a quick and a
//! full check of a healthy company find nothing, runs are stored and read
//! back, the summary reports them, and a fix of a finding the run does not
//! have changes nothing.

use cl_api::{Capabilities, CheckStatus, FixSkipReason, IntegrityId, IntegrityLevel};

use crate::support::Harness;

pub fn verify_data_on_a_healthy_company_u12(h: &Harness) {
    if !h.require_cap(Capabilities::INTEGRITY, "U-12") {
        return;
    }
    let s = h.s3();
    h.run(s.open_company(h.tenant_b())).expect("open");
    let quick = h
        .run(s.integrity().check(IntegrityLevel::Quick))
        .expect("quick");
    assert_eq!(
        quick
            .checks
            .iter()
            .map(|c| c.code.as_str())
            .collect::<Vec<_>>(),
        [
            "UNBALANCED_TRANSACTION",
            "ACCOUNT_BALANCE_DRIFT",
            "DRAFT_AFFECTS_BOOKS"
        ]
    );
    let full = h
        .run(s.integrity().check(IntegrityLevel::Full))
        .expect("full");
    assert_eq!(full.checks.len(), 7);
    assert!(full.findings.is_empty(), "{:?}", full.findings);
    assert!(
        full.checks
            .iter()
            .all(|c| matches!(c.status, CheckStatus::Ok | CheckStatus::Skipped)),
        "{:?}",
        full.checks
    );
    let again = h.run(s.integrity().run(&full.id)).expect("stored run");
    assert_eq!(again.id, full.id);
    assert_eq!(again.ledger_version, full.ledger_version);
    let summary = h.run(s.integrity().summary()).expect("summary");
    assert_eq!(summary.last_full.as_ref().map(|r| &r.id), Some(&full.id));
    assert_eq!(summary.last_quick.as_ref().map(|r| &r.id), Some(&quick.id));
    let fixed = h
        .run(s.integrity().fix(
            &full,
            &[IntegrityId::parse("ACCOUNT_BALANCE_DRIFT:none").expect("id")],
            Some("contract"),
        ))
        .expect("fix");
    assert_eq!(fixed.fixed, Vec::<IntegrityId>::new());
    assert_eq!(fixed.skipped[0].reason, FixSkipReason::NotFixable);
    assert_eq!(
        fixed.ledger_version, full.ledger_version,
        "a fix never moves the ledger"
    );
}
