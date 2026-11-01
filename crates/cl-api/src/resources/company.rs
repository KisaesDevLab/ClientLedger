//! The open company's profile and settings (P05 plan §2.8).
//!
//! - `GET /api/v1/company` and `PUT /api/v1/company` carry the profile
//!   (name, address, contact details, entity type). The engine's
//!   `updateCompanySchema` has no `lockDate`, and [`CompanyProfileUpdate`]
//!   has no such field either (NN8; Q-P05g).
//! - `GET /api/v1/company/settings` is read-only in P05. A typed settings
//!   writer comes with P08/P09 and must omit `lockDate` until U-15 (§2.7):
//!   [`CompanySettings`] deliberately does not implement `Serialize`.
//! - Company list, create, open and close live on [`Session`] because they
//!   change the session's scope.

use cl_core::ids::{AccountId, CompanyId, UserId};
use cl_core::{CalendarDate, Rate, Secret, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::errors::LocalValidation;
use crate::idempotency::IdempotencyKey;
use crate::resources::common::requested_fp;
use crate::resources::operations::{
    CLOSING_PASSWORD_GET, CLOSING_PASSWORD_PUT, CLOSING_PASSWORD_RESET, COMPANY_GET, COMPANY_PUT,
    COMPANY_SETTINGS, COMPANY_SETTINGS_PUT, EQUITY_ROLES,
};
use crate::resources::reconcile::row_fp;
use crate::resources::versioning::{
    EditableFields, GuardedUpdate, RecordVersion, Versioned, get_versioned, pick, update_guarded,
};
use crate::session::{Scope, Session, decode, json_body};
use crate::wire::Capabilities;

/// Legal form of the business (`entityType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    /// Sole proprietorship.
    SoleProp,
    /// Single-member LLC.
    SingleMemberLlc,
    /// S corporation.
    SCorp,
    /// C corporation.
    CCorp,
    /// Partnership.
    Partnership,
}

impl EntityType {
    /// The engine's wire name.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            EntityType::SoleProp => "sole_prop",
            EntityType::SingleMemberLlc => "single_member_llc",
            EntityType::SCorp => "s_corp",
            EntityType::CCorp => "c_corp",
            EntityType::Partnership => "partnership",
        }
    }

    /// From the engine's wire name; `None` for a type this build does not
    /// know (the owner then picks the equity account, Q-P14m).
    #[must_use]
    pub fn from_wire(s: &str) -> Option<EntityType> {
        [
            EntityType::SoleProp,
            EntityType::SingleMemberLlc,
            EntityType::SCorp,
            EntityType::CCorp,
            EntityType::Partnership,
        ]
        .into_iter()
        .find(|e| e.as_wire() == s)
    }
}

/// Reads an entity type leniently: an unknown or missing one is `None`
/// rather than a decode failure (P14 C1.1; a newer engine may add types).
fn lenient_entity<'de, D>(d: D) -> Result<Option<EntityType>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<Value> = Option::deserialize(d)?;
    Ok(raw
        .as_ref()
        .and_then(Value::as_str)
        .and_then(EntityType::from_wire))
}

/// An equity role the engine tags an account with (`accounts.system_tag`,
/// U-56).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EquityRole {
    /// Owner's draw (sole proprietor, single-member LLC).
    OwnerDraw,
    /// Owner's contribution (sole proprietor, single-member LLC).
    OwnerContribution,
    /// Partner draws.
    PartnerDraws,
    /// Partner contributions.
    PartnerContributions,
    /// Shareholder distributions (S corporation; "Dividends Paid" for a C
    /// corporation).
    ShareholderDistributions,
    /// Paid-in capital (corporations).
    PaidInCapital,
    /// Retained earnings.
    RetainedEarnings,
}

impl EquityRole {
    /// Every role, in the engine's order.
    pub const ALL: [EquityRole; 7] = [
        EquityRole::OwnerDraw,
        EquityRole::OwnerContribution,
        EquityRole::PartnerDraws,
        EquityRole::PartnerContributions,
        EquityRole::ShareholderDistributions,
        EquityRole::PaidInCapital,
        EquityRole::RetainedEarnings,
    ];

    /// The engine's key.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            EquityRole::OwnerDraw => "owner_draw",
            EquityRole::OwnerContribution => "owner_contribution",
            EquityRole::PartnerDraws => "partner_draws",
            EquityRole::PartnerContributions => "partner_contributions",
            EquityRole::ShareholderDistributions => "shareholder_distributions",
            EquityRole::PaidInCapital => "paid_in_capital",
            EquityRole::RetainedEarnings => "retained_earnings",
        }
    }
}

/// `GET /api/v1/company/equity-roles` (`equity-roles-v1`): the company's
/// entity type and the account the engine keeps for each equity role.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EquityRoles {
    /// The legal form (`None`: not set, or a type this build doesn't know).
    pub entity_type: Option<EntityType>,
    /// The account for each role the engine has one for.
    pub roles: std::collections::BTreeMap<EquityRole, AccountId>,
}

impl EquityRoles {
    /// The account the engine keeps for `role`.
    #[must_use]
    pub fn account(&self, role: EquityRole) -> Option<AccountId> {
        self.roles.get(&role).copied()
    }

    /// Reads the engine's answer. Unknown role keys are ignored; a role
    /// whose value is not an account id or `null` is a decode failure.
    ///
    /// # Errors
    /// `Decode` for a malformed body.
    pub fn from_wire(v: &Value) -> Result<EquityRoles, ApiError> {
        let bad = || ApiError::Decode {
            what: "equity roles",
        };
        let entity_type = match v.get("entityType") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => EntityType::from_wire(s),
            Some(_) => return Err(bad()),
        };
        let map = v.get("roles").and_then(Value::as_object).ok_or_else(bad)?;
        let mut roles = std::collections::BTreeMap::new();
        for role in EquityRole::ALL {
            match map.get(role.as_wire()) {
                None | Some(Value::Null) => {}
                Some(Value::String(s)) => {
                    roles.insert(role, AccountId::parse(s).map_err(|_| bad())?);
                }
                Some(_) => return Err(bad()),
            }
        }
        Ok(EquityRoles { entity_type, roles })
    }
}

