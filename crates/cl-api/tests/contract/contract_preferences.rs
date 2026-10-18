//! P09 Preferences contracts against the live engine: AI config, the two
//! disclosures and consent (Q-136, Q-140), the provider test against a
//! loopback Ollama stub, status and usage, and the company SMTP settings.
//! The E6 parts (session secrets, integer usage summary, SMTP password
//! from the session) are PENDING by capability until the engine branch
//! lands (`target/plans/P09-scout.md` §3).

use cl_api::{
    AiConfigUpdate, AiTaskToggles, ApiError, Capabilities, IdempotencyKey, LocalValidation, Method,
    OllamaTarget, SecretSlot, SessionSecrets, SmtpSettingsUpdate, SmtpTest,
};
use cl_core::Secret;

use crate::common::{date, key};
use crate::support::{Harness, OllamaStub};

const MODEL: &str = "llama3.1:8b";

/// The AI config reads without key values; the system disclosure can be
/// accepted (audit row) and AI turned on for Ollama with
/// `fallbackChain: ["ollama"]`; the company accepts its disclosure; task
/// switches round-trip; status and usage answer with integers.
pub fn ai_config_disclosure_consent_and_tasks(h: &Harness) {
    let s = &h.s1;
    let before = h.run(s.ai_config()).expect("config");
    assert!(!before.has_anthropic_key && !before.has_openai_key);
    let d = h.run(s.ai_disclosure()).expect("disclosure");
    assert!(d.version >= 1);
    assert!(d.text.len() > 20, "the disclosure has text");
    let accepted = h.run(s.accept_ai_disclosure(key())).expect("accept");
    assert!(accepted.accepted_at.is_some(), "accept records a time");

    let stub = OllamaStub::shared(h);
    let hits = stub.hits(h);
    let update = AiConfigUpdate {
        ollama_base_url: Some(stub.base_url.clone()),
        categorization_model: Some(MODEL.into()),
        ..AiConfigUpdate::for_provider("ollama", true)
    };
    let cfg = h.run(s.update_ai_config(&update, key())).expect("update");
    assert!(cfg.is_enabled);
    assert_eq!(cfg.fallback_chain, vec!["ollama"]);
    assert_eq!(cfg.ollama_base_url.as_deref(), Some(stub.base_url.as_str()));
    assert_eq!(cfg.pii_protection_level.as_deref(), Some("strict"));
    assert!(!cfg.has_anthropic_key, "no key was ever written");

    let company = h
        .run(s.company_ai_disclosure())
        .expect("company disclosure");
    assert!(!company.ai_enabled);
    let company = h
        .run(s.accept_company_ai_consent(key()))
        .expect("company accept");
    assert!(company.ai_enabled);
    assert!(company.accepted_at.is_some());
    assert_eq!(
        company.accepted_version,
        Some(cfg.disclosure_version.unwrap_or(1))
    );
    assert_eq!(company.enabled_tasks.categorization, Some(true));

    let tasks = h
        .run(s.set_ai_tasks(
            &AiTaskToggles {
                report_summary: Some(true),
                receipt_ocr: Some(false),
                ..AiTaskToggles::default()
            },
            key(),
        ))
        .expect("tasks");
    assert_eq!(tasks.report_summary, Some(true));
    assert_eq!(tasks.receipt_ocr, Some(false));
    assert_eq!(
        tasks.categorization,
        Some(true),
        "other switches keep their value"
    );

    let consent = h.run(s.ai_consent()).expect("consent");
    let row = consent
        .companies
        .iter()
        .find(|c| c.ai_enabled)
        .expect("the company is listed as consented");
    assert!(!row.is_stale);

    let status = h.run(s.ai_status()).expect("status");
    assert!(status.is_enabled && status.has_categorization);
    let usage = h.run(s.ai_usage(1)).expect("usage");
    assert_eq!(usage.total_calls, 0, "nothing was asked of a model");
    assert_eq!(usage.total_input + usage.total_output, 0);
    assert_eq!(stub.hits(h), hits, "config writes never contact Ollama");
}

