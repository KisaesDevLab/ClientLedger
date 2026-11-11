//! Ask a question (U-206, `reports-ask-v1`, Q-73, Q-P28g): the company's AI
//! provider maps a plain-language question to one catalog report and its
//! options; the window then runs that report and the engine computes every
//! figure. The answer here never carries an amount, and nothing in it sets
//! an amount filter (non-negotiable 4).
//!
//! The engine already refuses anything outside the catalog sent; the
//! answer is still checked again here against that same catalog
//! ([`validate`]): a report key, preset, column or extra that was not sent,
//! an id that doesn't parse, or an unresolved name that is not the owner's
//! own words is refused as [`ApiError::ReportShape`], and the window shows
//! nothing from it.

use cl_core::CalendarDate;
use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::report::Basis;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::REPORT_ASK;
use crate::session::Session;
use crate::wire::Capabilities;

/// The longest question the engine takes.
pub const MAX_QUESTION_CHARS: usize = 500;

/// An option a catalog report takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AskParam {
    /// A from/to period.
    Range,
    /// A balance date.
    AsOf,
    /// Cash or accrual.
    Basis,
    /// Account filter.
    Accounts,
    /// Client or vendor filter.
    Contacts,
    /// Tag filter.
    Tags,
}

/// One report of the catalog sent with a question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AskReport {
    /// The catalog key (`[a-z0-9_]{1,64}`).
    pub key: String,
    /// Its title, in the owner's language.
    pub title: String,
    /// The options it takes.
    pub params: Vec<AskParam>,
    /// Its column choices (`total`, `month`, ...).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// Its extras (`pct_of_income`, `ytd`, ...).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extras: Vec<String>,
}

/// What is sent: the question, the catalog and the date presets (the app
/// resolves presets itself: fiscal years, 4-4-5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskRequest {
    /// The owner's question.
    pub question: String,
    /// The catalog.
    pub reports: Vec<AskReport>,
    /// The preset keys.
    pub date_presets: Vec<String>,
    /// The owner's today (relative questions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub today: Option<CalendarDate>,
}

/// The dates of a mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskDates {
    /// A preset the app sent.
    Preset(String),
    /// A period.
    Range(CalendarDate, CalendarDate),
    /// A balance date.
    AsOf(CalendarDate),
    /// The report's default.
    Default,
}

/// What a name the engine could not find was meant to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    /// An account.
    Account,
    /// A client or vendor.
    Contact,
    /// A tag.
    Tag,
}

/// A checked mapping: only tokens the app sent, dates, and ids the engine
/// resolved from names. No figure, no amount filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskMapping {
    /// The report.
    pub report_key: String,
    /// Its dates.
    pub dates: AskDates,
    /// Its basis.
    pub basis: Option<Basis>,
    /// A column choice the report offers.
    pub columns: Option<String>,
    /// Extras the report offers.
    pub extras: Vec<String>,
    /// Accounts.
    pub account_ids: Vec<AccountId>,
    /// Clients or vendors.
    pub contact_ids: Vec<ContactId>,
    /// Tags.
    pub tag_ids: Vec<TagId>,
    /// Names in the question the engine couldn't find (the owner's words,
    /// shown as chips).
    pub unresolved: Vec<(NameKind, String)>,
    /// `self_hosted` (Ollama) or `cloud`.
    pub reader: Option<String>,
}

/// Why a question got no mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskRefusal {
    /// AI is off for the company or Ask is switched off (403
    /// `AI_CONSENT_REQUIRED`; no model was called).
    ConsentRequired,
    /// No report fits, or the model's answer was refused (422
    /// `ASK_UNMAPPED`).
    Unmapped,
    /// The AI provider can't be reached or isn't set up (503
    /// `AI_UNAVAILABLE`): reports still work.
    Unavailable,
}

/// The ask refusal in `e`, if it is one.
#[must_use]
pub fn ask_refusal(e: &ApiError) -> Option<AskRefusal> {
    let code = match e {
        ApiError::Forbidden { code: Some(c) } | ApiError::Server { code: Some(c), .. } => {
            c.as_str()
        }
        ApiError::Validation(v) => v.code().map_or("", crate::errors::ErrorCode::as_str),
        ApiError::OfflineUnavailable { .. } => return Some(AskRefusal::Unavailable),
        _ => return None,
    };
    match code {
        "AI_CONSENT_REQUIRED" => Some(AskRefusal::ConsentRequired),
        "ASK_UNMAPPED" => Some(AskRefusal::Unmapped),
        "AI_UNAVAILABLE" => Some(AskRefusal::Unavailable),
        _ => None,
    }
}

fn bad() -> ApiError {
    ApiError::ReportShape { what: "ask" }
}

fn token<'a>(v: &'a Value, key: &str, allowed: &[String]) -> Result<Option<&'a str>, ApiError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if allowed.iter().any(|a| a == s) => Ok(Some(s)),
        Some(_) => Err(bad()),
    }
}

