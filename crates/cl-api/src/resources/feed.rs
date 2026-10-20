//! Bank-file deposits waiting for an account and the engine's suggested
//! groupings of pending payments that add up to one (P14 lane A, Q-P14e).
//!
//! - [`Feed::deposits_waiting`]: money-in items of the bank feed that are
//!   still pending (`GET /banking/feed?status=pending`; the feed stores
//!   money in as a negative amount, shown here as the positive deposit).
//! - [`Feed::deposit_groupings`]: `GET /banking/feed/{id}/deposit-groupings`
//!   (`feed-deposit-groupings-v1`; refused locally without it). Each
//!   candidate is a set of pending payments whose exact amounts add up to
//!   the bank deposit; the screen only ticks them, it never saves on its
//!   own. Read-only; feed matching is P26.
//!
//! Every amount is an exact decimal string; a JSON number where a feed
//! amount belongs leaves the item out (it could never match exactly).

use cl_core::ids::parse_uuid;
use cl_core::{CalendarDate, Money};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::{FEED_DEPOSIT_GROUPINGS, FEED_LIST};
use crate::resources::payments::{PendingPayment, pending_from};
use crate::session::Session;
use crate::wire::Capabilities;

/// The most suggested groupings asked for at once (the engine allows 20).
pub const GROUPINGS_LIMIT: u8 = 5;
/// The most waiting feed items read for the panel.
const FEED_PAGE: u16 = 200;

/// A bank-feed item's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeedItemId(uuid::Uuid);

impl FeedItemId {
    /// Parses a feed item id.
    ///
    /// # Errors
    /// `Decode` when it is not a UUID.
    pub fn parse(s: &str) -> Result<FeedItemId, ApiError> {
        parse_uuid(s).map(FeedItemId).map_err(|_| ApiError::Decode {
            what: "feed item id",
        })
    }
}

impl std::fmt::Display for FeedItemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A bank-file deposit waiting in the feed. `Debug` prints the id and date.
#[derive(Clone, PartialEq, Eq)]
pub struct FeedDeposit {
    /// The feed item.
    pub id: FeedItemId,
    /// The date the bank shows.
    pub date: CalendarDate,
    /// The deposit as the bank reports it (positive), exactly.
    pub amount: Money,
    /// The bank's description (owner data).
    pub description: Option<String>,
    /// The name of the account the bank file belongs to.
    pub account_name: Option<String>,
}

impl std::fmt::Debug for FeedDeposit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedDeposit")
            .field("id", &self.id)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// One suggested set of pending payments.
#[derive(Clone, Debug)]
pub struct FeedGrouping {
    /// The payments, by date.
    pub items: Vec<PendingPayment>,
}

/// The engine's suggestions for one bank-file deposit.
#[derive(Clone)]
pub struct FeedGroupings {
    /// The bank deposit (positive).
    pub target: Money,
    /// The sets whose exact amounts add up to `target`, best first.
    pub candidates: Vec<FeedGrouping>,
    /// The search was cut short (more sets may exist).
    pub truncated: bool,
}

impl std::fmt::Debug for FeedGroupings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedGroupings")
            .field("candidates", &self.candidates.len())
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireFeedItem {
    id: String,
    feed_date: CalendarDate,
    amount: Box<RawValue>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    bank_account_name: Option<String>,
}

#[derive(Deserialize)]
struct WireFeed {
    data: Vec<WireFeedItem>,
}

