use super::*;
use crate::resources::testkit::{company_session, json, query, requests, session_with_caps};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const A: &str = "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const B: &str = "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const ACCT: &str = "3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ACCT2: &str = "3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const TXN: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const LINE: &str = "6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

fn id(s: &str) -> FeedItemId {
    FeedItemId::parse(s).expect("id")
}

fn page_body() -> String {
    format!(
        r#"{{"data":[
        {{"id":"{A}","bankConnectionId":"{B}","bankAccountName":"Operating","feedDate":"2026-11-03",
          "amount":"84.1700","description":"RIVERBEND FUEL #212","status":"pending","bucket":"ready",
          "suggestedAccountId":"{ACCT}","suggestedAccountName":"Fuel","confidenceScore":"0.9500","matchType":"rule",
          "suggestedRule":{{"id":"r1","kind":"conditional","name":"Riverbend","autoAccept":false,"updatedAt":"2026-11-01T10:00:00Z"}},
          "possibleDuplicate":false,"duplicateOf":null,"entryVoided":false,"matchCandidateCount":0,"checkNumber":4412,
          "transferCandidate":{{"feedItemId":"{B}","accountId":"{ACCT}","accountName":"Savings"}}}},
        {{"id":"{B}","feedDate":"2026-11-02","amount":"-8450.00","description":"ACH CREDIT","status":"categorized",
          "bucket":null,"entryVoided":true,"entryVoidedAt":"2026-11-04T09:00:00Z","matchedTransactionId":"{TXN}",
          "matchedJournalLineId":"{LINE}"}},
        {{"id":"7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03","feedDate":"2026-11-02","amount":-10,"status":"pending"}}
    ],"total":40}}"#
    )
}

#[test]
fn a_page_reads_exact_rows_in_the_feed_sign() {
    let p = feed_page_from(page_body().as_bytes()).expect("page");
    assert_eq!(p.total, 40);
    assert_eq!(p.skipped, 1, "a JSON number is never an amount");
    assert_eq!(p.rows.len(), 2);
    let r = &p.rows[0];
    assert_eq!(r.amount.to_wire(), "84.1700");
    assert!(r.is_money_out());
    assert_eq!(r.display_amount().to_wire(), "-84.1700");
    assert_eq!(r.bucket, Some(FeedBucket::Ready));
    assert_eq!(r.status, FeedStatus::Pending);
    assert_eq!(r.confidence, Some(Decimal::new(9500, 4)));
    assert_eq!(r.check_number.as_deref(), Some("4412"));
    assert_eq!(
        r.suggested_account.as_ref().map(|a| a.name.as_str()),
        Some("Fuel")
    );
    let rule = r.suggested_rule.as_ref().expect("rule");
    assert_eq!(
        (rule.kind.as_str(), rule.auto_accept),
        ("conditional", false)
    );
    let hint = r.transfer_candidate.as_ref().expect("transfer counterpart");
    assert_eq!(hint.feed_item, id(B));
    assert_eq!(hint.account_name.as_deref(), Some("Savings"));
    let v = &p.rows[1];
    assert!(v.transfer_candidate.is_none());
    assert!(v.entry_voided);
    assert!(v.status.is_accepted());
    assert_eq!(v.matched_line.map(|l| l.to_string()).as_deref(), Some(LINE));
    let dbg = format!("{p:?}");
    assert!(!dbg.contains("84.17") && !dbg.contains("RIVERBEND") && !dbg.contains("Fuel"));
}

#[test]
fn the_accepted_filter_is_one_status() {
    let q = FeedQuery {
        accepted: true,
        waiting_only: true,
        ..FeedQuery::default()
    }
    .to_query()
    .expect("query");
    assert!(q.contains(&("status".into(), "accepted".into())));
    assert!(!q.iter().any(|(k, _)| k == "actionableOnly"));
}

#[test]
fn counts_and_queries() {
    let c = feed_counts_from(
        br#"{"waiting":37,"ready":28,"review":6,"matches":3,"possibleDuplicate":1,"accepted":4,"entryVoided":1,"excluded":2,"inProgress":0}"#,
    )
    .expect("counts");
    assert_eq!((c.waiting, c.ready, c.review, c.matches), (37, 28, 6, 3));
    assert_eq!(c.waiting, c.ready + c.review + c.matches);
    assert_eq!(c.entry_voided, 1);
    let q = FeedQuery {
        waiting_only: true,
        bucket: Some(FeedBucket::Review),
        entry_voided: Some(false),
        limit: 500,
        offset: 500,
        ..FeedQuery::default()
    };
    let text: Vec<String> = q
        .to_query()
        .expect("q")
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    assert_eq!(
        text,
        [
            "actionableOnly=true",
            "bucket=review",
            "entryVoided=false",
            "sortBy=feedDate",
            "sortDir=desc",
            "limit=500",
            "offset=500"
        ]
    );
    let bad = FeedQuery {
        limit: 501,
        ..FeedQuery::default()
    };
    assert!(matches!(
        bad.to_query(),
        Err(ApiError::Local(LocalValidation::PageLimit))
    ));
}

