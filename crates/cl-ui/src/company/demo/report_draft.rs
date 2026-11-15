//! The demo's `POST /report-builder/draft` (A10, Q-A10h): no AI. A few
//! fixed descriptions get prepared drafts built from the demo books' names;
//! anything else is refused as "not available in the demo".
//!
//! - "fuel" → a clean, savable draft (Fuel by month).
//! - "supplies" → the account name matches two accounts: a `$ref:0`
//!   placeholder with both as candidates.
//! - "vendor" → spending by vendor; any amount in the description is left
//!   out with `amount_filter_left_out` (Q-A10e), and never echoed.
//!
//! With AI off for the company the demo refuses 403 `AI_CONSENT_REQUIRED`
//! before reading the description, as the engine does. Every draft goes
//! through the same check as the engine's ([`validate`]).

use cl_api::resources::reports::draft::{DraftReply, DraftRequest, description_ok, validate};
use cl_api::{ApiError, engine_refusal};
use serde_json::{Value, json};

use super::DemoState;

/// The reason the demo gives for a description it has no draft for.
pub const DEMO_ONLY: &str = "demo";

fn section(id: &str, caption: &str, filters: &Value, group_by: &Value) -> Value {
    json!({"type": "section", "id": id, "caption": caption, "source": "gl",
           "measure": "amount", "filters": filters, "groupBy": group_by,
           "display": "summary", "defaultExpanded": false, "showTotal": true,
           "hideZero": true})
}

/// A draft's definition; `title` becomes the Report name.
fn definition(title: &str, node: &Value, periods: Option<&str>, preset: &str) -> Value {
    let mut columns = json!({"detailFields": [], "compare": [], "headers": {}});
    if let Some(p) = periods {
        columns["periods"] = json!(p);
    }
    let mut dates = json!({"mode": "range"});
    if !preset.is_empty() {
        dates["preset"] = json!(preset);
    }
    json!({"schemaVersion": 1, "kind": "builder", "title": title,
           "dates": dates, "columns": columns, "nodes": [node.clone()]})
}

/// The words before a number that make it an amount filter ("over 500").
const AMOUNT_WORDS: [&str; 8] = [
    "over",
    "under",
    "above",
    "below",
    "more than",
    "less than",
    "at least",
    "at most",
];

/// What the engine notes about numbers in the description: an amount (a
/// currency sign, or a comparison word with a digit) is left to the owner;
/// any other number ("in 2026") just wasn't used.
fn number_note(q: &str) -> Option<&'static str> {
    let digit = cl_api::resources::reports::draft::has_digit(q);
    if q.contains('$') || (digit && AMOUNT_WORDS.iter().any(|w| q.contains(w))) {
        Some("amount_filter_left_out")
    } else if digit {
        Some("numbers_not_used")
    } else {
        None
    }
}

fn refused(status: u16, code: &str, details: &Value) -> ApiError {
    engine_refusal(
        status,
        &json!({"error": {"message": "refused", "code": code, "details": details}}),
    )
}

/// The demo books' accounts whose label holds `word`.
fn accounts(d: &mut DemoState, word: &str) -> Vec<(String, String)> {
    super::report_transfer::pick_list(d, "account")
        .into_iter()
        .filter(|(_, l)| l.to_lowercase().contains(word))
        .collect()
}

/// A preset the request offers, else none.
fn preset<'a>(req: &'a DraftRequest, want: &'a str) -> Option<&'a str> {
    req.date_presets.iter().any(|p| p == want).then_some(want)
}

