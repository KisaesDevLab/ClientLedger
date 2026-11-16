//! P29 WP5 (Q-155, U-229): only an Admin moves the closing date or changes
//! its password; a bookkeeper with company-settings rights is refused with
//! 403 `CLOSING_DATE_ADMIN_ONLY` and other settings still save.

use cl_api::{ClosedPeriodAuthorization, Method, PostingOptions};
use cl_core::Secret;
use serde_json::json;

use crate::common::{date, key, m};
use crate::support::{Harness, unique};

/// U-229's capability.
const CLOSING_ADMIN: &str = "closing-date-admin-v1";
/// U-204's capability (the exceptions report).
const CLOSING_EXCEPTIONS: &str = "closing-exceptions-v1";

/// AC2 (P29): a closed-period edit made with the closing-date password is
/// listed in `GET /reports/closing-date-exceptions` with the transaction,
/// the action and the override method. In tenant B (`h.s3()`), which is put
/// back to no closing date and no password before anything is asserted.
pub fn closed_period_edit_is_in_the_exceptions_report(h: &Harness) {
    use crate::contract_bank_postings::{accounts, check};
    if !h.require_cap(CLOSING_EXCEPTIONS, "U-204") {
        return;
    }
    let s = h.s3();
    let settings = |body: serde_json::Value| {
        h.run(s.raw_mapped(Method::Put, "/api/v1/company/settings", Some(&body)))
            .expect("company settings");
    };
    settings(json!({"lockDate": null}));
    let a = accounts(h, &s);
    let memo = unique("exceptions");
    let made = h
        .run(s.transactions().create_bank_posting(
            &check(&a, date(2026, 1, 12), &memo, ("10.0000", "2.0000")),
            &PostingOptions::default(),
            key(),
        ))
        .expect("open check");
    settings(json!({"lockDate": "2026-01-31"}));
    let pw = Secret::new("exceptions-pass-1");
    assert!(
        h.run(s.set_closing_password(None, Some(&pw), key()))
            .expect("set password")
    );
    let edited = h.run(s.transactions().update_bank_posting(
        made.record.id,
        &made.version,
        &check(&a, date(2026, 1, 12), &memo, ("11.0000", "2.0000")),
        &PostingOptions {
            closed_period: Some(ClosedPeriodAuthorization::with_password(pw.clone())),
        },
        key(),
    ));
    let report = h.run(s.raw(
        Method::Get,
        "/api/v1/reports/closing-date-exceptions",
        &[],
        None,
    ));
    // Put tenant B back before any assertion.
    let cleared = h.run(s.set_closing_password(Some(&pw), None, key()));
    settings(json!({"lockDate": null}));
    assert!(!cleared.expect("clear password"));

    edited.unwrap_or_else(|e| panic!("edit with the password: {e:?}"));
    let report = report.expect("exceptions");
    assert_eq!(report.status, 200, "{}", report.body);
    let id = made.record.id.to_string();
    let rows: Vec<&serde_json::Value> = report.body["data"]
        .as_array()
        .expect("data")
        .iter()
        .filter(|r| r["transactionId"].as_str() == Some(id.as_str()))
        .collect();
    assert!(
        rows.iter()
            .any(|r| r["action"] == "update" && r["method"] == "password"),
        "the edit is listed with the password override: {rows:#?}"
    );
    let row = rows
        .iter()
        .find(|r| r["action"] == "update")
        .expect("update row");
    assert_eq!(row["closingDate"], "2026-01-31", "{row}");
    assert_eq!(row["txnDateBefore"], "2026-01-12", "{row}");
    let amount = |k: &str| m(row[k].as_str().unwrap_or_else(|| panic!("{k}: {row}")));
    assert_eq!(amount("amountBeforeExact"), m("12.00"), "{row}");
    assert_eq!(amount("amountAfterExact"), m("13.00"), "{row}");
}

