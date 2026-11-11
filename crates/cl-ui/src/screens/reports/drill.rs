//! Drill-down (plan §4): a list row opens its form through the existing
//! router (closed periods go through the form's own password flow); an
//! amount opens the lines behind it, and an aging cell the detail for that
//! contact and bucket, each in a new report tab.

use cl_core::report::{DateRange, Drill, Source};
use cl_model::CommandId;
use cl_model::reports::{DateChoice, ReportOptions};

use super::window::ReportWindow;
use super::{KIND, ReportOpen};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::windows::OpenPayload;

/// The form that shows a transaction of the engine's type `txn_type`.
/// Checks, card charges, owner draws, loan payments and the deposit-like
/// forms share the engine's `expense` and `deposit` types: those are read
/// first ([`Target::Probe`], QA-1).
#[must_use]
pub(crate) fn form_of(txn_type: &str) -> Option<&'static str> {
    Some(match txn_type {
        "invoice" => "clients.create_invoice",
        "credit_memo" => "clients.credit_memo",
        "cash_sale" => "clients.sales_receipt",
        "customer_payment" => "clients.receive_payment",
        "bill" | "vendor_credit" => "vendors.enter_bill",
        "bill_payment" => "vendors.bill_payment",
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
    /// An `expense` or `deposit`: which form shows it depends on the
    /// transaction (a check, a card charge, an owner draw...), so it is
    /// read first (QA-1).
    Probe(cl_core::ids::TransactionId),
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
            None if matches!(txn_type.as_str(), "expense" | "deposit") => Target::Probe(*id),
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
            o.filters.set_contact_scope(contact);
            o.filters.adjusting = *adjusting;
            o.filters.set_tag_scope(*tag);
            Target::Report(Box::new(ReportOpen {
                key: spec.key.to_string(),
                options: Some(o),
            }))
        }
        Drill::ProfitLoss {
            from,
            to,
            basis,
            adjusting,
            tag_ids,
        } => {
            let mut open = profit_loss_for(DateRange {
                from: *from,
                to: *to,
            });
            // The sheet's basis, adjusting lens and tags (S3).
            if let Some(o) = open.options.as_mut() {
                o.basis = basis.or(o.basis);
                o.filters.adjusting = *adjusting;
                o.filters.tag_ids.clone_from(tag_ids);
            }
            Target::Report(Box::new(open))
        }
        Drill::EntriesOfTypes {
            account_ids,
            txn_types,
            to,
            basis,
        } => {
            let Some(spec) = cl_model::reports::find("transaction_detail_by_account") else {
                return Target::Nothing("reports.drill.no_form");
            };
            let mut o = ReportOptions::for_spec(spec);
            o.dates = DateChoice::Fixed {
                from: cl_core::CalendarDate::min(),
                to: *to,
            };
            o.basis = basis.or(o.basis);
            o.filters.account_ids.clone_from(account_ids);
            o.filters.txn_types.clone_from(txn_types);
            Target::Report(Box::new(ReportOpen {
                key: spec.key.to_string(),
                options: Some(o),
            }))
        }
        Drill::Aging {
            contact,
            as_of,
            bucket,
        } => {
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
            // The "(no client)" row opens only the rows without one.
            o.filters.no_contact = contact.is_none();
            o.filters.bucket = Some(*bucket).filter(|b| *b != cl_core::report::AgingBucket::Total);
            Target::Report(Box::new(ReportOpen {
                key: key.to_string(),
                options: Some(o),
            }))
        }
    }
}

/// True when clicking `drill` shows something now: a form, or a report the
/// engine can run. Amounts with a dead-end drill are drawn without a link.
pub(crate) fn live(link: &crate::company::CompanyLink, source: Source, drill: &Drill) -> bool {
    match target(source, drill) {
        Target::Form(..) | Target::Probe(_) => true,
        Target::Report(open) => {
            cl_model::reports::find(&open.key).is_some_and(|s| super::available(link, s))
        }
        // A subtotal over too many accounts stays a link: the click says
        // to narrow the report (review nit).
        Target::Nothing(key) => key == "reports.drill.narrow",
    }
}

/// Opens what `drill` points at.
pub(super) fn open(w: &mut ReportWindow, drill: &Drill) {
    w.notice = None;
    match target(w.spec.source, drill) {
        Target::Form(form, id) => w
            .link
            .push(CompanyEvent::OpenRecord(CommandId::new(form), id)),
        Target::Probe(id) => probe(w, id),
        Target::Report(open) => w.link.push(CompanyEvent::OpenWith(
            CommandId::new(KIND),
            Box::new(OpenPayload::Report(open)),
        )),
        Target::Nothing(key) => w.notice = Some(t(key)),
    }
}

/// Where an `expense` or `deposit` opens (QA-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opens {
    /// Its form.
    Form(&'static str),
    /// No form owns it: its register, where it opens in place.
    Register(cl_core::ids::AccountId),
    /// Nothing shows it.
    Nothing,
}

/// The register's routing (P14): a posted check opens in Write Check, any
/// other entry in the form that owns its shape, else its register. A
/// template-posted expense that isn't a check never opens a blank check.
#[must_use]
pub(crate) fn route(
    t: &cl_api::Transaction,
    is_check: bool,
    accounts: &[cl_api::Account],
) -> Opens {
    use crate::screens::register::{class_of, form_for};
    if is_check && t.status == cl_api::TxnStatus::Posted {
        return Opens::Form(crate::screens::write_check::KIND);
    }
    let class =
        |id: cl_core::ids::AccountId| accounts.iter().find(|a| a.id == id).and_then(class_of);
    if let Some(f) = form_for(t, &class) {
        return Opens::Form(f);
    }
    t.lines
        .iter()
        .map(|l| l.account_id)
        .find(|a| class(*a).is_some())
        .map_or(Opens::Nothing, Opens::Register)
}