/// The profile fields the company form edits. Sent whole on save: `None`
/// clears a nullable field. `country` and `entity_type` cannot be cleared
/// (the engine refuses `null` there), so `None` leaves them unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanyProfileUpdate {
    /// Business name (required, 1-255 characters).
    pub business_name: String,
    /// Legal name.
    #[serde(default)]
    pub legal_name: Option<String>,
    /// Employer identification number.
    #[serde(default)]
    pub ein: Option<String>,
    /// Address line 1.
    #[serde(default)]
    pub address_line1: Option<String>,
    /// Address line 2.
    #[serde(default)]
    pub address_line2: Option<String>,
    /// City.
    #[serde(default)]
    pub city: Option<String>,
    /// State.
    #[serde(default)]
    pub state: Option<String>,
    /// ZIP code.
    #[serde(default)]
    pub zip: Option<String>,
    /// Two- or three-letter country code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Phone.
    #[serde(default)]
    pub phone: Option<String>,
    /// Email.
    #[serde(default)]
    pub email: Option<String>,
    /// Website.
    #[serde(default)]
    pub website: Option<String>,
    /// Industry.
    #[serde(default)]
    pub industry: Option<String>,
    /// Legal form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<EntityType>,
}

/// `GET /api/v1/company` (fields the app uses; others, including the
/// encrypted SMTP password column, are never read).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanyProfile {
    /// The company row.
    pub id: CompanyId,
    /// The editable profile.
    #[serde(flatten)]
    pub profile: CompanyProfileUpdate,
    /// Whether first-run setup finished.
    #[serde(default)]
    pub setup_complete: Option<bool>,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// Last changed.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

impl CompanyProfile {
    /// Wire names of the fingerprinted fields.
    pub const EDITABLE_FIELDS: &'static [&'static str] = &[
        "businessName",
        "legalName",
        "ein",
        "addressLine1",
        "addressLine2",
        "city",
        "state",
        "zip",
        "country",
        "phone",
        "email",
        "website",
        "industry",
        "entityType",
    ];
}

impl EditableFields for CompanyProfile {
    const EDITABLE: &'static [&'static str] = CompanyProfile::EDITABLE_FIELDS;

    fn editable(&self) -> Value {
        let v = serde_json::to_value(&self.profile).unwrap_or(Value::Null);
        pick(&v, Self::EDITABLE)
    }
}

/// `GET /api/v1/company/settings` (read-only; no `Serialize`, so it can
/// never be sent back with a `lockDate`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanySettings {
    /// First month of the fiscal year (1-12).
    #[serde(default)]
    pub fiscal_year_start_month: Option<u8>,
    /// `cash` or `accrual`.
    #[serde(default)]
    pub accounting_method: Option<String>,
    /// Default payment terms key.
    #[serde(default)]
    pub default_payment_terms: Option<String>,
    /// Invoice number prefix.
    #[serde(default)]
    pub invoice_prefix: Option<String>,
    /// Next invoice number.
    #[serde(default)]
    pub invoice_next_number: Option<u64>,
    /// Default sales tax rate (exact, `decimal(5,4)`).
    #[serde(default)]
    pub default_sales_tax_rate: Option<Rate>,
    /// Currency code.
    #[serde(default)]
    pub currency: Option<String>,
    /// Date format preference.
    #[serde(default)]
    pub date_format: Option<String>,
    /// The closing date (shown only; there is no writer until U-15).
    #[serde(default)]
    pub lock_date: Option<CalendarDate>,
    /// The legal form when the settings carry it (P14 C1.1). Read
    /// leniently: an unknown type is `None`.
    #[serde(default, deserialize_with = "lenient_entity")]
    pub entity_type: Option<EntityType>,
    /// Where Pay Bills posts early-payment discounts by default
    /// (`billDiscountAccountId`, `bill-discounts-v1`, Q-P18a).
    #[serde(default)]
    pub bill_discount_account_id: Option<AccountId>,
    /// Where markups on billable costs post (`billableMarkupAccountId`,
    /// `billable-costs-v1`, Q-P20b; an income account).
    #[serde(default)]
    pub billable_markup_account_id: Option<AccountId>,
    /// Where early-payment discounts on customer payments post
    /// (`salesDiscountAccountId`, U-134, Q-P21q).
    #[serde(default)]
    pub sales_discount_account_id: Option<AccountId>,
    /// Where underpayments written off post (`writeOffAccountId`, U-133).
    #[serde(default)]
    pub write_off_account_id: Option<AccountId>,
    /// Shortfalls up to this suggest the write-off (`writeOffThreshold`,
    /// U-133, Q-P21h; `None`: never suggested).
    #[serde(default, deserialize_with = "money_text_opt")]
    pub write_off_threshold: Option<cl_core::Money>,
    /// The expense account of a bounced check's bank fee
    /// (`nsfBankFeeAccountId`, U-132).
    #[serde(default)]
    pub nsf_bank_fee_account_id: Option<AccountId>,
    /// The income account of the bounced-check fee invoice
    /// (`nsfFeeIncomeAccountId`, Q-P21p).
    #[serde(default)]
    pub nsf_fee_income_account_id: Option<AccountId>,
    /// The client's bounced-check fee, blank until set
    /// (`nsfCustomerFee`, Q-P21d).
    #[serde(default, deserialize_with = "money_text_opt")]
    pub nsf_customer_fee: Option<cl_core::Money>,
    /// Annual finance-charge percent (`financeChargeAnnualRate`, U-141;
    /// `None`: finance charges are off, Q-P22a).
    #[serde(default)]
    pub finance_charge_annual_rate: Option<crate::resources::finance_charges::AnnualRate>,
    /// The smallest charge per client (`financeChargeMinimum`).
    #[serde(default, deserialize_with = "money_text_opt")]
    pub finance_charge_minimum: Option<cl_core::Money>,
    /// Days after the due date before charging (`financeChargeGraceDays`).
    #[serde(default)]
    pub finance_charge_grace_days: Option<u16>,
    /// Charge on earlier finance charges (`financeChargeOnFinanceCharges`).
    #[serde(default)]
    pub finance_charge_on_finance_charges: Option<bool>,
    /// The finance-charge income account (`financeChargeIncomeAccountId`).
    #[serde(default)]
    pub finance_charge_income_account_id: Option<AccountId>,
    /// Finance-charge invoices wait to be printed
    /// (`financeChargePrintLater`, default on).
    #[serde(default)]
    pub finance_charge_print_later: Option<bool>,
}

