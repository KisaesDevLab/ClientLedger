use super::*;
use crate::resources::testkit::{company_session, json, requests, session_with_caps};
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const ROUTE: &str = "/api/v1/document-reading/assist";

/// The engine's answer: amounts as decimal strings or null, a JSON number
/// confidence, a rate per line.
const ANSWER: &str = r#"{"suggestion":{"jobId":"j1","reader":"cloud","usedImages":false,
    "vendor":"Canary Rental","vendorAddress":null,"vendorInvoiceNumber":"INV-55871",
    "billDate":"2026-11-03","dueDate":null,"paymentTerms":"net_15","total":"1480.00",
    "subtotal":"1330.00","tax":null,"lineItems":[
    {"description":"Excavator","amount":"1330.00","quantity":"2","unitPrice":"665.00","confidence":"high"},
    {"description":"Delivery","amount":"150.005","quantity":null,"unitPrice":"x","confidence":"low"}],
    "notes":null,"confidence":0.8575,"fieldConfidence":{"vendor":"high","total":"medium","tax":"low"},
    "qualityWarnings":[]}}"#;

fn request() -> AssistRequest {
    AssistRequest {
        text: "Canary Rental INV-55871 Total 1,480.00".into(),
        page_images: Vec::new(),
        kind_hint: Some(DraftKind::Bill),
    }
}

fn png() -> Vec<u8> {
    let mut p = PNG_MAGIC.to_vec();
    p.extend_from_slice(b"rest");
    p
}

#[test]
fn the_answer_is_read_exactly_and_never_as_a_float() {
    let s = suggestion_from_json(ANSWER.as_bytes()).expect("suggestion");
    assert_eq!(s.reader, AssistReader::Cloud);
    assert_eq!(s.total.map(Money::to_wire).as_deref(), Some("1480.0000"));
    assert_eq!(s.subtotal.map(Money::to_wire).as_deref(), Some("1330.0000"));
    assert_eq!(s.tax, None);
    assert_eq!(s.bill_date, CalendarDate::from_ymd(2026, 11, 3).ok());
    assert_eq!(s.lines.len(), 2);
    assert_eq!(
        s.lines[0].unit_price.map(Money::to_wire).as_deref(),
        Some("665.0000")
    );
    assert_eq!(
        s.lines[0].quantity.map(Quantity::to_wire).as_deref(),
        Some("2.0000")
    );
    // A sub-cent amount is unreadable (never rounded); a bad rate is no rate.
    assert_eq!((s.lines[1].amount, s.lines[1].unit_price), (None, None));
    assert_eq!(s.unreadable, vec![ReadField::LineAmount(1)]);
    // 0.8575 → 8575 basis points, from the number's text.
    assert_eq!(s.confidence_bp, Some(8575));
    assert_eq!(s.field_confidence.get("tax"), Some(&Confidence::Low));
    let shown = format!("{s:?} {:?}", request());
    for canary in ["Canary", "1480", "55871", "Excavator"] {
        assert!(!shown.contains(canary), "{shown}");
    }
}

