//! Bank feed review against the live engine (P26 plan §6; E1 U-164, E2
//! U-166/U-168/U-172–U-174, E4 U-171/U-77). Gated on the capabilities:
//! each test returns early (PENDING) on engines before the P26 pin bump.
//!
//! Every test imports its own rows into its own fresh bank account in
//! Contract Co, so feeds never mix. Rows are synthetic and built inline.

use cl_api::{
    AccountType, ApiError, BankFileFormat, BankImportRequest, BankImportRow, Capabilities,
    DuplicateDecision, FeedBucket, FeedChoice, FeedQuery, FeedRefusal, FeedRow, FeedSplitLine,
    FeedStatus, Method, Session, TransferTarget, feed_refusal,
};
use cl_core::ids::AccountId;
use rust_decimal::Decimal;

use crate::common::{account_fields, date, key, m};
use crate::support::{Harness, unique};

fn review(h: &Harness) -> bool {
    h.require_cap(Capabilities::FEED_REVIEW, "U-174")
        && h.require_cap(Capabilities::BANK_IMPORT_BATCH, "U-06")
}

fn fresh_account(h: &Harness, s: &Session, t: AccountType, detail: Option<&str>) -> AccountId {
    let mut f = account_fields(t, "Feed");
    f.detail_type = detail.map(str::to_string);
    h.run(s.accounts().create(&f, key()))
        .unwrap_or_else(|e| panic!("create account: {e}"))
        .record
        .id
}

fn row(line: u32, day: u32, amount: &str, text: &str) -> BankImportRow {
    BankImportRow {
        line,
        date: date(2027, 4, day),
        amount: amount.parse::<Decimal>().expect("decimal"),
        description: text.into(),
        memo: None,
        check_number: None,
        fitid: Some(unique("R")),
        include: None,
        changed_action: None,
    }
}

