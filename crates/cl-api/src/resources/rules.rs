//! The owner's rule that matches a form (P13 WP2, U-51, capability
//! `rules-evaluate-v1`): `POST /practice/conditional-rules/evaluate`.
//!
//! The rule only names an account, a tag and a contact. The amount sent is
//! the owner's typed amount as an exact decimal string, and nothing is read
//! back as money. Works offline (the rules live in the books). The call is
//! refused locally on an engine that does not advertise the capability.
//! The route sits under the engine's practice router and carries its gate:
//! a 404 while the tenant's conditional rules (`CONDITIONAL_RULES_V1`) are
//! off. New companies have them on; a company restored from an older
//! archive may not, and that 404 reads as "no rule matched".

use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money};
use serde::{Deserialize, Serialize};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::RULES_EVALUATE;
use crate::session::{Session, decode, json_body};
use crate::wire::Capabilities;

/// The longest description the engine reads for a rule match (UTF-16
/// code units, as the engine's schema counts them).
pub const RULE_DESCRIPTION_MAX_CHARS: usize = 1000;

/// What the form holds when it asks for a rule.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleQuery {
    /// Payee name or memo the rules match on.
    pub description: String,
    /// The owner's typed amount (exact string on the wire).
    pub amount: Money,
    /// The chosen payee, if any.
    #[serde(rename = "contactId", skip_serializing_if = "Option::is_none")]
    pub contact: Option<ContactId>,
    /// The bank account the check is written from.
    #[serde(rename = "bankAccountId")]
    pub bank_account: AccountId,
    /// The form's date (rules may test the date or the weekday); the
    /// engine takes today (UTC) without it.
    #[serde(rename = "txnDate", skip_serializing_if = "Option::is_none")]
    pub date: Option<CalendarDate>,
}

/// Hides the description and the amount (owner data).
impl std::fmt::Debug for RuleQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuleQuery")
            .field("contact", &self.contact)
            .field("bank_account", &self.bank_account)
            .finish_non_exhaustive()
    }
}

/// The rule that matched and what it suggests (never an amount).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleMatch {
    /// The rule.
    pub rule_id: String,
    /// The rule's name, shown in the suggestion caption.
    pub rule_name: String,
    /// Suggested account.
    #[serde(default)]
    pub account_id: Option<AccountId>,
    /// Suggested tag.
    #[serde(default)]
    pub tag_id: Option<TagId>,
    /// Suggested payee.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
}

/// Hides the rule name (the owner's own words, may name a vendor).
impl std::fmt::Debug for RuleMatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuleMatch")
            .field("account_id", &self.account_id)
            .field("tag_id", &self.tag_id)
            .field("contact_id", &self.contact_id)
            .finish_non_exhaustive()
    }
}

/// Rule operations of a session (`session.rules()`).
#[derive(Clone, Copy, Debug)]
pub struct Rules<'a>(pub(crate) &'a Session);

impl Session {
    /// The owner's rules.
    #[must_use]
    pub fn rules(&self) -> Rules<'_> {
        Rules(self)
    }
}

