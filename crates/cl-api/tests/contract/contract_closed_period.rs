//! Closed periods (AC3, NN8), in tenant B so no other test sees the lock.
//!
//! The closing date is set through the engine's documented company-settings
//! route (`PUT /api/v1/company/settings {lockDate}`, `company.routes.ts:142`,
//! `updateCompanySettingsSchema.lockDate`), sent raw because cl-api has no
//! typed way to set it (P05 plan §2.7, N7). The ledger checks both the new
//! and the original date (`ledger.service.ts` `checkLockDate`).

use cl_api::{
    ApiError, Capabilities, ClosedPeriodAuthorization, DeleteOptions, LocalValidation, Method,
    PostingOptions,
};
use cl_core::Secret;
use serde_json::json;

use crate::common::{date, je2, key, pair, post};
use crate::support::{Harness, unique};

fn assert_closed(e: &ApiError) {
    match e {
        ApiError::ClosedPeriod(c) => {
            assert_eq!(c.lock_date, Some(date(2026, 1, 31)), "{e:?}");
            assert_ne!(c.password_required, Some(true), "no password is set");
            eprintln!("    closed period: can_override={}", c.can_override);
        }
        other => panic!("expected ClosedPeriod, got {other:?}"),
    }
}

pub fn locked_dates_are_refused_both_ways(h: &Harness) {
    let s = h.s3();
    let (cash, supplies) = pair(h, &s);
    // Open entry first (dated after the lock date to be set).
    let memo = unique("period");
    let open = post(
        h,
        &s,
        &je2(date(2026, 2, 15), &memo, supplies.id, cash.id, "5.0000"),
    );
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": "2026-01-31"})),
    ))
    .expect("set the closing date");
    let settings = h.run(s.company_settings()).expect("settings");
    assert_eq!(settings.lock_date, Some(date(2026, 1, 31)));

    // A new entry inside the closed period.
    let e = h
        .run(s.transactions().create_journal_entry(
            &je2(date(2026, 1, 15), &memo, supplies.id, cash.id, "5.0000"),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("closed");
    assert_closed(&e);

    // Moving an open entry into the closed period.
    let e = h
        .run(s.transactions().update_journal_entry(
            open.record.id,
            &open.version,
            &je2(date(2026, 1, 20), &memo, supplies.id, cash.id, "5.0000"),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("closed");
    assert_closed(&e);

    // No override exists until U-15: a password is refused before sending.
    let before = h.observer.total();
    let opts = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "closing password",
        ))),
    };
    let e = h
        .run(s.transactions().create_journal_entry(
            &je2(date(2026, 1, 16), &memo, supplies.id, cash.id, "5.0000"),
            &opts,
            key(),
        ))
        .expect_err("no override");
    assert!(
        matches!(
            e,
            ApiError::Local(LocalValidation::ClosedPeriodOverrideUnavailable)
        ),
        "{e:?}"
    );
    assert_eq!(h.observer.total(), before, "nothing may be sent");

    // Clear the closing date again.
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": null})),
    ))
    .expect("clear the closing date");
}

