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

/// Words right before a number that make it an amount or a limit ("over
/// 500", "costing 40"): the engine's `COMPARISON` list (`draft-numbers.ts`).
const AMOUNT_BEFORE: [&str; 51] = [
    "over",
    "under",
    "above",
    "below",
    "more than",
    "less than",
    "fewer than",
    "at least",
    "at most",
    "greater than",
    "exceeding",
    "exceeds",
    "exceed",
    "bigger than",
    "larger than",
    "smaller than",
    "higher than",
    "lower than",
    "minimum of",
    "minimum",
    "maximum of",
    "maximum",
    "min",
    "max",
    "up to",
    "between",
    "of",
    "totalling",
    "totaling",
    "totals",
    "total",
    "total of",
    "worth",
    "costing",
    "cost",
    "costs",
    "equal to",
    "equals",
    "equal",
    "amount",
    "amounts",
    "amounting to",
    "limit",
    "limit of",
    "threshold",
    "threshold of",
    "value",
    "value of",
    "balance",
    "balance of",
    "sum of",
];

/// Words right after a number that make it an amount ("500 dollars", "5k",
/// "500 or more"): the engine's amount units and `AMOUNT_AFTER`.
const AMOUNT_AFTER: [&str; 18] = [
    "dollars", "dollar", "bucks", "buck", "grand", "cents", "cent", "usd", "k", "m", "mm", "bn",
    "+", "plus", "or more", "or less", "and up", "and over",
];

/// True when `rest` starts with `w` as a whole word (or sign).
fn starts_word(rest: &str, w: &str) -> bool {
    rest.strip_prefix(w).is_some_and(|after| {
        !w.chars().all(char::is_alphanumeric) || !after.starts_with(char::is_alphanumeric)
    })
}

/// What the engine notes about numbers in the description (its
/// `draftNotes`, A10 review Minor 4): a number that reads as an amount or a
/// limit (a currency sign before it, a comparison word right before it, an
/// amount unit or "or more" right after it) is left to the owner; any other
/// number ("Vendors in 2026") just wasn't used. Digits only: the demo
/// doesn't read numbers in words.
fn number_note(q: &str) -> Option<&'static str> {
    let q = q.to_lowercase();
    let mut digit = false;
    let mut amount = false;
    let mut chars = q.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !c.is_numeric() {
            continue;
        }
        digit = true;
        let mut end = i + c.len_utf8();
        while let Some(&(j, n)) = chars.peek() {
            if n.is_numeric() || n == ',' || n == '.' {
                end = j + n.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let before = q[..i].trim_end();
        let after = q[end..].trim_start();
        let word_before = AMOUNT_BEFORE.iter().any(|w| {
            before
                .strip_suffix(w)
                .is_some_and(|b| !b.ends_with(char::is_alphanumeric))
        });
        amount |= before.ends_with(['$', '\u{20ac}', '\u{a3}', '<', '>', '='])
            || word_before
            || AMOUNT_AFTER.iter().any(|w| starts_word(after, w));
    }
    if amount {
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

    /// A10 review Minor 4: the engine's rule for which note a number gets.
    #[test]
    fn the_number_note_follows_the_engine_s_rule() {
        for q in [
            "Vendors I paid over $500",
            "Vendors over 500",
            "Bills of at least 1,000",
            "Vendors 500 dollars",
            "Vendors 5k",
            "Vendors 500 or more",
            "Bills costing 40",
        ] {
            assert_eq!(number_note(q), Some("amount_filter_left_out"), "{q}");
        }
        for q in [
            "Vendors in 2026",
            "Vendors over the year 2026",
            "Top 10 vendors",
            "Vendors since 2025",
        ] {
            assert_eq!(number_note(q), Some("numbers_not_used"), "{q}");
        }
        assert_eq!(number_note("Vendors by month"), None);
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
