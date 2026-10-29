//! Bill payments: the list the vendor statement match reads (P17 WP2, D4)
//! and Pay Bills (P18): the payable bills and credits, the pay itself, one
//! payment read back and its void (in `ap_void`).
//!
//! Every amount is the engine's own figure, read exactly from its decimal
//! string; the discount a bill qualifies for is the engine's
//! `discountAvailable` (U-101), never a percent multiplied here. A JSON
//! number where money belongs is a decode error.
//!
//! The pay (`POST /bill-payments`) sends `bills[].amount` with the engine's
//! meaning: what the payment covers on that bill, cash **and** the credits
//! applied to it. The cash is `amount` minus those credits; an early-payment
//! discount (`discountAmount`) comes on top (`amount + discount` at most the
//! balance due).
//!
//! A lost reply on the pay: keyed and replayed only under
//! `idempotency-pay-bills-v1` (U-102). Before it the route is sent once and
//! a lost reply is `OutcomeUnknown { keyed: false }` (no probe: one pay can
//! make a payment per vendor, there is no single record to find), which the
//! screen treats like any unknown save (PR-1: blocked until the owner
//! checks).

use std::fmt;

use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Timestamp};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, ErrorCode, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::ap_doc::{BillStatus, Percent, ap_body};
use crate::resources::bills::Bills;
use crate::resources::operations::{
    BILL_PAYMENT_GET, BILL_PAYMENTS_CREATE, BILL_PAYMENTS_LIST, BILLS_PAYABLE,
};
use crate::resources::posting::PostingOptions;
use crate::resources::transactions::TxnStatus;
use crate::session::{Session, decode};
use crate::wire::{Capabilities, PageRequest};

/// One bill payment (`GET /bill-payments` row).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillPaymentSummary {
    /// The payment.
    pub id: TransactionId,
    /// Book number.
    #[serde(default, rename = "txnNumber")]
    pub number: Option<String>,
    /// Check number, for a check.
    #[serde(default)]
    pub check_number: Option<u64>,
    /// ACH trace or confirmation number (owner data).
    #[serde(default)]
    pub reference_number: Option<String>,
    /// Date.
    pub txn_date: CalendarDate,
    /// The vendor.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// Amount paid.
    #[serde(default)]
    pub total: Option<Money>,
    /// Posted or void (the statement match keeps posted ones).
    pub status: TxnStatus,
}

impl fmt::Debug for BillPaymentSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillPaymentSummary")
            .field("id", &self.id)
            .field("txn_date", &self.txn_date)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// Bill payment readers of a session (`session.bill_payments()`).
#[derive(Clone, Copy, Debug)]
pub struct BillPayments<'a>(pub(crate) &'a Session);

impl Session {
    /// Bill payments of the open company (read-only).
    #[must_use]
    pub fn bill_payments(&self) -> BillPayments<'_> {
        BillPayments(self)
    }
}

