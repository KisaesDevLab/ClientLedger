//! Pay Sales Tax (P23, U-150): the checked plan as the engine's request and
//! the engine's preview as the model's figures. Nothing is added up here:
//! every figure is the preview's, and posting sends each line's
//! `expectedOwed` as a byte copy.

use cl_api::{
    PayAdjustment, PayLine, PayPreview, PaySalesTaxRequest, SalesTaxPayMethod,
    SalesTaxPayProblemKind,
};
use cl_model::forms::sales_tax::pay::{
    PayFigures, PayLineFigure, PayMethod, PayPostPlan, PayRequestPlan,
};
use cl_model::forms::sales_tax::recovery::RecoveryPlan;

/// The engine's payment method of the form's.
#[must_use]
pub fn method(m: PayMethod) -> SalesTaxPayMethod {
    match m {
        PayMethod::Check => SalesTaxPayMethod::Check,
        PayMethod::Electronic => SalesTaxPayMethod::Electronic,
    }
}

/// The request the engine takes for a plan (`lines`: the posted lines).
#[must_use]
pub fn request(p: &PayRequestPlan, lines: Option<Vec<PayLine>>) -> PaySalesTaxRequest {
    PaySalesTaxRequest {
        agency_id: p.agency,
        through_date: p.through,
        date: p.date,
        bank_account_id: p.bank,
        method: method(p.method),
        lines: lines.or_else(|| {
            p.amounts.as_ref().map(|a| {
                a.iter()
                    .map(|(i, m)| PayLine {
                        tax_item_id: *i,
                        // `None`: what the item owes plus its adjustments,
                        // the engine's figure (review finding 5).
                        amount: *m,
                        expected_owed: None,
                    })
                    .collect()
            })
        }),
        adjustments: p
            .adjustments
            .iter()
            .map(|a| PayAdjustment {
                tax_item_id: a.item,
                account_id: a.account,
                amount: a.amount,
                reason: a.reason.clone(),
            })
            .collect(),
        print_later: p.print_later,
        check_number: p.check_number,
        memo: None,
        expected_owed: None,
    }
}

/// The preview as the model's figures (copied, never recomputed).
#[must_use]
pub fn figures_of(p: &PayPreview) -> PayFigures {
    PayFigures {
        lines: p
            .lines
            .iter()
            .map(|l| PayLineFigure {
                item: l.tax_item_id,
                owed: l.owed,
                adjustment: l.adjustment,
                amount: l.amount,
                expected_owed: l.expected_owed,
            })
            .collect(),
        total: p.total,
        owed: p.expected_owed_total(),
        legacy_unassigned: p
            .problems
            .iter()
            .any(|x| x.kind() == SalesTaxPayProblemKind::LegacyUnassigned),
        more_than_owed: p
            .problems
            .iter()
            .any(|x| x.kind() == SalesTaxPayProblemKind::AmountMoreThanOwed),
    }
}

/// The request that posts a previewed plan: each line with the preview's
/// owed figure, and the preview's owed total (engine PR #219), copied.
#[must_use]
pub fn post_request(plan: &PayPostPlan) -> PaySalesTaxRequest {
    let lines: Vec<PayLine> = plan
        .lines
        .iter()
        .map(|(i, a, e)| PayLine {
            tax_item_id: *i,
            amount: *a,
            expected_owed: Some(*e),
        })
        .collect();
    let mut r = request(&plan.request, Some(lines));
    r.expected_owed = Some(plan.expected_owed);
    r
}
/// A bad-debt recovery plan (P23) as the engine's request.
#[must_use]
pub fn recovery(plan: &RecoveryPlan) -> cl_api::NewRecovery {
    cl_api::NewRecovery {
        invoice_id: plan.invoice,
        date: plan.date,
        amount: plan.amount,
        memo: plan.memo.clone(),
    }
}
