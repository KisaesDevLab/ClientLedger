//! Who answers Receive Payment: the live engine on the window's request
//! group or the `--demo` engine (`company::demo::receivables`), through the
//! invoice screen's [`Job`] helpers. Also the one place the API's and the
//! form model's types meet: the model's request becomes the API's, and the
//! engine's preview becomes the model's figures (copied, never computed).

use cl_api::{
    ApiError, ApplicationRequest, CreditRequest, NewCustomerPayment, OrphanLabel, PaymentPreview,
    PaymentPreviewRequest, PostingOptions, SaveApplication, SaveCredit,
};
use cl_core::Secret;
use cl_model::forms::payment::{
    FigureCredit, FigureCreditUse, FigureRow, FigureShortfall, PaymentFigures, PaymentRequest,
    PaymentSave,
};

pub(crate) use crate::screens::invoice::engine::{Job, poll, read, write};

/// The model's request as the API's.
#[must_use]
pub(crate) fn preview_request(r: &PaymentRequest) -> PaymentPreviewRequest {
    PaymentPreviewRequest {
        customer_id: r.client,
        date: r.date,
        amount: r.amount,
        deposit_to: r.deposit_to,
        applications: r.rows.as_ref().map(|rows| {
            rows.iter()
                .map(|x| ApplicationRequest {
                    invoice_id: x.invoice,
                    amount: x.amount,
                    discount_amount: x.discount,
                    write_off_amount: x.write_off,
                })
                .collect()
        }),
        credit_applications: r
            .credits
            .iter()
            .map(|c| CreditRequest {
                source_payment_id: c.source,
                invoice_id: c.invoice,
                amount: c.amount,
            })
            .collect(),
        hold_as_deposit: r.hold,
        editing_payment_id: r.editing,
    }
}

/// The engine's preview as the model's figures.
#[must_use]
pub(crate) fn figures_of(p: &PaymentPreview) -> PaymentFigures {
    let mut credits: Vec<FigureCredit> = p
        .credits
        .iter()
        .map(|c| FigureCredit {
            source: c.source_payment_id,
            date: c.date,
            ref_no: c.ref_no.clone(),
            remaining: c.remaining,
            deposit: false,
        })
        .collect();
    credits.extend(p.deposits_on_hand.iter().map(|d| FigureCredit {
        source: d.payment_id,
        date: d.date,
        ref_no: d.ref_no.clone(),
        remaining: d.remaining,
        deposit: true,
    }));
    PaymentFigures {
        balance_before: p.contact_balance_before,
        balance_after: p.contact_balance_after,
        amount: p.amount,
        amount_from_rows: p.amount_source == cl_api::AmountSource::FromApplications,
        rows: p
            .invoices
            .iter()
            .map(|i| FigureRow {
                invoice: i.invoice_id,
                number: i.invoice_number.clone(),
                date: i.txn_date,
                due: i.due_date,
                original: i.total,
                balance: i.balance_due,
                token: i.balance_due_exact.clone(),
                payment: i.payment,
                remaining: i.remaining,
                discount: i.discount,
                write_off: i.write_off,
                credit: i.credit,
                discount_available: i.discount_available,
                discount_date: i.discount_date,
                discount_percent: i.discount_percent.clone(),
                suggest_discount: i.suggest_discount,
            })
            .collect(),
        amount_due: p.totals.amount_due,
        applied: p.totals.applied,
        discounts_and_credits: p.totals.discounts_and_credits,
        unapplied: p.totals.unapplied,
        write_offs: p.totals.write_offs,
        shortfalls: p
            .shortfalls
            .iter()
            .map(|s| FigureShortfall {
                invoice: s.invoice_id,
                amount: s.amount,
                within_limit: s.within_write_off_limit,
            })
            .collect(),
        overpayment: p.overpayment,
        credits,
        credit_uses: p
            .credit_applications
            .iter()
            .map(|c| FigureCreditUse {
                source: c.source_payment_id,
                invoice: c.invoice_id,
                amount: c.amount,
            })
            .collect(),
        hold_offered: p.hold_as_deposit_offered,
        pending: p.pending_deposits.as_ref().map(|d| (d.count, d.total)),
        average_days: p.average_days_to_pay,
        problems: p
            .problems
            .iter()
            .map(|x| (x.code.clone(), x.invoice_id))
            .collect(),
    }
}

/// The model's save as the API's payment.
///
/// # Errors
/// `Decode` for a bank-file id that isn't one.
pub(crate) fn new_payment(s: &PaymentSave) -> Result<NewCustomerPayment, ApiError> {
    Ok(NewCustomerPayment {
        customer_id: s.client,
        date: s.date,
        amount: s.amount,
        deposit_to: s.deposit_to,
        payment_method: s.method.clone(),
        ref_no: s.ref_no.clone(),
        memo: s.memo.clone(),
        applications: s
            .rows
            .iter()
            .map(|r| SaveApplication {
                invoice_id: r.invoice,
                amount: r.amount,
                expected_balance_due: r.token.clone(),
                discount_amount: r.discount,
                write_off_amount: r.write_off,
            })
            .collect(),
        credit_applications: s
            .credits
            .iter()
            .map(|c| SaveCredit {
                source_payment_id: c.source,
                invoice_id: c.invoice,
                amount: c.amount,
                expected_balance_due: c.token.clone(),
            })
            .collect(),
        write_off_account_id: s.write_off_account,
        discount_account_id: s.discount_account,
        hold_as_deposit: s.hold,
        bank_feed_item_id: s
            .feed_item
            .as_deref()
            .map(cl_api::FeedItemId::parse)
            .transpose()?,
    })
}

/// The posting options of a write with the closing-date password.
#[must_use]
pub(crate) fn options(password: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: password.map(cl_api::ClosedPeriodAuthorization::with_password),
    }
}

/// The orphan-log label of a payment write (never a value).
#[must_use]
pub(crate) fn label(key: &cl_api::IdempotencyKey) -> OrphanLabel {
    OrphanLabel {
        what: "orphan.receive_payment",
        key: Some(key.clone()),
    }
}
