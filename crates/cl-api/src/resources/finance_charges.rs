//! Finance charges (P22, engine U-141; contract per the P22 plan §2.7,
//! verified at the D8 pin bump).
//!
//! - The engine computes everything: the period, each source invoice's
//!   days and charge, the minimum, the charged-through dates and which
//!   clients are skipped. Assess sends each ticked client's charge back as
//!   `expectedCharge`, a byte copy of the preview string (409
//!   `FINANCE_CHARGE_CHANGED` when it moved).
//! - Assess is all or nothing per call and once per client per month (409
//!   `FINANCE_CHARGE_ASSESSED`; void the invoice to charge again).
//! - The settings live on `/company/settings`; a blank rate is "off"
//!   (Q-P22a).

use std::fmt;
use std::str::FromStr;

use cl_core::ids::{AccountId, ContactId, IdError, TransactionId, parse_uuid};
use cl_core::{CalendarDate, Money};
use rust_decimal::Decimal;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::attachments::uuid_id;
use crate::resources::customer_payments::{
    cents_wire, de_cents, de_cents_opt, de_stored, error_code, null_false, null_vec, require,
    ser_cents,
};
use crate::resources::operations::{
    FINANCE_CHARGE_ASSESS, FINANCE_CHARGE_PREVIEW, FINANCE_CHARGES_LIST,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::session::{Session, decode, json_body};
use crate::wire::Capabilities;

/// Clients one assess call may take.
pub const MAX_ASSESS_CONTACTS: usize = 500;
/// The longest grace period (days).
pub const GRACE_DAYS_MAX: u16 = 365;

uuid_id!(
    /// One finance-charge run (the idempotency entity, U-141).
    FinanceChargeRunId
);

uuid_id!(
    /// One client's finance-charge assessment for a month.
    FinanceChargeAssessmentId
);

/// The engine's refusals of an assessment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FinanceChargeRefusal {
    /// No rate or income account set (400 `FINANCE_CHARGE_NOT_SET_UP`).
    NotSetUp,
    /// A client was already charged for the month (409
    /// `FINANCE_CHARGE_ASSESSED`).
    Assessed,
    /// A client's charge changed since the preview (409
    /// `FINANCE_CHARGE_CHANGED`): preview again.
    Changed,
}

const FC_REFUSALS: &[(&str, FinanceChargeRefusal)] = &[
    ("FINANCE_CHARGE_NOT_SET_UP", FinanceChargeRefusal::NotSetUp),
    ("FINANCE_CHARGE_ASSESSED", FinanceChargeRefusal::Assessed),
    ("FINANCE_CHARGE_CHANGED", FinanceChargeRefusal::Changed),
];

/// Every engine code [`finance_charge_refusal`] knows.
#[must_use]
pub fn finance_charge_refusal_codes() -> Vec<&'static str> {
    FC_REFUSALS.iter().map(|(c, _)| *c).collect()
}

/// Classifies `e` (`None` for anything else).
#[must_use]
pub fn finance_charge_refusal(e: &ApiError) -> Option<FinanceChargeRefusal> {
    let code = error_code(e)?;
    FC_REFUSALS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, r)| *r)
}

// ---- The annual rate --------------------------------------------------------------

/// An annual finance-charge rate in percent (`18` = 18 % a year): above 0,
/// at most 100, at most 4 decimals. Exact; never a float. Not money.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnnualRate(Decimal);

impl AnnualRate {
    /// Strict parse of the owner's or the engine's text (`18`, `18.00`,
    /// `1.5`).
    ///
    /// # Errors
    /// `InvalidValue` for anything else.
    pub fn parse(s: &str) -> Result<AnnualRate, LocalValidation> {
        let t = s.trim().trim_end_matches('%').trim();
        let (int, frac) = t.split_once('.').unwrap_or((t, ""));
        let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if int.is_empty()
            || int.len() > 3
            || !digits(int)
            || !digits(frac)
            || frac.len() > 4
            || (t.contains('.') && frac.is_empty())
        {
            return Err(LocalValidation::InvalidValue);
        }
        let d = Decimal::from_str(t).map_err(|_| LocalValidation::InvalidValue)?;
        if d <= Decimal::ZERO || d > Decimal::ONE_HUNDRED {
            return Err(LocalValidation::InvalidValue);
        }
        Ok(AnnualRate(d.normalize()))
    }

    /// The wire text, trailing zeros removed (`18`, `1.5`).
    #[must_use]
    pub fn to_wire(self) -> String {
        self.0.normalize().to_string()
    }

    /// The exact value.
    #[must_use]
    pub fn as_decimal(self) -> Decimal {
        self.0
    }
}

impl fmt::Debug for AnnualRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AnnualRate({})", self.to_wire())
    }
}

