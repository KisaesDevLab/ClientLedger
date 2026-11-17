//! Unit tests of the payroll import resource (P30), against a mock engine.
//! The live engine is covered by `tests/contract/contract_payroll.rs`
//! (pending until the pin has the payroll capabilities).

use super::*;
use crate::resources::testkit::{json, requests, session_with_caps};
use cl_core::Secret;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer};

const SESSION: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TAG: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const ACCOUNT: &str = "5c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03";
const ENTRY: &str = "6d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a04";

const ALL: [&str; 4] = [
    Capabilities::PAYROLL_IMPORT_EXACT,
    Capabilities::PAYROLL_IMPORT_POST,
    Capabilities::PAYROLL_IMPORT_SENSITIVE,
    Capabilities::CLOSING_PASSWORD,
];

fn day(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn sid() -> PayrollSessionId {
    SESSION.parse().expect("session")
}

fn body_of(r: &wiremock::Request) -> Value {
    serde_json::from_slice(&r.body).expect("json body")
}

const PREVIEW: &str = r#"{"previewToken":"tok-1","previews":[
 {"date":"2025-01-10","memo":"Payroll 2024-12-28–2025-01-10 — Gusto — pay date 2025-01-10",
  "lines":[
   {"lineType":"gross_wages_expense","description":"Gross Wages Expense","accountId":"5c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03",
    "accountName":"Wages","accountNumber":"6000","debit":"1191.04","credit":"0.00","department":"Front","tagId":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02"},
   {"lineType":"payroll_clearing","description":"Payroll Clearing / Cash","accountId":null,"accountName":null,
    "accountNumber":null,"debit":"0.00","credit":"1191.04"}],
  "totalDebits":"1191.04","totalCredits":"1191.04","difference":"0.00","isBalanced":true}]}"#;

#[tokio::test]
async fn nothing_is_sent_without_all_three_capabilities() {
    let server = MockServer::start().await;
    let s = session_with_caps(
        &server,
        &[
            Capabilities::PAYROLL_IMPORT_EXACT,
            Capabilities::PAYROLL_IMPORT_POST,
        ],
    )
    .await;
    let e = s.payroll().validate(sid()).await.expect_err("refused");
    assert!(matches!(e, ApiError::Local(LocalValidation::Unsupported)));
    assert!(requests(&server, "POST").await.is_empty());
    assert!(!payroll_supported(&s.client().capabilities()));
}

#[tokio::test]
async fn preview_reads_exact_strings_and_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/payroll-import/sessions/{SESSION}/generate-je"
        )))
        .respond_with(json(200, PREVIEW))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let mut depts = BTreeMap::new();
    depts.insert("Front".to_string(), Some(TAG.parse().expect("tag")));
    depts.insert("Kitchen".to_string(), None);
    let p = s.payroll().preview(sid(), &depts).await.expect("preview");
    assert_eq!(p.token.as_deref(), Some("tok-1"));
    assert!(p.is_balanced());
    let e = &p.entries[0];
    assert_eq!(e.date, day("2025-01-10"));
    assert_eq!(e.total_debits, Money::parse_wire("1191.04").expect("m"));
    assert_eq!(e.difference, Money::parse_wire("0.00").expect("m"));
    assert_eq!(e.lines[0].department.as_deref(), Some("Front"));
    assert_eq!(e.lines[0].account, Some(ACCOUNT.parse().expect("account")));
    assert_eq!(e.lines[1].account, None);
    let sent = body_of(&requests(&server, "POST").await[0]);
    assert_eq!(sent["aggregationMode"], "summary");
    assert_eq!(sent["departmentTagMap"]["Front"], TAG);
    assert!(sent["departmentTagMap"]["Kitchen"].is_null());
    // Debug never shows department names or amounts.
    let dbg = format!("{p:?}");
    assert!(!dbg.contains("Front") && !dbg.contains("1191"), "{dbg}");
}