#[test]
fn a_split_must_have_two_to_fifty_whole_cent_lines() {
    let acct: AccountId = ACCT.parse().expect("acct");
    let line = |a: &str| FeedSplitLine {
        account: acct,
        amount: Money::parse_wire(a).expect("m"),
        tag: None,
        memo: None,
    };
    let mut c = FeedChoice {
        account: Some(acct),
        splits: vec![line("10.00")],
        ..FeedChoice::default()
    };
    assert!(matches!(
        choice_body(&c),
        Err(ApiError::Local(LocalValidation::TooFewLines))
    ));
    c.splits = vec![line("10.00"), line("0.005")];
    assert!(matches!(
        choice_body(&c),
        Err(ApiError::Local(LocalValidation::NotWholeCents))
    ));
    c.splits = vec![line("10.00"), line("0.00")];
    assert!(matches!(
        choice_body(&c),
        Err(ApiError::Local(LocalValidation::NonPositiveAmount))
    ));
    c.splits = vec![line("999999999999.99"), line("0.01")];
    let body = choice_body(&c).expect("body");
    assert_eq!(body["splits"][0]["amount"], "999999999999.99");
    assert_eq!(body["splits"][1]["amount"], "0.01");
    c.splits = (0..51).map(|_| line("1.00")).collect();
    assert!(matches!(
        choice_body(&c),
        Err(ApiError::Local(LocalValidation::TooManyLines))
    ));
    let none = FeedChoice::default();
    assert!(choice_body(&none).is_err(), "an account is needed");
}

#[test]
fn a_deposit_body_lists_payments_once_and_at_most_ten_fees() {
    let p: TransactionId = TXN.parse().expect("txn");
    let fee = FeeLine {
        account: ACCT.parse().expect("acct"),
        amount: Money::parse_wire("20.54").expect("m"),
        memo: Some("processor".into()),
    };
    let body = deposit_body(&[p], std::slice::from_ref(&fee)).expect("body");
    assert_eq!(body["feeLines"][0]["amount"], "20.54");
    assert_eq!(body["sourceTransactionIds"][0], TXN);
    assert!(deposit_body(&[p, p], &[]).is_err(), "no repeats");
    assert!(deposit_body(&[], &[]).is_err());
    let eleven: Vec<FeeLine> = (0..11).map(|_| fee.clone()).collect();
    assert!(deposit_body(&[p], &eleven).is_err());
    assert!(!format!("{fee:?}").contains("20.54"));
}

