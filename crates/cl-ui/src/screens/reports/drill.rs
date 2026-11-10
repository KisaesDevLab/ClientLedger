//! Drill-down (plan §4): a list row opens its form through the existing
//! router (closed periods go through the form's own password flow); an
//! amount opens the lines behind it, and an aging cell the detail for that
//! contact and bucket, each in a new report tab.

use cl_core::report::{DateRange, Drill, Source, TagScope};
use cl_model::CommandId;
use cl_model::reports::{DateChoice, ReportOptions};

use super::window::ReportWindow;
use super::{KIND, ReportOpen};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::windows::OpenPayload;

/// The form that shows a transaction of the engine's type `txn_type`.
#[must_use]
pub(crate) fn form_of(txn_type: &str) -> Option<&'static str> {
    Some(match txn_type {
        "invoice" => "clients.create_invoice",
        "credit_memo" => "clients.credit_memo",
        "cash_sale" => "clients.sales_receipt",
        "customer_payment" => "clients.receive_payment",
        "bill" | "vendor_credit" => "vendors.enter_bill",
        "bill_payment" => "vendors.bill_payment",
        "expense" => "vendors.write_check",
        "deposit" => "banking.deposit",
        "transfer" => "banking.transfer",
        "journal_entry" => "company.journal_entry",
        _ => return None,
    })
}

/// What a drill opens: a form, or a report tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// A transaction in its form.
    Form(&'static str, cl_core::ids::TransactionId),
    /// A report on these options.
    Report(Box<ReportOpen>),
    /// Nothing can show it yet (the plain reason).
    Nothing(&'static str),
}

/// The target of `drill` clicked in a report of `source`.
#[must_use]
pub(crate) fn target(source: Source, drill: &Drill) -> Target {
    match drill {
        Drill::Transaction { id, txn_type } => match form_of(txn_type) {
            Some(f) => Target::Form(f, *id),
            None => Target::Nothing("reports.drill.no_form"),
        },
        Drill::Lines {
            account_ids,
            from,
            to,
            contact,
            tag,
            basis,
            adjusting,
        } => {
            if account_ids.len() > 500 {
                return Target::Nothing("reports.drill.narrow");
            }
            let Some(spec) = cl_model::reports::find("transaction_detail_by_account") else {
                return Target::Nothing("reports.drill.no_form");
            };
            let mut o = ReportOptions::for_spec(spec);
            // A balance runs from the first entry.
            let from = from.unwrap_or(cl_core::CalendarDate::min());
            o.dates = DateChoice::Fixed { from, to: *to };
            o.basis = basis.or(o.basis);
            o.filters.account_ids.clone_from(account_ids);
            o.filters.contact_ids = contact.iter().copied().collect();
            o.filters.adjusting = *adjusting;
            match tag {
                TagScope::Any => {}
                TagScope::Tag(t) => o.filters.tag_ids = vec![*t],
                // U-202's untagged scopes need the engine update.
                TagScope::Untagged | TagScope::UntaggedInGroup(_) => {
                    return Target::Nothing("reports.drill.untagged_later");
                }
            }
            Target::Report(Box::new(ReportOpen {
                key: spec.key.to_string(),
                options: Some(o),
            }))
        }
        Drill::Aging { contact, as_of, .. } => {
            let key = if source == Source::ApAgingSummary {
                "ap_aging_detail"
            } else {
                "ar_aging_detail"
            };
            let Some(spec) = cl_model::reports::find(key) else {
                return Target::Nothing("reports.drill.no_form");
            };
            let mut o = ReportOptions::for_spec(spec);
            o.dates = DateChoice::AsOf(*as_of);
            o.filters.contact_ids = contact.iter().copied().collect();
            Target::Report(Box::new(ReportOpen {
                key: key.to_string(),
                options: Some(o),
            }))
        }
    }
}

/// Opens what `drill` points at.
pub(super) fn open(w: &mut ReportWindow, drill: &Drill) {
    w.notice = None;
    match target(w.spec.source, drill) {
        Target::Form(form, id) => w
            .link
            .push(CompanyEvent::OpenRecord(CommandId::new(form), id)),
        Target::Report(open) => w.link.push(CompanyEvent::OpenWith(
            CommandId::new(KIND),
            Box::new(OpenPayload::Report(open)),
        )),
        Target::Nothing(key) => w.notice = Some(t(key)),
    }
}

/// The P&L of one fiscal period (Home's chart month, Q-P16h).
#[must_use]
pub(crate) fn profit_loss_for(period: DateRange) -> ReportOpen {
    let spec = cl_model::reports::find("profit_loss");
    let mut o = spec.map(ReportOptions::for_spec);
    if let Some(o) = o.as_mut() {
        o.dates = DateChoice::Fixed {
            from: period.from,
            to: period.to,
        };
    }
    ReportOpen {
        key: "profit_loss".into(),
        options: o,
    }
}
