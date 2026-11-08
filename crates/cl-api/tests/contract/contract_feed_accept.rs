//! The one accept and the one undo route against the live engine (P26 E3,
//! U-170 / U-169; plan §6 "Accept from rule, logged and reversible",
//! "Undo", "Closed period with password", tests #1, #6). Gated on
//! `feed-accept-v1` and `feed-undo-v1`: PENDING on engines before the E3
//! pin bump.
//!
//! Every test imports its own rows into its own fresh bank account.

use cl_api::{
    AcceptFailure, AcceptItem, AcceptRequest, AcceptSkip, AccountType, BankFileFormat,
    BankImportRequest, BankImportRow, Capabilities, ClosedPeriodAuthorization, FeedChoice,
    FeedQuery, FeedRefusal, FeedRow, FeedStatus, Method, RuleAccept, Session, UndoTarget,
    feed_refusal,
};
use cl_core::Secret;
use cl_core::ids::AccountId;
use rust_decimal::Decimal;
use serde_json::json;

use crate::common::{account_fields, date, key};
use crate::support::{Harness, unique};

fn gated(h: &Harness) -> bool {
    h.require_cap(Capabilities::FEED_ACCEPT, "U-170")
        && h.require_cap(Capabilities::FEED_UNDO, "U-169")
        && h.require_cap(Capabilities::BANK_IMPORT_BATCH, "U-06")
}

fn fresh(h: &Harness, s: &Session, t: AccountType, detail: Option<&str>) -> AccountId {
    let mut f = account_fields(t, "Accept");
    f.detail_type = detail.map(str::to_string);
    h.run(s.accounts().create(&f, key()))
        .unwrap_or_else(|e| panic!("create account: {e}"))
        .record
        .id
}

fn import(
    h: &Harness,
    s: &Session,
    account: AccountId,
    rows: &[(cl_core::CalendarDate, &str, &str)],
) -> Vec<FeedRow> {
    let rows: Vec<BankImportRow> = rows
        .iter()
        .enumerate()
        .map(|(i, (d, amount, text))| BankImportRow {
            line: u32::try_from(i + 1).unwrap_or(1),
            date: *d,
            amount: amount.parse::<Decimal>().expect("decimal"),
            description: (*text).into(),
            memo: None,
            check_number: None,
            fitid: Some(unique("A")),
            include: None,
            changed_action: None,
        })
        .collect();
    let req = BankImportRequest {
        account,
        format: BankFileFormat::Qfx,
        file_name: "accept.qfx".into(),
        source_file_sha256: "ab".repeat(32),
        section_index: 0,
        section_start: rows.iter().map(|r| r.date).min(),
        section_end: rows.iter().map(|r| r.date).max(),
        amounts_flipped: false,
        csv_mapping: None,
        relink_identity: false,
        bank_identity: None,
        file_ending_balance: None,
        file_ending_date: None,
        rows,
    };
    let ids = h
        .run(s.bank_import().commit(&req, key()))
        .expect("import")
        .inserted_items;
    let page = h
        .run(s.feed().list(&FeedQuery {
            waiting_only: true,
            limit: 500,
            ..FeedQuery::default()
        }))
        .expect("list");
    let mut out: Vec<FeedRow> = page
        .rows
        .into_iter()
        .filter(|r| ids.contains(&r.id))
        .collect();
    out.sort_by_key(|r| r.description.clone());
    out
}

fn status_of(h: &Harness, s: &Session, id: cl_api::FeedItemId) -> Option<FeedStatus> {
    let page = h
        .run(s.feed().list(&FeedQuery {
            limit: 500,
            ..FeedQuery::default()
        }))
        .expect("list");
    page.rows.iter().find(|r| r.id == id).map(|r| r.status)
}

fn values(account: AccountId) -> FeedChoice {
    FeedChoice {
        account: Some(account),
        ..FeedChoice::default()
    }
}

/// A selection posts in one call: rows with values post, a row without an
/// account is skipped `incomplete`; the replay under the same key is
/// identical; Undo of the group deletes the entries and the rows wait
/// again; a second Undo of the same rows does nothing for them.
pub fn accept_selection_and_undo_group(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let fuel = fresh(h, s, AccountType::Expense, None);
    let rows = import(
        h,
        s,
        bank,
        &[
            (date(2027, 5, 2), "84.17", "A RIVERBEND FUEL"),
            (date(2027, 5, 3), "999999999.99", "B BIG ONE"),
            (date(2027, 5, 4), "12.00", "C NOTHING STAGED"),
        ],
    );
    assert_eq!(rows.len(), 3);
    let req = AcceptRequest::Items(vec![
        AcceptItem::Values(rows[0].id, values(fuel)),
        AcceptItem::Values(rows[1].id, values(fuel)),
        AcceptItem::AsProposed(rows[2].id),
    ]);
    let k = key();
    let o = h
        .run(s.feed().accept(&req, None, k.clone()))
        .expect("accept");
    assert_eq!((o.posted, o.matched), (2, 0));
    assert_eq!(o.skipped, vec![(rows[2].id, AcceptSkip::Incomplete)]);
    assert_eq!(o.failures.len(), 0);
    let again = h.run(s.feed().accept(&req, None, k)).expect("replay");
    assert_eq!(again, o, "a replay answers the same body");
    let group = h.run(s.feed().accept_group(o.undo_group)).expect("group");
    assert_eq!(group.posted, 2);
    for r in &rows[..2] {
        assert_eq!(status_of(h, s, r.id), Some(FeedStatus::Categorized));
    }
    let u = h
        .run(
            s.feed()
                .undo(&UndoTarget::Group(o.undo_group), false, None, key()),
        )
        .expect("undo");
    assert_eq!(u.undone.len(), 2);
    assert!(u.undone.iter().all(|x| x.deleted));
    for r in &rows[..2] {
        assert!(
            status_of(h, s, r.id).is_some_and(FeedStatus::is_waiting),
            "back to waiting"
        );
    }
    // A stale Undo does nothing for those rows.
    let stale = h
        .run(
            s.feed()
                .undo(&UndoTarget::Items(vec![rows[0].id]), false, None, key()),
        )
        .expect("stale undo");
    assert_eq!(stale.undone.len(), 0);
    assert_eq!(stale.failures[0].code, AcceptFailure::NOT_ACCEPTED);
}

