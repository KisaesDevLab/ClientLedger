//! The sales tax liability and payments reports (P23, engine U-149; the
//! v2 shape per the P23 plan §2.5, verified at the WP-4 pin bump).
//!
//! - The v1 report's `totalSales`/`totalTax` are JavaScript numbers and are
//!   deliberately not read; only the v2 string fields are (`Capabilities::
//!   SALES_TAX_LIABILITY`).
//! - Every column is the engine's: collected, discounts, bad debts,
//!   recoveries, adjustments, paid and owed at the end, per agency and
//!   item; the footer rows and the balance check. The screen shows them
//!   and adds nothing up (plan §4). Bad debts and recoveries are always
//!   0.00 on cash basis, where the screen hides them (Q-SPF3).
//! - The cash rounding difference may carry up to six places, so it is a
//!   [`SixPlaceAmount`], display only.
//! - The query keeps the v1 route's names (`start_date`, `end_date`), plus
//!   `basis` and `agency_id` (engine PR #218). Filtered by agency, the GL
//!   fields (`glMovement`, `otherEntries`, `roundingDifference`,
//!   `balanceCheck`) are `null`: the screen hides those footer rows.

use std::fmt;
use std::str::FromStr;

use cl_core::{CalendarDate, Money, TaxRate};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde::de::Deserializer;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::customer_payments::{de_cents_zero, null_vec};
use crate::resources::operations::{REPORT_SALES_TAX_LIABILITY, REPORT_SALES_TAX_PAYMENTS};
use crate::resources::reports::Reports;
use crate::resources::sales_tax::{
    SalesTaxAgencyId, SalesTaxBasis, SalesTaxItemId, TaxCodeId, decode_list,
};
use crate::resources::sales_tax_payments::SalesTaxPaymentRecord;
use crate::session::decode;
use crate::wire::Capabilities;

/// An amount with up to six decimals (the cash rounding difference,
/// `85.79722 − 85.80`). Exact, display only: never money arithmetic.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct SixPlaceAmount(Decimal);

impl SixPlaceAmount {
    /// Strict parse: `^-?\d{1,15}(\.\d{1,6})?$`.
    ///
    /// # Errors
    /// `InvalidValue` for anything else.
    pub fn parse(s: &str) -> Result<SixPlaceAmount, LocalValidation> {
        let body = s.strip_prefix('-').unwrap_or(s);
        let (int, frac) = body.split_once('.').unwrap_or((body, ""));
        let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if int.is_empty()
            || int.len() > 15
            || !digits(int)
            || !digits(frac)
            || frac.len() > 6
            || (body.contains('.') && frac.is_empty())
        {
            return Err(LocalValidation::InvalidValue);
        }
        let d = Decimal::from_str(s).map_err(|_| LocalValidation::InvalidValue)?;
        let mut n = d.normalize();
        if n.is_zero() {
            n.set_sign_positive(true);
        }
        Ok(SixPlaceAmount(n))
    }

    /// The exact value.
    #[must_use]
    pub fn as_decimal(self) -> Decimal {
        self.0
    }

    /// True for zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }

    /// The text, trailing zeros removed (`-0.00278`).
    #[must_use]
    pub fn to_text(self) -> String {
        self.0.to_string()
    }
}

/// An amount: not shown in logs.
impl fmt::Debug for SixPlaceAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SixPlaceAmount(..)")
    }
}

impl<'de> Deserialize<'de> for SixPlaceAmount {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        SixPlaceAmount::parse(&s).map_err(|_| serde::de::Error::custom("amount"))
    }
}

/// The tax columns of an item, an agency's totals or the report's.
#[derive(Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiabilityColumns {
    /// Tax charged (accrual) or collected (cash) in the period.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub collected: Money,
    /// SSUTA early-payment discount shares (negative).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub discounts: Money,
    /// SSUTA bad-debt deductions (negative; accrual only).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub bad_debts: Money,
    /// Recovered bad debts (accrual only).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub recoveries: Money,
    /// Pay Sales Tax adjustments in the period.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub adjustments: Money,
    /// Paid in the period.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub paid: Money,
    /// Owed at the end date (cumulative).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub owed_at_end: Money,
}

