//! Bank Feed rules (P26 DI-2): the engine's conditional rules as the
//! desktop edits them, and its rule suggestions (Q-141, Q-P26k: 5 accepts
//! and an override rate under 10%, counted by the engine).
//!
//! The desktop edits one shape of rule ([`FeedRule`]): every condition is
//! combined with AND — the description contains a text, the direction (money in or
//! out, `amount_sign` in the feed sign), an exact amount (`amount_abs`,
//! compared as a decimal string), one bank account (`account_source_id`)
//! — and the actions set one account and, optionally, a tag. A rule the
//! engine holds in any other shape (OR groups, splits, branches) is listed
//! read-only ([`FeedRule::simple`] false). Rules never post on their own;
//! "auto-accept" only lets the owner accept a rule's rows together.

use std::fmt;

use cl_core::Money;
use cl_core::ids::{AccountId, TagId};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::bank_import::wire_decimal;
use crate::resources::operations::{
    FEED_RULE_CREATE, FEED_RULE_DELETE, FEED_RULE_LIST, FEED_RULE_SUGGESTION_DISMISS,
    FEED_RULE_SUGGESTIONS, FEED_RULE_UPDATE,
};
use crate::session::{Session, json_body};

/// Money in, money out, or either (`amount_sign` in the feed sign: money
/// out is 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuleSign {
    /// Either way.
    #[default]
    Either,
    /// Money out (`amount_sign` 1).
    MoneyOut,
    /// Money in (`amount_sign` -1).
    MoneyIn,
}

/// A rule as the desktop edits it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FeedRule {
    /// The rule (`None` for a new one).
    pub id: Option<String>,
    /// Its name (owner data).
    pub name: String,
    /// On.
    pub active: bool,
    /// "Accept all from this rule" is offered (never posts on its own).
    pub auto_accept: bool,
    /// The change token (`updatedAt`).
    pub updated_at: Option<String>,
    /// The bank's description contains this (owner data).
    pub description: Option<String>,
    /// Money in or out.
    pub sign: RuleSign,
    /// Exactly this amount, either way (`amount_abs`).
    pub amount: Option<Money>,
    /// Only this bank account.
    pub bank_account: Option<AccountId>,
    /// The account it sets.
    pub account: Option<AccountId>,
    /// The tag it sets.
    pub tag: Option<TagId>,
    /// The desktop can edit it (one AND of the conditions above, set
    /// account / set tag actions).
    pub simple: bool,
}

impl fmt::Debug for FeedRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeedRule")
            .field("id", &self.id)
            .field("active", &self.active)
            .field("auto_accept", &self.auto_accept)
            .field("simple", &self.simple)
            .finish_non_exhaustive()
    }
}

/// A rule the engine suggests from the owner's repeated choices.
#[derive(Clone, PartialEq, Eq)]
pub struct RuleSuggestion {
    /// The description text the choices share (owner data).
    pub payee_pattern: String,
    /// The account chosen.
    pub account: AccountId,
    /// Its name.
    pub account_name: String,
    /// Times the owner accepted it.
    pub times_confirmed: u32,
}

impl fmt::Debug for RuleSuggestion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuleSuggestion")
            .field("account", &self.account)
            .field("times_confirmed", &self.times_confirmed)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------- wire

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRule {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    active: Option<bool>,
    #[serde(default)]
    auto_accept: bool,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    conditions: Value,
    #[serde(default)]
    actions: Value,
}