impl Serialize for AnnualRate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for AnnualRate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        AnnualRate::parse(&s).map_err(|_| serde::de::Error::custom("rate"))
    }
}

// ---- Settings ---------------------------------------------------------------------

/// The finance-charge settings Preferences › Sales and the Assess screen
/// write (`PUT /company/settings`): each `Some` is sent, `Some(None)`
/// clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FinanceChargeSettingsUpdate {
    /// `financeChargeAnnualRate` (`Some(None)`: off).
    pub annual_rate: Option<Option<AnnualRate>>,
    /// `financeChargeMinimum` (whole cents, not negative).
    pub minimum: Option<Money>,
    /// `financeChargeGraceDays` (0-365).
    pub grace_days: Option<u16>,
    /// `financeChargeOnFinanceCharges`.
    pub on_finance_charges: Option<bool>,
    /// `financeChargeIncomeAccountId`.
    pub income_account_id: Option<Option<AccountId>>,
    /// `financeChargePrintLater`.
    pub print_later: Option<bool>,
}

impl FinanceChargeSettingsUpdate {
    /// True when nothing would be sent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == FinanceChargeSettingsUpdate::default()
    }

    /// The body.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `finance-charges-v1`;
    /// `Local(InvalidValue)` for a negative minimum, a fraction of a cent
    /// or grace days over 365.
    pub fn body(&self, caps: &Capabilities) -> Result<Value, ApiError> {
        if !caps.has(Capabilities::FINANCE_CHARGES) {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let mut m = serde_json::Map::new();
        if let Some(r) = self.annual_rate {
            m.insert(
                "financeChargeAnnualRate".into(),
                r.map_or(Value::Null, |r| Value::String(r.to_wire())),
            );
        }
        if let Some(v) = self.minimum {
            if v.is_negative() {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
            m.insert("financeChargeMinimum".into(), Value::String(cents_wire(v)?));
        }
        if let Some(d) = self.grace_days {
            if d > GRACE_DAYS_MAX {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
            m.insert("financeChargeGraceDays".into(), json!(d));
        }
        if let Some(b) = self.on_finance_charges {
            m.insert("financeChargeOnFinanceCharges".into(), json!(b));
        }
        if let Some(a) = self.income_account_id {
            m.insert(
                "financeChargeIncomeAccountId".into(),
                a.map_or(Value::Null, |a| Value::String(a.to_string())),
            );
        }
        if let Some(b) = self.print_later {
            m.insert("financeChargePrintLater".into(), json!(b));
        }
        Ok(Value::Object(m))
    }
}

// ---- Preview ----------------------------------------------------------------------

/// Why a client gets no charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinanceChargeSkip {
    /// The client's A/R is 0.00 or less as of the date.
    InCredit,
    /// The client is inactive.
    Inactive,
    /// Anything a newer engine adds.
    #[serde(other)]
    Other,
}

/// One source invoice of a client's charge.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinanceChargeSource {
    /// The overdue invoice.
    pub invoice_id: TransactionId,
    /// Its number (owner data).
    #[serde(default)]
    pub number: Option<String>,
    /// When it was due (its date when it had none).
    pub due_date: CalendarDate,
    /// Its open balance as of the date.
    #[serde(deserialize_with = "de_cents")]
    pub open: Money,
    /// Charged from (the due date or the last charged-through date).
    pub from_date: CalendarDate,
    /// Days charged.
    pub days: u32,
    /// The charge on it.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
}

impl fmt::Debug for FinanceChargeSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinanceChargeSource")
            .field("invoice_id", &self.invoice_id)
            .field("due_date", &self.due_date)
            .field("from_date", &self.from_date)
            .field("days", &self.days)
            .finish_non_exhaustive()
    }
}

/// The finance-charge invoice that already charged a client this month.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlreadyAssessed {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// Its number.
    #[serde(default)]
    pub number: Option<String>,
}

impl fmt::Debug for AlreadyAssessed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlreadyAssessed")
            .field("invoice_id", &self.invoice_id)
            .finish_non_exhaustive()
    }
}

/// One client row of a finance-charge preview.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinanceChargeRow {
    /// The client.
    pub contact_id: ContactId,
    /// Name (owner data).
    #[serde(default)]
    pub name: String,
    /// Overdue balance as of the date.
    #[serde(deserialize_with = "de_cents")]
    pub overdue_balance: Money,
    /// The charge (0.00 when skipped or nothing to charge).
    #[serde(deserialize_with = "de_cents")]
    pub charge: Money,
    /// The source invoices.
    #[serde(default, deserialize_with = "null_vec")]
    pub sources: Vec<FinanceChargeSource>,
    /// Already charged this month.
    #[serde(default)]
    pub already_assessed: Option<AlreadyAssessed>,
    /// Skipped.
    #[serde(default)]
    pub skipped_reason: Option<FinanceChargeSkip>,
    /// The client's open credits (shown unticked with a hint, Q-P22s).
    #[serde(default, deserialize_with = "de_cents_opt")]
    pub has_open_credits: Option<Money>,
}

