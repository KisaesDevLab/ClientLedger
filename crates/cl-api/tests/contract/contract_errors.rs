//! Error mapping against the live engine (AC3): validation with a field,
//! code-less validation, not found, conflicts, closed period, bad login.

use cl_api::{
    AccountType, ApiError, AuthFailure, Basis, Capabilities, ConflictReason, JeLine, LoginOutcome,
    Method, NewJournalEntry, PostingOptions, Side, TagFields,
};
use cl_core::Secret;
use cl_core::ids::AccountId;
use serde_json::json;

use crate::common::{account, date, je2, key, line, m, pair, post};
use crate::support::{Harness, unique};

fn validation_field(e: &ApiError) -> Option<String> {
    match e {
        ApiError::Validation(v) => v.field().map(|f| f.as_str().to_string()),
        _ => None,
    }
}

pub fn validation_errors_carry_the_field(h: &Harness) {
    let s = &h.s1;
    let (cash, _) = pair(h, s);
    // One line (cl-api refuses this locally; sent raw).
    let one_line = json!({
        "txnType": "journal_entry",
        "txnDate": "2026-07-01",
        "lines": [{"accountId": cash.id, "debit": "1.0000", "credit": "0"}],
    });
    let e = h
        .run(s.raw_mapped(Method::Post, "/api/v1/transactions", Some(&one_line)))
        .expect_err("one line");
    let field = validation_field(&e).unwrap_or_default();
    assert!(field.starts_with("lines"), "{e:?} field {field}");

    // An item without its income account.
    let item = json!({"name": unique("No income")});
    let e = h
        .run(s.raw_mapped(Method::Post, "/api/v1/items", Some(&item)))
        .expect_err("item");
    assert_eq!(
        validation_field(&e).as_deref(),
        Some("incomeAccountId"),
        "{e:?}"
    );

    // Unbalanced by 1.00: a 400 the engine raises without a code.
    let (a, b) = pair(h, s);
    let unbalanced = json!({
        "txnType": "journal_entry",
        "txnDate": "2026-07-01",
        "lines": [
            {"accountId": a.id, "debit": "11.0000", "credit": "0"},
            {"accountId": b.id, "debit": "0", "credit": "10.0000"},
        ],
    });
    let e = h
        .run(s.raw_mapped(Method::Post, "/api/v1/transactions", Some(&unbalanced)))
        .expect_err("unbalanced");
    match &e {
        // Without U-21 the engine sends no code; with `error-codes-v1` it is
        // TXN_UNBALANCED (probed in `capabilities::error_codes_u21`).
        ApiError::Validation(v) if h.caps.has(Capabilities::ERROR_CODES) => {
            assert_eq!(
                v.code().map(cl_api::ErrorCode::as_str),
                Some("TXN_UNBALANCED")
            );
        }
        ApiError::Validation(v) => assert!(v.code().is_none(), "{e:?}"),
        other => panic!("expected Validation, got {other:?}"),
    }
}

pub fn not_found_and_deactivate_with_balance(h: &Harness) {
    let s = &h.s1;
    let missing = AccountId::parse("00000000-0000-4000-8000-00000000abcd").expect("id");
    let e = h.run(s.accounts().get(missing)).expect_err("missing");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");

    let (cash, supplies) = pair(h, s);
    post(
        h,
        s,
        &je2(
            date(2026, 7, 2),
            &unique("balance"),
            supplies.id,
            cash.id,
            "3.0000",
        ),
    );
    let e = h
        .run(s.accounts().deactivate(cash.id, key()))
        .expect_err("non-zero balance");
    assert!(matches!(e, ApiError::Validation(_)), "{e:?}");
}

pub fn tag_conflicts_in_use_and_duplicate(h: &Harness) {
    let s = &h.s1;
    let fields = TagFields {
        name: unique("Dup"),
        color: None,
        group_id: None,
        description: None,
        tag_number: None,
    };
    let tag = h.run(s.tags().create(&fields, key())).expect("tag").record;
    let e = h
        .run(s.tags().create(&fields, key()))
        .expect_err("duplicate");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Unspecified,
                ..
            }
        ),
        "{e:?}"
    );

    let cash = account(h, s, AccountType::Asset, "Tagged cash");
    let exp = account(h, s, AccountType::Expense, "Tagged exp");
    let entry = NewJournalEntry::new(
        date(2026, 7, 3),
        None,
        Basis::Both,
        vec![
            JeLine {
                tag: Some(tag.id),
                ..line(exp.id, Side::Debit(m("2.0000")))
            },
            JeLine {
                tag: Some(tag.id),
                ..line(cash.id, Side::Credit(m("2.0000")))
            },
        ],
    )
    .expect("entry");
    h.run(
        s.transactions()
            .create_journal_entry(&entry, &PostingOptions::default(), key()),
    )
    .expect("tagged entry");
    let e = h.run(s.tags().delete(tag.id, key())).expect_err("in use");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::InUse,
                ..
            }
        ),
        "{e:?}"
    );
}

/// A wrong login uses an email that does not exist, never the owner's (a
/// wrong owner password counts toward the super-admin lockout).
pub fn bad_login_is_invalid_credentials(h: &Harness) {
    let r = h.run(
        h.client
            .login("nobody@example.com", &Secret::new("not the password 1")),
    );
    match r {
        Err(ApiError::Unauthorized(AuthFailure::InvalidCredentials)) => {}
        Err(other) => panic!("expected InvalidCredentials, got {other:?}"),
        Ok(LoginOutcome::Session(_) | LoginOutcome::TfaRequired(_)) => {
            panic!("a login for an unknown email succeeded")
        }
    }
}