/// An optional money string (a JSON number is refused).
fn money_text_opt<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<cl_core::Money>, D::Error> {
    match Option::<String>::deserialize(d)? {
        Some(s) => cl_core::Money::parse_wire(&s)
            .map(Some)
            .map_err(|_| serde::de::Error::custom("money")),
        None => Ok(None),
    }
}

/// The receivables settings Preferences › Sales writes (P21, Q-P21q): each
/// `Some` field is sent, `Some(None)` clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReceivablesSettingsUpdate {
    /// `salesDiscountAccountId` (needs `payment-discounts-v1`).
    pub sales_discount_account_id: Option<Option<AccountId>>,
    /// `writeOffAccountId` (needs `payment-settle-v1`).
    pub write_off_account_id: Option<Option<AccountId>>,
    /// `writeOffThreshold` (needs `payment-settle-v1`).
    pub write_off_threshold: Option<Option<cl_core::Money>>,
    /// `nsfBankFeeAccountId` (needs `payment-bounce-v1`).
    pub nsf_bank_fee_account_id: Option<Option<AccountId>>,
    /// `nsfFeeIncomeAccountId` (needs `payment-bounce-v1`).
    pub nsf_fee_income_account_id: Option<Option<AccountId>>,
    /// `nsfCustomerFee` (needs `payment-bounce-v1`).
    pub nsf_customer_fee: Option<Option<cl_core::Money>>,
}

impl ReceivablesSettingsUpdate {
    /// True when nothing would be sent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == ReceivablesSettingsUpdate::default()
    }

    /// The body, or `Local(Unsupported)` for a field the engine does not
    /// advertise (an older engine would drop it and answer success), or
    /// `Local(InvalidValue)` for a negative amount or a fraction of a cent.
    ///
    /// # Errors
    /// As above.
    pub fn body(&self, caps: &Capabilities) -> Result<Value, ApiError> {
        let mut m = serde_json::Map::new();
        let need = |on: bool, cap: &str| -> Result<(), ApiError> {
            if on && !caps.has(cap) {
                return Err(ApiError::Local(LocalValidation::Unsupported));
            }
            Ok(())
        };
        need(
            self.sales_discount_account_id.is_some(),
            Capabilities::PAYMENT_DISCOUNTS,
        )?;
        need(
            self.write_off_account_id.is_some() || self.write_off_threshold.is_some(),
            Capabilities::PAYMENT_SETTLE,
        )?;
        need(
            self.nsf_bank_fee_account_id.is_some()
                || self.nsf_fee_income_account_id.is_some()
                || self.nsf_customer_fee.is_some(),
            Capabilities::PAYMENT_BOUNCE,
        )?;
        let account =
            |a: Option<AccountId>| a.map_or(Value::Null, |a| Value::String(a.to_string()));
        let money = |v: Option<cl_core::Money>| -> Result<Value, ApiError> {
            match v {
                None => Ok(Value::Null),
                Some(v) if v.is_negative() => Err(ApiError::Local(LocalValidation::InvalidValue)),
                Some(v) => crate::resources::customer_payments::cents_wire(v).map(Value::String),
            }
        };
        if let Some(a) = self.sales_discount_account_id {
            m.insert("salesDiscountAccountId".into(), account(a));
        }
        if let Some(a) = self.write_off_account_id {
            m.insert("writeOffAccountId".into(), account(a));
        }
        if let Some(v) = self.write_off_threshold {
            m.insert("writeOffThreshold".into(), money(v)?);
        }
        if let Some(a) = self.nsf_bank_fee_account_id {
            m.insert("nsfBankFeeAccountId".into(), account(a));
        }
        if let Some(a) = self.nsf_fee_income_account_id {
            m.insert("nsfFeeIncomeAccountId".into(), account(a));
        }
        if let Some(v) = self.nsf_customer_fee {
            m.insert("nsfCustomerFee".into(), money(v)?);
        }
        Ok(Value::Object(m))
    }
}

/// Accounting basis of the company (`accountingMethod`, Q-44).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingMethod {
    /// Accrual (the default).
    Accrual,
    /// Cash.
    Cash,
}

/// The settings the setup interview and Preferences write
/// (`PUT /api/v1/company/settings`). Every field is optional and only
/// present fields are sent. The closing date is deliberately not here:
/// [`ClosingDateChange`] is the only writer and it is capability-gated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanySettingsUpdate {
    /// First month of the fiscal year (1-12).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fiscal_year_start_month: Option<u8>,
    /// Basis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounting_method: Option<AccountingMethod>,
    /// Default payment terms key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_payment_terms: Option<String>,
    /// Invoice prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invoice_prefix: Option<String>,
    /// Next invoice number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invoice_next_number: Option<u64>,
    /// Default sales tax rate (exact).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_sales_tax_rate: Option<Rate>,
    /// Currency code (USD, Q-41).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Date format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_format: Option<String>,
}