impl fmt::Debug for FinanceChargeRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinanceChargeRow")
            .field("contact_id", &self.contact_id)
            .field("sources", &self.sources.len())
            .field("already_assessed", &self.already_assessed)
            .field("skipped_reason", &self.skipped_reason)
            .field("has_open_credits", &self.has_open_credits.is_some())
            .finish_non_exhaustive()
    }
}

impl FinanceChargeRow {
    /// True when the row can be ticked: a charge above zero, not already
    /// assessed, not skipped.
    #[must_use]
    pub fn chargeable(&self) -> bool {
        self.charge.is_positive()
            && self.already_assessed.is_none()
            && self.skipped_reason.is_none()
    }
}

/// The preview's totals.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct FinanceChargeTotals {
    /// Clients charged.
    pub count: u32,
    /// Σ charges.
    #[serde(deserialize_with = "de_cents")]
    pub charge: Money,
}

impl fmt::Debug for FinanceChargeTotals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinanceChargeTotals")
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

/// A setup problem of the preview.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FinanceChargeProblem {
    /// `RATE_NOT_SET` or `INCOME_ACCOUNT_NOT_SET`.
    pub code: String,
}

/// The settings the preview used.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinanceChargeSettings {
    /// Annual percent (`None`: off).
    #[serde(default)]
    pub annual_rate: Option<AnnualRate>,
    /// Minimum charge.
    #[serde(default, deserialize_with = "de_cents_opt")]
    pub minimum: Option<Money>,
    /// Grace days.
    #[serde(default)]
    pub grace_days: Option<u16>,
    /// Charge on earlier finance charges.
    #[serde(default, deserialize_with = "null_false")]
    pub on_finance_charges: bool,
    /// The income account.
    #[serde(default)]
    pub income_account_id: Option<AccountId>,
}

impl fmt::Debug for FinanceChargeSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinanceChargeSettings")
            .field("annual_rate", &self.annual_rate)
            .field("grace_days", &self.grace_days)
            .field("on_finance_charges", &self.on_finance_charges)
            .field("income_account_id", &self.income_account_id)
            .finish_non_exhaustive()
    }
}

/// `POST /finance-charges/preview`.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinanceChargePreview {
    /// The month (`YYYY-MM`).
    pub period: String,
    /// The settings used.
    #[serde(default)]
    pub settings: Option<FinanceChargeSettings>,
    /// One per client.
    #[serde(default, deserialize_with = "null_vec")]
    pub rows: Vec<FinanceChargeRow>,
    /// Totals.
    pub totals: FinanceChargeTotals,
    /// Setup problems.
    #[serde(default, deserialize_with = "null_vec")]
    pub problems: Vec<FinanceChargeProblem>,
}

impl fmt::Debug for FinanceChargePreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinanceChargePreview")
            .field("period", &self.period)
            .field("rows", &self.rows.len())
            .field("problems", &self.problems)
            .finish_non_exhaustive()
    }
}

impl FinanceChargePreview {
    /// True when the rate or income account is missing: show the setup.
    #[must_use]
    pub fn not_set_up(&self) -> bool {
        self.problems
            .iter()
            .any(|p| p.code == "RATE_NOT_SET" || p.code == "INCOME_ACCOUNT_NOT_SET")
    }
}

// ---- Assess -----------------------------------------------------------------------

/// One client to charge, with the preview's charge copied.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessContact {
    /// The client.
    pub contact_id: ContactId,
    /// The charge the owner saw.
    #[serde(serialize_with = "ser_cents")]
    pub expected_charge: Money,
}

impl fmt::Debug for AssessContact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssessContact")
            .field("contact_id", &self.contact_id)
            .finish_non_exhaustive()
    }
}

/// `POST /finance-charges/assess`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessRequest {
    /// The assessment date (its month is the period).
    pub assessment_date: CalendarDate,
    /// The ticked clients (1-500).
    pub contacts: Vec<AssessContact>,
}

/// One finance-charge invoice an assessment posted.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessedInvoice {
    /// The client.
    pub contact_id: ContactId,
    /// The invoice.
    pub invoice_id: TransactionId,
    /// Its number.
    #[serde(default)]
    pub number: Option<String>,
    /// Its total.
    #[serde(deserialize_with = "de_cents")]
    pub total: Money,
}

impl fmt::Debug for AssessedInvoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssessedInvoice")
            .field("contact_id", &self.contact_id)
            .field("invoice_id", &self.invoice_id)
            .finish_non_exhaustive()
    }
}

/// What assess answered.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessResult {
    /// The run.
    pub run_id: FinanceChargeRunId,
    /// The month.
    pub period: String,
    /// One invoice per client.
    pub invoices: Vec<AssessedInvoice>,
}