/// A single categorize is taken back by its row id (Undo toast).
pub fn undo_one_row(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let exp = fresh(h, s, AccountType::Expense, None);
    let rows = import(h, s, bank, &[(date(2027, 5, 6), "45.00", "SQ MARKET")]);
    let t = h
        .run(s.feed().categorize(rows[0].id, &values(exp), None, key()))
        .expect("posted")
        .expect("entry");
    let u = h
        .run(
            s.feed()
                .undo(&UndoTarget::Items(vec![rows[0].id]), false, None, key()),
        )
        .expect("undo");
    assert_eq!(u.undone.len(), 1);
    assert_eq!(u.undone[0].transaction, t);
    assert!(u.undone[0].deleted);
}

/// The two routes need a key; a rule that doesn't exist is a plain 404.
pub fn keys_required_and_rule_not_found(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    for path in ["/api/v1/banking/feed/accept", "/api/v1/banking/feed/undo"] {
        let raw = h
            .run(s.raw(
                Method::Post,
                path,
                &[],
                Some(&json!({ "feedItemIds": [uuid::Uuid::new_v4().to_string()] })),
            ))
            .expect("raw");
        assert_eq!(raw.status, 400, "{path}: {}", raw.body);
    }
    let e = h
        .run(s.feed().accept(
            &AcceptRequest::Rule(RuleAccept {
                rule_id: uuid::Uuid::new_v4().to_string(),
                kind: "conditional".into(),
                updated_at: "2026-10-01T09:00:00.000Z".into(),
                connection: None,
                after: None,
            }),
            None,
            key(),
        ))
        .expect_err("no such rule");
    assert_eq!(feed_refusal(&e), Some(FeedRefusal::RuleNotFound), "{e:?}");
}

/// Plan §6 test #1 in small: one row on or before the closing date and one
/// open → the open one posts, the locked one fails `TB_PERIOD_LOCKED`;
/// the retry of that row with the password (fresh key) posts it. Runs on
/// tenant B; sets and clears its own closing date and password.
pub fn locked_rows_need_the_password(h: &Harness) {
    if !gated(h)
        || !h.require_cap(Capabilities::FEED_CLOSING_DATE, "U-164")
        || !h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15")
    {
        return;
    }
    let s = h.s3();
    let settings = |lock: serde_json::Value| {
        h.run(s.raw_mapped(
            Method::Put,
            "/api/v1/company/settings",
            Some(&json!({ "lockDate": lock })),
        ))
        .expect("closing date");
    };
    let (lock, set_here) = if let Some(d) = h.run(s.company_settings()).expect("settings").lock_date
    {
        (d, false)
    } else {
        settings(json!("2026-09-30"));
        (date(2026, 9, 30), true)
    };
    let pw = Secret::new(unique("closing-feed"));
    assert!(
        h.run(s.set_closing_password(None, Some(&pw), key()))
            .expect("set password")
    );
    let bank = fresh(h, &s, AccountType::Asset, Some("bank"));
    let exp = fresh(h, &s, AccountType::Expense, None);
    let open = lock.add_days(40).expect("date");
    let rows = import(
        h,
        &s,
        bank,
        &[(lock, "10.00", "A LOCKED"), (open, "20.00", "B OPEN")],
    );
    let req = AcceptRequest::Items(vec![
        AcceptItem::Values(rows[0].id, values(exp)),
        AcceptItem::Values(rows[1].id, values(exp)),
    ]);
    let o = h.run(s.feed().accept(&req, None, key())).expect("accept");
    assert_eq!(o.posted, 1);
    assert_eq!(o.locked(), vec![rows[0].id]);
    let retry = AcceptRequest::Items(vec![AcceptItem::Values(rows[0].id, values(exp))]);
    let auth = ClosedPeriodAuthorization::with_password(pw.clone());
    let o2 = h
        .run(s.feed().accept(&retry, Some(&auth), key()))
        .expect("with password");
    assert_eq!((o2.posted, o2.failures.len()), (1, 0));
    // Undo of the locked entry needs the password too.
    let u = h
        .run(
            s.feed()
                .undo(&UndoTarget::Items(vec![rows[0].id]), false, None, key()),
        )
        .expect("undo");
    assert_eq!(u.locked(), vec![rows[0].id]);
    let u = h
        .run(s.feed().undo(
            &UndoTarget::Items(vec![rows[0].id]),
            false,
            Some(&auth),
            key(),
        ))
        .expect("undo with password");
    assert_eq!(u.undone.len(), 1);
    assert!(
        !h.run(s.set_closing_password(Some(&pw), None, key()))
            .expect("clear password")
    );
    if set_here {
        settings(serde_json::Value::Null);
    }
}