/// A closing-date change (Q-48, Q-P02d). Sent only when the engine
/// advertises `closing-password-v1` (U-15), because before that the engine
/// accepts the lock date without any password.
#[derive(Clone)]
pub struct ClosingDateChange {
    /// The new closing date, or `None` to clear it.
    pub closing_date: Option<CalendarDate>,
    /// The closing-date password (set or confirm). Never logged.
    pub closing_password: Option<Secret>,
}

impl std::fmt::Debug for ClosingDateChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosingDateChange")
            .field("closing_date", &self.closing_date)
            .finish_non_exhaustive()
    }
}

impl Session {
    fn require_company(&self) -> Result<(), ApiError> {
        match self.scope() {
            Scope::Company { .. } => Ok(()),
            Scope::Workspace => Err(ApiError::NoCompanyOpen),
        }
    }

    /// The open company's profile with its version.
    ///
    /// # Errors
    /// `NoCompanyOpen` in the workspace (nothing sent); any [`ApiError`].
    pub async fn company_profile(&self) -> Result<Versioned<CompanyProfile>, ApiError> {
        self.require_company()?;
        get_versioned(
            self,
            &COMPANY_GET,
            COMPANY_GET.route().to_string(),
            "company",
        )
        .await
    }

    /// Saves the profile if it is still at `expected` (§2.5).
    ///
    /// # Errors
    /// `Conflict { Stale }` when it changed since it was read (nothing
    /// sent); `NoCompanyOpen`; any [`ApiError`].
    pub async fn update_company_profile(
        &self,
        expected: &RecordVersion,
        update: &CompanyProfileUpdate,
        key: IdempotencyKey,
    ) -> Result<Versioned<CompanyProfile>, ApiError> {
        self.require_company()?;
        let body = json_body(update)?;
        let u = GuardedUpdate {
            get: &COMPANY_GET,
            put: &COMPANY_PUT,
            path: COMPANY_PUT.route().to_string(),
            key: "company",
            requested: requested_fp::<CompanyProfile>(&body),
            body,
            idempotency_key: key,
            fp: row_fp::<CompanyProfile>,
            stamp_covered: false,
        };
        update_guarded(self, expected, u).await
    }

    /// Writes company settings (never the closing date).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a month outside 1-12 (nothing sent);
    /// `NoCompanyOpen`; `Validation`; any [`ApiError`].
    pub async fn update_company_settings(
        &self,
        update: &CompanySettingsUpdate,
        key: IdempotencyKey,
    ) -> Result<CompanySettings, ApiError> {
        self.require_company()?;
        if update
            .fiscal_year_start_month
            .is_some_and(|m| !(1..=12).contains(&m))
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call =
            Call::at(Method::Put, COMPANY_SETTINGS_PUT.route()).json_body(json_body(update)?);
        call.idempotency_key = Some(key);
        let reply = self.send(&COMPANY_SETTINGS_PUT, call, None).await?;
        settings_from(&reply)
    }

