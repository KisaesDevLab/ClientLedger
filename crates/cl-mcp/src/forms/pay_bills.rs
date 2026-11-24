//! Pay Bills (P18): the checked plan as the engine's one-transaction
//! request, field by field (the cash, discount and credit amounts are the
//! plan's, never recomputed).

use cl_api::{BillToPay, CreditToApply, PayBillsRequest, PayMethod};
use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use cl_model::forms::bill_payment::PayPlan;

/// The plan takes a discount, so it needs a discount account (Q-P18a).
#[must_use]
pub fn takes_discount(plan: &PayPlan) -> bool {
    plan.bills.iter().any(|b| b.discount.is_positive())
}

/// The checked plan paid from `bank_account` on `date` by `method`. The
/// discount account goes only when the plan takes a discount.
#[must_use]
pub fn request(
    plan: &PayPlan,
    bank_account: AccountId,
    date: CalendarDate,
    method: PayMethod,
    discount_account: Option<AccountId>,
) -> PayBillsRequest {
    let any_discount = takes_discount(plan);
    PayBillsRequest {
        bank_account,
        date,
        bills: plan
            .bills
            .iter()
            .map(|b| BillToPay {
                bill: b.bill,
                cash: b.cash,
                discount: b.discount,
            })
            .collect(),
        credits: plan
            .credits
            .iter()
            .map(|c| CreditToApply {
                credit: c.credit,
                bill: c.bill,
                amount: c.amount,
            })
            .collect(),
        discount_account: if any_discount { discount_account } else { None },
        method,
        memo: None,
    }
}
