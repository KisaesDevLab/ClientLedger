//! Voids of bills, vendor credits and bill payments through their own
//! engine routes (P17 `WP2b`, plan review B1).
//!
//! The engine's generic `POST /transactions/{id}/void` and
//! `DELETE /transactions/{id}` do no Accounts Payable checks on the pinned
//! engine: voiding a paid bill, an applied vendor credit or a bill payment
//! there leaves the payment links in place, and A/P and the documents
//! disagree. These routes are the safe ones:
//!
//! - `POST /bills/{id}/void` refuses a bill with payments applied;
//! - `POST /vendor-credits/{id}/void` refuses a credit applied to bills;
//! - `POST /bill-payments/{id}/void` reverses the payment under a row lock
//!   and takes its applications off the bills and credits it paid.
//!
//! None of them takes a closing-date password on the pinned engine, so
//! these wrappers send none; a closed-period date comes back as
//! `ApiError::ClosedPeriod`. P17 WP2's `Bills::void` and
//! `VendorCredits::void` use the same path ([`void_with`]) with the A/P
//! options (closing-date password with `ap-closing-v1`, the stale guard
//! with `ap-edit-v1`).

use cl_core::ids::TransactionId;
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::ap_doc::{ApWriteOptions, ap_body};
use crate::resources::operations::{
    BILL_PAYMENT_VOID, BILL_VOID, Operation, TXN_GET, VENDOR_CREDIT_VOID,
};
use crate::resources::reconcile::{OutcomeMatcher, Probe};
use crate::resources::transactions::SPEC;
use crate::session::Session;

/// The A/P document voids of a session (`session.ap_voids()`).
#[derive(Clone, Copy, Debug)]
pub struct ApVoids<'a>(pub(crate) &'a Session);

impl Session {
    /// Voids of bills, vendor credits and bill payments.
    #[must_use]
    pub fn ap_voids(&self) -> ApVoids<'_> {
        ApVoids(self)
    }
}

impl ApVoids<'_> {
    /// Voids a bill (`POST /bills/{id}/void`). A reason is required.
    ///
    /// # Errors
    /// `Local(MissingReason)` for a blank reason (nothing sent);
    /// `Validation` when the bill has payments applied (or is not a bill,
    /// or is already void); `ClosedPeriod`; `NotFound`; any [`ApiError`].
    pub async fn bill(
        &self,
        id: TransactionId,
        reason: &str,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        void(self.0, &BILL_VOID, "/api/v1/bills", id, reason, key).await
    }

    /// Voids a vendor credit (`POST /vendor-credits/{id}/void`). A reason
    /// is required.
    ///
    /// # Errors
    /// `Local(MissingReason)` for a blank reason (nothing sent);
    /// `Validation` when the credit has been applied to bills; any
    /// [`ApiError`] as for [`ApVoids::bill`].
    pub async fn vendor_credit(
        &self,
        id: TransactionId,
        reason: &str,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        void(
            self.0,
            &VENDOR_CREDIT_VOID,
            "/api/v1/vendor-credits",
            id,
            reason,
            key,
        )
        .await
    }

    /// Voids a bill payment (`POST /bill-payments/{id}/void`): the engine
    /// reverses it and takes its applications off the bills and credits it
    /// paid. A reason is required.
    ///
    /// # Errors
    /// `Local(MissingReason)` for a blank reason (nothing sent);
    /// `Validation` when it is in a finished reconciliation or already
    /// void; any [`ApiError`] as for [`ApVoids::bill`].
    pub async fn bill_payment(
        &self,
        id: TransactionId,
        reason: &str,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        void(
            self.0,
            &BILL_PAYMENT_VOID,
            "/api/v1/bill-payments",
            id,
            reason,
            key,
        )
        .await
    }
}

async fn void(
    session: &Session,
    op: &'static Operation,
    base: &str,
    id: TransactionId,
    reason: &str,
    key: IdempotencyKey,
) -> Result<(), ApiError> {
    void_with(
        session,
        op,
        base,
        id,
        reason,
        &ApWriteOptions::default(),
        key,
    )
    .await
}

/// A void through a document's own route with the A/P options (P17 WP2):
/// the closing-date password needs `ap-closing-v1` (refused locally
/// without it), the stale guard goes out with `ap-edit-v1` only. A lost
/// reply is reconciled by reading the transaction back (D2).
pub(crate) async fn void_with(
    session: &Session,
    op: &'static Operation,
    base: &str,
    id: TransactionId,
    reason: &str,
    opts: &ApWriteOptions,
    key: IdempotencyKey,
) -> Result<(), ApiError> {
    // One company for the stamp read, the probe and the write.
    let s = &session.pinned();
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(ApiError::Local(LocalValidation::MissingReason));
    }
    let body = ap_body(
        s,
        json!({ "reason": reason }),
        &opts.posting,
        opts.expected_updated_at,
    )?;
    // A lost reply is resolved by reading the transaction back: void means
    // the void went through.
    let matcher = OutcomeMatcher::Field {
        get: &TXN_GET,
        path: SPEC.path(&id.to_string()),
        key: SPEC.item_key,
        field: "status",
        value: Value::String("void".into()),
    };
    let probe = Probe::new(s, matcher, true).await;
    let mut call = Call::new(Method::Post, op.route(), format!("{base}/{id}/void")).json_body(body);
    call.idempotency_key = Some(key);
    s.send(op, call, Some(&probe)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const ID: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

    fn id() -> TransactionId {
        TransactionId::parse(ID).expect("id")
    }

    #[tokio::test]
    async fn each_kind_goes_to_its_own_route_never_the_generic_one() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/bills/{ID}/void")))
            .respond_with(json(200, r#"{"bill":{"id":"x"}}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/vendor-credits/{ID}/void")))
            .respond_with(json(200, r#"{"voided":true}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/bill-payments/{ID}/void")))
            .respond_with(json(200, r#"{"voided":true}"#))
            .mount(&server)
            .await;
        let s = company_session(&server);
        s.ap_voids()
            .bill(id(), " paid twice ", IdempotencyKey::new())
            .await
            .expect("bill");
        s.ap_voids()
            .vendor_credit(id(), "x", IdempotencyKey::new())
            .await
            .expect("credit");
        s.ap_voids()
            .bill_payment(id(), "x", IdempotencyKey::new())
            .await
            .expect("payment");
        let posts = requests(&server, "POST").await;
        let paths: Vec<String> = posts.iter().map(|r| r.url.path().to_string()).collect();
        assert_eq!(
            paths,
            vec![
                format!("/api/v1/bills/{ID}/void"),
                format!("/api/v1/vendor-credits/{ID}/void"),
                format!("/api/v1/bill-payments/{ID}/void"),
            ]
        );
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(
            body,
            json!({ "reason": "paid twice" }),
            "no password, trimmed"
        );
    }

    #[tokio::test]
    async fn a_blank_reason_sends_nothing_and_a_refusal_is_validation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/bills/{ID}/void")))
            .respond_with(json(
                400,
                r#"{"error":{"message":"Cannot void a bill that has payments applied. Void the payments first."}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let e = s
            .ap_voids()
            .bill(id(), "  ", IdempotencyKey::new())
            .await
            .expect_err("blank");
        assert_eq!(e, ApiError::Local(LocalValidation::MissingReason));
        assert!(requests(&server, "POST").await.is_empty());
        let e = s
            .ap_voids()
            .bill(id(), "x", IdempotencyKey::new())
            .await
            .expect_err("paid");
        assert!(matches!(e, ApiError::Validation(_)), "{e:?}");
    }
}
