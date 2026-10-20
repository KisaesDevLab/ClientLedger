//! Daily Sales Summary (P14 lane B): the engine's daily-sales layouts
//! (`/daily-sales/templates`) and days (`/daily-sales/entries`, preview,
//! post and void; `closingPassword` with `daily-sales-lock-v1`).
//!
//! Amounts go out as `Money::to_wire()` strings (the engine refuses a JSON
//! number) and come back as exact `Money`; the engine computes the posting
//! and its over/short, the app only compares.

use crate::session::Session;

/// Daily-sales layouts and days of a session (`session.daily_sales()`).
#[derive(Clone, Copy, Debug)]
pub struct DailySales<'a>(
    #[allow(dead_code, reason = "P14 lane B adds the calls")] pub(crate) &'a Session,
);

impl Session {
    /// Daily-sales layouts and days of the open company.
    #[must_use]
    pub fn daily_sales(&self) -> DailySales<'_> {
        DailySales(self)
    }
}

// ---- WP-D (sample loader): seeding one layout --------------------------------
// Kept apart from lane B's typed calls: the sample loader only needs to
// create a layout from a preset and replace its lines. No amounts.

/// One line of a layout the sample loader seeds (`PUT /templates/{id}/lines`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutSeedLine {
    /// Engine section (`sales`, `tax`, `payment`, `payout`, ...).
    pub section: String,
    /// Label.
    pub label: String,
    /// The account (`None`: unmapped).
    pub account_id: Option<cl_core::ids::AccountId>,
    /// `debit` or `credit`.
    pub normal_side: &'static str,
    /// Position.
    pub sort_order: u32,
}

impl DailySales<'_> {
    /// Creates a layout from `preset` (`retail`, `restaurant`, `custom`)
    /// and replaces its lines with `lines` (the sample loader). Returns the
    /// new layout's id as the engine sent it.
    ///
    /// # Errors
    /// `Local(TooManyLines)` above 200 lines (nothing sent); any
    /// [`crate::ApiError`].
    pub async fn seed_layout(
        &self,
        name: &str,
        preset: &str,
        lines: &[LayoutSeedLine],
        key: crate::idempotency::IdempotencyKey,
    ) -> Result<String, crate::errors::ApiError> {
        use crate::client::{Call, Method};
        use crate::errors::{ApiError, LocalValidation};
        use crate::resources::operations::{DAILY_TEMPLATE_LINES, DAILY_TEMPLATES_CREATE};
        use crate::session::{decode, json_body};

        #[derive(serde::Deserialize)]
        struct Created {
            template: Id,
        }
        #[derive(serde::Deserialize)]
        struct Id {
            id: String,
        }
        if lines.len() > 200 {
            return Err(ApiError::Local(LocalValidation::TooManyLines));
        }
        let body = serde_json::json!({ "name": name, "presetType": preset });
        let mut call =
            Call::at(Method::Post, DAILY_TEMPLATES_CREATE.route()).json_body(json_body(&body)?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&DAILY_TEMPLATES_CREATE, call, None).await?;
        let id = decode::<Created>(&reply, "daily-sales layout")?.template.id;
        if uuid::Uuid::parse_str(&id).is_err() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let body = serde_json::json!({ "lines": lines });
        let mut call = Call::new(
            Method::Put,
            DAILY_TEMPLATE_LINES.route(),
            format!("/api/v1/daily-sales/templates/{id}/lines"),
        )
        .json_body(json_body(&body)?);
        call.idempotency_key = Some(crate::idempotency::IdempotencyKey::new());
        self.0.send(&DAILY_TEMPLATE_LINES, call, None).await?;
        Ok(id)
    }
}

#[cfg(test)]
mod seed_tests {
    use super::LayoutSeedLine;

    /// The seed body names the engine's keys and carries no amounts.
    #[test]
    fn seed_line_body_uses_engine_keys() {
        let l = LayoutSeedLine {
            section: "tax".into(),
            label: "Sales Tax Collected".into(),
            account_id: None,
            normal_side: "credit",
            sort_order: 1,
        };
        let v = serde_json::to_value(&l).expect("json");
        assert_eq!(
            v,
            serde_json::json!({
                "section": "tax",
                "label": "Sales Tax Collected",
                "accountId": null,
                "normalSide": "credit",
                "sortOrder": 1
            })
        );
    }
}
