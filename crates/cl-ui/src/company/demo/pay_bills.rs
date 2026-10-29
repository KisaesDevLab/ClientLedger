//! The demo "engine" for Pay Bills (P18): the payable list, the pay, a
//! payment read back and its void, with the engine's rules (U-101 to
//! U-104) for `--demo` only.
//!
//! - The payable bills are the sample's open bills (and the bills entered in
//!   the demo) less what the demo paid on them. The mockup's story fields the
//!   fixture does not carry (vendor invoice numbers, the statement note, the
//!   7781 discount terms) are laid over them by vendor and amount
//!   ([`STORY`]); the Table Rock credit memo CM-114 is the demo's own
//!   ([`STORY_CREDIT`]).
//! - `discountAvailable` is worked out the engine's way: the balance due ×
//!   the percent ÷ 100, rounded to the cent half away from zero, when the
//!   payment date is on or before the discount date (218.44 at 2% → 4.37).
//!   The screen never computes it.
//! - A pay posts one payment per vendor in the order the bills came: Dr
//!   Accounts Payable (cash + discount), Cr the bank or card account (cash),
//!   Cr the discount account (discount). Credits move nothing on the ledger
//!   (Accounts Payable both ways) and are tracked here. The rules: closing
//!   date (password "demo-close"), cash + credits + discount ≤ balance due,
//!   a credit within what it has left, a discount needs an account
//!   (`DISCOUNT_ACCOUNT_REQUIRED`), a check number free in the account
//!   (`CHECK_NUMBER_IN_USE`).
//!
//! All data is synthetic.

use std::collections::BTreeMap;
use std::fmt;

