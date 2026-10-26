//! Document reading assist and its consent switches against the real
//! engine (A03 step 21; engine U-92d, `document-assist-v1`;
//! `target/plans/A03.md` §6 T13). One binary, one engine, one fresh
//! cluster; tests run in the order listed in `main`.
//!
//! Every test is PENDING by `document-assist-v1` on an engine without it
//! and then pins today's behaviour instead: the typed calls send nothing
//! (`Local(Unsupported)`) and the route is the engine's JSON 404.
//!
//! No online provider is ever reached: the cloud case is refused by consent
//! before any provider is asked (and has no key), and the Ollama case talks
//! to a loopback stub.
//!
//! Run: `cargo xtask engine && cargo xtask pg-fetch`, then
//! `cargo test -p cl-api --test contract_document_assist`. Synthetic data
//! only.

#![allow(
    clippy::too_many_lines,
    reason = "contract scenarios read top to bottom like the other contract binaries"
)]

#[path = "support/mod.rs"]
mod support;

use cl_api::{
    AiConfigUpdate, AiTaskToggles, ApiError, AssistReader, AssistRequest, CLOUD_VISION_DISABLED,
    CONSENT_REQUIRED, Capabilities, DraftKind, IdempotencyKey, LocalValidation, Method,
    OllamaTarget, Session, SessionSecrets,
};
use cl_core::Money;
use serde_json::json;

use support::{EngineMode, Harness, TestFn, run_suite};

const ROUTE: &str = "/api/v1/document-reading/assist";
const U92D: &str = "U-92d";

/// The PNG signature and a few bytes: the engine checks the signature only.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR synthetic";

fn key() -> IdempotencyKey {
    IdempotencyKey::new()
}

fn text_request() -> AssistRequest {
    AssistRequest {
        text: "Synthetic Supply Co\nInvoice INV-9001\nTotal 125.00".into(),
        page_images: Vec::new(),
        kind_hint: Some(DraftKind::Bill),
    }
}

/// `true` with `document-assist-v1`. Without it, records PENDING(U-92d)
/// and checks today's behaviour: nothing is sent and the route is 404.
fn live(h: &Harness, s: &Session) -> bool {
    if h.require_cap(Capabilities::DOCUMENT_ASSIST, U92D) {
        return true;
    }
    let before = h.observer.total();
    assert_eq!(
        h.run(s.document_assist(&text_request(), key())),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(
        h.run(s.set_ai_tasks(
            &AiTaskToggles {
                document_text_cloud: Some(true),
                ..AiTaskToggles::default()
            },
            key()
        )),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(
        h.observer.total(),
        before,
        "nothing sent without the capability"
    );
    let raw = h
        .run(s.raw(Method::Post, ROUTE, &[], Some(&json!({"text": "x"}))))
        .expect("raw");
    assert_eq!(raw.status, 404, "{}", raw.body);
    false
}

fn is_consent_refusal(e: &ApiError) -> bool {
    cl_api::has_code(e, CONSENT_REQUIRED)
}

/// The company's two switches as the engine stores them.
fn switches(h: &Harness, s: &Session) -> (bool, bool) {
    let c = h.run(s.company_ai_disclosure()).expect("consent");
    (
        c.enabled_tasks.document_text_cloud_on(),
        c.enabled_tasks.document_images_cloud == Some(true),
    )
}

/// The engine's audit rows of the company's task switches.
fn task_audit_rows(h: &Harness, s: &Session) -> usize {
    let raw = h
        .run(s.raw(
            Method::Get,
            "/api/v1/audit-log",
            &[("entity_type", "company_ai_tasks"), ("limit", "100")],
            None,
        ))
        .expect("audit");
    assert_eq!(raw.status, 200, "{}", raw.body);
    raw.body["data"].as_array().map_or(0, Vec::len)
}

/// T13: a company that reads bills with AI (`receipt_ocr`) has both new
/// switches off, and a cloud provider is refused until the owner turns the
/// matching switch on; pictures need their own switch and the engine's
/// cloud-vision setting. Each change writes an audit row.
fn t13_receipt_ocr_never_opens_cloud_reading(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    h.run(s.accept_ai_disclosure(key())).expect("disclosure");
    // A cloud provider with no key at all: consent is checked before any
    // provider is asked, so nothing can leave this computer.
    h.run(s.update_ai_config(&AiConfigUpdate::for_provider("anthropic", true), key()))
        .expect("AI on, cloud provider");
    h.run(s.accept_company_ai_consent(key())).expect("consent");
    h.run(s.set_ai_tasks(
        &AiTaskToggles {
            receipt_ocr: Some(true),
            ..AiTaskToggles::default()
        },
        key(),
    ))
    .expect("bill reading on");
    assert_eq!(
        switches(h, s),
        (false, false),
        "receipt_ocr and the disclosure never turn them on"
    );
    let e = h
        .run(s.document_assist(&text_request(), key()))
        .expect_err("refused");
    assert!(is_consent_refusal(&e), "{e:?}");

    // The owner turns page text on: the consent gate opens (the call then
    // fails for want of a key or a network, never for consent).
    let audits = task_audit_rows(h, s);
    let stored = h
        .run(s.set_ai_tasks(
            &AiTaskToggles {
                document_text_cloud: Some(true),
                ..AiTaskToggles::default()
            },
            key(),
        ))
        .expect("text on");
    assert!(stored.document_text_cloud_on());
    assert_eq!(stored.receipt_ocr, Some(true), "other switches untouched");
    assert_eq!(switches(h, s), (true, false));
    assert!(task_audit_rows(h, s) > audits, "the change is audited");
    match h.run(s.document_assist(&text_request(), key())) {
        Err(e) => assert!(!is_consent_refusal(&e), "{e:?}"),
        Ok(_) => panic!("no provider can answer without a key"),
    }

    // Pictures: their own switch first, then the engine's cloud vision
    // (strict in the desktop, Q-139).
    let with_page = AssistRequest {
        page_images: vec![PNG.to_vec()],
        ..text_request()
    };
    let e = h
        .run(s.document_assist(&with_page, key()))
        .expect_err("pictures refused");
    assert!(is_consent_refusal(&e), "{e:?}");
    h.run(s.set_ai_tasks(
        &AiTaskToggles {
            document_images_cloud: Some(true),
            ..AiTaskToggles::default()
        },
        key(),
    ))
    .expect("pictures on");
    assert_eq!(switches(h, s), (true, true));
    let e = h
        .run(s.document_assist(&with_page, key()))
        .expect_err("cloud vision off");
    assert!(cl_api::has_code(&e, CLOUD_VISION_DISABLED), "{e:?}");

    // Back to the product default.
    h.run(s.set_ai_tasks(
        &AiTaskToggles {
            document_text_cloud: Some(false),
            document_images_cloud: Some(false),
            ..AiTaskToggles::default()
        },
        key(),
    ))
    .expect("both off");
    assert_eq!(switches(h, s), (false, false));
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            is_enabled: Some(false),
            ..AiConfigUpdate::default()
        },
        key(),
    ))
    .expect("AI off");
}