/// Reads the transaction behind a drill, then opens it where it belongs.
fn probe(w: &mut ReportWindow, id: cl_core::ids::TransactionId) {
    use crate::screens::invoice::engine;
    w.drill_error = None;
    let job = engine::read(
        &w.link,
        w.group.as_ref(),
        move |d| {
            let chart = d
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let contacts = d.contacts_mut().clone();
            let v = d
                .ledger
                .get(&chart, &contacts, id)
                .ok_or(cl_api::ApiError::NotFound { code: None })?;
            let check = v.record.txn_type == cl_api::TxnType::Expense
                && (d.checks.knows(id) || v.record.check_number.is_some());
            Ok(route(&v.record, check, &chart))
        },
        move |s| async move {
            let v = s.transactions().get(id).await?;
            // A check has a print status: only the check read tells.
            let check = v.record.txn_type == cl_api::TxnType::Expense
                && match s.checks().get(id).await {
                    Ok(_) => true,
                    Err(cl_api::ApiError::Decode { .. }) => false,
                    Err(e) => return Err(e),
                };
            let accounts = s
                .accounts()
                .list_all(&cl_api::AccountFilter::default())
                .await?;
            Ok(route(&v.record, check, &accounts))
        },
    );
    w.drill_id = Some(id);
    w.drill_job = Some(job);
    poll_probe(w);
}

/// Opens what a finished read found.
pub(super) fn poll_probe(w: &mut ReportWindow) {
    let Some(r) = crate::screens::invoice::engine::poll(&mut w.drill_job) else {
        return;
    };
    let Some(id) = w.drill_id.take() else {
        return;
    };
    match r {
        Ok(Opens::Form(f)) => w.link.push(CompanyEvent::OpenRecord(CommandId::new(f), id)),
        Ok(Opens::Register(a)) => {
            w.notice = Some(t("reports.drill.in_register"));
            w.link.push(CompanyEvent::OpenRegister(a));
        }
        Ok(Opens::Nothing) => w.notice = Some(t("reports.drill.no_form")),
        Err(e) => w.drill_error = Some(e),
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

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::{Transaction, TransactionLine, TxnStatus, TxnType};
    use cl_core::Money;
    use cl_core::ids::{AccountId, JournalLineId, TransactionId};

    fn line(i: u32, account: AccountId, debit: &str, credit: &str) -> TransactionLine {
        TransactionLine {
            id: JournalLineId::parse(&format!("d0000000-0000-4000-8000-c{i:011}")).expect("id"),
            account_id: account,
            account_name: None,
            account_number: None,
            debit: Money::parse_wire(debit).expect("money"),
            credit: Money::parse_wire(credit).expect("money"),
            description: None,
            line_order: Some(i64::from(i)),
            tag_id: None,
            tag_name: None,
            contact_id: None,
            contact_name: None,
            is_billable: false,
            billed_status: None,
            source_transaction_id: None,
            line_role: None,
            item_id: None,
            quantity: None,
            unit_price: None,
            unit_id: None,
        }
    }

    fn expense(lines: Vec<TransactionLine>) -> Transaction {
        Transaction {
            id: TransactionId::parse("d0000000-0000-4000-8000-b00000000001").expect("id"),
            txn_type: TxnType::Expense,
            txn_number: None,
            check_number: None,
            txn_date: cl_core::CalendarDate::from_ymd(2026, 10, 15).expect("date"),
            status: TxnStatus::Posted,
            basis: cl_api::Basis::Both,
            contact_id: None,
            contact_name: None,
            memo: None,
            total: None,
            void_reason: None,
            created_at: None,
            updated_at: None,
            lines,
            is_adjusting: false,
            reverses_transaction_id: None,
            reversal_transaction_id: None,
            reverse_on: None,
            inventory_lines: Vec::new(),
        }
    }

    /// QA-1: an expense a template posted (no print status, so not a
    /// check) opens in its register, never as a blank Write Check; a check
    /// opens in Write Check; a card charge in Card Charge.
    #[test]
    fn expenses_open_where_they_belong() {
        let chart = crate::company::demo::chart();
        let bank = crate::company::demo::account_id(1010);
        let cost = crate::company::demo::account_id(6100);
        let t = expense(vec![
            line(0, cost, "49.00", "0"),
            line(1, bank, "0", "49.00"),
        ]);
        assert_eq!(route(&t, false, &chart), Opens::Register(bank));
        assert_eq!(
            route(&t, true, &chart),
            Opens::Form(crate::screens::write_check::KIND)
        );
        let card = chart
            .iter()
            .find(|a| {
                crate::screens::register::class_of(a)
                    == Some(cl_model::register::AccountClass::CreditCard)
            })
            .map(|a| a.id)
            .expect("the demo has a card");
        let c = expense(vec![
            line(0, cost, "12.00", "0"),
            line(1, card, "0", "12.00"),
        ]);
        assert_eq!(
            route(&c, false, &chart),
            Opens::Form(crate::screens::card_charge::KIND)
        );
        assert!(matches!(
            target(
                Source::GeneralLedger,
                &Drill::Transaction {
                    id: t.id,
                    txn_type: "expense".into()
                }
            ),
            Target::Probe(_)
        ));
    }
}