#[tokio::test]
async fn a_json_number_amount_is_refused_never_rounded() {
    let body = PREVIEW.replace(r#""debit":"1191.04""#, r#""debit":1191.04"#);
    assert!(matches!(
        preview_from(body.as_bytes(), true),
        Err(ApiError::InexactMoney { .. })
    ));
    let no_diff = PREVIEW.replace(r#""difference":"0.00","#, "");
    assert!(
        preview_from(no_diff.as_bytes(), true).is_err(),
        "difference is required"
    );
}

/// S8: with `payroll-import-post-v1` the engine always sends the token; a
/// preview without one is a contract break, never a post that skips the
/// engine's "entries changed" check.
#[tokio::test]
async fn a_missing_preview_token_is_a_contract_break() {
    for missing in [
        PREVIEW.replace(r#""previewToken":"tok-1","#, ""),
        PREVIEW.replace(r#""previewToken":"tok-1""#, r#""previewToken":"""#),
    ] {
        assert!(matches!(
            preview_from(missing.as_bytes(), true),
            Err(ApiError::Decode { .. })
        ));
        assert_eq!(
            preview_from(missing.as_bytes(), false)
                .expect("older")
                .token,
            None
        );
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v1/payroll-import/sessions/{SESSION}/generate-je"
            )))
            .respond_with(json(200, &missing))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &ALL).await;
        assert!(matches!(
            s.payroll().preview(sid(), &BTreeMap::new()).await,
            Err(ApiError::Decode { .. })
        ));
    }
}

/// M1: History sends the status and reads every page.
#[tokio::test]
async fn history_sends_the_status_and_reads_every_page() {
    let row = |n: u32| {
        format!(
            r#"{{"id":"3a0c4e8e-6f6e-4c1d-8a37-{n:012}","status":"mapped","importMode":"employee_level",
            "detectedProvider":"custom","payPeriodStart":"2025-07-01","payPeriodEnd":"2025-07-11",
            "checkDate":"2025-07-11","rowCount":3,"errorCount":0,"journalEntryIds":[]}}"#
        )
    };
    let first: Vec<String> = (0..50).map(row).collect();
    let page1 = format!(r#"{{"data":[{}],"total":51}}"#, first.join(","));
    let page2 = format!(r#"{{"data":[{}],"total":51}}"#, row(50));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/payroll-import/sessions"))
        .and(wiremock::matchers::query_param("offset", "0"))
        .respond_with(json(200, &page1))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/payroll-import/sessions"))
        .and(wiremock::matchers::query_param("offset", "50"))
        .respond_with(json(200, &page2))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let all = s
        .payroll()
        .sessions_all(Some(SessionStatus::Mapped))
        .await
        .expect("history");
    assert_eq!(all.sessions.len(), 51);
    assert_eq!(all.total, 51);
    let sent: Vec<_> = requests(&server, "GET")
        .await
        .into_iter()
        .filter(|r| r.url.path() == "/api/v1/payroll-import/sessions")
        .collect();
    assert_eq!(sent.len(), 2, "two pages");
    for r in &sent {
        assert!(
            r.url.query().unwrap_or_default().contains("status=mapped"),
            "{:?}",
            r.url.query()
        );
    }
}

/// S3: the check lists every department in the file (with the review
/// capability); an older engine's answer has none (the screen reads the
/// sample rows). Debug never shows department names.
#[test]
fn the_check_lists_every_department() {
    let v = validation_from(
        br#"{"totalRows":30,"errorRows":0,"warningRows":0,"messages":[],
        "departments":["Kitchen"," Front ","","Patio"],"departmentsTruncated":true}"#,
    )
    .expect("check");
    assert_eq!(
        v.departments.as_deref(),
        Some(&["Kitchen".to_string(), "Front".into(), "Patio".into()][..])
    );
    assert!(v.departments_truncated);
    let dbg = format!("{v:?}");
    assert!(!dbg.contains("Kitchen") && !dbg.contains("Patio"), "{dbg}");
    let old = validation_from(br#"{"totalRows":3,"errorRows":0,"warningRows":0,"messages":[]}"#)
        .expect("old");
    assert_eq!(old.departments, None);
    assert!(!old.departments_truncated);
}

#[tokio::test]
async fn post_always_asks_for_summary_and_discard_and_sends_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/payroll-import/sessions/{SESSION}/post"
        )))
        .respond_with(json(
            200,
            &format!(r#"{{"journalEntryIds":["{ENTRY}"],"count":1}}"#),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let req = PostRequest {
        line_tags: vec![LineTag {
            entry: 0,
            line: 1,
            date: day("2025-01-10"),
            tag: Some(TAG.parse().expect("tag")),
        }],
        preview_token: Some("tok-1".into()),
        ..PostRequest::default()
    };
    let key = IdempotencyKey::new();
    let out = s
        .payroll()
        .post(sid(), &req, key.clone())
        .await
        .expect("post");
    assert_eq!(
        out,
        PostOutcome::Posted {
            entries: vec![ENTRY.parse().expect("entry")],
            replayed: false
        }
    );
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 1, "sent once");
    let sent = body_of(&posts[0]);
    assert_eq!(sent["aggregationMode"], "summary");
    assert_eq!(sent["discardEmployeeDetail"], true);
    assert_eq!(sent["forcePost"], false);
    assert_eq!(sent["previewToken"], "tok-1");
    assert_eq!(sent["lineTags"][0]["jeIndex"], 0);
    assert_eq!(sent["lineTags"][0]["lineIndex"], 1);
    assert_eq!(sent["lineTags"][0]["date"], "2025-01-10");
    assert_eq!(sent["lineTags"][0]["tagId"], TAG);
    assert!(
        sent.get("tagId").is_none(),
        "the post-level tag is never used"
    );
    assert!(sent.get("closingPassword").is_none());
    assert!(posts[0].headers.get("idempotency-key").is_some());
}