#[test]
fn refusals_name_the_engine_codes() {
    let body = br#"{"error":{"message":"m","code":"SPLITS_DONT_BALANCE","details":{"kind":"fixed","difference":"0.01","splitTotal":"84.16","amount":"84.17"}}}"#;
    let e = crate::errors::from_response_for_tests(400, body);
    let Some(FeedRefusal::SplitsDontBalance { difference }) = feed_refusal(&e) else {
        panic!("refusal {e:?}");
    };
    assert_eq!(difference.map(Money::to_wire).as_deref(), Some("0.0100"));
    let e = crate::errors::from_response_for_tests(
        409,
        br#"{"error":{"message":"m","code":"FEED_ITEM_NOT_PENDING","details":{"status":"categorized"}}}"#,
    );
    assert!(matches!(
        feed_refusal(&e),
        Some(FeedRefusal::NotPending {
            status: Some(FeedStatus::Categorized)
        })
    ));
    assert!(is_stale_row(&e));
    let e = crate::errors::from_response_for_tests(
        400,
        format!(r#"{{"error":{{"message":"m","code":"LINE_REQUIRED","details":{{"journalLineIds":["{LINE}"]}}}}}}"#)
            .as_bytes(),
    );
    let Some(FeedRefusal::LineRequired { lines }) = feed_refusal(&e) else {
        panic!("line required");
    };
    assert_eq!(lines.len(), 1);
    let e = crate::errors::from_response_for_tests(
        400,
        br#"{"error":{"message":"m","code":"FEED_ITEM_MISMATCH","details":{"difference":"-0.01"}}}"#,
    );
    assert!(matches!(
        feed_refusal(&e),
        Some(FeedRefusal::ItemMismatch {
            difference: Some(_),
            ..
        })
    ));
    let e = crate::errors::from_response_for_tests(
        403,
        br#"{"error":{"message":"no","code":"FORBIDDEN"}}"#,
    );
    assert!(is_view_only(&e));
}

#[tokio::test]
async fn categorize_needs_feed_exact_and_sends_its_key_and_split() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    let choice = FeedChoice {
        account: Some(ACCT.parse().expect("acct")),
        ..FeedChoice::default()
    };
    assert!(matches!(
        s.feed()
            .categorize(id(A), &choice, None, IdempotencyKey::new())
            .await,
        Err(ApiError::Local(LocalValidation::Unsupported))
    ));
    assert!(requests(&server, "PUT").await.is_empty(), "nothing sent");

    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/banking/feed/{A}/categorize")))
        .respond_with(json(200, &format!(r#"{{"transaction":{{"id":"{TXN}"}}}}"#)))
        .mount(&server)
        .await;
    let s = session_with_caps(
        &server,
        &[Capabilities::FEED_EXACT, Capabilities::FEED_IDEMPOTENCY],
    )
    .await;
    let acct2: AccountId = ACCT2.parse().expect("acct");
    let split = FeedChoice {
        account: Some(ACCT.parse().expect("acct")),
        splits: vec![
            FeedSplitLine {
                account: ACCT.parse().expect("acct"),
                amount: Money::parse_wire("50.00").expect("m"),
                tag: None,
                memo: None,
            },
            FeedSplitLine {
                account: acct2,
                amount: Money::parse_wire("34.17").expect("m"),
                tag: None,
                memo: None,
            },
        ],
        ..FeedChoice::default()
    };
    let key = IdempotencyKey::new();
    let t = s
        .feed()
        .categorize(id(A), &split, None, key.clone())
        .await
        .expect("posted");
    assert_eq!(t.map(|t| t.to_string()).as_deref(), Some(TXN));
    let puts = requests(&server, "PUT").await;
    assert_eq!(puts.len(), 1);
    assert_eq!(
        puts[0]
            .headers
            .get("idempotency-key")
            .and_then(|h| h.to_str().ok()),
        Some(key.as_header().as_str())
    );
    let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
    assert_eq!(body["splits"][1]["amount"], "34.17");
    assert!(body.get("closingPassword").is_none());
}

#[tokio::test]
async fn review_routes_need_their_capabilities() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    let f = s.feed();
    let q = FeedQuery {
        bucket: Some(FeedBucket::Ready),
        ..FeedQuery::default()
    };
    let local = |r: Result<(), ApiError>| {
        assert!(matches!(
            r,
            Err(ApiError::Local(LocalValidation::Unsupported))
        ));
    };
    local(f.list(&q).await.map(|_| ()));
    local(f.counts(None).await.map(|_| ()));
    local(f.match_candidates(id(A)).await.map(|_| ()));
    local(f.review_again(id(A), IdempotencyKey::new()).await);
    local(
        f.decide_duplicate(id(A), DuplicateDecision::Keep, IdempotencyKey::new())
            .await,
    );
    local(
        f.transfer(
            id(A),
            TransferTarget::OtherItem(id(B)),
            None,
            IdempotencyKey::new(),
        )
        .await
        .map(|_| ()),
    );
    local(f.account_balances().await.map(|_| ()));
    let all = server.received_requests().await.unwrap_or_default();
    assert!(all.is_empty(), "nothing sent");
}

#[tokio::test]
async fn list_counts_and_decisions_reach_the_engine() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/banking/feed"))
        .respond_with(json(200, &page_body()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/banking/feed/counts"))
        .respond_with(json(
            200,
            r#"{"waiting":3,"ready":1,"review":1,"matches":1,"possibleDuplicate":0,"accepted":0,"entryVoided":0,"excluded":0,"inProgress":0}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/banking/feed/{A}/duplicate")))
        .respond_with(json(200, r#"{"item":{},"decision":"remove"}"#))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/banking/feed/{A}/transfer")))
        .respond_with(json(
            200,
            &format!(
                r#"{{"transaction":{{"id":"{TXN}"}},"items":[{{"feedItemId":"{A}","journalLineId":"{LINE}"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(
        &server,
        &[
            Capabilities::FEED_REVIEW,
            Capabilities::FEED_MATCH_V2,
            Capabilities::FEED_IDEMPOTENCY,
            Capabilities::FEED_MATCH_POST,
        ],
    )
    .await;
    let f = s.feed();
    let conn = BankConnectionId::parse(B).expect("conn");
    let page = f
        .list(&FeedQuery {
            bucket: Some(FeedBucket::Ready),
            connection: Some(conn),
            ..FeedQuery::default()
        })
        .await
        .expect("page");
    assert_eq!(page.rows.len(), 2);
    let c = f.counts(Some(conn)).await.expect("counts");
    assert_eq!(c.waiting, 3);
    f.decide_duplicate(id(A), DuplicateDecision::Remove, IdempotencyKey::new())
        .await
        .expect("decided");
    let posted = f
        .transfer(
            id(A),
            TransferTarget::OtherItem(id(B)),
            None,
            IdempotencyKey::new(),
        )
        .await
        .expect("transfer");
    assert_eq!(posted.items.len(), 1);
    let gets: Vec<String> = requests(&server, "GET")
        .await
        .iter()
        .filter(|r| r.url.path().starts_with("/api/v1/banking"))
        .map(query)
        .collect();
    assert_eq!(
        gets[0],
        format!("bucket=ready&bankConnectionId={B}&sortBy=feedDate&sortDir=desc&limit=50&offset=0")
    );
    assert_eq!(gets[1], format!("bankConnectionId={B}"));
    let posts = requests(&server, "POST").await;
    let dup: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(dup, json!({"decision": "remove"}));
    assert!(
        posts
            .iter()
            .all(|p| p.headers.get("idempotency-key").is_some())
    );
    let tr: Value = serde_json::from_slice(&posts[1].body).expect("json");
    assert_eq!(tr, json!({"otherFeedItemId": B}));
}

#[test]
fn balances_are_exact_or_refused() {
    let ok = format!(
        r#"[{{"accountId":"{ACCT}","name":"Operating","accountNumber":"1010","kind":"bank","booksBalance":"86902.13",
        "bankFileBalance":"86902.13","bankFileDate":"2026-10-31","booksBalanceAtFileDate":"86902.13"}},
        {{"accountId":"{ACCT2}","name":"Card","accountNumber":null,"kind":"card","booksBalance":"12.00",
        "bankFileBalance":null,"bankFileDate":null,"booksBalanceAtFileDate":null}}]"#
    );
    let b = account_balances_from(ok.as_bytes()).expect("balances");
    assert_eq!(
        b[0].bank_file.map(Money::to_wire).as_deref(),
        Some("86902.1300")
    );
    assert!(b[1].is_card && b[1].bank_file.is_none());
    let bad = format!(
        r#"[{{"accountId":"{ACCT}","name":"Operating","kind":"bank","booksBalance":86902.13}}]"#
    );
    assert!(account_balances_from(bad.as_bytes()).is_err());
}

#[test]
fn candidates_and_overlaps_read_lines() {
    let c = match_candidates_from(
        format!(
            r#"{{"candidates":[{{"id":"{TXN}","journalLineId":"{LINE}","txnType":"check","txnNumber":4412,"txnDate":"2026-11-02","amount":"925.00","memo":null,"contactName":"Midwest"}},
            {{"id":"{TXN}","journalLineId":"{LINE}","amount":925}}]}}"#
        )
        .as_bytes(),
    )
    .expect("candidates");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].txn_number.as_deref(), Some("4412"));
    let o = payroll_overlaps_from(
        format!(
            r#"{{"overlaps":[{{"txnId":"{TXN}","journalLineId":"{LINE}","memo":"Net pay","date":"2026-11-01","amount":"7412.38"}}]}}"#
        )
        .as_bytes(),
    )
    .expect("overlaps");
    assert_eq!(o[0].amount.to_wire(), "7412.3800");
    let t = transfer_candidates_from(
        format!(
            r#"{{"candidates":[{{"feedItemId":"{B}","accountId":"{ACCT2}","accountName":"Payroll","date":"2026-11-02","amount":"-3000.00"}}]}}"#
        )
        .as_bytes(),
    )
    .expect("transfers");
    assert_eq!(t[0].amount.to_wire(), "-3000.0000");
    let p = posted_from(
        format!(
            r#"{{"transaction":{{"id":"{TXN}"}},"items":[{{"feedItemId":"{A}","journalLineId":"{LINE}"}}],"payments":[{{"id":"{TXN}","amount":"720.00"}}]}}"#
        )
        .as_bytes(),
    )
    .expect("posted");
    assert_eq!(p.payments[0].1.to_wire(), "720.0000");
}
