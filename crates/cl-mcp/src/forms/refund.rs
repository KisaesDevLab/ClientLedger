//! Refund a Credit (P22 D5): the checked refund as the engine's request,
//! field by field.

use cl_api::{NewRefund, RefundCheck, RefundMethod, RefundSource};
use cl_model::forms::refund::{CheckPlan, RefundMethodChoice, RefundPlan, RefundSourceKind};
/// The engine's refund method.
#[must_use]
pub fn method_of(m: RefundMethodChoice) -> RefundMethod {
    match m {
        RefundMethodChoice::Check => RefundMethod::Check,
        RefundMethodChoice::Cash => RefundMethod::Cash,
        RefundMethodChoice::Card => RefundMethod::CreditCard,
        RefundMethodChoice::BankTransfer => RefundMethod::Ach,
        RefundMethodChoice::Other => RefundMethod::Other,
    }
}

/// The checked refund as the engine's request (field by field).
#[must_use]
pub fn new_refund(p: &RefundPlan) -> NewRefund {
    NewRefund {
        customer_id: p.client,
        date: p.date,
        from_account: p.from_account,
        amount: p.amount,
        source: match p.source.kind {
            RefundSourceKind::PaymentCredit => RefundSource::Payment(p.source.id),
            RefundSourceKind::CreditMemo => RefundSource::CreditMemo(p.source.id),
            RefundSourceKind::HeldDeposit => RefundSource::Deposit(p.source.id),
        },
        method: method_of(p.method),
        check: match &p.check {
            CheckPlan::NotACheck => RefundCheck::NotACheck,
            CheckPlan::PrintLater => RefundCheck::PrintLater,
            CheckPlan::Number(n) => RefundCheck::Number(n.clone()),
        },
        ref_no: None,
        memo: p.memo.clone(),
    }
}