/// Reads one leaf into `r`; false when the desktop can't edit it.
fn leaf(r: &mut FeedRule, v: &Value) -> bool {
    let field = v.get("field").and_then(Value::as_str).unwrap_or_default();
    let op = v
        .get("operator")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let value = v.get("value");
    match (field, op) {
        ("descriptor", "contains") if r.description.is_none() => {
            r.description = value.and_then(Value::as_str).map(str::to_string);
            r.description.is_some()
        }
        ("amount_sign", "eq") => {
            r.sign = match value.and_then(Value::as_i64) {
                Some(1) => RuleSign::MoneyOut,
                Some(-1) => RuleSign::MoneyIn,
                _ => return false,
            };
            true
        }
        ("amount_abs", "eq") if r.amount.is_none() => {
            // A decimal string, never a float (a number is read from its
            // text).
            let text = match value {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => return false,
            };
            r.amount = Money::parse_wire(&text).ok();
            r.amount.is_some()
        }
        ("account_source_id", "eq") if r.bank_account.is_none() => {
            r.bank_account = value.and_then(Value::as_str).and_then(|s| s.parse().ok());
            r.bank_account.is_some()
        }
        _ => false,
    }
}

fn conditions(r: &mut FeedRule, v: &Value) -> bool {
    match v.get("type").and_then(Value::as_str) {
        Some("leaf") => leaf(r, v),
        Some("group") if v.get("op").and_then(Value::as_str) == Some("AND") => v
            .get("children")
            .and_then(Value::as_array)
            .is_some_and(|c| c.iter().all(|x| leaf(r, x))),
        _ => false,
    }
}

fn actions(r: &mut FeedRule, v: &Value) -> bool {
    let Some(list) = v.as_array() else {
        return false;
    };
    list.iter()
        .all(|a| match a.get("type").and_then(Value::as_str) {
            Some("set_account") if r.account.is_none() => {
                r.account = a
                    .get("accountId")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse().ok());
                r.account.is_some()
            }
            Some("set_tag") if r.tag.is_none() => {
                r.tag = a
                    .get("tagId")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse().ok());
                r.tag.is_some()
            }
            _ => false,
        })
}

impl WireRule {
    fn into_rule(self) -> FeedRule {
        let mut r = FeedRule {
            id: Some(self.id),
            name: self.name,
            active: self.active.unwrap_or(true),
            auto_accept: self.auto_accept,
            updated_at: self.updated_at,
            ..FeedRule::default()
        };
        let ok_c = conditions(&mut r, &self.conditions);
        let ok_a = actions(&mut r, &self.actions);
        r.simple = ok_c && ok_a && r.account.is_some();
        r
    }
}

#[derive(Deserialize)]
struct WireRules {
    rules: Vec<WireRule>,
}