/// Checks, deposits, transfers and card charges in a closed period (P12
/// WP2): create, update into it and void are refused with `ClosedPeriod`.
/// With `closing-password-v1` (U-15) a wrong password is
/// `ClosingPasswordRejected`, the right one saves and leaves a
/// `closing_date_override` audit row; PENDING(U-15) until then.
pub fn bank_postings_in_a_closed_period(h: &Harness) {
    use crate::contract_bank_postings::{accounts, check};
    use cl_api::{AuditAction, AuditEntity};

    let s = h.s3();
    let a = accounts(h, &s);
    let memo = unique("closed bank");
    let posted = |on| {
        h.run(s.transactions().create_bank_posting(
            &check(&a, on, &memo, ("10.0000", "2.5000")),
            &PostingOptions::default(),
            key(),
        ))
        .expect("open check")
    };
    let inside = posted(date(2026, 1, 10));
    let inside_del = posted(date(2026, 1, 11));
    let open = posted(date(2026, 2, 15));
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": "2026-01-31"})),
    ))
    .expect("set the closing date");
    let is_closed = |e: &ApiError| match e {
        ApiError::ClosedPeriod(c) => {
            assert_eq!(c.lock_date, Some(date(2026, 1, 31)), "{e:?}");
            assert_ne!(c.password_required, Some(true), "no password is set");
        }
        other => panic!("expected ClosedPeriod, got {other:?}"),
    };
    let e = h
        .run(s.transactions().create_bank_posting(
            &check(&a, date(2026, 1, 15), &memo, ("1.0000", "1.0000")),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("closed create");
    is_closed(&e);
    let e = h
        .run(s.transactions().update_bank_posting(
            open.record.id,
            &open.version,
            &check(&a, date(2026, 1, 20), &memo, ("10.0000", "2.5000")),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("closed update");
    is_closed(&e);
    let e = h
        .run(s.transactions().void(
            inside.record.id,
            "closed",
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("closed void");
    is_closed(&e);
    // R7: a delete in the closed period is refused the same way.
    if h.require_cap(Capabilities::TXN_DELETE, "R7") {
        let e = h
            .run(
                s.transactions()
                    .delete(inside_del.record.id, &DeleteOptions::default(), key()),
            )
            .expect_err("closed delete");
        is_closed(&e);
    }

    if h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15") {
        let pw = Secret::new("closing-pass-1");
        assert!(
            h.run(s.set_closing_password(None, Some(&pw), key()))
                .expect("set password")
        );
        let with = |p: &str| PostingOptions {
            closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(p))),
        };
        let e = h
            .run(s.transactions().create_bank_posting(
                &check(&a, date(2026, 1, 16), &memo, ("1.0000", "1.0000")),
                &PostingOptions::default(),
                key(),
            ))
            .expect_err("password required");
        assert!(
            matches!(&e, ApiError::ClosedPeriod(c) if c.password_required == Some(true)),
            "{e:?}"
        );
        let e = h
            .run(s.transactions().create_bank_posting(
                &check(&a, date(2026, 1, 16), &memo, ("1.0000", "1.0000")),
                &with("not-the-password"),
                key(),
            ))
            .expect_err("wrong password");
        assert_eq!(e, ApiError::ClosingPasswordRejected);
        let saved = h
            .run(s.transactions().create_bank_posting(
                &check(&a, date(2026, 1, 16), &memo, ("1.0000", "1.0000")),
                &with("closing-pass-1"),
                key(),
            ))
            .expect("right password");
        let rows = h
            .run(
                s.audit()
                    .for_record(AuditEntity::ClosingDateOverride, *saved.record.id.as_uuid()),
            )
            .expect("audit");
        assert!(
            rows.iter().any(|r| r.action == AuditAction::Override),
            "closed-period change is audited"
        );
        let voided = h
            .run(
                s.transactions()
                    .void(inside.record.id, "closed", &with("closing-pass-1"), key()),
            )
            .expect("void with password");
        assert_eq!(voided.record.status, cl_api::TxnStatus::Void);
        if h.require_cap(Capabilities::TXN_DELETE, "R7") {
            let del = |p: Option<&str>| DeleteOptions {
                reason: Some("closed-period delete".into()),
                closing_password: p.map(Secret::new),
            };
            let e = h
                .run(
                    s.transactions()
                        .delete(inside_del.record.id, &del(None), key()),
                )
                .expect_err("delete needs the password");
            assert!(
                matches!(&e, ApiError::ClosedPeriod(c) if c.password_required == Some(true)),
                "{e:?}"
            );
            let e = h
                .run(s.transactions().delete(
                    inside_del.record.id,
                    &del(Some("not-the-password")),
                    key(),
                ))
                .expect_err("wrong password");
            assert_eq!(e, ApiError::ClosingPasswordRejected);
            let gone = h
                .run(s.transactions().delete(
                    inside_del.record.id,
                    &del(Some("closing-pass-1")),
                    key(),
                ))
                .expect("delete with password");
            assert_eq!(gone.record.status, cl_api::TxnStatus::Deleted);
        }
        // Q-239: the owner resets a forgotten password with their own
        // sign-in password; a wrong one is refused and changes nothing.
        let e = h
            .run(s.reset_closing_password(&Secret::new("not-my-sign-in"), None, key()))
            .expect_err("wrong sign-in password");
        assert_eq!(e, ApiError::AccountPasswordRejected);
        let pw2 = Secret::new("closing-pass-2");
        assert!(
            h.run(s.reset_closing_password(&h.password, Some(&pw2), key()))
                .expect("owner reset")
        );
        assert!(
            !h.run(s.set_closing_password(Some(&pw2), None, key()))
                .expect("clear password")
        );
    }

    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": null})),
    ))
    .expect("clear the closing date");
}
