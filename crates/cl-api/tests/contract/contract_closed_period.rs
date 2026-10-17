//! Closed periods (AC3, NN8), in tenant B so no other test sees the lock.
//!
//! The closing date is set through the engine's documented company-settings
//! route (`PUT /api/v1/company/settings {lockDate}`, `company.routes.ts:142`,
//! `updateCompanySettingsSchema.lockDate`), sent raw because cl-api has no
//! typed way to set it (P05 plan §2.7, N7). The ledger checks both the new
//! and the original date (`ledger.service.ts` `checkLockDate`).

use cl_api::{ApiError, ClosedPeriodAuthorization, LocalValidation, Method, PostingOptions};
use cl_core::Secret;
use serde_json::json;

use crate::common::{date, je2, key, pair, post};
use crate::support::{Harness, unique};

fn assert_closed(e: &ApiError) {
    match e {
        ApiError::ClosedPeriod(c) => {
            assert_eq!(c.lock_date, Some(date(2026, 1, 31)), "{e:?}");
            assert_eq!(c.password_required, None);
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