/// The Ollama test reaches only the loopback stub and reports the model
/// as installed; a missing model is an honest failure; the cloud
/// providers fail without a key and nothing leaves the computer.
pub fn provider_test_ollama_stub_and_keyless_cloud(h: &Harness) {
    let s = &h.s1;
    let stub = OllamaStub::shared(h);
    let hits = stub.hits(h);
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            ollama_base_url: Some(stub.base_url.clone()),
            categorization_model: Some(MODEL.into()),
            ..AiConfigUpdate::for_provider("ollama", true)
        },
        key(),
    ))
    .expect("update");
    let r = h.run(s.test_ai_provider("ollama")).expect("test");
    assert!(r.success, "{:?}", r.error);
    assert_eq!(stub.hits(h), hits + 1);

    h.run(s.update_ai_config(
        &AiConfigUpdate {
            categorization_model: Some("not-installed:1b".into()),
            ..AiConfigUpdate::default()
        },
        key(),
    ))
    .expect("update");
    let r = h.run(s.test_ai_provider("ollama")).expect("test");
    assert!(!r.success);
    assert!(
        r.error
            .as_deref()
            .is_some_and(|e| e.contains("not installed")),
        "{:?}",
        r.error
    );
    assert_eq!(stub.hits(h), hits + 2);

    // No key in the engine and none in the session: the cloud test fails
    // inside the engine before any provider client exists.
    let sent = h.observer.total();
    let r = h.run(s.test_ai_provider("anthropic"));
    match r {
        Ok(r) => assert!(!r.success, "no key must not test as connected"),
        Err(ApiError::OfflineUnavailable { .. } | ApiError::FeatureNotEnabled { .. }) => {}
        Err(e) => panic!("unexpected {e:?}"),
    }
    assert_eq!(
        h.observer.total() - sent,
        1,
        "one engine call, never retried"
    );
    assert_eq!(
        stub.hits(h),
        hits + 2,
        "the cloud test never touches Ollama"
    );
    // Providers outside the allowlist are refused before anything is sent.
    assert_eq!(
        h.run(s.test_ai_provider("gemini")).expect_err("gemini"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
    assert_eq!(h.observer.total() - sent, 1);
    // Leave AI off, as the product default (Q-38).
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            is_enabled: Some(false),
            ..AiConfigUpdate::default()
        },
        key(),
    ))
    .expect("off");
    assert!(!h.run(s.ai_status()).expect("status").is_enabled);
}

/// Company SMTP settings round-trip without a password column (the engine
/// refuses a loopback host for stored settings: `SMTP_HOST_BLOCKED`); the
/// connection test takes the password in the body (never stored) and an
/// unresolvable host fails without a retry.
pub fn smtp_settings_round_trip_and_test(h: &Harness) {
    let s = &h.s1;
    let loopback = SmtpSettingsUpdate {
        host: "127.0.0.1".into(),
        port: 2525,
        user: "books".into(),
        from: "books@example.com".into(),
        from_name: "Contract Co".into(),
    };
    let e = h
        .run(s.update_smtp_settings(&loopback, key()))
        .expect_err("loopback host");
    assert!(
        matches!(&e, ApiError::Validation(v) if v.code().is_some_and(|c| c.as_str() == "SMTP_HOST_BLOCKED")),
        "{e:?}"
    );
    let update = SmtpSettingsUpdate {
        host: "smtp.example.com".into(),
        port: 587,
        user: "books".into(),
        from: "books@example.com".into(),
        from_name: "Contract Co".into(),
    };
    h.run(s.update_smtp_settings(&update, key())).expect("put");
    let got = h.run(s.smtp_settings()).expect("get");
    assert_eq!(got.smtp_host, "smtp.example.com");
    assert_eq!(got.smtp_port, 587);
    assert_eq!(got.smtp_from, "books@example.com");
    assert!(got.configured);
    assert!(
        !got.password_configured,
        "the desktop never writes a password column"
    );
    let before = h.observer.sent(Method::Post, "/api/v1/company/smtp/test");
    let r = h.run_for(
        std::time::Duration::from_secs(40),
        s.test_company_smtp(&SmtpTest {
            host: "smtp.invalid".into(),
            port: 2525,
            username: "books".into(),
            password: Some(Secret::new("smtp-body-canary-7")),
            from: "books@example.com".into(),
            from_name: None,
            test_email: None,
        }),
    );
    match r {
        Ok(r) => assert!(!r.success, "an unresolvable host cannot connect"),
        Err(
            ApiError::OfflineUnavailable { .. }
            | ApiError::FeatureNotEnabled { .. }
            | ApiError::Validation(_),
        ) => {}
        Err(e) => panic!("unexpected {e:?}"),
    }
    assert_eq!(
        h.observer.sent(Method::Post, "/api/v1/company/smtp/test") - before,
        1,
        "never retried"
    );
    let again = h.run(s.smtp_settings()).expect("get");
    assert!(!again.password_configured, "the test keeps nothing");
    assert!(
        !h.logs.text().contains("smtp-body-canary"),
        "the body password never reaches the logs"
    );
}