/// The money-in items of a feed page, as deposits (positive amounts).
pub(crate) fn deposits_from(body: &[u8]) -> Result<Vec<FeedDeposit>, ApiError> {
    const WHAT: &str = "bank feed";
    let w: WireFeed = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let mut out = Vec::new();
    for i in w.data {
        if i.status.as_deref().is_some_and(|s| s != "pending") {
            continue;
        }
        // An exact string only: a number can never match to the cent.
        let text = i.amount.get();
        let Some(m) = text
            .strip_prefix('"')
            .and_then(|t| t.strip_suffix('"'))
            .and_then(|t| Money::parse_wire(t).ok())
        else {
            continue;
        };
        if !m.is_negative() {
            continue;
        }
        out.push(FeedDeposit {
            id: FeedItemId::parse(&i.id)?,
            date: i.feed_date,
            amount: m.abs(),
            description: i.description,
            account_name: i.bank_account_name,
        });
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCandidate {
    items: Box<RawValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireGroupings {
    target_amount: String,
    candidates: Vec<WireCandidate>,
    #[serde(default)]
    truncated: bool,
}

/// Reads a `deposit-groupings` answer. A candidate whose items are not all
/// exact is dropped (it could not be deposited as it is).
pub(crate) fn groupings_from(body: &[u8]) -> Result<FeedGroupings, ApiError> {
    const WHAT: &str = "deposit groupings";
    let w: WireGroupings =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let target =
        Money::parse_wire(&w.target_amount).map_err(|_| ApiError::Decode { what: WHAT })?;
    let mut candidates = Vec::with_capacity(w.candidates.len());
    for c in w.candidates {
        // The items have the pending-deposits row shape.
        let wrapped = format!(
            r#"{{"paymentsClearingBalance":"0","items":{}}}"#,
            c.items.get()
        );
        let p = pending_from(wrapped.as_bytes())?;
        if p.items.iter().all(|i| i.exact().is_some()) {
            candidates.push(FeedGrouping { items: p.items });
        }
    }
    Ok(FeedGroupings {
        target,
        candidates,
        truncated: w.truncated,
    })
}

/// Bank feed readers of a session (`session.feed()`).
#[derive(Clone, Copy, Debug)]
pub struct Feed<'a>(pub(crate) &'a Session);

impl Session {
    /// The bank feed of the open company.
    #[must_use]
    pub fn feed(&self) -> Feed<'_> {
        Feed(self)
    }
}

impl Feed<'_> {
    /// Bank-file deposits still waiting in the feed (newest first, at most
    /// 200).
    ///
    /// # Errors
    /// `Decode` for an unreadable answer; any [`ApiError`].
    pub async fn deposits_waiting(&self) -> Result<Vec<FeedDeposit>, ApiError> {
        let mut call = Call::at(Method::Get, FEED_LIST.route());
        call.query = vec![
            ("status".into(), "pending".into()),
            ("limit".into(), FEED_PAGE.to_string()),
        ];
        let reply = self.0.send(&FEED_LIST, call, None).await?;
        deposits_from(&reply.body)
    }

    /// The engine's suggested groupings of pending payments for a bank-file
    /// deposit (`feed-deposit-groupings-v1`).
    ///
    /// # Errors
    /// `Local(FeedGroupingsUnavailable)` without the capability (nothing
    /// sent); `NotFound` for another company's item; any [`ApiError`].
    pub async fn deposit_groupings(&self, id: FeedItemId) -> Result<FeedGroupings, ApiError> {
        if !self
            .0
            .client()
            .capabilities()
            .has(Capabilities::FEED_DEPOSIT_GROUPINGS)
        {
            return Err(ApiError::Local(LocalValidation::FeedGroupingsUnavailable));
        }
        let mut call = Call::new(
            Method::Get,
            FEED_DEPOSIT_GROUPINGS.route(),
            format!("/api/v1/banking/feed/{id}/deposit-groupings"),
        );
        call.query = vec![("limit".into(), GROUPINGS_LIMIT.to_string())];
        let reply = self.0.send(&FEED_DEPOSIT_GROUPINGS, call, None).await?;
        groupings_from(&reply.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests, session_with_caps};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const GROUPINGS: &str = include_str!("../../../../fixtures/engine/feed-deposit-groupings.json");

    #[test]
    fn groupings_read_exact_sets() {
        let g = groupings_from(GROUPINGS.as_bytes()).expect("groupings");
        assert_eq!(g.target.to_wire(), "2092.5000");
        assert_eq!(g.candidates.len(), 1);
        let set = &g.candidates[0].items;
        assert_eq!(set.len(), 2);
        let sum = Money::sum(set.iter().filter_map(PendingPayment::exact)).expect("sum");
        assert_eq!(sum, g.target, "the engine's set adds up exactly");
        assert!(!g.truncated);
        assert!(!format!("{g:?}").contains("2092"));
    }

    #[test]
    fn a_set_with_a_number_is_dropped() {
        let body = br#"{"feedItemId":"x","targetAmount":"5.0000","candidates":[
            {"total":"5.0000","itemCount":1,"items":[{"transactionId":"5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
            "txnType":"customer_payment","date":"2026-11-02","amount":5}]}],"truncated":true}"#;
        let g = groupings_from(body).expect("groupings");
        assert!(g.candidates.is_empty());
        assert!(g.truncated);
    }

    #[test]
    fn feed_money_in_becomes_positive_deposits() {
        let body = br#"{"data":[
            {"id":"7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","feedDate":"2026-11-04","amount":"-2092.5000","status":"pending","description":"DEPOSIT","bankAccountName":"Operating"},
            {"id":"7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02","feedDate":"2026-11-04","amount":"64.2000","status":"pending"},
            {"id":"7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03","feedDate":"2026-11-04","amount":-10,"status":"pending"},
            {"id":"7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a04","feedDate":"2026-11-04","amount":"-5.0000","status":"matched"}
        ],"total":4}"#;
        let d = deposits_from(body).expect("feed");
        assert_eq!(d.len(), 1, "money in, exact, pending only");
        assert_eq!(d[0].amount.to_wire(), "2092.5000");
        assert_eq!(d[0].account_name.as_deref(), Some("Operating"));
        assert!(!format!("{:?}", d[0]).contains("2092"));
    }

    #[tokio::test]
    async fn groupings_need_the_capability() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        let id = FeedItemId::parse("7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
        assert!(matches!(
            s.feed().deposit_groupings(id).await,
            Err(ApiError::Local(LocalValidation::FeedGroupingsUnavailable))
        ));
        assert!(requests(&server, "GET").await.is_empty(), "nothing sent");

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/banking/feed/{id}/deposit-groupings")))
            .respond_with(json(200, GROUPINGS))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/banking/feed"))
            .respond_with(json(200, r#"{"data":[],"total":0}"#))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[Capabilities::FEED_DEPOSIT_GROUPINGS]).await;
        let g = s.feed().deposit_groupings(id).await.expect("groupings");
        assert_eq!(g.candidates.len(), 1);
        let waiting = s.feed().deposits_waiting().await.expect("feed");
        assert_eq!(waiting, Vec::<FeedDeposit>::new());
        let asked: Vec<String> = requests(&server, "GET")
            .await
            .iter()
            .filter(|r| r.url.path().starts_with("/api/v1/banking"))
            .map(query)
            .collect();
        assert_eq!(asked, ["limit=5", "status=pending&limit=200"]);
    }
}