/// The demo's draft for `req`.
///
/// # Errors
/// 403 `AI_CONSENT_REQUIRED` with AI off; 400 `VALIDATION_ERROR` for an
/// empty or over-long description; 422 `DRAFT_UNMAPPED {reason: "demo"}`
/// for any description without a prepared draft.
pub fn draft(d: &mut DemoState, req: &DraftRequest) -> Result<DraftReply, ApiError> {
    if !d.ai.enabled {
        return Err(refused(
            403,
            "AI_CONSENT_REQUIRED",
            &json!({"reason": "ai_disabled", "task": "report_summary"}),
        ));
    }
    if !description_ok(&req.description) {
        return Err(refused(400, "VALIDATION_ERROR", &json!({})));
    }
    let q = req.description.to_lowercase();
    let year = preset(req, "this_year").unwrap_or("");
    let unmapped = || {
        refused(
            422,
            "DRAFT_UNMAPPED",
            &json!({"field": "description", "reason": DEMO_ONLY}),
        )
    };
    let answer = if q.contains("fuel") {
        let fuel = accounts(d, "fuel");
        let Some((id, _)) = fuel.first() else {
            return Err(unmapped());
        };
        json!({
            "definition": definition("Fuel by month",
                &section("row_a", "Fuel",
                        &json!([{"field": "account", "op": "in", "ids": [id]}]),
                        &json!([{"key": "account"}])),
                Some("month"), year),
            "savable": true, "unresolved": [], "notes": [],
            "reader": "demo", "provider": "demo"
        })
    } else if q.contains("supplies") {
        let found = accounts(d, "supplies");
        if found.len() < 2 {
            return Err(unmapped());
        }
        let candidates: Vec<Value> = found
            .iter()
            .map(|(id, label)| json!({"id": id, "label": label}))
            .collect();
        json!({
            "definition": definition("Supplies by customer",
                &section("row_a", "Supplies",
                        &json!([{"field": "account", "op": "in", "ids": ["$ref:0"]}]),
                        &json!([{"key": "contact"}])),
                None, year),
            "savable": false,
            "unresolved": [{"ref": "$ref:0", "kind": "account", "name": "Supplies",
                "reason": "ambiguous",
                "sites": [{"nodeId": "row_a", "path": "definition.nodes.0.filters.0.ids.0"}],
                "candidates": candidates}],
            "notes": [{"code": "pick_unresolved", "message": "Pick the account."}],
            "reader": "demo", "provider": "demo"
        })
    } else if q.contains("vendor") {
        // The engine masks any amount before the model sees the words and
        // never puts it back: the draft has no amount filter, only a note.
        let notes: Vec<Value> = number_note(&q)
            .map(|code| json!({"code": code, "message": "x", "nodeId": "row_a"}))
            .into_iter()
            .collect();
        json!({
            "definition": definition("Spending by vendor",
                &section("row_a", "Spending by vendor",
                        &json!([{"field": "account_type", "op": "in", "values": ["expense"]}]),
                        &json!([{"key": "contact"}])),
                None, year),
            "savable": true, "unresolved": [], "notes": notes,
            "reader": "demo", "provider": "demo"
        })
    } else {
        return Err(unmapped());
    };
    validate(req, &answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(text: &str) -> DraftRequest {
        DraftRequest {
            description: text.into(),
            date_presets: vec!["this_year".into(), "last_month".into()],
        }
    }

    fn demo() -> DemoState {
        let mut d = DemoState::new();
        d.ai.enabled = true;
        d
    }

    #[test]
    fn the_three_prepared_drafts() {
        let mut d = demo();
        let fuel = draft(&mut d, &req("Fuel by month this year")).expect("fuel");
        assert!(fuel.savable);
        let sup = draft(&mut d, &req("Supplies by customer")).expect("supplies");
        assert!(!sup.savable);
        assert!(sup.unresolved[0].candidates.len() >= 2);
        let v = draft(&mut d, &req("Vendors I paid over $500")).expect("vendor");
        assert!(v.savable);
        assert_eq!(
            v.notes.first().map(|n| n.code),
            Some(cl_api::resources::reports::draft::NoteCode::AmountFilterLeftOut)
        );
        // The amount is nowhere in the draft.
        assert!(!v.definition.to_string().contains("500"));
        // Each draft names the report; node ids are the engine's shape.
        for r in [&fuel, &sup, &v] {
            assert!(
                r.definition["title"]
                    .as_str()
                    .is_some_and(|t| !t.is_empty())
            );
            assert_eq!(r.definition["nodes"][0]["id"], "row_a");
        }
        // A number that isn't an amount is only "not used" (as the engine).
        let y = draft(&mut d, &req("Vendors in 2026")).expect("vendor");
        assert_eq!(
            y.notes.first().map(|n| n.code),
            Some(cl_api::resources::reports::draft::NoteCode::NumbersNotUsed)
        );
    }

    #[test]
    fn other_words_and_ai_off_are_refused() {
        use cl_api::resources::reports::draft::{DraftRefusal, draft_refusal};
        let mut d = demo();
        let e = draft(&mut d, &req("Profit by planet")).expect_err("demo only");
        assert_eq!(
            draft_refusal(&e),
            Some(DraftRefusal::Unmapped {
                reason: Some(DEMO_ONLY.into())
            })
        );
        d.ai.enabled = false;
        let e = draft(&mut d, &req("Fuel by month")).expect_err("ai off");
        assert_eq!(draft_refusal(&e), Some(DraftRefusal::ConsentRequired));
    }
}