/// E6: per-session secrets. Without `session-secrets-v1` nothing is sent.
pub fn session_secrets_e6(h: &Harness) {
    let s = &h.s1;
    let secrets = SessionSecrets {
        anthropic_key: Some(Secret::new("sk-ant-contract-canary-1")),
        ollama: Some(OllamaTarget {
            base_url: "http://localhost:11434".into(),
            model: Some(MODEL.into()),
        }),
        smtp_password: Some(Secret::new("smtp-session-canary-2")),
        ..SessionSecrets::default()
    };
    if h.require_cap(Capabilities::SESSION_SECRETS, "U-09") {
        let p = h.run(s.put_session_secrets(&secrets)).expect("put");
        assert!(p[SecretSlot::AiAnthropic.wire()].present);
        assert!(p[SecretSlot::SmtpPassword.wire()].present);
        let got = h.run(s.session_secrets()).expect("get");
        assert!(got[SecretSlot::AiAnthropic.wire()].fingerprint.is_some());
        assert!(
            !format!("{got:?}").contains("canary"),
            "GET never returns a value"
        );
        h.run(s.delete_session_secret(SecretSlot::AiAnthropic))
            .expect("delete");
        let got = h.run(s.session_secrets()).expect("get");
        assert!(
            !got.get(SecretSlot::AiAnthropic.wire())
                .is_some_and(|p| p.present)
        );
        // The engine's config row never learns the key.
        assert!(!h.run(s.ai_config()).expect("config").has_anthropic_key);
    } else {
        let before = h.observer.total();
        assert_eq!(
            h.run(s.put_session_secrets(&secrets)).expect_err("gated"),
            ApiError::Local(LocalValidation::SessionSecretsUnavailable)
        );
        assert_eq!(h.observer.total(), before, "nothing may be sent");
    }
    assert!(
        !h.logs.text().contains("canary"),
        "session secrets never reach the logs"
    );
}

/// E6: the integer usage summary (`ai-desktop-v1`).
pub fn usage_summary_e6(h: &Harness) {
    let s = &h.s1;
    let (from, to) = (date(2026, 1, 1), date(2026, 12, 31));
    if h.require_cap(Capabilities::AI_DESKTOP, "U-09") {
        let sum = h.run(s.ai_usage_summary(from, to)).expect("summary");
        assert_eq!(sum.input_tokens + sum.output_tokens, 0);
    } else {
        let before = h.observer.total();
        assert_eq!(
            h.run(s.ai_usage_summary(from, to)).expect_err("gated"),
            ApiError::Local(LocalValidation::AiUsageSummaryUnavailable)
        );
        assert_eq!(h.observer.total(), before);
    }
}

/// U-13: with `smtp-session-v1` the SMTP password comes from the session
/// secret and `GET /company/smtp` reports it configured; until then the
/// engine reports no password after a desktop write.
pub fn smtp_session_u13(h: &Harness) {
    let s = &h.s1;
    let got = h.run(s.smtp_settings()).expect("get");
    if h.require_cap(Capabilities::SMTP_SESSION, "U-13") {
        h.run(s.put_session_secrets(&SessionSecrets {
            smtp_password: Some(Secret::new("smtp-session-canary-3")),
            ..SessionSecrets::default()
        }))
        .expect("put");
        let got = h.run(s.smtp_settings()).expect("get");
        assert!(got.password_configured, "the session password counts");
    } else {
        assert!(!got.password_configured);
    }
}

#[allow(dead_code)]
fn _idempotency_key_type_is_used(_: IdempotencyKey) {}