/// Imports `rows` into `account` and returns the account's waiting rows.
fn import(h: &Harness, s: &Session, account: AccountId, rows: Vec<BankImportRow>) -> Vec<FeedRow> {
    let req = BankImportRequest {
        account,
        format: BankFileFormat::Qfx,
        file_name: "contract.qfx".into(),
        source_file_sha256: "cd".repeat(32),
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
    let done = h.run(s.bank_import().commit(&req, key())).expect("import");
    let ids = done.inserted_items;
    let page = h
        .run(s.feed().list(&FeedQuery {
            waiting_only: true,
            limit: 500,
            ..FeedQuery::default()
        }))
        .expect("list");
    page.rows
        .into_iter()
        .filter(|r| ids.contains(&r.id))
        .collect()
}

/// The P26 capabilities arrive together.
pub fn capabilities(h: &Harness) {
    if !review(h) {
        return;
    }
    let caps = h.s1.client().capabilities();
    for c in [
        Capabilities::FEED_EXACT,
        Capabilities::FEED_MATCH_V2,
        Capabilities::FEED_IDEMPOTENCY,
    ] {
        assert!(caps.has(c), "{c}");
    }
}

/// Amounts come back exactly in the feed sign, and the counts agree with
/// the bucket lists (test 16 in small).
pub fn rows_are_exact_and_counts_agree(h: &Harness) {
    if !review(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, Some("bank"));
    let rows = import(
        h,
        s,
        acct,
        vec![
            row(1, 2, "-84.17", "RIVERBEND FUEL"),
            row(2, 3, "8450.00", "ACH CREDIT HOA"),
            row(3, 3, "-999999999.99", "BIG ONE"),
        ],
    );
    assert_eq!(rows.len(), 3);
    let mut amounts: Vec<String> = rows.iter().map(|r| r.amount.to_wire()).collect();
    amounts.sort();
    // Feed sign: money out positive.
    assert_eq!(amounts, ["-8450.0000", "84.1700", "999999999.9900"]);
    assert!(rows.iter().all(|r| r.bucket.is_some()));
    let conn = rows[0].connection.expect("connection");
    let c = h.run(s.feed().counts(Some(conn))).expect("counts");
    assert_eq!(c.waiting, 3);
    assert_eq!(c.waiting, c.ready + c.review + c.matches);
    for (b, n) in [
        (FeedBucket::Ready, c.ready),
        (FeedBucket::Review, c.review),
        (FeedBucket::Matches, c.matches),
    ] {
        let p = h
            .run(s.feed().list(&FeedQuery {
                bucket: Some(b),
                connection: Some(conn),
                ..FeedQuery::default()
            }))
            .expect("bucket");
        assert_eq!(p.total, n, "{b:?}");
    }
}

/// An owner split posts exactly; one that doesn't add up is refused with
/// the difference.
pub fn split_posts_exactly(h: &Harness) {
    if !review(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, Some("bank"));
    let exp1 = fresh_account(h, s, AccountType::Expense, None);
    let exp2 = fresh_account(h, s, AccountType::Expense, None);
    let rows = import(h, s, acct, vec![row(1, 5, "-84.17", "RIVERBEND FUEL")]);
    let id = rows[0].id;
    let line = |a, v: &str| FeedSplitLine {
        account: a,
        amount: m(v),
        tag: None,
        memo: None,
    };
    let bad = FeedChoice {
        account: Some(exp1),
        splits: vec![line(exp1, "50.00"), line(exp2, "34.16")],
        ..FeedChoice::default()
    };
    let e = h
        .run(s.feed().categorize(id, &bad, None, key()))
        .expect_err("unbalanced");
    let Some(FeedRefusal::SplitsDontBalance { difference }) = feed_refusal(&e) else {
        panic!("refusal {e:?}");
    };
    assert_eq!(difference, Some(m("0.01")));
    let good = FeedChoice {
        splits: vec![line(exp1, "50.00"), line(exp2, "34.17")],
        ..bad
    };
    let k = key();
    let t = h
        .run(s.feed().categorize(id, &good, None, k.clone()))
        .expect("posted");
    assert!(t.is_some());
    // A replay with the same key answers the same entry.
    let again = h
        .run(s.feed().categorize(id, &good, None, k))
        .expect("replay");
    assert_eq!(again, t);
    // Anything else on it now: 409 not pending.
    let e = h.run(s.feed().exclude(id)).expect_err("not waiting");
    assert!(matches!(
        feed_refusal(&e),
        Some(FeedRefusal::NotPending { .. })
    ));
}

/// The review decisions need a key; Review again only on a voided row.
pub fn decisions_need_a_key(h: &Harness) {
    if !review(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, Some("bank"));
    let rows = import(h, s, acct, vec![row(1, 6, "-45.00", "SQ FARMERS MKT")]);
    let id = rows[0].id;
    let raw = h
        .run(s.raw(
            Method::Post,
            &format!("/api/v1/banking/feed/{id}/review-again"),
            &[],
            Some(&serde_json::json!({})),
        ))
        .expect("raw");
    assert_eq!(raw.status, 400, "{}", raw.body);
    assert_eq!(raw.body["error"]["code"], "IDEMPOTENCY_KEY_REQUIRED");
    let e = h
        .run(s.feed().review_again(id, key()))
        .expect_err("not voided");
    assert_eq!(feed_refusal(&e), Some(FeedRefusal::EntryNotVoided));
    let e = h
        .run(
            s.feed()
                .decide_duplicate(id, DuplicateDecision::Keep, key()),
        )
        .expect_err("not a duplicate");
    assert_eq!(feed_refusal(&e), Some(FeedRefusal::NotAPossibleDuplicate));
    h.run(s.feed().exclude(id)).expect("exclude");
    h.run(s.feed().unexclude(id)).expect("restore");
}

/// Plan §6 "Transfer pair → one transfer": money out of one account and in
/// to another post one transfer and both rows are matched.
pub fn transfer_pair(h: &Harness) {
    if !review(h) || !h.require_cap(Capabilities::FEED_MATCH_POST, "U-171") {
        return;
    }
    let s = &h.s1;
    let a = fresh_account(h, s, AccountType::Asset, Some("bank"));
    let b = fresh_account(h, s, AccountType::Asset, Some("bank"));
    let out = import(h, s, a, vec![row(1, 8, "-3000.00", "ONLINE TRANSFER TO")]);
    let inn = import(h, s, b, vec![row(1, 9, "3000.00", "ONLINE TRANSFER FROM")]);
    let c = h
        .run(s.feed().transfer_candidates(out[0].id, 3))
        .expect("candidates");
    assert!(c.iter().any(|t| t.feed_item == inn[0].id));
    let posted = h
        .run(
            s.feed()
                .transfer(out[0].id, TransferTarget::OtherItem(inn[0].id), None, key()),
        )
        .expect("transfer");
    assert!(posted.transaction.is_some());
    assert_eq!(posted.items.len(), 2);
    let page = h
        .run(s.feed().list(&FeedQuery {
            status: Some(FeedStatus::Matched),
            limit: 500,
            ..FeedQuery::default()
        }))
        .expect("matched");
    for id in [out[0].id, inn[0].id] {
        assert!(page.rows.iter().any(|r| r.id == id), "both matched");
    }
}

/// The dashboard reads exact balances for bank accounts.
pub fn account_balances(h: &Harness) {
    if !h.require_cap(Capabilities::ACCOUNT_BALANCES, "U-77") {
        return;
    }
    let b = h.run(h.s1.feed().account_balances()).expect("balances");
    assert!(
        b.iter()
            .all(|x| x.bank_file.is_some() == x.bank_file_date.is_some())
    );
}

/// Without the capabilities nothing is sent and the call says so.
pub fn older_engines_refuse_locally(h: &Harness) {
    if h.s1.client().capabilities().has(Capabilities::FEED_REVIEW) {
        return;
    }
    let e = h.run(h.s1.feed().counts(None)).expect_err("local");
    assert!(matches!(e, ApiError::Local(_)));
}