#[tokio::test]
async fn post_and_reverse_are_sent_once_even_on_a_gateway_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/(post|reverse)$"))
        .respond_with(json(503, r#"{"error":{"message":"busy"}}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let _ = s
        .payroll()
        .post(sid(), &PostRequest::default(), IdempotencyKey::new())
        .await
        .expect_err("busy");
    let _ = s
        .payroll()
        .reverse(sid(), "Wrong file", None, IdempotencyKey::new())
        .await
        .expect_err("busy");
    assert_eq!(
        requests(&server, "POST").await.len(),
        2,
        "one each, never retried"
    );
}

#[tokio::test]
async fn an_overlap_answer_needs_confirmation_and_drops_file_names() {
    let reply = r#"{"requiresConfirmation":true,"overlaps":[{"sessionId":"3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a09",
      "filename":"Test Worker payroll.csv","payPeriod":"2024-12-28 to 2025-01-10","postedDate":"1/11/2025",
      "matchingDates":["2025-01-10"]}]}"#;
    let out = post_from(reply.as_bytes(), false).expect("read");
    let PostOutcome::NeedsConfirmation { overlaps } = out else {
        panic!("needs confirmation")
    };
    assert_eq!(
        overlaps[0].period,
        Some((day("2024-12-28"), day("2025-01-10")))
    );
    assert_eq!(overlaps[0].matching_dates, vec![day("2025-01-10")]);
    assert!(!format!("{overlaps:?}").contains("Test Worker"));
}