/// Reads the rule list.
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn feed_rules_from(body: &[u8]) -> Result<Vec<FeedRule>, ApiError> {
    let w: WireRules = serde_json::from_slice(body).map_err(|_| ApiError::Decode {
        what: "conditional rules",
    })?;
    Ok(w.rules.into_iter().map(WireRule::into_rule).collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSuggestion {
    payee_pattern: String,
    account_id: String,
    #[serde(default)]
    account_name: String,
    #[serde(default)]
    times_confirmed: u32,
}

#[derive(Deserialize)]
struct WireSuggestions {
    suggestions: Vec<WireSuggestion>,
}

/// Reads the rule suggestions (an unreadable account id is left out).
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn rule_suggestions_from(body: &[u8]) -> Result<Vec<RuleSuggestion>, ApiError> {
    let w: WireSuggestions = serde_json::from_slice(body).map_err(|_| ApiError::Decode {
        what: "rule suggestions",
    })?;
    Ok(w.suggestions
        .into_iter()
        .filter_map(|s| {
            Some(RuleSuggestion {
                account: s.account_id.parse().ok()?,
                payee_pattern: s.payee_pattern,
                account_name: s.account_name,
                times_confirmed: s.times_confirmed,
            })
        })
        .collect())
}

/// The JSON of a rule (create and update).
///
/// # Errors
/// `Local(InvalidValue)` without a name, an account or any condition, or
/// with an amount that isn't positive with at most 2 decimals.
pub fn feed_rule_body(r: &FeedRule) -> Result<Value, ApiError> {
    let bad = || ApiError::Local(LocalValidation::InvalidValue);
    let name = r.name.trim();
    let account = r.account.ok_or_else(bad)?;
    if name.is_empty() {
        return Err(bad());
    }
    let leaf = |field: &str, operator: &str, value: Value| json!({ "type": "leaf", "field": field, "operator": operator, "value": value });
    let mut children = Vec::new();
    if let Some(d) = r
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        children.push(leaf("descriptor", "contains", json!(d)));
    }
    match r.sign {
        RuleSign::MoneyOut => children.push(leaf("amount_sign", "eq", json!(1))),
        RuleSign::MoneyIn => children.push(leaf("amount_sign", "eq", json!(-1))),
        RuleSign::Either => {}
    }
    if let Some(a) = r.amount {
        if !a.is_positive() {
            return Err(bad());
        }
        let text = wire_decimal(a.as_decimal()).map_err(ApiError::Local)?;
        children.push(leaf("amount_abs", "eq", json!(text)));
    }
    if let Some(b) = r.bank_account {
        children.push(leaf("account_source_id", "eq", json!(b.to_string())));
    }
    let conditions = match children.len() {
        0 => return Err(bad()),
        1 => children.remove(0),
        _ => json!({ "type": "group", "op": "AND", "children": children }),
    };
    let mut acts = vec![json!({ "type": "set_account", "accountId": account.to_string() })];
    if let Some(t) = r.tag {
        acts.push(json!({ "type": "set_tag", "tagId": t.to_string() }));
    }
    let mut m = Map::new();
    m.insert("name".into(), json!(name));
    m.insert("active".into(), json!(r.active));
    m.insert("autoAccept".into(), json!(r.auto_accept));
    m.insert("conditions".into(), conditions);
    m.insert("actions".into(), Value::Array(acts));
    Ok(Value::Object(m))
}

// ---------------------------------------------------------------- calls

/// The Bank Feed rules (`/practice/conditional-rules`).
#[derive(Clone, Copy)]
pub struct FeedRules<'a>(&'a Session);

impl Session {
    /// The Bank Feed rules.
    #[must_use]
    pub fn feed_rules(&self) -> FeedRules<'_> {
        FeedRules(self)
    }
}

fn rule_path(id: &str) -> Result<String, ApiError> {
    cl_core::ids::parse_uuid(id).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
    Ok(format!("/api/v1/practice/conditional-rules/{id}"))
}