#[test]
fn odd_confidences_clamp_and_unknown_readers_are_refused() {
    for (raw, want) in [
        ("1.5", Some(10_000)),
        ("-0.2", Some(0)),
        ("1e-1", Some(1_000)),
    ] {
        let body = format!(r#"{{"suggestion":{{"reader":"self_hosted","confidence":{raw}}}}}"#);
        let s = suggestion_from_json(body.as_bytes()).expect("s");
        assert_eq!(s.confidence_bp, want, "{raw}");
        assert_eq!(s.reader, AssistReader::SelfHosted);
    }
    assert!(matches!(
        suggestion_from_json(br#"{"suggestion":{"reader":"elsewhere"}}"#),
        Err(ApiError::Decode { .. })
    ));
}

#[test]
fn the_limits_are_checked_before_sending() {
    assert_eq!(request().check(), Ok(()));
    let bad = |r: AssistRequest| {
        assert_eq!(
            r.check(),
            Err(ApiError::Local(LocalValidation::InvalidValue))
        );
    };
    bad(AssistRequest {
        text: "x".repeat(ASSIST_MAX_TEXT_CHARS + 1),
        ..request()
    });
    bad(AssistRequest {
        text: "  ".into(),
        ..request()
    });
    bad(AssistRequest {
        page_images: vec![png(); ASSIST_MAX_IMAGES + 1],
        ..request()
    });
    bad(AssistRequest {
        page_images: vec![b"GIF89a".to_vec()],
        ..request()
    });
    let mut big = png();
    big.resize(ASSIST_MAX_IMAGE_BYTES + 1, 0);
    bad(AssistRequest {
        page_images: vec![big],
        ..request()
    });
    // Pictures alone are something to read.
    assert_eq!(
        AssistRequest {
            text: String::new(),
            page_images: vec![png()],
            kind_hint: None,
        }
        .check(),
        Ok(())
    );
}

#[tokio::test]
async fn nothing_is_sent_without_the_capability_or_over_a_limit() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    assert_eq!(
        s.document_assist(&request(), IdempotencyKey::new())
            .await
            .expect_err("gated"),
        ApiError::Local(LocalValidation::Unsupported)
    );
    let s = session_with_caps(&server, &[Capabilities::DOCUMENT_ASSIST]).await;
    let before = server.received_requests().await.expect("r").len();
    assert!(
        s.document_assist(
            &AssistRequest {
                page_images: vec![b"no".to_vec()],
                ..request()
            },
            IdempotencyKey::new()
        )
        .await
        .is_err()
    );
    assert_eq!(server.received_requests().await.expect("r").len(), before);
}

#[tokio::test]
async fn the_body_carries_text_pictures_and_hint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(ROUTE))
        .respond_with(json(200, ANSWER))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::DOCUMENT_ASSIST]).await;
    let r = AssistRequest {
        page_images: vec![png()],
        ..request()
    };
    let got = s
        .document_assist(&r, IdempotencyKey::new())
        .await
        .expect("assist");
    assert_eq!(got.vendor_invoice_number.as_deref(), Some("INV-55871"));
    let body: Value =
        serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
    assert_eq!(body["text"], r.text);
    assert_eq!(body["kindHint"], "bill");
    assert_eq!(
        body["pageImages"][0],
        base64::engine::general_purpose::STANDARD.encode(png())
    );
    // Without pictures the key is left out.
    let _ = s
        .document_assist(&request(), IdempotencyKey::new())
        .await
        .expect("text only");
    let body: Value =
        serde_json::from_slice(&requests(&server, "POST").await[1].body).expect("json");
    assert!(body.get("pageImages").is_none());
}

#[tokio::test]
async fn refusals_keep_their_meaning() {
    for (status, body, check) in [
        (
            400,
            r#"{"error":{"message":"AI processing is not enabled.","code":"ai_disabled"}}"#,
            "off",
        ),
        (
            400,
            r#"{"error":{"message":"off","code":"ai_function_disabled"}}"#,
            "off",
        ),
        (
            403,
            r#"{"error":{"message":"no","code":"CONSENT_REQUIRED","details":{"task":"document_text_cloud"}}}"#,
            CONSENT_REQUIRED,
        ),
        (
            403,
            r#"{"error":{"message":"no","code":"CLOUD_VISION_DISABLED"}}"#,
            CLOUD_VISION_DISABLED,
        ),
        (
            400,
            r#"{"error":{"message":"x","code":"ai_ocr_failed"}}"#,
            crate::AI_OCR_FAILED,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(ROUTE))
            .respond_with(json(status, body))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[Capabilities::DOCUMENT_ASSIST]).await;
        let e = s
            .document_assist(&request(), IdempotencyKey::new())
            .await
            .expect_err("refused");
        if check == "off" {
            assert_eq!(e, ApiError::AiUnavailable, "{body}");
        } else {
            assert!(crate::has_code(&e, check), "{e:?}");
        }
    }
}