impl fmt::Debug for AssessResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssessResult")
            .field("run_id", &self.run_id)
            .field("period", &self.period)
            .field("invoices", &self.invoices.len())
            .finish()
    }
}

/// One assessment of a period (`GET /finance-charges?period=`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    /// The assessment.
    pub assessment_id: FinanceChargeAssessmentId,
    /// Its run.
    pub run_id: FinanceChargeRunId,
    /// The client.
    pub contact_id: ContactId,
    /// The month.
    pub period: String,
    /// The assessment date.
    pub date: CalendarDate,
    /// The finance-charge invoice.
    pub invoice_id: TransactionId,
    /// Its number.
    #[serde(default)]
    pub number: Option<String>,
    /// Its total.
    #[serde(deserialize_with = "de_stored")]
    pub total: Money,
    /// The invoice was voided (the period is free again).
    #[serde(default, deserialize_with = "null_false")]
    pub voided: bool,
}

impl fmt::Debug for Assessment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Assessment")
            .field("assessment_id", &self.assessment_id)
            .field("contact_id", &self.contact_id)
            .field("period", &self.period)
            .field("voided", &self.voided)
            .finish_non_exhaustive()
    }
}

/// Checks an assess request before anything is sent.
///
/// # Errors
/// `Local(Unsupported)` without `finance-charges-v1`; `TooFewLines` /
/// `TooManyLines` outside 1-500 clients; `InvalidValue` for a charge not
/// above zero or not whole cents.
pub fn check_assess(r: &AssessRequest, caps: &Capabilities) -> Result<(), ApiError> {
    if !caps.has(Capabilities::FINANCE_CHARGES) {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    if r.contacts.is_empty() {
        return Err(ApiError::Local(LocalValidation::TooFewLines));
    }
    if r.contacts.len() > MAX_ASSESS_CONTACTS {
        return Err(ApiError::Local(LocalValidation::TooManyLines));
    }
    for c in &r.contacts {
        if !c.expected_charge.is_positive() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        cents_wire(c.expected_charge)?;
    }
    Ok(())
}

/// Finance-charge operations of a session (`session.finance_charges()`).
#[derive(Clone, Copy, Debug)]
pub struct FinanceCharges<'a>(pub(crate) &'a Session);

impl Session {
    /// Finance charges of the open company (P22).
    #[must_use]
    pub fn finance_charges(&self) -> FinanceCharges<'_> {
        FinanceCharges(self)
    }
}

impl FinanceCharges<'_> {
    /// What assessing on `date` would charge, posting nothing (`clients`:
    /// `None` for every client).
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Validation`; `Decode`; any [`ApiError`].
    pub async fn preview(
        &self,
        date: CalendarDate,
        clients: Option<&[ContactId]>,
    ) -> Result<FinanceChargePreview, ApiError> {
        require(self.0, Capabilities::FINANCE_CHARGES)?;
        let mut body = json!({"assessmentDate": date});
        if let Some(c) = clients {
            body["contactIds"] = json!(c);
        }
        let call =
            Call::at(Method::Post, FINANCE_CHARGE_PREVIEW.route()).json_body(json_body(&body)?);
        let reply = self.0.send(&FINANCE_CHARGE_PREVIEW, call, None).await?;
        decode(&reply, "finance charge preview")
    }

    /// Assesses the ticked clients, all or nothing (keyed under
    /// `idempotency-finance-charges-v1`: a replay answers the same
    /// invoices).
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)` ([`check_assess`]); from the engine
    /// see [`finance_charge_refusal`]; closing on the date; any
    /// [`ApiError`].
    pub async fn assess(
        &self,
        r: &AssessRequest,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<AssessResult, ApiError> {
        let s = &self.0.pinned();
        check_assess(r, &s.client().capabilities())?;
        let body =
            serde_json::to_value(r).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let body = posting_body(s, body, opts)?;
        let mut call = Call::at(Method::Post, FINANCE_CHARGE_ASSESS.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&FINANCE_CHARGE_ASSESS, call, None).await?;
        decode(&reply, "finance charge assessment")
            .map_err(crate::resources::versioning::saved_unreadable)
    }

    /// The assessments of `period` (`YYYY-MM`).
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Decode`; any [`ApiError`].
    pub async fn list(&self, period: &str) -> Result<Vec<Assessment>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            data: Vec<Assessment>,
        }
        require(self.0, Capabilities::FINANCE_CHARGES)?;
        let mut call = Call::at(Method::Get, FINANCE_CHARGES_LIST.route());
        call.query = vec![("period".into(), period.to_string())];
        let reply = self.0.send(&FINANCE_CHARGES_LIST, call, None).await?;
        decode::<W>(&reply, "finance charges").map(|w| w.data)
    }
}