/// A local Ollama needs neither cloud switch: the answer's amounts come
/// back as the model read them, exactly, marked as read on this network.
fn ollama_needs_no_cloud_switch(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let canned = json!({
        "vendor": "Synthetic Supply Co", "vendor_invoice_number": "INV-9001",
        "bill_date": "2026-11-03", "due_date": null, "payment_terms": null,
        "subtotal": "125.00", "tax": null, "total": "125.00",
        "line_items": [{"description": "Widgets", "amount": "125.00", "quantity": "5",
            "unit_price": "25.00", "confidence": 0.9}],
        "confidence": 0.9, "field_confidence": {},
    })
    .to_string();
    let stub = support::OllamaStub::shared(h);
    stub.serve_chat(h, &canned);
    h.run(s.accept_ai_disclosure(key())).expect("disclosure");
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            ollama_base_url: Some(stub.base_url.clone()),
            categorization_model: Some("llama3.1:8b".into()),
            ocr_model: Some("llama3.1:8b".into()),
            ..AiConfigUpdate::for_provider("ollama", true)
        },
        key(),
    ))
    .expect("AI on, Ollama");
    if h.caps.has(Capabilities::SESSION_SECRETS) {
        h.run(s.put_session_secrets(&SessionSecrets {
            ollama: Some(OllamaTarget {
                base_url: stub.base_url.clone(),
                model: None,
            }),
            ..SessionSecrets::default()
        }))
        .expect("session ollama");
    }
    h.run(s.accept_company_ai_consent(key())).expect("consent");
    assert_eq!(switches(h, s), (false, false));
    let got = h.run(s.document_assist(&text_request(), key()));
    let chats = stub.chat_bodies(h);
    // AI off before asserting, so a failure leaves the product default.
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            is_enabled: Some(false),
            ..AiConfigUpdate::default()
        },
        key(),
    ))
    .expect("AI off");
    let got = got.expect("assist");
    assert_eq!(got.reader, AssistReader::SelfHosted);
    assert!(!got.used_images);
    assert_eq!(got.vendor_invoice_number.as_deref(), Some("INV-9001"));
    assert_eq!(got.total, Money::parse_wire("125.00").ok());
    assert_eq!(got.lines.len(), 1);
    assert_eq!(got.lines[0].unit_price, Money::parse_wire("25.00").ok());
    // The page text went to the stub, never anywhere else.
    assert!(
        chats.iter().any(|b| b.contains("INV-9001")),
        "the text reached the local model"
    );
}

fn main() -> std::process::ExitCode {
    let tests: &[(&str, TestFn)] = &[
        (
            "assist::t13_receipt_ocr_never_opens_cloud_reading",
            t13_receipt_ocr_never_opens_cloud_reading,
        ),
        (
            "assist::ollama_needs_no_cloud_switch",
            ollama_needs_no_cloud_switch,
        ),
    ];
    run_suite(EngineMode::Normal, tests)
}