fn date(v: &Value) -> Result<CalendarDate, ApiError> {
    v.as_str()
        .and_then(|s| CalendarDate::parse_wire(s).ok())
        .ok_or_else(bad)
}

fn ids<T>(v: &Value, key: &str, parse: impl Fn(&str) -> Option<T>) -> Result<Vec<T>, ApiError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().and_then(&parse).ok_or_else(bad))
            .collect(),
        Some(_) => Err(bad()),
    }
}

/// Checks the engine's answer against what was sent.
///
/// # Errors
/// [`ApiError::ReportShape`] for anything outside the catalog, presets,
/// columns or extras sent, an option the report doesn't take, an id that
/// doesn't parse, or an unresolved name that is not in the question.
pub fn validate(req: &AskRequest, v: &Value) -> Result<AskMapping, ApiError> {
    let keys: Vec<String> = req.reports.iter().map(|r| r.key.clone()).collect();
    let key = token(v, "reportKey", &keys)?.ok_or_else(bad)?;
    let report = req.reports.iter().find(|r| r.key == key).ok_or_else(bad)?;
    let takes = |p: AskParam| report.params.contains(&p);
    let dates = match v.get("dates") {
        None | Some(Value::Null) => AskDates::Default,
        Some(d) => {
            if let Some(p) = token(d, "preset", &req.date_presets)? {
                if !takes(AskParam::Range) && !takes(AskParam::AsOf) {
                    return Err(bad());
                }
                AskDates::Preset(p.to_string())
            } else if let Some(a) = d.get("asOf") {
                if !takes(AskParam::AsOf) {
                    return Err(bad());
                }
                AskDates::AsOf(date(a)?)
            } else if let (Some(f), Some(t)) = (d.get("from"), d.get("to")) {
                if !takes(AskParam::Range) {
                    return Err(bad());
                }
                let (f, t) = (date(f)?, date(t)?);
                if f > t {
                    return Err(bad());
                }
                AskDates::Range(f, t)
            } else {
                return Err(bad());
            }
        }
    };
    let basis = match v.get("basis").and_then(Value::as_str) {
        None => None,
        Some(_) if !takes(AskParam::Basis) => return Err(bad()),
        Some("cash") => Some(Basis::Cash),
        Some("accrual") => Some(Basis::Accrual),
        Some(_) => return Err(bad()),
    };
    let columns = token(v, "columns", &report.columns)?.map(str::to_string);
    let extras = match v.get("extras") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x.as_str() {
                Some(s) if report.extras.iter().any(|e| e == s) => Ok(s.to_string()),
                _ => Err(bad()),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(bad()),
    };
    let f = v.get("filters").cloned().unwrap_or(Value::Null);
    // No amount filter, whatever the answer says (non-negotiable 4).
    if let Value::Object(m) = &f
        && m.keys().any(|k| k.to_ascii_lowercase().contains("amount"))
    {
        return Err(bad());
    }
    let account_ids = ids(&f, "accountIds", |s| AccountId::parse(s).ok())?;
    let contact_ids = ids(&f, "contactIds", |s| ContactId::parse(s).ok())?;
    let tag_ids = ids(&f, "tagIds", |s| TagId::parse(s).ok())?;
    if (!account_ids.is_empty() && !takes(AskParam::Accounts))
        || (!contact_ids.is_empty() && !takes(AskParam::Contacts))
        || (!tag_ids.is_empty() && !takes(AskParam::Tags))
    {
        return Err(bad());
    }
    let question = req.question.to_lowercase();
    let mut unresolved = Vec::new();
    for u in v
        .get("unresolved")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let kind = match u.get("kind").and_then(Value::as_str) {
            Some("account") => NameKind::Account,
            Some("contact") => NameKind::Contact,
            Some("tag") => NameKind::Tag,
            _ => return Err(bad()),
        };
        let text = u.get("text").and_then(Value::as_str).unwrap_or("").trim();
        // Only the owner's own words, and never a number.
        if text.is_empty()
            || text.chars().any(|c| c.is_ascii_digit())
            || !question.contains(&text.to_lowercase())
        {
            continue;
        }
        unresolved.push((kind, text.to_string()));
    }
    Ok(AskMapping {
        report_key: key.to_string(),
        dates,
        basis,
        columns,
        extras,
        account_ids,
        contact_ids,
        tag_ids,
        unresolved,
        reader: v.get("reader").and_then(Value::as_str).map(str::to_string),
    })
}

impl Session {
    /// True when the engine maps questions (`reports-ask-v1`).
    #[must_use]
    pub fn can_ask_reports(&self) -> bool {
        self.client().capabilities().has(Capabilities::REPORTS_ASK)
    }

