//! Describe the report against a live engine (A10, U-120
//! `report-builder-ai-v1`, engine PR #56). No network: the model is a
//! loopback Ollama stub, as Ask a question's contract test uses.
//!
//! - The capability is listed.
//! - With AI off for reports, the draft is refused 403 before any model
//!   call (the stub is never asked).
//! - Through the stub: a name in the books is resolved to its id, a name
//!   that isn't is a `$ref:N` placeholder, the amount the section was
//!   limited by is left out with `amount_filter_left_out`, and the reply
//!   passes the desktop's own check (no number but the structure's).

use cl_api::resources::reports::draft::{DraftRefusal, DraftRequest, NoteCode, draft_refusal};
use cl_api::{AccountType, Capabilities};

use crate::common::{account_fields, key};
use crate::support::{Harness, OllamaStub, unique};

/// A books name with no digit (a name with one is refused by the engine).
fn letters_only(prefix: &str) -> String {
    unique(prefix)
        .chars()
        .map(|c| match c.to_digit(10) {
            Some(d) => char::from(b'a' + u8::try_from(d).unwrap_or(0)),
            None => c,
        })
        .collect()
}

fn req(text: &str) -> DraftRequest {
    DraftRequest {
        description: text.into(),
        date_presets: vec!["this_year".into(), "last_month".into()],
    }
}

/// The capability the builder checks before it offers to draft.
pub fn the_capability_is_listed(h: &Harness) {
    if !h.require_cap(Capabilities::REPORT_BUILDER_AI, "U-120") {
        return;
    }
    assert!(h.s1.can_draft_reports());
}

/// AI off: refused, nothing asked. Then through a loopback Ollama model:
/// names resolved by lookup, an unknown name left as a placeholder, the
/// amount left out with a note. AI is switched off again.
pub fn draft_off_then_ollama(h: &Harness) {
    use cl_api::{AiConfigUpdate, AiTaskToggles, OllamaTarget, SessionSecrets};
    if !h.require_cap(Capabilities::REPORT_BUILDER_AI, "U-120") {
        return;
    }
    let s = &h.s1;
    let fuel = letters_only("Draftfuel");
    let mut f = account_fields(AccountType::Expense, &fuel);
    f.name.clone_from(&fuel);
    let acct = h
        .run(s.accounts().create(&f, key()))
        .expect("account")
        .record;
    let stub = OllamaStub::shared(h);
    let answer = serde_json::json!({
        "title": "Fuel by month", "dates": "range", "datePreset": "this_year", "periods": "month",
        "rows": [{"type": "section", "caption": "Fuel", "source": "gl", "measure": "amount",
                  "filters": [{"field": "account", "op": "in", "names": [fuel, "Nosuchaccountname"]}],
                  "groupBy": [{"key": "account"}], "amountCondition": true}]
    });
    stub.serve_chat_first(h, &answer.to_string());
    let before = stub.chat_bodies(h).len();
    let off = h
        .run(s.draft_report(&req("Fuel by month over $500")))
        .expect_err("AI off");
    assert_eq!(draft_refusal(&off), Some(DraftRefusal::ConsentRequired));
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
    let toggles = |on| AiTaskToggles {
        report_summary: Some(on),
        ..AiTaskToggles::default()
    };
    h.run(s.set_ai_tasks(&toggles(true), key())).expect("on");
    let drafted = h.run(s.draft_report(&req(&format!("{fuel} by month over $500"))));
    // Off again before asserting, so a failure leaves the product default.
    h.run(s.set_ai_tasks(&toggles(false), key())).expect("off");
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            is_enabled: Some(false),
            ..AiConfigUpdate::default()
        },
        key(),
    ))
    .expect("AI off");
    let d = drafted.unwrap_or_else(|e| panic!("draft through Ollama: {e}"));
    let text = d.definition.to_string();
    assert!(
        text.contains(&acct.id.to_string()),
        "the books name became its id"
    );
    assert!(!text.contains("500"), "the amount is nowhere in the draft");
    assert!(!d.savable);
    assert_eq!(d.unresolved.len(), 1, "{:?}", d.unresolved);
    assert!(text.contains(&d.unresolved[0].token));
    assert!(
        d.notes
            .iter()
            .any(|n| n.code == NoteCode::AmountFilterLeftOut),
        "{:?}",
        d.notes
    );
    assert!(
        stub.chat_bodies(h).len() > before,
        "the loopback model was asked"
    );
    // What the model was sent carries no amount.
    assert!(
        stub.chat_bodies(h)[before..]
            .iter()
            .all(|b| !b.contains("500")),
        "the description's amount was masked"
    );
}