/// A bookkeeper with full company-settings rights cannot set the closing
/// date or the closing-date password; their other settings still save.
pub fn bookkeeper_cannot_move_the_closing_date(h: &Harness) {
    if !h.require_cap(CLOSING_ADMIN, "U-229") {
        return;
    }
    let s = &h.s1;
    let (user, bk) = h.bookkeeper();
    let r = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/company/users/{user}/permissions"),
            &[],
            Some(&json!({ "overrides": { "company_settings": "full" } })),
        ))
        .expect("permissions");
    assert_eq!(r.status, 200, "{}", r.body);
    let put = |path: &str, body: serde_json::Value| {
        h.run(bk.raw(Method::Put, path, &[], Some(&body)))
            .expect("raw")
    };
    let r = put(
        "/api/v1/company/settings",
        json!({ "lockDate": "2026-01-31", "closingPassword": "bk-tries-1" }),
    );
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(
        r.body["error"]["code"], "CLOSING_DATE_ADMIN_ONLY",
        "{}",
        r.body
    );
    let r = put(
        "/api/v1/company/closing-password",
        json!({ "newPassword": "bk-tries-1" }),
    );
    assert_eq!(r.status, 403, "{}", r.body);
    // Other settings are still the bookkeeper's to change.
    let r = put(
        "/api/v1/company/settings",
        json!({ "dateFormat": "MM/DD/YYYY" }),
    );
    assert_eq!(r.status, 200, "{}", r.body);
}

/// The exceptions report's rows for one transaction.
fn rows_for(report: &serde_json::Value, id: &str) -> Vec<serde_json::Value> {
    report["data"]
        .as_array()
        .expect("data")
        .iter()
        .filter(|r| r["transactionId"].as_str() == Some(id))
        .cloned()
        .collect()
}

/// P29 review S1 (AC2): a void and a delete in a closed period, and a
/// Bulk Entry row saved into it, each with the closing-date password, are
/// listed in the exceptions report with their action and the password
/// method. In tenant B, put back before anything is asserted.
pub fn closed_period_void_delete_and_batch_are_in_the_exceptions_report(h: &Harness) {
    use crate::contract_bank_postings::{accounts, check};
    use cl_api::{BatchKind, DeleteOptions};
    if !h.require_cap(CLOSING_EXCEPTIONS, "U-204")
        || !h.require_cap(cl_api::Capabilities::TXN_DELETE, "R7 txn-delete-v1")
        || !crate::contract_batch::save_ready(h, &[crate::contract_batch::CLOSING_CAP])
    {
        return;
    }
    let s = h.s3();
    let settings = |body: serde_json::Value| {
        h.run(s.raw_mapped(Method::Put, "/api/v1/company/settings", Some(&body)))
            .expect("company settings");
    };
    settings(json!({"lockDate": null}));
    let a = accounts(h, &s);
    let b = crate::contract_batch::books(h, &s);
    let open = |tag: &str| {
        h.run(s.transactions().create_bank_posting(
            &check(&a, date(2026, 1, 12), &unique(tag), ("10.0000", "2.0000")),
            &PostingOptions::default(),
            key(),
        ))
        .expect("open check")
    };
    let to_void = open("exc-void");
    let to_delete = open("exc-delete");
    settings(json!({"lockDate": "2026-01-31"}));
    let pw = Secret::new(unique("exc-pass"));
    assert!(
        h.run(s.set_closing_password(None, Some(&pw), key()))
            .expect("set password")
    );
    let voided = h.run(s.transactions().void(
        to_void.record.id,
        "closed-period void",
        &PostingOptions {
            closed_period: Some(ClosedPeriodAuthorization::with_password(pw.clone())),
        },
        key(),
    ));
    let deleted = h.run(s.transactions().delete(
        to_delete.record.id,
        &DeleteOptions {
            reason: Some("closed-period delete".into()),
            closing_password: Some(pw.clone()),
        },
        key(),
    ));
    let mut row = crate::contract_batch::deposit_row(&b, 1, "12.00");
    row.date = date(2026, 1, 15);
    row.closing_password = Some(pw.clone());
    let saved = h.run(s.batch().save(
        &[crate::contract_batch::group(
            BatchKind::Deposit,
            Some(b.bank),
            vec![row],
        )],
        key(),
        false,
    ));
    let report = h.run(s.raw(
        Method::Get,
        "/api/v1/reports/closing-date-exceptions",
        &[],
        None,
    ));
    // Put tenant B back before any assertion.
    let cleared = h.run(s.set_closing_password(Some(&pw), None, key()));
    settings(json!({"lockDate": null}));
    assert!(!cleared.expect("clear password"));

    voided.unwrap_or_else(|e| panic!("void with the password: {e:?}"));
    deleted.unwrap_or_else(|e| panic!("delete with the password: {e:?}"));
    let saved = saved.unwrap_or_else(|e| panic!("batch with the password: {e:?}"));
    let batch_txn = saved
        .saved
        .iter()
        .find(|t| t.row_number == 1)
        .expect("row 1")
        .id
        .to_string();
    let report = report.expect("exceptions");
    assert_eq!(report.status, 200, "{}", report.body);
    for (id, action) in [
        (to_void.record.id.to_string(), "void"),
        (to_delete.record.id.to_string(), "delete"),
        (batch_txn, "create"),
    ] {
        let rows = rows_for(&report.body, &id);
        assert!(
            rows.iter()
                .any(|r| r["action"] == action && r["method"] == "password"),
            "{action} listed with the password: {rows:#?}"
        );
    }
}