    /// Asks the company's AI provider which report answers `req.question`.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `reports-ask-v1`; `Local(InvalidValue)`
    /// for an empty or over-long question (nothing sent); [`ask_refusal`]
    /// for the engine's; [`ApiError::ReportShape`] when the answer goes
    /// outside what was sent; any [`ApiError`].
    pub async fn ask_report(&self, req: &AskRequest) -> Result<AskMapping, ApiError> {
        if !self.can_ask_reports() {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let q = req.question.trim();
        if q.is_empty() || q.chars().count() > MAX_QUESTION_CHARS || req.reports.is_empty() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let body =
            serde_json::to_vec(req).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let call = Call::at(Method::Post, REPORT_ASK.route()).json_body(body);
        let reply = self.send(&REPORT_ASK, call, None).await?;
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| bad())?;
        validate(req, &v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ACCT: &str = "11111111-1111-4111-8111-111111111111";

    fn req() -> AskRequest {
        AskRequest {
            question: "What did we spend on Fuel last month, cash basis?".into(),
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
            today: CalendarDate::from_ymd(2026, 11, 10).ok(),
        }
    }

    fn answer() -> Value {
        json!({"reportKey": "profit_loss", "dates": {"preset": "last_month"}, "basis": "cash",
               "columns": "total", "extras": ["pct_of_income"],
               "filters": {"accountIds": [ACCT], "contactIds": [], "tagIds": []},
               "unresolved": [{"kind": "contact", "text": "Fuel"}],
               "reader": "self_hosted", "provider": "ollama", "model": "llama3"})
    }

    #[test]
    fn a_mapping_inside_the_catalog_is_taken() {
        let m = validate(&req(), &answer()).expect("mapping");
        assert_eq!(m.report_key, "profit_loss");
        assert_eq!(m.dates, AskDates::Preset("last_month".into()));
        assert_eq!(m.basis, Some(Basis::Cash));
        assert_eq!(m.account_ids.len(), 1);
        assert_eq!(m.unresolved, vec![(NameKind::Contact, "Fuel".to_string())]);
    }

    #[test]
    fn anything_outside_what_was_sent_is_refused() {
        let cases: Vec<(&str, Value)> = vec![
            ("reportKey", json!("cash_flow")),
            ("dates", json!({"preset": "last_decade"})),
            ("columns", json!("by_vendor")),
            ("extras", json!(["ytd"])),
            ("basis", json!("modified_cash")),
            ("filters", json!({"accountIds": ["not-an-id"]})),
            ("filters", json!({"amountMin": "100"})),
            ("filters", json!({"contactIds": [ACCT]})),
        ];
        for (k, bad_value) in cases {
            let mut v = answer();
            v[k] = bad_value.clone();
            assert!(validate(&req(), &v).is_err(), "{k} = {bad_value}");
        }
        // A balance sheet takes no period.
        let v = json!({"reportKey": "balance_sheet", "dates": {"from": "2026-10-01", "to": "2026-10-31"}});
        assert!(validate(&req(), &v).is_err());
    }

    #[test]
    fn unresolved_names_are_only_the_owner_s_words_without_digits() {
        let mut v = answer();
        v["unresolved"] = json!([
            {"kind": "account", "text": "Payroll 2024"},
            {"kind": "account", "text": "Something the model made up"},
            {"kind": "tag", "text": "fuel"}
        ]);
        let m = validate(&req(), &v).expect("mapping");
        assert_eq!(m.unresolved, vec![(NameKind::Tag, "fuel".to_string())]);
    }

    #[test]
    fn the_request_sends_no_amount_and_camel_case_presets() {
        let v = serde_json::to_value(req()).expect("json");
        assert_eq!(v["datePresets"][0], "last_month");
        assert_eq!(v["reports"][0]["params"][1], "basis");
        assert_eq!(v["today"], "2026-11-10");
        assert!(!v.to_string().to_lowercase().contains("amount"));
    }

    #[test]
    fn refusals_are_recognised() {
        let consent = crate::engine_refusal(
            403,
            &json!({"error": {"message": "x", "code": "AI_CONSENT_REQUIRED", "details": {"reason": "ai_disabled", "task": "report_summary"}}}),
        );
        assert_eq!(ask_refusal(&consent), Some(AskRefusal::ConsentRequired));
        let unmapped = crate::engine_refusal(
            422,
            &json!({"error": {"message": "x", "code": "ASK_UNMAPPED", "details": {"field": "reportKey", "reason": "no_fit"}}}),
        );
        assert_eq!(ask_refusal(&unmapped), Some(AskRefusal::Unmapped));
        let down = crate::engine_refusal(
            503,
            &json!({"error": {"message": "x", "code": "AI_UNAVAILABLE", "details": {"provider": "ollama", "reason": "offline"}}}),
        );
        assert_eq!(ask_refusal(&down), Some(AskRefusal::Unavailable));
    }
}