impl fmt::Debug for LiabilityColumns {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LiabilityColumns(..)")
    }
}

/// Non-taxable sales under one tax code.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NonTaxableSales {
    /// The code (`None`: no code, legacy).
    #[serde(default)]
    pub code_id: Option<TaxCodeId>,
    /// Its text (`Non`, `Res`, …).
    #[serde(default)]
    pub code: String,
    /// Sales under it.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub amount: Money,
}

impl fmt::Debug for NonTaxableSales {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NonTaxableSales")
            .field("code_id", &self.code_id)
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}

/// One sales tax item's row.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiabilityItem {
    /// The item (`None`: legacy rows without one).
    #[serde(default)]
    pub tax_item_id: Option<SalesTaxItemId>,
    /// Its name (owner data).
    #[serde(default)]
    pub name: String,
    /// Its rate.
    #[serde(default)]
    pub rate: Option<TaxRate>,
    /// Taxable sales (a group's base shows under each member).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub taxable_sales: Money,
    /// Non-taxable sales by code.
    #[serde(default, deserialize_with = "null_vec")]
    pub non_taxable: Vec<NonTaxableSales>,
    /// The tax columns.
    #[serde(flatten)]
    pub tax: LiabilityColumns,
}

impl fmt::Debug for LiabilityItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiabilityItem")
            .field("tax_item_id", &self.tax_item_id)
            .field("rate", &self.rate)
            .field("non_taxable", &self.non_taxable)
            .finish_non_exhaustive()
    }
}

/// One agency's rows.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiabilityAgency {
    /// The agency (`None`: items without one).
    #[serde(default)]
    pub agency_id: Option<SalesTaxAgencyId>,
    /// Its name (owner data).
    #[serde(default)]
    pub name: String,
    /// Per item.
    #[serde(default, deserialize_with = "null_vec")]
    pub items: Vec<LiabilityItem>,
    /// The agency's tax totals (taxable sales are not totalled).
    #[serde(default)]
    pub totals: LiabilityColumns,
}

impl fmt::Debug for LiabilityAgency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiabilityAgency")
            .field("agency_id", &self.agency_id)
            .field("items", &self.items)
            .finish_non_exhaustive()
    }
}

/// The balance check: Sales Tax Payable is owed plus opening paid plus
/// unassigned plus other entries plus the cash rounding, all to the end
/// date (the engine's figures).
#[derive(Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiabilityBalanceCheck {
    /// The Sales Tax Payable balance at the end date.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub stp_balance: Money,
    /// Owed at the end date.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub owed_at_end: Money,
    /// Tax on Sales Tax Payable with no tax item, to the end date.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub unassigned: Money,
    /// Other entries to Sales Tax Payable to the end date.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub other_entries_cumulative: Money,
    /// Tax paid before the items existed (backfill).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub opening_paid: Money,
    /// Cash basis: the running-total rounding to the end date (up to six
    /// places).
    #[serde(default)]
    pub rounding_difference_cumulative: Option<SixPlaceAmount>,
}

impl fmt::Debug for LiabilityBalanceCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LiabilityBalanceCheck(..)")
    }
}

/// `GET /reports/sales-tax-liability` (v2).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesTaxLiability {
    /// The basis it was run on.
    #[serde(default)]
    pub basis: SalesTaxBasis,
    /// The period.
    #[serde(default)]
    pub start_date: Option<CalendarDate>,
    /// The period's end.
    #[serde(default)]
    pub end_date: Option<CalendarDate>,
    /// The agency it was filtered to (`None`: every agency).
    #[serde(default)]
    pub agency_id: Option<SalesTaxAgencyId>,
    /// Per agency.
    #[serde(default, deserialize_with = "null_vec")]
    pub agencies: Vec<LiabilityAgency>,
    /// The report's tax totals over every agency shown.
    #[serde(default)]
    pub totals: LiabilityColumns,
    /// Collected + discounts + bad debts + recoveries in the period (the
    /// engine's figure).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub total: Money,
    /// Sales Tax Payable rows without an item (legacy).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub unassigned: Money,
    /// Paid before tax items existed.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub opening_paid: Money,
    /// Entries to Sales Tax Payable in the period that are not sales tax
    /// rows or payments (journal entries, …). `None` when filtered by
    /// agency (the GL is not split by agency).
    #[serde(
        default,
        deserialize_with = "crate::resources::customer_payments::de_cents_opt"
    )]
    pub other_entries: Option<Money>,
    /// The Sales Tax Payable movement in the period on the chosen basis
    /// (`None` when filtered by agency).
    #[serde(
        default,
        deserialize_with = "crate::resources::customer_payments::de_cents_opt"
    )]
    pub gl_movement: Option<Money>,
    /// Cash basis only: the running-total rounding left in the period.
    #[serde(default)]
    pub rounding_difference: Option<SixPlaceAmount>,
    /// The balance check (`None` when filtered by agency).
    #[serde(default)]
    pub balance_check: Option<LiabilityBalanceCheck>,
}

