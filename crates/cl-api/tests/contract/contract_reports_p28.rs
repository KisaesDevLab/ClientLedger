//! The P28 report routes against the real engine (WP3/WP4): as-of agings
//! and balances, open invoices, 1099 by box, Closing Date Exceptions, the
//! audit trail with its filters (each through `Reports::run`, the table the
//! window reads), ask a question with AI off and through a loopback
//! Ollama model, and report email with no mail server set up.

use cl_api::resources::reports::ask::{
    AskDates, AskParam, AskRefusal, AskReport, AskRequest, ask_refusal,
};
use cl_api::resources::reports::email::{EmailRefusal, ReportEmail, email_refusal};
use cl_api::resources::reports::run::{AP_AGING, AUDIT, CLOSING, OPEN_ASOF};
use cl_api::{ApiError, Capabilities, Session};
use cl_core::report::{DateRange, ReportDates, ReportRequest, ReportTable, Source};

use crate::common::{date, key};
use crate::support::{Harness, OllamaStub};

fn run(h: &Harness, s: &Session, r: &ReportRequest) -> ReportTable {
    for _ in 0..10 {
        match h.run(s.reports().run(r)) {
            Ok(t) => return t,
            Err(ApiError::RateLimited { retry_after }) => {
                std::thread::sleep(retry_after.unwrap_or(std::time::Duration::from_secs(15)));
            }
            Err(e) => panic!("{:?}: {e}", r.source),
        }
    }
    panic!("{:?}: still rate limited", r.source)
}

fn year() -> DateRange {
    DateRange {
        from: date(2026, 1, 1),
        to: date(2026, 12, 31),
    }
}

/// Every WP3 route the window reads answers in the exact shape: columns
/// and rows, totals from the engine (the goldens check the figures).
pub fn who_owes_what_lists_and_controls(h: &Harness) {
    if !h.require_cap(Capabilities::REPORTS_EXACT, "U-14") {
        return;
    }
    let s = &h.s1;
    let as_of = ReportDates::AsOf(date(2026, 12, 31));
    let mut cases = vec![
        (Source::ArAgingSummary, as_of.clone(), None),
        (Source::ArAgingDetail, as_of.clone(), None),
        (Source::ApAgingSummary, as_of.clone(), Some(AP_AGING)),
        (Source::ApAgingDetail, as_of.clone(), Some(AP_AGING)),
        (Source::UnpaidBills, as_of.clone(), Some(AP_AGING)),
        (Source::InvoiceList, as_of.clone(), Some(OPEN_ASOF)),
        (
            Source::CustomerBalanceSummary,
            as_of.clone(),
            Some(OPEN_ASOF),
        ),
        (Source::VendorBalanceSummary, as_of.clone(), Some(OPEN_ASOF)),
        (Source::Vendor1099Summary, ReportDates::Range(year()), None),
        (Source::Ap1099Prep, ReportDates::Range(year()), None),
        (Source::SalesByCustomer, ReportDates::Range(year()), None),
        (Source::ExpenseByVendor, ReportDates::Range(year()), None),
        (Source::DepositDetail, ReportDates::Range(year()), None),
        (
            Source::ClosingDateExceptions,
            ReportDates::Range(year()),
            Some(CLOSING),
        ),
        (Source::AuditLog, ReportDates::Range(year()), Some(AUDIT)),
        (Source::Duplicates, ReportDates::Range(year()), None),
    ];
    let caps = s.client().capabilities();
    cases.retain(|(src, _, cap)| {
        let ok = cap.is_none_or(|c| caps.has(c));
        if !ok {
            h.pending(&format!("{src:?}"), "its engine capability is missing");
        }
        ok
    });
    for (src, dates, _) in cases {
        let t = run(h, s, &ReportRequest::new(src, dates));
        assert!(!t.columns.is_empty(), "{src:?}: columns");
        for r in &t.rows {
            assert_eq!(r.cells.len(), t.columns.len(), "{src:?}: cells line up");
        }
    }
    audit_record_filter(h, s);
}

/// U-204 (`audit-filters-v1`): the audit trail of one record lists that
/// record's changes and none of another record's.
fn audit_record_filter(h: &Harness, s: &Session) {
    if !s.client().capabilities().has(AUDIT) {
        h.pending("audit record filter", "audit-filters-v1 is missing");
        return;
    }
    let a = crate::contract_pay_bills::vendor(h, s, "Audit filter vendor A");
    let b = crate::contract_pay_bills::vendor(h, s, "Audit filter vendor B");
    let all = ReportDates::Range(DateRange {
        from: date(2000, 1, 1),
        to: date(2099, 12, 31),
    });
    // One header row per audit entry.
    let entries = |c: cl_core::ids::ContactId| -> usize {
        let mut r = ReportRequest::new(Source::AuditLog, all.clone());
        r.filters.contact_ids = vec![c];
        run(h, s, &r)
            .rows
            .iter()
            .filter(|row| row.kind == cl_core::report::RowKind::Header)
            .count()
    };
    let everything = run(h, s, &ReportRequest::new(Source::AuditLog, all.clone()))
        .rows
        .iter()
        .filter(|row| row.kind == cl_core::report::RowKind::Header)
        .count();
    assert!(everything >= 2, "the company's trail has both creations");
    // Each vendor's trail is its own creation only: one entry, not the
    // company's whole trail and not the other vendor's.
    assert_eq!(entries(a), 1, "vendor A's trail");
    assert_eq!(entries(b), 1, "vendor B's trail");
}