impl FeedRules<'_> {
    /// Every rule.
    ///
    /// # Errors
    /// Any [`ApiError`] (`Forbidden` for a view-only user).
    pub async fn list(&self) -> Result<Vec<FeedRule>, ApiError> {
        let call = Call::at(Method::Get, FEED_RULE_LIST.route());
        let reply = self.0.send(&FEED_RULE_LIST, call, None).await?;
        feed_rules_from(&reply.body)
    }

    /// Saves a rule: creates it without an id, else replaces it.
    ///
    /// # Errors
    /// [`feed_rule_body`]'s checks; any [`ApiError`].
    pub async fn save(&self, r: &FeedRule) -> Result<(), ApiError> {
        let body = json_body(&feed_rule_body(r)?)?;
        match &r.id {
            None => {
                let call = Call::at(Method::Post, FEED_RULE_CREATE.route()).json_body(body);
                self.0.send(&FEED_RULE_CREATE, call, None).await?;
            }
            Some(id) => {
                let call = Call::new(Method::Put, FEED_RULE_UPDATE.route(), rule_path(id)?)
                    .json_body(body);
                self.0.send(&FEED_RULE_UPDATE, call, None).await?;
            }
        }
        Ok(())
    }

    /// Deletes a rule.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn delete(&self, id: &str) -> Result<(), ApiError> {
        let call = Call::new(Method::Delete, FEED_RULE_DELETE.route(), rule_path(id)?);
        self.0.send(&FEED_RULE_DELETE, call, None).await?;
        Ok(())
    }

    /// Rules the engine suggests from repeated choices.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn suggestions(&self) -> Result<Vec<RuleSuggestion>, ApiError> {
        let call = Call::at(Method::Get, FEED_RULE_SUGGESTIONS.route());
        let reply = self.0.send(&FEED_RULE_SUGGESTIONS, call, None).await?;
        rule_suggestions_from(&reply.body)
    }

    /// Never suggests this one again.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn dismiss(&self, s: &RuleSuggestion) -> Result<(), ApiError> {
        let body = json_body(&json!({
            "payeePattern": s.payee_pattern,
            "accountId": s.account.to_string(),
        }))?;
        let call = Call::at(Method::Post, FEED_RULE_SUGGESTION_DISMISS.route()).json_body(body);
        self.0
            .send(&FEED_RULE_SUGGESTION_DISMISS, call, None)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCT: &str = "55555555-5555-4555-8555-555555555555";
    const BANK: &str = "66666666-6666-4666-8666-666666666666";

    fn rule() -> FeedRule {
        FeedRule {
            name: "Fuel".into(),
            active: true,
            auto_accept: true,
            description: Some("RIVERBEND".into()),
            sign: RuleSign::MoneyOut,
            amount: Some(Money::parse_wire("84.17").expect("m")),
            bank_account: Some(BANK.parse().expect("bank")),
            account: Some(ACCT.parse().expect("acct")),
            ..FeedRule::default()
        }
    }

    /// Plan §6: "Money out" is `amount_sign` 1; the amount is an exact
    /// string; the round trip reads back the same rule.
    #[test]
    fn money_out_is_sign_1_and_the_rule_round_trips() {
        let body = feed_rule_body(&rule()).expect("body");
        let kids = body["conditions"]["children"].as_array().expect("and");
        assert!(
            kids.contains(&json!({"type":"leaf","field":"amount_sign","operator":"eq","value":1}))
        );
        assert!(kids.contains(
            &json!({"type":"leaf","field":"amount_abs","operator":"eq","value":"84.17"})
        ));
        assert_eq!(body["autoAccept"], true);
        let listed = json!({ "rules": [{
            "id": "77777777-7777-4777-8777-777777777777",
            "name": "Fuel", "active": true, "autoAccept": true,
            "updatedAt": "2026-11-01T00:00:00.000Z",
            "conditions": body["conditions"], "actions": body["actions"],
        }]});
        let back = feed_rules_from(listed.to_string().as_bytes()).expect("list");
        let r = &back[0];
        assert!(r.simple);
        assert_eq!(
            (r.sign, r.amount, r.description.as_deref()),
            (RuleSign::MoneyOut, rule().amount, Some("RIVERBEND"))
        );
        assert_eq!(r.account, rule().account);
        assert_eq!(r.bank_account, rule().bank_account);
    }

    #[test]
    fn other_shapes_are_read_only_and_bodies_need_a_condition() {
        let listed = json!({ "rules": [{
            "id": "77777777-7777-4777-8777-777777777777", "name": "Split",
            "conditions": {"type":"group","op":"OR","children":[]},
            "actions": [{"type":"split_by_percentage","splits":[]}],
        }]});
        assert!(!feed_rules_from(listed.to_string().as_bytes()).expect("l")[0].simple);
        let bare = FeedRule {
            description: None,
            sign: RuleSign::Either,
            amount: None,
            bank_account: None,
            ..rule()
        };
        assert!(feed_rule_body(&bare).is_err());
        let one = FeedRule {
            sign: RuleSign::Either,
            amount: None,
            bank_account: None,
            ..rule()
        };
        assert_eq!(
            feed_rule_body(&one).expect("b")["conditions"]["type"],
            "leaf"
        );
    }

    #[test]
    fn suggestions_read_without_floats() {
        let s = rule_suggestions_from(
            format!(r#"{{"suggestions":[{{"payeePattern":"RIVERBEND","accountId":"{ACCT}","accountName":"Fuel","timesConfirmed":6,"overrideRate":0.0}}]}}"#)
                .as_bytes(),
        )
        .expect("s");
        assert_eq!(s[0].times_confirmed, 6);
        assert!(!format!("{s:?}").contains("RIVERBEND"));
    }
}