#[tokio::test]
async fn the_closing_password_rides_in_the_body_and_423_maps_to_closed_period() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/post$"))
        .respond_with(json(
            423,
            r#"{"error":{"message":"locked","code":"TB_PERIOD_LOCKED","details":{"lockDate":"2025-03-31","canOverride":false,"closingPasswordRequired":true}}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/reverse$"))
        .respond_with(json(
            403,
            r#"{"error":{"message":"wrong","code":"CLOSING_PASSWORD_INVALID"}}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let e = s
        .payroll()
        .post(sid(), &PostRequest::default(), IdempotencyKey::new())
        .await
        .expect_err("locked");
    assert!(matches!(e, ApiError::ClosedPeriod(_)));
    let e = s
        .payroll()
        .reverse(
            sid(),
            "Wrong file",
            Some(ClosedPeriodAuthorization::with_password(Secret::new("pw"))),
            IdempotencyKey::new(),
        )
        .await
        .expect_err("wrong password");
    assert!(matches!(e, ApiError::ClosingPasswordRejected));
    let posts = requests(&server, "POST").await;
    let rev = body_of(&posts[1]);
    assert_eq!(rev["reason"], "Wrong file");
    assert_eq!(rev["closingPassword"], "pw");
    let blank = s
        .payroll()
        .reverse(sid(), "  ", None, IdempotencyKey::new())
        .await
        .expect_err("blank reason");
    assert!(matches!(
        blank,
        ApiError::Local(LocalValidation::InvalidValue)
    ));
}

/// The engine's refusal as the client maps it (status and body).
async fn refusal(status: u16, code: &str, details: &str) -> ApiError {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/post$"))
        .respond_with(json(
            status,
            &format!(r#"{{"error":{{"message":"x","code":"{code}","details":{details}}}}}"#),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    s.payroll()
        .post(sid(), &PostRequest::default(), IdempotencyKey::new())
        .await
        .expect_err("refused")
}

#[tokio::test]
async fn refusals_are_read_from_codes_and_details() {
    assert_eq!(
        payroll_refusal(
            &refusal(
                409,
                "PAYROLL_DUPLICATE_FILE",
                &format!(r#"{{"priorSessionId":"{SESSION}","postedAt":"2025-01-11"}}"#)
            )
            .await
        ),
        Some(PayrollRefusal::DuplicateFile {
            prior: Some(sid()),
            posted_at: Some("2025-01-11".into())
        })
    );
    for (code, want) in [
        ("PAYROLL_ALREADY_POSTED", PayrollRefusal::AlreadyPosted),
        ("PAYROLL_NOT_READY", PayrollRefusal::NotReady),
        ("PAYROLL_PREVIEW_CHANGED", PayrollRefusal::PreviewChanged),
        ("PAYROLL_NOT_POSTED", PayrollRefusal::NotPosted),
    ] {
        let e = refusal(409, code, r#"{"status":"posted"}"#).await;
        assert_eq!(payroll_refusal(&e), Some(want), "{code}");
    }
    let unbalanced = refusal(
        422,
        "PAYROLL_UNBALANCED",
        r#"{"date":"2025-01-10","totalDebits":"10.00","totalCredits":"9.95","difference":"0.05"}"#,
    )
    .await;
    assert_eq!(
        payroll_refusal(&unbalanced),
        Some(PayrollRefusal::Unbalanced {
            date: Some(day("2025-01-10")),
            difference: Some(Money::parse_wire("0.05").expect("m"))
        })
    );
    let unmapped = refusal(
        422,
        "PAYROLL_UNMAPPED",
        r#"{"lineTypes":["fit_payable","payroll_clearing"]}"#,
    )
    .await;
    assert_eq!(
        payroll_refusal(&unmapped),
        Some(PayrollRefusal::Unmapped {
            line_types: vec!["fit_payable".into(), "payroll_clearing".into()]
        })
    );
    assert_eq!(
        payroll_refusal(&refusal(409, "SOMETHING_ELSE", "{}").await),
        None
    );
}
#[tokio::test]
async fn upload_sends_the_period_and_a_neutral_file_name() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/payroll-import/upload"))
        .respond_with(json(
            201,
            &format!(
                r#"{{"session":{{"id":"{SESSION}"}},"preview":{{"headers":["Employee Name","Gross Pay"],
                "sampleRows":[["Test Worker 01","$1,845.35"]],"detectedProvider":"gusto","importMode":"employee_level",
                "isDuplicate":false,"isDuplicateByKey":true,"rowCount":8}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let req = UploadRequest {
        mode: ImportMode::EmployeeLevel,
        bytes: b"Employee Name,Gross Pay\nTest Worker 01,1.00\n".to_vec(),
        file_name: "Jan payroll for Test Worker.CSV".into(),
        period_start: day("2024-12-28"),
        period_end: day("2025-01-24"),
        check_date: day("2025-01-24"),
    };
    let out = s.payroll().upload(&req).await.expect("upload");
    assert_eq!(out.session, sid());
    assert!(out.is_duplicate_by_key && !out.is_duplicate);
    assert_eq!(out.row_count, 8);
    assert!(!format!("{out:?}").contains("Test Worker"));
    let sent = String::from_utf8_lossy(&requests(&server, "POST").await[0].body).to_string();
    assert!(sent.contains("filename=\"payroll.csv\""));
    assert!(!sent.contains("Jan payroll"));
    for part in ["2024-12-28", "2025-01-24", "employee_level"] {
        assert!(sent.contains(part), "{part}");
    }
}

#[test]
fn uploads_are_checked_before_sending() {
    let ok = UploadRequest {
        mode: ImportMode::PrebuiltJe,
        bytes: b"x".to_vec(),
        file_name: "a.xlsx".into(),
        period_start: day("2025-01-01"),
        period_end: day("2025-01-14"),
        check_date: day("2025-01-14"),
    };
    assert_eq!(ok.check(), Ok("xlsx".into()));
    let big = UploadRequest {
        bytes: vec![b'a'; MAX_UPLOAD_BYTES + 1],
        ..ok.clone()
    };
    assert_eq!(big.check(), Err(LocalValidation::AttachmentTooLarge));
    let pdf = UploadRequest {
        file_name: "a.pdf".into(),
        ..ok.clone()
    };
    assert_eq!(pdf.check(), Err(LocalValidation::AttachmentType));
    let backwards = UploadRequest {
        period_end: day("2024-12-31"),
        ..ok
    };
    assert_eq!(backwards.check(), Err(LocalValidation::InvalidValue));
}

#[test]
fn column_maps_round_trip_through_a_template() {
    let mut map = ColumnMap {
        header_row: 3,
        data_start_row: 4,
        skip_footer_rows: 1,
        date_format: Some("MM/DD/YYYY".into()),
        ..ColumnMap::default()
    };
    map.mappings.insert(
        "gross_pay".into(),
        ColumnSource {
            source: "Wages".into(),
            format: None,
        },
    );
    map.skip_rules.push(SkipRule {
        field: "employee_name".into(),
        values: Some(vec!["Total".into()]),
    });
    map.department_tags
        .insert("Front".into(), Some(TAG.parse().expect("tag")));
    // A layout saved before S4: no left-out list (every unmatched column
    // is new next time).
    let back = ColumnMap::from_value(&map.template_value()).expect("read back");
    assert_eq!(back.left_out, None);
    map.left_out = Some(vec!["Notes".into(), "Hours".into()]);
    let back = ColumnMap::from_value(&map.template_value()).expect("read back");
    assert_eq!(back.sources(), vec!["Wages".to_string()]);
    assert_eq!(back, map);
    let apply = map.apply_body();
    assert!(
        apply.get("clDepartmentTags").is_none() && apply.get("clLeftOutColumns").is_none(),
        "desktop-only keys stay out of apply-mapping"
    );
    assert_eq!(apply["mappings"]["gross_pay"]["source"], "Wages");
}

#[test]
fn history_never_reads_the_file_name() {
    let body = format!(
        r#"{{"data":[{{"id":"{SESSION}","status":"posted","importMode":"employee_level","detectedProvider":"gusto",
        "originalFilename":"Test Worker.csv","payPeriodStart":"2024-12-28","payPeriodEnd":"2025-01-24",
        "checkDate":"2025-01-24","rowCount":8,"errorCount":0,"journalEntryIds":["{ENTRY}"],
        "detailDiscardedAt":"2025-01-25T10:00:00.000Z"}}],"total":1}}"#
    );
    let page = sessions_from(body.as_bytes()).expect("page");
    assert_eq!(page.total, 1);
    let s = &page.sessions[0];
    assert_eq!(s.status, SessionStatus::Posted);
    assert!(s.detail_discarded);
    assert_eq!(s.entries.len(), 1);
    assert!(!format!("{page:?}").contains("Test Worker"));
}