fn ask_req(question: &str) -> AskRequest {
    AskRequest {
        question: question.into(),
        reports: vec![
            AskReport {
                key: "profit_loss".into(),
                title: "Profit & Loss".into(),
                params: vec![AskParam::Range, AskParam::Basis, AskParam::Accounts],
                columns: vec!["total".into(), "month".into()],
                extras: vec!["pct_of_income".into()],
            },
            AskReport {
                key: "balance_sheet".into(),
                title: "Balance Sheet".into(),
                params: vec![AskParam::AsOf],
                columns: vec![],
                extras: vec![],
            },
        ],
        date_presets: vec!["last_month".into(), "this_year".into()],
        today: Some(date(2026, 11, 10)),
    }
}

/// Ask with AI off: refused before any model call. Then through a
/// loopback Ollama model (nothing leaves the computer): the mapping comes
/// back inside the catalog sent, with no figure. AI is switched off again.
pub fn ask_off_then_ollama(h: &Harness) {
    use cl_api::{AiConfigUpdate, AiTaskToggles, OllamaTarget, SessionSecrets};
    if !h.require_cap(Capabilities::REPORTS_ASK, "U-206") {
        return;
    }
    let s = &h.s1;
    let stub = OllamaStub::shared(h);
    // The model would answer, but with AI off it is never asked.
    stub.serve_chat_first(
        h,
        r#"{"reportKey":"profit_loss","datePreset":"last_month","from":null,"to":null,"asOf":null,"basis":"cash","columns":null,"extras":[],"filters":{"accountNames":[],"contactNames":[],"tagNames":[]}}"#,
    );
    let before = stub.chat_bodies(h).len();
    let off = h
        .run(s.ask_report(&ask_req("What did we make last month on cash basis?")))
        .expect_err("AI off");
    assert_eq!(ask_refusal(&off), Some(AskRefusal::ConsentRequired));
    assert_eq!(
        stub.chat_bodies(h).len(),
        before,
        "no model call with AI off"
    );

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
    .expect("AI on");
    h.run(s.put_session_secrets(&SessionSecrets {
        ollama: Some(OllamaTarget {
            base_url: stub.base_url.clone(),
            model: None,
        }),
        ..SessionSecrets::default()
    }))
    .expect("session ollama");
    h.run(s.accept_company_ai_consent(key())).expect("consent");
    h.run(s.set_ai_tasks(
        &AiTaskToggles {
            report_summary: Some(true),
            ..AiTaskToggles::default()
        },
        key(),
    ))
    .expect("ask on");
    let mapped = h.run(s.ask_report(&ask_req("What did we make last month on cash basis?")));
    // Off again before asserting, so a failure leaves the product default.
    h.run(s.set_ai_tasks(
        &AiTaskToggles {
            report_summary: Some(false),
            ..AiTaskToggles::default()
        },
        key(),
    ))
    .expect("ask off");
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            is_enabled: Some(false),
            ..AiConfigUpdate::default()
        },
        key(),
    ))
    .expect("AI off");
    let m = mapped.unwrap_or_else(|e| panic!("ask through Ollama: {e}"));
    assert_eq!(m.report_key, "profit_loss");
    assert_eq!(m.dates, AskDates::Preset("last_month".into()));
    assert_eq!(m.basis, Some(cl_core::report::Basis::Cash));
    assert_eq!(m.reader.as_deref(), Some("self_hosted"));
    assert!(
        stub.chat_bodies(h).len() > before,
        "the loopback model was asked"
    );
}

/// Email with no mail server set up: a plain refusal, nothing sent.
pub fn email_without_smtp(h: &Harness) {
    if !h.require_cap(Capabilities::REPORTS_EMAIL, "U-207") {
        return;
    }
    let s = &h.s1;
    let mail = ReportEmail {
        to: vec!["owner@example.test".into()],
        cc: vec![],
        subject: "Profit & Loss".into(),
        body: "Attached.".into(),
        filename: "Profit and Loss.csv".into(),
        file: b"kind,level,path,caption,total\n".to_vec(),
        report_key: Some("profit_loss".into()),
    };
    let e = h
        .run(s.email_report(&mail, key()))
        .expect_err("no mail server");
    assert!(
        matches!(
            email_refusal(&e),
            Some(EmailRefusal::SmtpNotConfigured | EmailRefusal::Offline)
        ),
        "{e:?}"
    );
}