use cl_api::{
    ApiError, BillApplication, BillPaymentRecord, CheckNumbering, CreatedPayment,
    CreditApplication, LocalValidation, PayBillsRequest, PayMethod, PayableBill, PayableCredit,
    PayableFilter, PayableList,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use super::DemoState;
use super::register::{DemoJournal, DemoLine, check_closing};

/// The mockup's story fields by (vendor, amount due): vendor invoice number,
/// the note under the vendor, the terms text, the discount date and the
/// discount percent.
#[allow(clippy::type_complexity, reason = "a fixed table of story fields")]
pub const STORY: [(
    &str,
    &str,
    &str,
    Option<&str>,
    &str,
    Option<(u32, u32)>,
    Option<&str>,
); 5] = [
    (
        "Ozark Feed & Seed",
        "1286.40",
        "20931",
        None,
        "Net 30",
        None,
        None,
    ),
    (
        "Riverbend Fuel",
        "1412.06",
        "STMT-1031",
        Some("October statement"),
        "Net 30",
        None,
        None,
    ),
    (
        "Monett Tire & Lube",
        "640.00",
        "2214",
        None,
        "Net 30",
        None,
        None,
    ),
    (
        "Ozark Office Supply",
        "218.44",
        "7781",
        None,
        "2% 5, Net 15",
        Some((11, 6)),
        Some("2"),
    ),
    (
        "Table Rock Nursery",
        "3915.00",
        "8813",
        None,
        "Net 30",
        None,
        None,
    ),
];

/// The demo's vendor credit (CM-114, Table Rock Nursery, 250.00).
pub const STORY_CREDIT: (&str, &str, &str) = ("Table Rock Nursery", "CM-114", "250.00");

/// The story credit's id.
#[must_use]
pub fn story_credit_id() -> TransactionId {
    TransactionId::parse("d0000000-0000-4000-8000-c00000000114")
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

/// One payment the demo made.
#[derive(Clone)]
struct DemoPayment {
    id: TransactionId,
    vendor: ContactId,
    date: CalendarDate,
    cash: Money,
    check_number: Option<u64>,
    print_later: bool,
    bills: Vec<(TransactionId, Money, Money)>,
    credits: Vec<(TransactionId, TransactionId, Money)>,
}

/// What Pay Bills did in the demo.
#[derive(Clone, Default)]
pub struct DemoPayBills {
    /// Cash + credits + discount applied per bill.
    applied: BTreeMap<TransactionId, Money>,
    /// Used per credit.
    credit_used: BTreeMap<TransactionId, Money>,
    payments: Vec<DemoPayment>,
    /// The company's default discount account (`billDiscountAccountId`).
    pub discount_account: Option<AccountId>,
    /// Pays that reached the demo engine (tests: a refused pay sent nothing).
    pub writes: u32,
}

impl fmt::Debug for DemoPayBills {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoPayBills")
            .field("payments", &self.payments.len())
            .field("writes", &self.writes)
            .finish_non_exhaustive()
    }
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap_or(Money::ZERO)
}

fn chart(state: &mut DemoState) -> Vec<cl_api::Account> {
    state.chart.get_or_insert_with(super::chart).clone()
}

fn closing(state: &DemoState) -> Option<CalendarDate> {
    let open = state.open?;
    state
        .companies
        .iter()
        .find(|c| c.tenant_id == open)
        .and_then(|c| c.closed_through)
}

/// The engine's refusal (`code` empty: an uncoded one, as "exceeds balance
/// due" is).
fn refusal(status: u16, code: &str, message: &str) -> ApiError {
    let body = if code.is_empty() {
        json!({ "error": { "message": message } })
    } else {
        json!({ "error": { "message": message, "code": code } })
    };
    cl_api::engine_refusal(status, &body)
}

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

/// The engine's discount: `balance × percent ÷ 100`, to the cent, half away
/// from zero (exact integer arithmetic in ten-thousandths).
#[must_use]
pub fn discount_of(balance: Money, percent: &str) -> Money {
    let Ok(p) = cl_core::Quantity::parse_wire(percent) else {
        return Money::ZERO;
    };
    // balance (×10⁴) × percent (×10⁴) is ×10⁸; ÷ 100 for the percent and
    // ×100 for cents: divide by 10⁸.
    let prod = balance.to_ten_thousandths() * p.to_ten_thousandths();
    let div: i128 = 100_000_000;
    let mut cents = prod / div;
    if (prod % div).unsigned_abs() * 2 >= div.unsigned_abs() {
        cents += prod.signum();
    }
    i64::try_from(cents)
        .ok()
        .and_then(|c| Money::from_cents(c).ok())
        .unwrap_or(Money::ZERO)
}
/// Every vendor (and its name) of the demo company.
fn vendors(state: &mut DemoState) -> Vec<(ContactId, String)> {
    use cl_api::ContactType;
    state
        .contacts_mut()
        .iter()
        .filter(|c| {
            matches!(
                c.fields.contact_type,
                ContactType::Vendor | ContactType::Both
            )
        })
        .map(|c| (c.id, c.fields.display_name.clone()))
        .collect()
}

/// The open bills at the story date as (vendor name, bill, number, due
/// date, amount owed before the demo's payments): a sample bill is settled
/// by a bill payment to the same vendor for the same amount on or after
/// its date (the fixture pays bills in full, `Sample::bills_due`), and the
/// bills entered in the demo owe what the demo engine says. Void bills are
/// left out.
fn open_bills(
    state: &mut DemoState,
) -> Vec<(String, TransactionId, String, Option<CalendarDate>, Money)> {
    let sample = cl_company::Sample::load();
    let ap = sample.company.system_accounts.accounts_payable.clone();
    let today = DemoState::today();
    let mut out = Vec::new();
    let mut sample_ids = std::collections::BTreeSet::new();
    for t in &sample.transactions {
        let id = super::register::DemoLedger::id_of(t);
        sample_ids.insert(id);
        if t.kind != "bill" || t.date > today || state.ledger.is_void(id) {
            continue;
        }
        let Some(name) = t.contact.clone() else {
            continue;
        };
        let amount = Money::sum(t.lines.iter().filter(|l| l.account == ap).map(|l| l.credit))
            .unwrap_or(Money::ZERO);
        let settled = sample.transactions.iter().any(|p| {
            p.kind == "bill_payment"
                && p.contact == t.contact
                && p.date >= t.date
                && p.lines.first().is_some_and(|l| l.debit == amount)
        });
        if !settled && amount.is_positive() {
            out.push((name, id, t.number.clone(), t.due, amount));
        }
    }
    for (id, _) in super::bills::list(state, false) {
        if sample_ids.contains(&id) || state.ledger.is_void(id) {
            continue;
        }
        let Some(v) = super::bills::get(state, id) else {
            continue;
        };
        let r = v.record;
        let owed = r.balance_due.unwrap_or(Money::ZERO);
        let name = r.txn.contact_name.clone().unwrap_or_default();
        if owed.is_positive() && r.bill_received {
            out.push((
                name,
                id,
                r.txn.txn_number.clone().unwrap_or_default(),
                r.due_date,
                owed,
            ));
        }
    }
    out
}

/// What the demo engine lists as payable (`GET /bills/payable`).
#[must_use]
pub fn payable(state: &mut DemoState, filter: &PayableFilter) -> PayableList {
    let mut bills = Vec::new();
    let all_vendors = vendors(state);
    let vendor_of = |name: &str, state: &DemoState| {
        all_vendors
            .iter()
            .find(|(_, n)| n == name)
            .map(|(id, _)| *id)
            .or_else(|| {
                state
                    .contact_names
                    .iter()
                    .find(|(_, n)| n.as_str() == name)
                    .map(|(id, _)| *id)
            })
    };
    for (name, id, number, due, owed) in open_bills(state) {
        let Some(vendor) = vendor_of(&name, state) else {
            continue;
        };
        if filter.vendor.is_some_and(|v| v != vendor) {
            continue;
        }
        let shown = all_vendors
            .iter()
            .find(|(v, _)| *v == vendor)
            .map_or(name.clone(), |(_, n)| n.clone());
        if let Some(b) = row(state, filter, vendor, &shown, id, Some(&number), due, owed) {
            bills.push(b);
        }
    }
    bills.sort_by(|a, b| {
        (a.due_date.is_none(), a.due_date, a.id).cmp(&(b.due_date.is_none(), b.due_date, b.id))
    });
    let mut credits = Vec::new();
    let (name, label, amount) = STORY_CREDIT;
    let vendor = all_vendors
        .into_iter()
        .find(|(_, n)| n == name)
        .map(|(id, _)| id);
    let used = state
        .pay_bills
        .credit_used
        .get(&story_credit_id())
        .copied()
        .unwrap_or(Money::ZERO);
    let left = m(amount).checked_sub(used).unwrap_or(Money::ZERO);
    if let Some(v) = vendor
        && left.is_positive()
        && bills.iter().any(|b| b.contact_id == Some(v))
    {
        credits.push(PayableCredit {
            id: story_credit_id(),
            number: Some(label.to_string()),
            vendor_invoice_number: Some(label.to_string()),
            contact_id: Some(v),
            txn_date: CalendarDate::from_ymd(2026, 10, 20).unwrap_or_else(|_| DemoState::today()),
            total: Some(m(amount)),
            balance_due: left,
            memo: None,
        });
    }
    PayableList { bills, credits }
}

#[allow(clippy::too_many_arguments)]
fn row(
    state: &DemoState,
    filter: &PayableFilter,
    vendor: ContactId,
    name: &str,
    id: TransactionId,
    number: Option<&str>,
    due: Option<CalendarDate>,
    owed: Money,
) -> Option<PayableBill> {
    let applied = state
        .pay_bills
        .applied
        .get(&id)
        .copied()
        .unwrap_or(Money::ZERO);
    let balance = owed.checked_sub(applied).ok()?;
    if !balance.is_positive() {
        return None;
    }
    if let Some(through) = filter.due_on_or_before
        && due.is_none_or(|d| d > through)
    {
        return None;
    }
    let story = STORY
        .iter()
        .find(|s| s.0 == name && m(s.1) == owed)
        .copied();
    let discount_date = story
        .and_then(|s| s.5)
        .and_then(|(mo, d)| CalendarDate::from_ymd(2026, mo, d).ok());
    let percent = story.and_then(|s| s.6);
    let discount_available = match (percent, discount_date, filter.payment_date) {
        (Some(p), Some(dd), Some(pay)) if pay <= dd => discount_of(balance, p),
        _ => Money::ZERO,
    };
    serde_json::from_value(json!({
        "id": id,
        "txnNumber": number,
        "contactId": vendor,
        "contactName": name,
        "txnDate": state.ledger.date_of(id).unwrap_or_else(DemoState::today).to_wire(),
        "dueDate": due.map(CalendarDate::to_wire),
        "discountDate": discount_date.map(CalendarDate::to_wire),
        "discountPercent": percent,
        "vendorInvoiceNumber": story.map(|s| s.2),
        "total": owed.to_wire(),
        "balanceDue": balance.to_wire(),
        "billStatus": if applied.is_positive() { "partial" } else { "unpaid" },
        "paymentTerms": story.map(|s| s.4),
        "memo": story.and_then(|s| s.3),
        "daysOverdue": 0,
        "discountAvailable": discount_available.to_wire(),
        "updatedAt": DemoState::now().to_wire(),
    }))
    .ok()
}

/// The demo payment's id for its ledger journal.
fn ledger_payment(state: &mut DemoState, j: &DemoJournal) -> Option<TransactionId> {
    let chart = chart(state);
    state.ledger.save_lines(&chart, j, None)
}

fn number_used(state: &mut DemoState, account: AccountId, n: u64) -> bool {
    let chart = chart(state);
    state
        .ledger
        .check_numbers(&chart, account)
        .iter()
        .any(|(_, num, void)| u64::from(*num) == n && !*void)
}

/// `POST /bill-payments` as the demo engine takes it.
///
/// # Errors
/// `ClosedPeriod` / `ClosingPasswordRejected`; 400 "exceeds balance due"
/// (no code); 400 `DISCOUNT_ACCOUNT_REQUIRED`; 409 `CHECK_NUMBER_IN_USE`.
pub fn pay(
    state: &mut DemoState,
    req: &PayBillsRequest,
    password: Option<&Secret>,
) -> Result<Vec<CreatedPayment>, ApiError> {
    state.pay_bills.writes += 1;
    check_closing(closing(state), &[req.date], password)?;
    let listed = payable(
        state,
        &PayableFilter {
            payment_date: Some(req.date),
            ..PayableFilter::default()
        },
    );
    // Check everything before posting anything.
    let mut credit_left: BTreeMap<TransactionId, Money> = listed
        .credits
        .iter()
        .map(|c| (c.id, c.balance_due))
        .collect();
    for c in &req.credits {
        let left = credit_left.get_mut(&c.credit).ok_or_else(invalid)?;
        *left = left.checked_sub(c.amount).map_err(|_| invalid())?;
        if left.is_negative() {
            return Err(refusal(400, "", "The credit has less available"));
        }
    }
    let mut groups: Vec<(ContactId, Vec<&cl_api::BillToPay>)> = Vec::new();
    for b in &req.bills {
        let row = listed
            .bills
            .iter()
            .find(|r| r.id == b.bill)
            .ok_or_else(|| refusal(400, "", "One or more bills not found"))?;
        let credits = Money::sum(
            req.credits
                .iter()
                .filter(|c| c.bill == b.bill)
                .map(|c| c.amount),
        )
        .map_err(|_| invalid())?;
        let covered = Money::sum([b.cash, credits, b.discount]).map_err(|_| invalid())?;
        if covered > row.balance_due {
            return Err(refusal(400, "", "Payment amount exceeds balance due"));
        }
        let vendor = row.contact_id.ok_or_else(invalid)?;
        match groups.iter_mut().find(|(v, _)| *v == vendor) {
            Some((_, list)) => list.push(b),
            None => groups.push((vendor, vec![b])),
        }
    }
    let any_discount = req.bills.iter().any(|b| b.discount.is_positive());
    let discount_account = req.discount_account.or(state.pay_bills.discount_account);
    if any_discount && discount_account.is_none() {
        return Err(refusal(
            400,
            cl_api::DISCOUNT_ACCOUNT_REQUIRED,
            "Choose an account for discounts",
        ));
    }
    let (card, mut next) = match &req.method {
        PayMethod::Check(CheckNumbering::Assign { first }) => (false, Some(*first)),
        PayMethod::Check(CheckNumbering::PrintLater) => (false, None),
        PayMethod::CreditCard { .. } => (true, None),
    };
    if let Some(first) = next {
        let count = u64::try_from(groups.len()).unwrap_or(u64::MAX);
        for n in first..first.saturating_add(count) {
            if number_used(state, req.bank_account, n) {
                return Err(cl_api::ApiError::Conflict {
                    reason: cl_api::ConflictReason::Unspecified,
                    current_version: None,
                    code: cl_api::ErrorCode::parse(cl_api::CHECK_NUMBER_IN_USE),
                    message: cl_api::UserText::default(),
                });
            }
        }
    }
    let chart = chart(state);
    let ap = chart
        .iter()
        .find(|a| a.system_tag.as_deref() == Some("accounts_payable"))
        .map(|a| a.id)
        .ok_or_else(invalid)?;
    let mut out = Vec::new();
    for (vendor, bills) in groups {
        let cash = Money::sum(bills.iter().map(|b| b.cash)).map_err(|_| invalid())?;
        let discount = Money::sum(bills.iter().map(|b| b.discount)).map_err(|_| invalid())?;
        let mut lines = vec![DemoLine {
            account: ap,
            debit: cash.checked_add(discount).map_err(|_| invalid())?,
            credit: Money::ZERO,
            memo: None,
        }];
        if cash.is_positive() {
            lines.push(DemoLine {
                account: req.bank_account,
                debit: Money::ZERO,
                credit: cash,
                memo: None,
            });
        }
        if let (true, Some(d)) = (discount.is_positive(), discount_account) {
            lines.push(DemoLine {
                account: d,
                debit: Money::ZERO,
                credit: discount,
                memo: None,
            });
        }
        // A vendor paid only from credits gets no check number.
        let check_number = if cash.is_positive() {
            let n = next;
            next = next.map(|x| x.saturating_add(1));
            n
        } else {
            None
        };
        let name = state
            .contacts_mut()
            .iter()
            .find(|c| c.id == vendor)
            .map(|c| c.fields.display_name.clone());
        let journal = DemoJournal {
            kind: "bill_payment",
            date: req.date,
            number: check_number.map(|n| n.to_string()).unwrap_or_default(),
            contact: name,
            memo: req.memo.clone().unwrap_or_default(),
            lines,
        };
        let id = ledger_payment(state, &journal).ok_or_else(invalid)?;
        let mut credits = Vec::new();
        let mut paid = Vec::new();
        for b in &bills {
            let cr: Vec<_> = req
                .credits
                .iter()
                .filter(|c| c.bill == b.bill)
                .map(|c| (c.credit, c.bill, c.amount))
                .collect();
            let credit = Money::sum(cr.iter().map(|c| c.2)).map_err(|_| invalid())?;
            let covered = Money::sum([b.cash, credit, b.discount]).map_err(|_| invalid())?;
            let e = state.pay_bills.applied.entry(b.bill).or_insert(Money::ZERO);
            *e = e.checked_add(covered).map_err(|_| invalid())?;
            for c in &cr {
                let u = state
                    .pay_bills
                    .credit_used
                    .entry(c.0)
                    .or_insert(Money::ZERO);
                *u = u.checked_add(c.2).map_err(|_| invalid())?;
            }
            paid.push((b.bill, b.cash, b.discount));
            credits.extend(cr);
        }
        let print_later = !card && check_number.is_none() && cash.is_positive();
        state.pay_bills.payments.push(DemoPayment {
            id,
            vendor,
            date: req.date,
            cash,
            check_number,
            print_later,
            bills: paid.clone(),
            credits: credits.clone(),
        });
        out.push(CreatedPayment {
            id,
            contact_id: Some(vendor),
            txn_date: req.date,
            check_number,
            print_status: if check_number.is_some() {
                Some("hand_written".into())
            } else if print_later {
                Some("queue".into())
            } else {
                None
            },
            net_payment: cash,
            discount_total: Some(discount),
            bills_paid: u32::try_from(paid.len()).unwrap_or(u32::MAX),
            credits_applied: u32::try_from(credits.len()).unwrap_or(u32::MAX),
        });
    }
    Ok(out)
}

/// `GET /bill-payments/{id}`.
#[must_use]
pub fn get(state: &DemoState, id: TransactionId) -> Option<BillPaymentRecord> {
    let p = state.pay_bills.payments.iter().find(|p| p.id == id)?;
    Some(BillPaymentRecord {
        id: p.id,
        txn_date: p.date,
        contact_id: Some(p.vendor),
        total: Some(p.cash),
        check_number: p.check_number,
        print_status: p.print_later.then(|| "queue".to_string()),
        status: if state.ledger.is_void(id) {
            cl_api::TxnStatus::Void
        } else {
            cl_api::TxnStatus::Posted
        },
        memo: None,
        updated_at: Some(DemoState::now()),
        bill_applications: p
            .bills
            .iter()
            .map(|(bill, cash, discount)| BillApplication {
                bill_id: *bill,
                amount: *cash,
                discount_amount: Some(*discount),
            })
            .collect(),
        credit_applications: p
            .credits
            .iter()
            .map(|(credit, bill, amount)| CreditApplication {
                credit_id: *credit,
                bill_id: *bill,
                amount: *amount,
            })
            .collect(),
    })
}

/// The payment's bank or card account (its credit line), read from the
/// ledger.
#[must_use]
pub fn account_of(state: &mut DemoState, id: TransactionId) -> Option<AccountId> {
    let chart = chart(state);
    let contacts = state.contacts_mut().clone();
    let txn = state.ledger.get(&chart, &contacts, id)?.record;
    let ap = chart
        .iter()
        .find(|a| a.system_tag.as_deref() == Some("accounts_payable"))
        .map(|a| a.id);
    txn.lines
        .iter()
        .find(|l| l.credit.is_positive() && Some(l.account_id) != ap)
        .map(|l| l.account_id)
}

/// `POST /bill-payments/{id}/void`: the payment comes off the bills and
/// credits it paid.
///
/// # Errors
/// `NotFound`; the closing-date rule.
pub fn void(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    let p = state
        .pay_bills
        .payments
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })?;
    if state.ledger.is_void(id) {
        return Err(refusal(400, "", "Bill payment is already void"));
    }
    check_closing(closing(state), &[p.date], password)?;
    state.ledger.void(id)?;
    for (bill, cash, discount) in &p.bills {
        let credit = Money::sum(p.credits.iter().filter(|c| c.1 == *bill).map(|c| c.2))
            .unwrap_or(Money::ZERO);
        let back = Money::sum([*cash, *discount, credit]).unwrap_or(Money::ZERO);
        if let Some(e) = state.pay_bills.applied.get_mut(bill) {
            *e = e.checked_sub(back).unwrap_or(Money::ZERO);
        }
    }
    for (credit, _, amount) in &p.credits {
        if let Some(u) = state.pay_bills.credit_used.get_mut(credit) {
            *u = u.checked_sub(*amount).unwrap_or(Money::ZERO);
        }
    }
    Ok(())
}