/// P29 review S1 (AC2): a bookkeeper's closed-period edit with the
/// password is listed under the bookkeeper. In tenant A (the harness's
/// bookkeeper), put back before anything is asserted.
pub fn bookkeeper_closed_period_edit_is_in_the_exceptions_report(h: &Harness) {
    use crate::contract_bank_postings::{accounts, check};
    if !h.require_cap(CLOSING_EXCEPTIONS, "U-204") {
        return;
    }
    let s = &h.s1;
    let (user, bk) = h.bookkeeper();
    // The shared bookkeeper keeps the overrides the last test set (first
    // live run: an earlier test left `transactions` off); set the ones this
    // edit needs.
    let perms = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/company/users/{user}/permissions"),
            &[],
            Some(&json!({ "overrides": {
                "transactions": "full", "banking": "full", "checks": "full", "accounts": "full"
            } })),
        ))
        .expect("bookkeeper permissions");
    assert_eq!(perms.status, 200, "{}", perms.body);
    let settings = |body: serde_json::Value| {
        h.run(s.raw_mapped(Method::Put, "/api/v1/company/settings", Some(&body)))
            .expect("company settings");
    };
    settings(json!({"lockDate": null}));
    let a = accounts(h, s);
    let memo = unique("exc-bk");
    let made = h
        .run(s.transactions().create_bank_posting(
            &check(&a, date(2026, 1, 12), &memo, ("10.0000", "2.0000")),
            &PostingOptions::default(),
            key(),
        ))
        .expect("open check");
    settings(json!({"lockDate": "2026-01-31"}));
    let pw = Secret::new(unique("exc-bk-pass"));
    assert!(
        h.run(s.set_closing_password(None, Some(&pw), key()))
            .expect("set password")
    );
    let edited = h.run(bk.transactions().update_bank_posting(
        made.record.id,
        &made.version,
        &check(&a, date(2026, 1, 12), &memo, ("11.0000", "2.0000")),
        &PostingOptions {
            closed_period: Some(ClosedPeriodAuthorization::with_password(pw.clone())),
        },
        key(),
    ));
    let report = h.run(s.raw(
        Method::Get,
        "/api/v1/reports/closing-date-exceptions",
        &[],
        None,
    ));
    let cleared = h.run(s.set_closing_password(Some(&pw), None, key()));
    settings(json!({"lockDate": null}));
    assert!(!cleared.expect("clear password"));

    edited.unwrap_or_else(|e| panic!("bookkeeper edit with the password: {e:?}"));
    let report = report.expect("exceptions");
    assert_eq!(report.status, 200, "{}", report.body);
    let rows = rows_for(&report.body, &made.record.id.to_string());
    assert!(
        rows.iter().any(|r| r["action"] == "update"
            && r["method"] == "password"
            && r["userId"].as_str() == Some(user.as_str())),
        "the bookkeeper's edit is listed with the password: {rows:#?}"
    );
}