impl Rules<'_> {
    /// The first of the owner's rules that matches, or `None`.
    ///
    /// # Errors
    /// `Local(RulesEvaluateUnavailable)` until the engine advertises
    /// `rules-evaluate-v1`, `Local(InvalidValue)` for a description over
    /// 1000 characters (nothing sent in either case); any [`ApiError`].
    /// While the company's conditional rules are off (the engine's 404)
    /// the answer is `None`: no rule suggests anything.
    pub async fn evaluate(&self, query: &RuleQuery) -> Result<Option<RuleMatch>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            #[serde(rename = "match", default)]
            matched: Option<RuleMatch>,
        }
        if !self
            .0
            .client()
            .capabilities()
            .has(Capabilities::RULES_EVALUATE)
        {
            return Err(ApiError::Local(LocalValidation::RulesEvaluateUnavailable));
        }
        if query.description.encode_utf16().count() > RULE_DESCRIPTION_MAX_CHARS {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let call = Call::at(Method::Post, RULES_EVALUATE.route()).json_body(json_body(query)?);
        let reply = match self.0.send(&RULES_EVALUATE, call, None).await {
            Ok(r) => r,
            Err(ApiError::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        decode::<W>(&reply, "rule match").map(|w| w.matched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests, session_with_caps};
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const BANK: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    const MULCH: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";

    fn q() -> RuleQuery {
        RuleQuery {
            description: "Ozark Feed canary".into(),
            amount: Money::parse_wire("1286.4000").expect("m"),
            contact: None,
            bank_account: AccountId::parse(BANK).expect("id"),
            date: None,
        }
    }

    const ROUTE: &str = "/api/v1/practice/conditional-rules/evaluate";

    #[tokio::test]
    async fn refused_without_the_capability() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        let e = s.rules().evaluate(&q()).await.expect_err("gated");
        assert_eq!(
            e,
            ApiError::Local(LocalValidation::RulesEvaluateUnavailable)
        );
        assert!(server.received_requests().await.expect("r").is_empty());
    }

    #[tokio::test]
    async fn sends_the_typed_amount_as_a_string_and_reads_the_match() {
        let server = MockServer::start().await;
        let s = session_with_caps(&server, &[Capabilities::RULES_EVALUATE]).await;
        Mock::given(method("POST"))
            .and(path(ROUTE))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"match":{{"ruleId":"r1","ruleName":"Feed store","accountId":"{MULCH}",
                    "tagId":null,"contactId":null,"amount":"9.99"}}}}"#
                ),
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(ROUTE))
            .respond_with(json(200, r#"{"match":null}"#))
            .mount(&server)
            .await;
        let m = s
            .rules()
            .evaluate(&q())
            .await
            .expect("eval")
            .expect("match");
        assert_eq!(m.account_id, AccountId::parse(MULCH).ok());
        assert_eq!(m.rule_name, "Feed store");
        assert!(!format!("{m:?} {:?}", q()).contains("canary"));
        assert!(!format!("{m:?}").contains("Feed store"));
        let body: Value =
            serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
        assert_eq!(
            body,
            serde_json::json!({"description":"Ozark Feed canary","amount":"1286.4000","bankAccountId":BANK})
        );
        let mut dated = q();
        dated.date = CalendarDate::from_ymd(2026, 11, 4).ok();
        assert_eq!(s.rules().evaluate(&dated).await.expect("none"), None);
        let body: Value =
            serde_json::from_slice(&requests(&server, "POST").await[1].body).expect("json");
        assert_eq!(body["txnDate"], "2026-11-04");

        // 1000 UTF-16 units pass; one more (an emoji counts two) is refused.
        let mut long = q();
        long.description = "x".repeat(1000);
        assert_eq!(s.rules().evaluate(&long).await.expect("at the limit"), None);
        long.description = format!("{}\u{1F33F}", "x".repeat(999));
        assert_eq!(
            s.rules().evaluate(&long).await.expect_err("long"),
            ApiError::Local(LocalValidation::InvalidValue)
        );
        assert_eq!(requests(&server, "POST").await.len(), 3);
    }

    #[tokio::test]
    async fn rules_switched_off_read_as_no_match() {
        let server = MockServer::start().await;
        let s = session_with_caps(&server, &[Capabilities::RULES_EVALUATE]).await;
        Mock::given(method("POST"))
            .and(path(ROUTE))
            .respond_with(json(
                404,
                r#"{"error":{"message":"Feature not available"}}"#,
            ))
            .mount(&server)
            .await;
        assert_eq!(s.rules().evaluate(&q()).await.expect("quiet"), None);
        assert_eq!(requests(&server, "POST").await.len(), 1, "never retried");
    }
}