/// The developer command `dev.demo_pay_bills_reset` (the control-channel
/// scenario runs light and dark in one app): every demo payment is voided
/// and the default discount account forgotten.
pub fn reset(state: &mut DemoState) {
    let ids: Vec<TransactionId> = state.pay_bills.payments.iter().map(|p| p.id).collect();
    let pw = Secret::new(super::register::DEMO_CLOSING_PASSWORD);
    for id in ids {
        if !state.ledger.is_void(id) {
            let _ = void(state, id, Some(&pw));
        }
    }
    state.pay_bills.discount_account = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mockup's list, figures and run through the demo engine.
    #[test]
    fn the_demo_lists_and_pays_the_mockup_bills() {
        let mut s = DemoState::new();
        let filter = PayableFilter {
            due_on_or_before: CalendarDate::from_ymd(2026, 11, 11).ok(),
            payment_date: Some(DemoState::today()),
            ..PayableFilter::default()
        };
        let list = payable(&mut s, &filter);
        let refs: Vec<_> = list
            .bills
            .iter()
            .map(|b| b.vendor_invoice_number.clone().unwrap_or_default())
            .collect();
        assert_eq!(refs, ["20931", "STMT-1031", "2214", "7781", "8813"]);
        let due = Money::sum(list.bills.iter().map(|b| b.balance_due)).expect("sum");
        assert_eq!(due, m("7471.90"));
        let office = &list.bills[3];
        assert_eq!(office.discount_available, Some(m("4.37")));
        assert_eq!(list.credits.len(), 1);
        assert_eq!(list.credits[0].balance_due, m("250.00"));
        // After the discount date: no discount.
        let late = payable(
            &mut s,
            &PayableFilter {
                payment_date: CalendarDate::from_ymd(2026, 11, 7).ok(),
                ..filter
            },
        );
        assert_eq!(late.bills[3].discount_available, Some(Money::ZERO));
        // The run: 1010 falls by exactly the cash.
        let bank = super::super::account_id(1010);
        let before = super::super::checks::balance(&mut s, bank).expect("balance");
        assert_eq!(before, m("84210.55"));
        let pick = |i: usize, cash: &str, disc: &str| cl_api::BillToPay {
            bill: list.bills[i].id,
            cash: m(cash),
            discount: m(disc),
        };
        let req = PayBillsRequest {
            bank_account: bank,
            date: DemoState::today(),
            bills: vec![
                pick(0, "1286.40", "0"),
                pick(1, "1412.06", "0"),
                pick(3, "214.07", "4.37"),
                pick(4, "3665.00", "0"),
            ],
            credits: vec![cl_api::CreditToApply {
                credit: story_credit_id(),
                bill: list.bills[4].id,
                amount: m("250.00"),
            }],
            discount_account: Some(super::super::account_id(5200)),
            method: PayMethod::Check(CheckNumbering::PrintLater),
            memo: None,
        };
        let paid = pay(&mut s, &req, None).expect("paid");
        assert_eq!(paid.len(), 4, "one payment per vendor");
        let after = super::super::checks::balance(&mut s, bank).expect("balance");
        assert_eq!(after, m("77633.02"));
        let left = payable(&mut s, &filter);
        assert_eq!(left.bills.len(), 1, "only 2214 is left");
        assert!(left.credits.is_empty(), "CM-114 is used up");
        // Without a discount account the engine refuses.
        let mut s2 = DemoState::new();
        let l2 = payable(&mut s2, &filter);
        let r2 = PayBillsRequest {
            bills: vec![cl_api::BillToPay {
                bill: l2.bills[3].id,
                cash: m("214.07"),
                discount: m("4.37"),
            }],
            credits: Vec::new(),
            discount_account: None,
            ..req
        };
        let e = pay(&mut s2, &r2, None).expect_err("no account");
        assert_eq!(
            cl_api::pay_problem(&e),
            Some(cl_api::PayProblem::DiscountAccountRequired)
        );
    }

    #[test]
    fn the_discount_is_worked_out_the_engine_way() {
        assert_eq!(discount_of(m("218.44"), "2"), m("4.37"));
        // 0.5 cent rounds away from zero.
        assert_eq!(discount_of(m("0.25"), "2"), m("0.01"));
        assert_eq!(discount_of(m("100.00"), "x"), Money::ZERO);
    }
}