    /// Sets or clears the closing date with its password (U-15). Refused
    /// locally until the engine advertises `closing-password-v1`: until
    /// then nothing in the typed API writes `lockDate` (NN8).
    ///
    /// # Errors
    /// `Local(ClosingDateWriteUnavailable)` without the capability (nothing
    /// sent); `Unauthorized(InvalidCredentials)` for a wrong password;
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn set_closing_date(
        &self,
        change: &ClosingDateChange,
        key: IdempotencyKey,
    ) -> Result<CompanySettings, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            lock_date: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            closing_password: Option<&'a str>,
        }
        self.require_company()?;
        if !self
            .client()
            .capabilities()
            .has(Capabilities::CLOSING_PASSWORD)
        {
            return Err(ApiError::Local(
                LocalValidation::ClosingDateWriteUnavailable,
            ));
        }
        let body = json_body(&Body {
            lock_date: change.closing_date.map(CalendarDate::to_wire),
            closing_password: change.closing_password.as_ref().map(Secret::expose),
        })?;
        let mut call = Call::at(Method::Put, COMPANY_SETTINGS_PUT.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.send(&COMPANY_SETTINGS_PUT, call, None).await?;
        settings_from(&reply)
    }

    /// Sets (or with `None` clears) the account Pay Bills posts early-payment
    /// discounts to by default (`billDiscountAccountId`, Q-P18a). Refused
    /// locally without `bill-discounts-v1`: an older engine would drop the
    /// field and answer success.
    ///
    /// # Errors
    /// `Local(Unsupported)` without the capability (nothing sent);
    /// `NoCompanyOpen`; `Validation` (not an account of this company); any
    /// [`ApiError`].
    pub async fn set_bill_discount_account(
        &self,
        account: Option<AccountId>,
        key: IdempotencyKey,
    ) -> Result<CompanySettings, ApiError> {
        self.require_company()?;
        if !self
            .client()
            .capabilities()
            .has(Capabilities::BILL_DISCOUNTS)
        {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let body = json_body(&serde_json::json!({
            "billDiscountAccountId": account.map(|a| a.to_string()),
        }))?;
        let mut call = Call::at(Method::Put, COMPANY_SETTINGS_PUT.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.send(&COMPANY_SETTINGS_PUT, call, None).await?;
        settings_from(&reply)
    }

    /// Sets (or with `None` clears) the income account markups on billable
    /// costs post to (`billableMarkupAccountId`, U-128). Refused locally
    /// without `billable-costs-v1`.
    ///
    /// # Errors
    /// `Local(Unsupported)` without the capability (nothing sent);
    /// `NoCompanyOpen`; `Validation` (`MARKUP_ACCOUNT_INVALID`: not an
    /// income account of this company); any [`ApiError`].
    pub async fn set_billable_markup_account(
        &self,
        account: Option<AccountId>,
        key: IdempotencyKey,
    ) -> Result<CompanySettings, ApiError> {
        self.require_company()?;
        if !self
            .client()
            .capabilities()
            .has(Capabilities::BILLABLE_COSTS)
        {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let body = json_body(&serde_json::json!({
            "billableMarkupAccountId": account.map(|a| a.to_string()),
        }))?;
        let mut call = Call::at(Method::Put, COMPANY_SETTINGS_PUT.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.send(&COMPANY_SETTINGS_PUT, call, None).await?;
        settings_from(&reply)
    }

    /// Saves the receivables settings of Preferences › Sales (P21,
    /// Q-P21q). Nothing is sent for an empty update.
    ///
    /// # Errors
    /// `Local(Unsupported)` for a field the engine does not advertise
    /// (nothing sent); `NoCompanyOpen`; `Validation` (an account the engine
    /// refuses); any [`ApiError`].
    pub async fn set_receivables_settings(
        &self,
        u: &ReceivablesSettingsUpdate,
        key: IdempotencyKey,
    ) -> Result<Option<CompanySettings>, ApiError> {
        self.require_company()?;
        if u.is_empty() {
            return Ok(None);
        }
        let body = json_body(&u.body(&self.client().capabilities())?)?;
        let mut call = Call::at(Method::Put, COMPANY_SETTINGS_PUT.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.send(&COMPANY_SETTINGS_PUT, call, None).await?;
        settings_from(&reply).map(Some)
    }

    /// Saves the finance-charge settings (P22, U-141, Q-P22a). Nothing is
    /// sent for an empty update.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `finance-charges-v1` (nothing sent);
    /// `Local(InvalidValue)`; `NoCompanyOpen`; `Validation`; any
    /// [`ApiError`].
    pub async fn set_finance_charge_settings(
        &self,
        u: &crate::resources::finance_charges::FinanceChargeSettingsUpdate,
        key: IdempotencyKey,
    ) -> Result<Option<CompanySettings>, ApiError> {
        self.require_company()?;
        if u.is_empty() {
            return Ok(None);
        }
        let body = json_body(&u.body(&self.client().capabilities())?)?;
        let mut call = Call::at(Method::Put, COMPANY_SETTINGS_PUT.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.send(&COMPANY_SETTINGS_PUT, call, None).await?;
        settings_from(&reply).map(Some)
    }

    /// The company's entity type and its equity role accounts (U-56).
    /// Refused locally until the engine advertises `equity-roles-v1`.
    ///
    /// # Errors
    /// `Local(EquityRolesUnavailable)` without the capability (nothing
    /// sent); `NoCompanyOpen`; `Decode`; any [`ApiError`].
    pub async fn equity_roles(&self) -> Result<EquityRoles, ApiError> {
        self.require_company()?;
        if !self.client().capabilities().has(Capabilities::EQUITY_ROLES) {
            return Err(ApiError::Local(LocalValidation::EquityRolesUnavailable));
        }
        let call = Call::at(Method::Get, EQUITY_ROLES.route());
        let reply = self.send(&EQUITY_ROLES, call, None).await?;
        let v: Value = decode(&reply, "equity roles")?;
        EquityRoles::from_wire(&v)
    }

    /// The open company's settings (read-only in P05).
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn company_settings(&self) -> Result<CompanySettings, ApiError> {
        #[derive(Deserialize)]
        struct Wrapper {
            settings: CompanySettings,
        }
        let call = Call::at(Method::Get, COMPANY_SETTINGS.route());
        let reply = self.send(&COMPANY_SETTINGS, call, None).await?;
        Ok(decode::<Wrapper>(&reply, "company settings")?.settings)
    }
}

/// The closing date and whether a closing-date password is set
/// (`GET /company/closing-password`, U-15). Never the password or its hash.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosingPasswordStatus {
    /// The closing date.
    #[serde(default)]
    pub closing_date: Option<CalendarDate>,
    /// Who set the closing date.
    #[serde(default)]
    pub set_by: Option<UserId>,
    /// When it was set.
    #[serde(default)]
    pub set_at: Option<Timestamp>,
    /// A closing-date password is set.
    #[serde(rename = "closingPasswordSet")]
    pub password_set: bool,
}

/// Shortest closing-date password (characters).
pub const CLOSING_PASSWORD_MIN_CHARS: usize = 4;
/// Longest closing-date password (UTF-8 bytes).
pub const CLOSING_PASSWORD_MAX_BYTES: usize = 72;

impl Session {
    fn require_closing_password(&self) -> Result<(), ApiError> {
        self.require_company()?;
        if self
            .client()
            .capabilities()
            .has(Capabilities::CLOSING_PASSWORD)
        {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::ClosingPasswordUnavailable))
        }
    }

    /// The closing date and whether a closing-date password is set.
    ///
    /// # Errors
    /// `Local(ClosingPasswordUnavailable)` without `closing-password-v1`
    /// (nothing sent); `NoCompanyOpen`; any [`ApiError`].
    pub async fn closing_password_status(&self) -> Result<ClosingPasswordStatus, ApiError> {
        self.require_closing_password()?;
        let call = Call::at(Method::Get, CLOSING_PASSWORD_GET.route());
        let reply = self.send(&CLOSING_PASSWORD_GET, call, None).await?;
        decode(&reply, "closing password status")
    }

    /// Sets, changes (`current` required once one is set) or clears (`new`
    /// = `None`) the closing-date password. Returns whether one is set now.
    /// Never retried; the passwords are never logged.
    ///
    /// # Errors
    /// `Local(ClosingPasswordUnavailable)` without `closing-password-v1`,
    /// `Local(InvalidValue)` for a new password under 4 characters or over
    /// 72 bytes (nothing sent); `ClosingPasswordRejected` (wrong current
    /// password), `ClosingPasswordOwnerRequired`,
    /// `ClosingPasswordRateLimited`; any [`ApiError`].
    pub async fn set_closing_password(
        &self,
        current: Option<&Secret>,
        new: Option<&Secret>,
        key: IdempotencyKey,
    ) -> Result<bool, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            current_password: Option<&'a str>,
            new_password: Option<&'a str>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Answer {
            closing_password_set: bool,
        }
        self.require_closing_password()?;
        if let Some(n) = new {
            let n = n.expose();
            if n.chars().count() < CLOSING_PASSWORD_MIN_CHARS
                || n.len() > CLOSING_PASSWORD_MAX_BYTES
            {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
        }
        let body = json_body(&Body {
            current_password: current.map(Secret::expose),
            new_password: new.map(Secret::expose),
        })?;
        let mut call = Call::at(Method::Put, CLOSING_PASSWORD_PUT.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.send(&CLOSING_PASSWORD_PUT, call, None).await?;
        decode::<Answer>(&reply, "closing password")
            .map(|a| a.closing_password_set)
            .map_err(crate::resources::versioning::saved_unreadable)
    }

    /// The owner's reset of a forgotten closing-date password (Q-239): with
    /// their own sign-in password they clear it (`new` = `None`) or replace
    /// it. The engine records the reset in the audit trail. Returns whether
    /// a closing-date password is set now. Never retried; neither password
    /// is ever logged.
    ///
    /// # Errors
    /// `Local(ClosingPasswordUnavailable)` without `closing-password-v1`,
    /// `Local(InvalidValue)` for an empty sign-in password or a new
    /// password under 4 characters or over 72 bytes (nothing sent);
    /// `AccountPasswordRejected` (wrong sign-in password),
    /// `ClosingPasswordOwnerRequired` (not the owner),
    /// `ClosingPasswordRateLimited`; any [`ApiError`].
    pub async fn reset_closing_password(
        &self,
        account_password: &Secret,
        new: Option<&Secret>,
        key: IdempotencyKey,
    ) -> Result<bool, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            account_password: &'a str,
            new_password: Option<&'a str>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Answer {
            closing_password_set: bool,
        }
        self.require_closing_password()?;
        if account_password.expose().is_empty() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        if let Some(n) = new {
            let n = n.expose();
            if n.chars().count() < CLOSING_PASSWORD_MIN_CHARS
                || n.len() > CLOSING_PASSWORD_MAX_BYTES
            {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
        }
        let body = json_body(&Body {
            account_password: account_password.expose(),
            new_password: new.map(Secret::expose),
        })?;
        let mut call = Call::at(Method::Post, CLOSING_PASSWORD_RESET.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.send(&CLOSING_PASSWORD_RESET, call, None).await?;
        decode::<Answer>(&reply, "closing password reset")
            .map(|a| a.closing_password_set)
            .map_err(crate::resources::versioning::saved_unreadable)
    }
}

/// Reads `{settings}` from a settings write; a 2xx that cannot be read is
/// `SavedUnreadable` (the change is recorded).
fn settings_from(reply: &crate::client::EngineReply) -> Result<CompanySettings, ApiError> {
    #[derive(Deserialize)]
    struct Wrapper {
        settings: CompanySettings,
    }
    decode::<Wrapper>(reply, "company settings")
        .map(|w| w.settings)
        .map_err(crate::resources::versioning::saved_unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer};

    const PROFILE: &str = r#"{"company":{"id":"0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
        "tenantId":"1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","businessName":"Canary Co",
        "legalName":null,"ein":null,"addressLine1":"1 Main","addressLine2":null,"city":"Town",
        "state":"WI","zip":"53000","country":"US","phone":null,"email":null,"website":null,
        "industry":null,"entityType":"single_member_llc","smtpPass":"enc:secret",
        "lockDate":"2026-01-31","setupComplete":true,
        "createdAt":"2026-10-01T10:00:00.000Z","updatedAt":"2026-10-02T10:00:00.000Z"}}"#;

    #[tokio::test]
    async fn profile_read_and_guarded_update_never_send_lock_date() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company"))
            .respond_with(json(200, PROFILE))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/company"))
            .and(header(
                "x-company-id",
                "0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
            ))
            .respond_with(json(200, &PROFILE.replace("Canary Co", "New Name")))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let v = s.company_profile().await.expect("profile");
        assert_eq!(v.record.profile.business_name, "Canary Co");
        assert_eq!(
            v.record.profile.entity_type,
            Some(EntityType::SingleMemberLlc)
        );
        assert!(matches!(v.version, RecordVersion::Fingerprint(_)));

        let mut update = v.record.profile.clone();
        update.business_name = "New Name".into();
        let saved = s
            .update_company_profile(&v.version, &update, IdempotencyKey::new())
            .await
            .expect("saved");
        assert_eq!(saved.record.profile.business_name, "New Name");
        let puts = requests(&server, "PUT").await;
        assert_eq!(puts.len(), 1);
        let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(body["businessName"], "New Name");
        assert_eq!(
            body["legalName"],
            Value::Null,
            "nullable fields are sent to clear"
        );
        assert!(body.get("lockDate").is_none());
        assert!(body.get("smtpPass").is_none());
        assert!(puts[0].headers.get("idempotency-key").is_some());
    }

    const ROLES: [(&str, &str, EntityType, [EquityRole; 2]); 5] = [
        (
            "sole-prop",
            include_str!("../../../../fixtures/engine/equity-roles-sole-prop.json"),
            EntityType::SoleProp,
            [EquityRole::OwnerDraw, EquityRole::OwnerContribution],
        ),
        (
            "single-member-llc",
            include_str!("../../../../fixtures/engine/equity-roles-single-member-llc.json"),
            EntityType::SingleMemberLlc,
            [EquityRole::OwnerDraw, EquityRole::OwnerContribution],
        ),
        (
            "partnership",
            include_str!("../../../../fixtures/engine/equity-roles-partnership.json"),
            EntityType::Partnership,
            [EquityRole::PartnerDraws, EquityRole::PartnerContributions],
        ),
        (
            "s-corp",
            include_str!("../../../../fixtures/engine/equity-roles-s-corp.json"),
            EntityType::SCorp,
            [
                EquityRole::ShareholderDistributions,
                EquityRole::PaidInCapital,
            ],
        ),
        (
            "c-corp",
            include_str!("../../../../fixtures/engine/equity-roles-c-corp.json"),
            EntityType::CCorp,
            [
                EquityRole::ShareholderDistributions,
                EquityRole::PaidInCapital,
            ],
        ),
    ];

    #[test]
    fn equity_role_goldens_read_for_all_five_types() {
        for (name, body, entity, roles) in ROLES {
            let v: Value = serde_json::from_str(body).expect(name);
            let r = EquityRoles::from_wire(&v).expect(name);
            assert_eq!(r.entity_type, Some(entity), "{name}");
            assert_eq!(r.roles.len(), 3, "{name}: two roles + retained earnings");
            for role in roles {
                assert!(r.account(role).is_some(), "{name}: {role:?}");
            }
            assert!(r.account(EquityRole::RetainedEarnings).is_some());
            assert_eq!(EntityType::from_wire(entity.as_wire()), Some(entity));
        }
    }

    #[test]
    fn equity_roles_tolerate_unknown_types_and_refuse_bad_shapes() {
        let v = serde_json::json!({"entityType": "cooperative", "roles": {
            "owner_draw": null, "future_role": "x"}});
        let r = EquityRoles::from_wire(&v).expect("lenient");
        assert_eq!(r.entity_type, None);
        assert!(r.roles.is_empty());
        for bad in [
            serde_json::json!({"entityType": "sole_prop"}),
            serde_json::json!({"entityType": 3, "roles": {}}),
            serde_json::json!({"roles": {"owner_draw": "not-a-uuid"}}),
            serde_json::json!({"roles": {"owner_draw": 12}}),
        ] {
            assert!(matches!(
                EquityRoles::from_wire(&bad),
                Err(ApiError::Decode { .. })
            ));
        }
        let st: CompanySettings =
            serde_json::from_value(serde_json::json!({"entityType": "trust"})).expect("lenient");
        assert_eq!(st.entity_type, None);
        let st: CompanySettings =
            serde_json::from_value(serde_json::json!({"entityType": "s_corp"})).expect("known");
        assert_eq!(st.entity_type, Some(EntityType::SCorp));
    }

    #[tokio::test]
    async fn equity_roles_are_gated_by_capability() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        assert_eq!(
            s.equity_roles().await.expect_err("gated"),
            ApiError::Local(LocalValidation::EquityRolesUnavailable)
        );
        assert!(server.received_requests().await.expect("r").is_empty());
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company/equity-roles"))
            .respond_with(json(200, ROLES[1].1))
            .mount(&server)
            .await;
        let s =
            crate::resources::testkit::session_with_caps(&server, &[Capabilities::EQUITY_ROLES])
                .await;
        let r = s.equity_roles().await.expect("roles");
        assert_eq!(r.entity_type, Some(EntityType::SingleMemberLlc));
        assert!(r.account(EquityRole::OwnerDraw).is_some());
        let ws = crate::session::tests::session_on(&server, Scope::Workspace);
        assert_eq!(
            ws.equity_roles().await.expect_err("ws"),
            ApiError::NoCompanyOpen
        );
    }

    #[tokio::test]
    async fn changed_profile_is_a_conflict_without_a_put() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company"))
            .respond_with(json(200, &PROFILE.replace("1 Main", "2 Main")))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let stale = RecordVersion::Fingerprint([7; 32]);
        let e = s
            .update_company_profile(
                &stale,
                &CompanyProfileUpdate::default(),
                IdempotencyKey::new(),
            )
            .await
            .expect_err("stale");
        assert!(matches!(
            e,
            ApiError::Conflict {
                reason: crate::ConflictReason::Stale,
                ..
            }
        ));
        assert_eq!(requests(&server, "PUT").await.len(), 0);
    }

    #[tokio::test]
    async fn settings_are_read_with_an_exact_rate_and_the_workspace_sends_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company/settings"))
            .respond_with(json(
                200,
                r#"{"settings":{"fiscalYearStartMonth":7,"accountingMethod":"accrual",
                    "defaultSalesTaxRate":"0.0550","invoiceNextNumber":1001,
                    "lockDate":"2026-06-30","chatSupportEnabled":false}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let st = s.company_settings().await.expect("settings");
        assert_eq!(st.fiscal_year_start_month, Some(7));
        assert_eq!(
            st.default_sales_tax_rate.map(Rate::to_wire).as_deref(),
            Some("0.0550")
        );
        assert_eq!(
            st.lock_date.map(CalendarDate::to_wire).as_deref(),
            Some("2026-06-30")
        );

        let ws = crate::session::tests::session_on(&server, Scope::Workspace);
        let before = server.received_requests().await.expect("r").len();
        assert_eq!(
            ws.company_profile().await.expect_err("ws"),
            ApiError::NoCompanyOpen
        );
        assert_eq!(
            ws.company_settings().await.expect_err("ws"),
            ApiError::NoCompanyOpen
        );
        assert_eq!(server.received_requests().await.expect("r").len(), before);
    }

    #[tokio::test]
    async fn settings_with_a_json_number_rate_are_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company/settings"))
            .respond_with(json(200, r#"{"settings":{"defaultSalesTaxRate":0.055}}"#))
            .mount(&server)
            .await;
        let e = company_session(&server)
            .company_settings()
            .await
            .expect_err("float");
        assert!(matches!(e, ApiError::Decode { .. }));
    }

    #[tokio::test]
    async fn settings_writer_sends_only_present_fields_and_never_the_lock_date() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/company/settings"))
            .respond_with(json(
                200,
                r#"{"settings":{"fiscalYearStartMonth":7,"accountingMethod":"cash","lockDate":null}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let st = s
            .update_company_settings(
                &CompanySettingsUpdate {
                    fiscal_year_start_month: Some(7),
                    accounting_method: Some(AccountingMethod::Cash),
                    ..CompanySettingsUpdate::default()
                },
                IdempotencyKey::new(),
            )
            .await
            .expect("settings");
        assert_eq!(st.fiscal_year_start_month, Some(7));
        let puts = requests(&server, "PUT").await;
        let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(body["fiscalYearStartMonth"], 7);
        assert_eq!(body["accountingMethod"], "cash");
        assert!(body.get("lockDate").is_none());
        assert!(body.get("currency").is_none());
        let e = s
            .update_company_settings(
                &CompanySettingsUpdate {
                    fiscal_year_start_month: Some(13),
                    ..CompanySettingsUpdate::default()
                },
                IdempotencyKey::new(),
            )
            .await
            .expect_err("month");
        assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
    }

    #[tokio::test]
    async fn closing_date_is_refused_locally_without_the_capability() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        let e = s
            .set_closing_date(
                &ClosingDateChange {
                    closing_date: Some(CalendarDate::from_ymd(2026, 9, 30).expect("d")),
                    closing_password: Some(Secret::new("closing-canary")),
                },
                IdempotencyKey::new(),
            )
            .await
            .expect_err("gated");
        assert_eq!(
            e,
            ApiError::Local(LocalValidation::ClosingDateWriteUnavailable)
        );
        assert!(server.received_requests().await.expect("r").is_empty());
        let change = ClosingDateChange {
            closing_date: None,
            closing_password: Some(Secret::new("closing-canary")),
        };
        assert!(!format!("{change:?}").contains("canary"));
    }

    #[tokio::test]
    async fn closing_password_status_and_set() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        assert_eq!(
            s.closing_password_status().await.expect_err("gated"),
            ApiError::Local(LocalValidation::ClosingPasswordUnavailable)
        );
        assert_eq!(
            s.set_closing_password(None, Some(&Secret::new("pw-canary")), IdempotencyKey::new())
                .await
                .expect_err("gated"),
            ApiError::Local(LocalValidation::ClosingPasswordUnavailable)
        );
        assert!(server.received_requests().await.expect("r").is_empty());

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company/closing-password"))
            .respond_with(json(
                200,
                r#"{"closingDate":"2026-09-30","setBy":"5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
                    "setAt":"2026-10-01T10:00:00.000Z","closingPasswordSet":true}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/company/closing-password"))
            .respond_with(json(200, r#"{"closingPasswordSet":false}"#))
            .mount(&server)
            .await;
        let s = crate::resources::testkit::session_with_caps(
            &server,
            &[Capabilities::CLOSING_PASSWORD],
        )
        .await;
        let st = s.closing_password_status().await.expect("status");
        assert!(st.password_set);
        assert_eq!(st.closing_date, CalendarDate::from_ymd(2026, 9, 30).ok());
        assert_eq!(
            s.set_closing_password(None, Some(&Secret::new("abc")), IdempotencyKey::new())
                .await
                .expect_err("short"),
            ApiError::Local(LocalValidation::InvalidValue)
        );
        assert_eq!(
            s.set_closing_password(
                None,
                Some(&Secret::new("x".repeat(73))),
                IdempotencyKey::new()
            )
            .await
            .expect_err("long"),
            ApiError::Local(LocalValidation::InvalidValue)
        );
        let set = s
            .set_closing_password(
                Some(&Secret::new("old-canary")),
                None,
                IdempotencyKey::new(),
            )
            .await
            .expect("cleared");
        assert!(!set);
        let puts = requests(&server, "PUT").await;
        assert_eq!(puts.len(), 1);
        let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(body["currentPassword"], "old-canary");
        assert_eq!(body["newPassword"], Value::Null);
    }

    #[tokio::test]
    async fn owner_reset_is_gated_checked_and_typed() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        assert_eq!(
            s.reset_closing_password(&Secret::new("signin-canary"), None, IdempotencyKey::new())
                .await
                .expect_err("gated"),
            ApiError::Local(LocalValidation::ClosingPasswordUnavailable)
        );
        assert!(server.received_requests().await.expect("r").is_empty());

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/company/closing-password/reset"))
            .respond_with(json(200, r#"{"closingPasswordSet":true}"#))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/company/closing-password/reset"))
            .respond_with(json(
                403,
                r#"{"error":{"message":"bad signin-canary","code":"ACCOUNT_PASSWORD_INVALID"}}"#,
            ))
            .mount(&server)
            .await;
        let s = crate::resources::testkit::session_with_caps(
            &server,
            &[Capabilities::CLOSING_PASSWORD],
        )
        .await;
        for (account, new) in [("", None), ("signin", Some("abc"))] {
            let new = new.map(Secret::new);
            assert_eq!(
                s.reset_closing_password(
                    &Secret::new(account),
                    new.as_ref(),
                    IdempotencyKey::new()
                )
                .await
                .expect_err("local"),
                ApiError::Local(LocalValidation::InvalidValue)
            );
        }
        let set = s
            .reset_closing_password(
                &Secret::new("signin-canary"),
                Some(&Secret::new("new-canary")),
                IdempotencyKey::new(),
            )
            .await
            .expect("reset");
        assert!(set);
        let posts = requests(&server, "POST").await;
        assert_eq!(posts.len(), 1);
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["accountPassword"], "signin-canary");
        assert_eq!(body["newPassword"], "new-canary");
        let e = s
            .reset_closing_password(&Secret::new("signin-canary"), None, IdempotencyKey::new())
            .await
            .expect_err("wrong");
        assert_eq!(e, ApiError::AccountPasswordRejected);
        assert!(!format!("{e} {e:?}").contains("canary"));
        // A credential change is never retried.
        assert_eq!(requests(&server, "POST").await.len(), 2);
    }
}