impl fmt::Debug for SalesTaxLiability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesTaxLiability")
            .field("basis", &self.basis)
            .field("start_date", &self.start_date)
            .field("end_date", &self.end_date)
            .field("agencies", &self.agencies)
            .finish_non_exhaustive()
    }
}

impl SalesTaxLiability {
    /// True on cash basis: the Bad debts and Recovered columns are hidden
    /// (always 0.00 there, Q-SPF3).
    #[must_use]
    pub fn hides_bad_debts(&self) -> bool {
        self.basis == SalesTaxBasis::Cash
    }
}

/// The liability report's query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiabilityQuery {
    /// From (inclusive).
    pub start_date: CalendarDate,
    /// To (inclusive).
    pub end_date: CalendarDate,
    /// `None`: the company's sales tax basis.
    pub basis: Option<SalesTaxBasis>,
    /// One agency (`None`: every agency; the GL fields come back `None`
    /// when filtered).
    pub agency_id: Option<SalesTaxAgencyId>,
}

impl Reports<'_> {
    /// The sales tax liability report (v2 columns).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `sales-tax-liability-v2` (the v1 report
    /// has only JavaScript numbers); `Local(BadRange)` for an end before the
    /// start; `Decode`; any [`ApiError`].
    pub async fn sales_tax_liability(
        &self,
        q: LiabilityQuery,
    ) -> Result<SalesTaxLiability, ApiError> {
        crate::resources::customer_payments::require(self.0, Capabilities::SALES_TAX_LIABILITY)?;
        if q.end_date < q.start_date {
            return Err(ApiError::Local(LocalValidation::BadRange));
        }
        let mut call = Call::at(Method::Get, REPORT_SALES_TAX_LIABILITY.route());
        call.query = vec![
            ("start_date".into(), q.start_date.to_string()),
            ("end_date".into(), q.end_date.to_string()),
        ];
        if let Some(b) = q.basis {
            call.query.push(("basis".into(), b.as_wire().into()));
        }
        if let Some(a) = q.agency_id {
            call.query.push(("agency_id".into(), a.to_string()));
        }
        let reply = self.0.send(&REPORT_SALES_TAX_LIABILITY, call, None).await?;
        decode(&reply, "sales tax liability")
    }

    /// The sales tax payments of a period (with `linked`).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `sales-tax-liability-v2`;
    /// `Local(BadRange)`; `Decode`; any [`ApiError`].
    pub async fn sales_tax_payments(
        &self,
        start_date: CalendarDate,
        end_date: CalendarDate,
        agency: Option<SalesTaxAgencyId>,
    ) -> Result<Vec<SalesTaxPaymentRecord>, ApiError> {
        crate::resources::customer_payments::require(self.0, Capabilities::SALES_TAX_LIABILITY)?;
        if end_date < start_date {
            return Err(ApiError::Local(LocalValidation::BadRange));
        }
        let mut call = Call::at(Method::Get, REPORT_SALES_TAX_PAYMENTS.route());
        call.query = vec![
            ("start_date".into(), start_date.to_string()),
            ("end_date".into(), end_date.to_string()),
        ];
        if let Some(a) = agency {
            call.query.push(("agency_id".into(), a.to_string()));
        }
        let reply = self.0.send(&REPORT_SALES_TAX_PAYMENTS, call, None).await?;
        decode_list(&reply.body, "payments", "sales tax payments report")
    }
}