impl BillPayments<'_> {
    /// One page of a vendor's bill payments dated within `start..=end`
    /// (either end open), newest first. The engine sends no total count: a
    /// page shorter than its limit is the last.
    ///
    /// # Errors
    /// `Decode` (a number where money belongs); any [`ApiError`].
    pub async fn list(
        &self,
        vendor: Option<ContactId>,
        start: Option<CalendarDate>,
        end: Option<CalendarDate>,
        page: PageRequest,
    ) -> Result<Vec<BillPaymentSummary>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            data: Vec<BillPaymentSummary>,
        }
        let mut call = Call::at(Method::Get, BILL_PAYMENTS_LIST.route());
        if let Some(v) = vendor {
            call.query.push(("contactId".into(), v.to_string()));
        }
        if let Some(d) = start {
            call.query.push(("startDate".into(), d.to_wire()));
        }
        if let Some(d) = end {
            call.query.push(("endDate".into(), d.to_wire()));
        }
        call.query.extend(page.query());
        let reply = self.0.send(&BILL_PAYMENTS_LIST, call, None).await?;
        decode::<W>(&reply, "bill payments").map(|w| w.data)
    }

    /// Pays bills (`POST /bill-payments`): one check per vendor, in the
    /// order the engine lists them. See the module notes for `amount`.
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)`: `InvalidValue` (no bill, a negative
    /// or over-precise amount, a credit for a bill not in the run, nothing
    /// to pay on a bill), `Unsupported` (a discount or discount account
    /// without `bill-discounts-v1`, a first check number without
    /// `bill-payment-check-number-v1`, a closing-date password without
    /// `ap-closing-v1`). From the engine: `ClosedPeriod`,
    /// `ClosingPasswordRejected`, the refusals [`pay_problem`] names,
    /// `OutcomeUnknown` (see the module notes), any [`ApiError`].
    pub async fn pay(
        &self,
        req: &PayBillsRequest,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Vec<CreatedPayment>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            payments: Vec<CreatedPayment>,
        }
        let s = &self.0.pinned();
        let body = ap_body(s, pay_body(req, &s.client().capabilities())?, opts, None)?;
        let mut call = Call::at(Method::Post, BILL_PAYMENTS_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&BILL_PAYMENTS_CREATE, call, None).await?;
        decode::<W>(&reply, "bill payments")
            .map(|w| w.payments)
            .map_err(crate::resources::versioning::saved_unreadable)
    }

    /// One bill payment with what it paid (`GET /bill-payments/{id}`).
    ///
    /// # Errors
    /// `NotFound`; `Decode` (a number where money belongs); any
    /// [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<BillPaymentRecord, ApiError> {
        #[derive(Deserialize)]
        struct W {
            payment: BillPaymentRecord,
        }
        let call = Call::new(
            Method::Get,
            BILL_PAYMENT_GET.route(),
            format!("/api/v1/bill-payments/{id}"),
        );
        let reply = self.0.send(&BILL_PAYMENT_GET, call, None).await?;
        decode::<W>(&reply, "bill payment").map(|w| w.payment)
    }
}

// ---- P18: the payable list ----------------------------------------------

/// What Pay Bills lists (`GET /bills/payable`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PayableFilter {
    /// One vendor (`None`: every vendor).
    pub vendor: Option<ContactId>,
    /// Due on or before this date (`None`: every open bill).
    pub due_on_or_before: Option<CalendarDate>,
    /// The payment date the engine works out `discountAvailable` for
    /// (`bill-discounts-v1`; not sent without it).
    pub payment_date: Option<CalendarDate>,
}

/// One bill Pay Bills can pay (`/bills/payable` row): posted, received,
/// with a balance due.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayableBill {
    /// The bill.
    pub id: TransactionId,
    /// The bill's own number in the books.
    #[serde(default, rename = "txnNumber")]
    pub number: Option<String>,
    /// The vendor.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// The vendor's name (owner data).
    #[serde(default)]
    pub contact_name: Option<String>,
    /// Bill date.
    pub txn_date: CalendarDate,
    /// Due date.
    #[serde(default)]
    pub due_date: Option<CalendarDate>,
    /// Last day of the early-payment discount.
    #[serde(default)]
    pub discount_date: Option<CalendarDate>,
    /// The discount's percent (shown only; the amount is
    /// [`PayableBill::discount_available`]).
    #[serde(default, deserialize_with = "lenient_percent")]
    pub discount_percent: Option<Percent>,
    /// The vendor's invoice number (owner data).
    #[serde(default)]
    pub vendor_invoice_number: Option<String>,
    /// The bill's total.
    #[serde(default)]
    pub total: Option<Money>,
    /// Paid so far.
    #[serde(default)]
    pub amount_paid: Option<Money>,
    /// Credits applied so far.
    #[serde(default)]
    pub credits_applied: Option<Money>,
    /// What is still owed.
    pub balance_due: Money,
    /// Payment state.
    #[serde(default)]
    pub bill_status: Option<BillStatus>,
    /// Terms code.
    #[serde(default)]
    pub payment_terms: Option<String>,
    /// The memo (owner data; Pay Bills shows it under the vendor).
    #[serde(default)]
    pub memo: Option<String>,
    /// Whole days past due, as the engine counted them today.
    #[serde(default)]
    pub days_overdue: u32,
    /// The discount the engine gives on the asked payment date
    /// (`bill-discounts-v1`; `None` from an older engine).
    #[serde(default)]
    pub discount_available: Option<Money>,
    /// The bill's version (`bill-discounts-v1`).
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

/// Ids, dates and states only: names, numbers and amounts are owner data.
impl fmt::Debug for PayableBill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayableBill")
            .field("id", &self.id)
            .field("contact_id", &self.contact_id)
            .field("due_date", &self.due_date)
            .field("discount_date", &self.discount_date)
            .field("bill_status", &self.bill_status)
            .finish_non_exhaustive()
    }
}

/// A vendor credit with something left to apply (`/bills/payable`
/// `credits[]`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayableCredit {
    /// The credit.
    pub id: TransactionId,
    /// Its number in the books.
    #[serde(default, rename = "txnNumber")]
    pub number: Option<String>,
    /// The vendor's credit memo number, when the engine sends it.
    #[serde(default)]
    pub vendor_invoice_number: Option<String>,
    /// The vendor.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// Date.
    pub txn_date: CalendarDate,
    /// The credit's total.
    #[serde(default)]
    pub total: Option<Money>,
    /// What is left to apply.
    pub balance_due: Money,
    /// Memo (owner data).
    #[serde(default)]
    pub memo: Option<String>,
}

/// Ids and dates only.
impl fmt::Debug for PayableCredit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayableCredit")
            .field("id", &self.id)
            .field("contact_id", &self.contact_id)
            .field("txn_date", &self.txn_date)
            .finish_non_exhaustive()
    }
}

/// The payable bills (soonest due first, the engine's order) and the
/// listed vendors' open credits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct PayableList {
    /// The bills.
    #[serde(default)]
    pub bills: Vec<PayableBill>,
    /// The credits.
    #[serde(default)]
    pub credits: Vec<PayableCredit>,
}

fn lenient_percent<'de, D>(d: D) -> Result<Option<Percent>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // A zero or unreadable percent is "no discount", never a decode error
    // of the whole list (the amount comes from `discountAvailable`).
    let v: Option<Value> = Option::deserialize(d)?;
    Ok(match v {
        Some(Value::String(s)) => Percent::parse(&s).ok().or_else(|| {
            s.parse::<rust_decimal::Decimal>()
                .ok()
                .and_then(|x| Percent::parse(&x.normalize().to_string()).ok())
        }),
        _ => None,
    })
}

impl Bills<'_> {
    /// The bills Pay Bills can pay and the open credits of their vendors
    /// (`GET /bills/payable`). `payment_date` goes out only with
    /// `bill-discounts-v1`.
    ///
    /// # Errors
    /// `Decode` (a number where money belongs); any [`ApiError`].
    pub async fn payable_list(&self, filter: &PayableFilter) -> Result<PayableList, ApiError> {
        let mut call = Call::at(Method::Get, BILLS_PAYABLE.route());
        if let Some(v) = filter.vendor {
            call.query.push(("contactId".into(), v.to_string()));
        }
        if let Some(d) = filter.due_on_or_before {
            call.query.push(("dueOnOrBefore".into(), d.to_wire()));
        }
        if let Some(d) = filter.payment_date
            && self
                .0
                .client()
                .capabilities()
                .has(Capabilities::BILL_DISCOUNTS)
        {
            call.query.push(("paymentDate".into(), d.to_wire()));
        }
        let reply = self.0.send(&BILLS_PAYABLE, call, None).await?;
        decode::<PayableList>(&reply, "payable bills")
    }
}

// ---- P18: the pay -------------------------------------------------------

/// How the checks of a pay get their numbers (Q-P18e).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckNumbering {
    /// Queued for printing; numbered when printed (Q-246).
    PrintLater,
    /// Numbered now from `first`, one per vendor in the engine's order
    /// (`bill-payment-check-number-v1`).
    Assign {
        /// The first check number.
        first: u64,
    },
}

/// One bill in a pay.
#[derive(Clone, PartialEq, Eq)]
pub struct BillToPay {
    /// The bill.
    pub bill: TransactionId,
    /// The cash paid on it.
    pub cash: Money,
    /// The early-payment discount taken on it (zero for none).
    pub discount: Money,
}

/// A vendor credit applied to one bill of the pay.
#[derive(Clone, PartialEq, Eq)]
pub struct CreditToApply {
    /// The credit.
    pub credit: TransactionId,
    /// The bill it is applied to.
    pub bill: TransactionId,
    /// How much of it.
    pub amount: Money,
}

/// A Pay Bills run: checks from one bank account on one date.
#[derive(Clone, PartialEq, Eq)]
pub struct PayBillsRequest {
    /// The bank account the checks are drawn on.
    pub bank_account: AccountId,
    /// Payment date.
    pub date: CalendarDate,
    /// The bills, in the screen's order.
    pub bills: Vec<BillToPay>,
    /// The credits applied.
    pub credits: Vec<CreditToApply>,
    /// Where discounts post (`None`: the company's default, Q-P18a).
    pub discount_account: Option<AccountId>,
    /// Print later or numbers now.
    pub numbering: CheckNumbering,
    /// Bookkeeping memo (owner data).
    pub memo: Option<String>,
}

/// Counts and ids only: amounts and the memo are owner data.
impl fmt::Debug for PayBillsRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayBillsRequest")
            .field("bank_account", &self.bank_account)
            .field("date", &self.date)
            .field("bills", &self.bills.len())
            .field("credits", &self.credits.len())
            .field("numbering", &self.numbering)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for BillToPay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillToPay")
            .field("bill", &self.bill)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for CreditToApply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreditToApply")
            .field("credit", &self.credit)
            .field("bill", &self.bill)
            .finish_non_exhaustive()
    }
}

/// Two decimal places at most (the engine's money on bills).
fn cents(m: Money) -> bool {
    m.to_ten_thousandths() % 100 == 0
}

/// The engine's `amount` for `bill`: its cash plus the credits applied to
/// it.
fn coverage(req: &PayBillsRequest, b: &BillToPay) -> Result<Money, ApiError> {
    let credits = Money::sum(
        req.credits
            .iter()
            .filter(|c| c.bill == b.bill)
            .map(|c| c.amount),
    )
    .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
    b.cash
        .checked_add(credits)
        .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))
}

/// The body of a pay, checked; see [`BillPayments::pay`] for the refusals.
pub(crate) fn pay_body(req: &PayBillsRequest, caps: &Capabilities) -> Result<Value, ApiError> {
    let invalid = || ApiError::Local(LocalValidation::InvalidValue);
    let unsupported = || ApiError::Local(LocalValidation::Unsupported);
    if req.bills.is_empty() {
        return Err(invalid());
    }
    let discounts = caps.has(Capabilities::BILL_DISCOUNTS);
    let any_discount = req.bills.iter().any(|b| !b.discount.is_zero());
    if (any_discount || req.discount_account.is_some()) && !discounts {
        return Err(unsupported());
    }
    for c in &req.credits {
        if !c.amount.is_positive() || !cents(c.amount) {
            return Err(invalid());
        }
        if !req.bills.iter().any(|b| b.bill == c.bill) {
            return Err(invalid());
        }
    }
    let mut bills = Vec::with_capacity(req.bills.len());
    for b in &req.bills {
        if b.cash.is_negative() || b.discount.is_negative() || !cents(b.cash) || !cents(b.discount)
        {
            return Err(invalid());
        }
        let amount = coverage(req, b)?;
        // Something must be applied: cash, a credit or a discount. Before
        // U-101 the engine wants a positive amount (cash plus credits).
        if !amount.is_positive() && (b.discount.is_zero() || !discounts) {
            return Err(invalid());
        }
        let mut row = Map::new();
        row.insert("billId".into(), Value::String(b.bill.to_string()));
        row.insert("amount".into(), Value::String(amount.to_wire()));
        if discounts && !b.discount.is_zero() {
            row.insert("discountAmount".into(), Value::String(b.discount.to_wire()));
        }
        bills.push(Value::Object(row));
    }
    let credits: Vec<Value> = req
        .credits
        .iter()
        .map(|c| {
            json!({
                "creditId": c.credit.to_string(),
                "billId": c.bill.to_string(),
                "amount": c.amount.to_wire(),
            })
        })
        .collect();
    let mut body = json!({
        "bankAccountId": req.bank_account.to_string(),
        "txnDate": req.date.to_wire(),
        "method": "check",
        "bills": bills,
    });
    let Value::Object(map) = &mut body else {
        return Err(invalid());
    };
    if !credits.is_empty() {
        map.insert("credits".into(), Value::Array(credits));
    }
    if let Some(a) = req.discount_account {
        map.insert("discountAccountId".into(), Value::String(a.to_string()));
    }
    match req.numbering {
        CheckNumbering::PrintLater => {
            map.insert("printLater".into(), Value::Bool(true));
        }
        CheckNumbering::Assign { first } => {
            if !caps.has(Capabilities::BILL_PAYMENT_CHECK_NUMBER) || first == 0 {
                return Err(unsupported());
            }
            map.insert("printLater".into(), Value::Bool(false));
            map.insert("checkNumber".into(), Value::String(first.to_string()));
        }
    }
    if let Some(m) = req.memo.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        map.insert("memo".into(), Value::String(m.to_string()));
    }
    Ok(body)
}

/// One payment a pay made (a check to one vendor).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedPayment {
    /// The payment.
    pub id: TransactionId,
    /// The vendor.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// Date.
    pub txn_date: CalendarDate,
    /// The check number (`None` while it waits to print).
    #[serde(default)]
    pub check_number: Option<u64>,
    /// `queue` while it waits to print.
    #[serde(default)]
    pub print_status: Option<String>,
    /// The cash the check pays.
    pub net_payment: Money,
    /// Bills it paid.
    #[serde(default)]
    pub bills_paid: u32,
    /// Credits it applied.
    #[serde(default)]
    pub credits_applied: u32,
}

/// Ids, dates and counts only.
impl fmt::Debug for CreatedPayment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedPayment")
            .field("id", &self.id)
            .field("txn_date", &self.txn_date)
            .field("bills_paid", &self.bills_paid)
            .field("credits_applied", &self.credits_applied)
            .finish_non_exhaustive()
    }
}

/// What a payment paid on one bill (`billApplications[]`): `amount` is the
/// **cash**, the discount beside it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillApplication {
    /// The bill.
    pub bill_id: TransactionId,
    /// Cash.
    pub amount: Money,
    /// Early-payment discount (`None` before U-101).
    #[serde(default)]
    pub discount_amount: Option<Money>,
}

/// A credit a payment applied (`creditApplications[]`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditApplication {
    /// The credit.
    pub credit_id: TransactionId,
    /// The bill it went to.
    pub bill_id: TransactionId,
    /// How much.
    pub amount: Money,
}

impl fmt::Debug for BillApplication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillApplication")
            .field("bill_id", &self.bill_id)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for CreditApplication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreditApplication")
            .field("credit_id", &self.credit_id)
            .field("bill_id", &self.bill_id)
            .finish_non_exhaustive()
    }
}

/// One bill payment as `GET /bill-payments/{id}` answers it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillPaymentRecord {
    /// The payment.
    pub id: TransactionId,
    /// Date.
    pub txn_date: CalendarDate,
    /// The vendor.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// The cash paid.
    #[serde(default)]
    pub total: Option<Money>,
    /// Check number (`None` while it waits to print).
    #[serde(default)]
    pub check_number: Option<u64>,
    /// `queue` while it waits to print.
    #[serde(default)]
    pub print_status: Option<String>,
    /// Posted or void.
    pub status: TxnStatus,
    /// Memo (owner data).
    #[serde(default)]
    pub memo: Option<String>,
    /// Version, for the void's stale guard.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    /// What it paid on each bill.
    #[serde(default)]
    pub bill_applications: Vec<BillApplication>,
    /// The credits it applied.
    #[serde(default)]
    pub credit_applications: Vec<CreditApplication>,
}

/// Ids, date, state and counts only.
impl fmt::Debug for BillPaymentRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillPaymentRecord")
            .field("id", &self.id)
            .field("txn_date", &self.txn_date)
            .field("status", &self.status)
            .field("bills", &self.bill_applications.len())
            .field("credits", &self.credit_applications.len())
            .finish_non_exhaustive()
    }
}

/// 400: a discount was taken but no discount account was sent and the
/// company has none (U-101).
pub const DISCOUNT_ACCOUNT_REQUIRED: &str = "DISCOUNT_ACCOUNT_REQUIRED";
/// 409: a check number is already used in this bank account (U-103).
pub const CHECK_NUMBER_IN_USE: &str = "CHECK_NUMBER_IN_USE";
/// 400: a bill is void or deleted.
pub const BILL_NOT_PAYABLE: &str = "BILL_NOT_PAYABLE";
/// 400: a credit is void.
pub const CREDIT_NOT_AVAILABLE: &str = "CREDIT_NOT_AVAILABLE";
/// 409: a bill is not marked received (U-87).
pub const BILL_NOT_RECEIVED: &str = "BILL_NOT_RECEIVED";

/// The engine's refusal of a pay, for the screen's plain-language message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PayProblem {
    /// A discount needs a discount account (Set Discount asks, Q-P18a).
    DiscountAccountRequired,
    /// The first check number is taken.
    CheckNumberInUse,
    /// The bills or credits changed since the list was read (paid,
    /// voided, not received, or an amount over the balance: Q-P18g).
    BillsChanged,
}

/// Classifies a pay refusal (`None` for closed periods, passwords,
/// network and anything else).
#[must_use]
pub fn pay_problem(e: &ApiError) -> Option<PayProblem> {
    let code = match e {
        ApiError::Conflict { code: Some(c), .. } => Some(c.as_str()),
        ApiError::Conflict { .. } => return Some(PayProblem::BillsChanged),
        ApiError::Validation(v) => v.code().map(ErrorCode::as_str),
        _ => return None,
    };
    match code {
        Some(DISCOUNT_ACCOUNT_REQUIRED) => Some(PayProblem::DiscountAccountRequired),
        Some(CHECK_NUMBER_IN_USE) => Some(PayProblem::CheckNumberInUse),
        // Before U-105 the engine says "exceeds balance due" with no code
        // (Q-P18g): an uncoded refusal of a pay the screen checked is a
        // changed list.
        Some(BILL_NOT_PAYABLE | CREDIT_NOT_AVAILABLE | BILL_NOT_RECEIVED | "STALE") | None => {
            Some(PayProblem::BillsChanged)
        }
        Some(_) => None,
    }
}

#[cfg(test)]
#[path = "pay_bills_tests.rs"]
mod pay_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const VENDOR: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

    #[tokio::test]
    async fn rows_read_exact_and_filters_go_out() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bill-payments"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"data":[{{"id":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnType":"bill_payment",
                    "txnNumber":null,"checkNumber":1042,"referenceNumber":"canary-ref","txnDate":"2026-10-07",
                    "contactId":"{VENDOR}","total":"148.0000","status":"posted","memo":"canary"}}]}}"#
                ),
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let vendor = ContactId::parse(VENDOR).expect("id");
        let rows = s
            .bill_payments()
            .list(
                Some(vendor),
                CalendarDate::from_ymd(2026, 10, 1).ok(),
                CalendarDate::from_ymd(2026, 10, 31).ok(),
                PageRequest::new(50, 0).expect("page"),
            )
            .await
            .expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].total.map(Money::to_wire).as_deref(),
            Some("148.0000")
        );
        assert_eq!(rows[0].check_number, Some(1042));
        assert_eq!(rows[0].status, TxnStatus::Posted);
        let shown = format!("{:?}", rows[0]);
        assert!(
            !shown.contains("148") && !shown.contains("canary"),
            "{shown}"
        );
        assert_eq!(
            query(&requests(&server, "GET").await[0]),
            format!("contactId={VENDOR}&startDate=2026-10-01&endDate=2026-10-31&limit=50&offset=0")
        );
    }

    #[tokio::test]
    async fn a_number_where_money_belongs_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bill-payments"))
            .respond_with(json(
                200,
                r#"{"data":[{"id":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnDate":"2026-10-07",
                "total":148.0,"status":"posted"}]}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let e = s
            .bill_payments()
            .list(None, None, None, PageRequest::first_max())
            .await
            .expect_err("float");
        assert_eq!(
            e,
            ApiError::Decode {
                what: "bill payments"
            }
        );
    }
}
