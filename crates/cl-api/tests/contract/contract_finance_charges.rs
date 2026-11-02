//! Finance charges against the real engine (P22 plan §3 D8, §6.1 golden
//! 6, §6.2 AC2, Q-P22a, Q-P22g, Q-P22p, Q-P22s).
//!
//! - `finance_charge_idempotent_per_period` (AC2): golden 6's figures
//!   (18 % a year, 10 grace days, 2.00 minimum; open credits unticked);
//!   the same key replays the same invoices, another key for the month is
//!   409 `FINANCE_CHARGE_ASSESSED`; the charge invoice can be edited and
//!   the month stays assessed; November charges from October's
//!   charged-through date; a backdated re-assessment charges nothing; a
//!   void frees the month and its days.
//! - `finance_charge_later_month_400`: a date in a later month is 400
//!   `FINANCE_CHARGE_DATE_IN_FUTURE` naming `assessmentDate`.
//! - `finance_charge_cross_company_404`.

use cl_api::{
    AccountType, AnnualRate, ApiError, AssessContact, AssessRequest, AssessResult, Capabilities,
    FinanceChargePreview, FinanceChargeRefusal, FinanceChargeRow, FinanceChargeSettingsUpdate,
    InvoiceLine, InvoiceWriteOptions, NewCreditMemo, NewInvoice, PostingOptions, Session,
    finance_charge_refusal,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity};

use crate::common::{account, date, key, m};
use crate::contract_credit_memos::{create, dated_invoice};
use crate::contract_invoices::customer;
use crate::contract_receive_payment::invoice_balance;
use crate::support::Harness;

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::FINANCE_CHARGES, "U-141")
        && h.require_cap(Capabilities::IDEMPOTENCY_FINANCE_CHARGES, "U-141")
        && crate::contract_credit_memos::caps(h)
}

fn oct31() -> CalendarDate {
    date(2026, 10, 31)
}

fn nov30() -> CalendarDate {
    date(2026, 11, 30)
}

/// Golden 6's settings: 18 % a year, 2.00 minimum, 10 grace days.
fn set_up(h: &Harness, s: &Session, compounding: bool) -> AccountId {
    let income = account(h, s, AccountType::Revenue, "Finance charges").id;
    h.run(s.set_finance_charge_settings(
        &FinanceChargeSettingsUpdate {
            annual_rate: Some(Some(AnnualRate::parse("18.00").expect("rate"))),
            minimum: Some(m("2.00")),
            grace_days: Some(10),
            on_finance_charges: Some(compounding),
            income_account_id: Some(Some(income)),
            print_later: Some(false),
        },
        key(),
    ))
    .expect("finance-charge settings");
    income
}

fn preview(h: &Harness, s: &Session, on: CalendarDate, only: &[ContactId]) -> FinanceChargePreview {
    h.run(s.finance_charges().preview(on, Some(only)))
        .unwrap_or_else(|e| panic!("finance-charge preview: {e}"))
}

fn row(p: &FinanceChargePreview, c: ContactId) -> Option<&FinanceChargeRow> {
    p.rows.iter().find(|r| r.contact_id == c)
}

fn charge_of(p: &FinanceChargePreview, c: ContactId) -> Money {
    row(p, c).map_or(Money::ZERO, |r| r.charge)
}

fn assess(
    h: &Harness,
    s: &Session,
    on: CalendarDate,
    c: ContactId,
    charge: Money,
    k: cl_api::IdempotencyKey,
) -> Result<AssessResult, ApiError> {
    h.run(s.finance_charges().assess(
        &AssessRequest {
            assessment_date: on,
            contacts: vec![AssessContact {
                contact_id: c,
                expected_charge: charge,
            }],
        },
        &PostingOptions::default(),
        k,
    ))
}

fn void_invoice(h: &Harness, s: &Session, id: TransactionId) {
    h.run(s.invoices().void(
        id,
        "contract: finance charge",
        &InvoiceWriteOptions::default(),
        key(),
    ))
    .unwrap_or_else(|e| panic!("void: {e}"));
}

struct Golden6 {
    prairie: ContactId,
    low_water: ContactId,
    cedar: ContactId,
}

fn golden6(h: &Harness, s: &Session) -> Golden6 {
    let income = account(h, s, AccountType::Revenue, "Kennel services").id;
    let prairie = customer(h, s, "Prairie Fork Kennels");
    dated_invoice(
        h,
        s,
        prairie,
        income,
        date(2026, 8, 31),
        date(2026, 9, 30),
        "1000.00",
    );
    dated_invoice(
        h,
        s,
        prairie,
        income,
        date(2026, 9, 15),
        date(2026, 10, 15),
        "50.00",
    );
    let low_water = customer(h, s, "Low Water Bait & Tackle");
    dated_invoice(
        h,
        s,
        low_water,
        income,
        date(2026, 9, 15),
        date(2026, 10, 15),
        "30.00",
    );
    let cedar = customer(h, s, "Cedar Gap Outfitters");
    dated_invoice(
        h,
        s,
        cedar,
        income,
        date(2026, 8, 31),
        date(2026, 9, 30),
        "600.00",
    );
    // A 400.00 credit memo kept as credit (Q-P22s).
    create(
        h,
        s,
        &NewCreditMemo::new(
            cedar,
            date(2026, 10, 1),
            vec![InvoiceLine {
                is_taxable: Some(false),
                ..InvoiceLine::account(income, Quantity::parse_wire("1").expect("qty"), m("400.00"))
            }],
        ),
    );
    Golden6 {
        prairie,
        low_water,
        cedar,
    }
}

/// AC2: assessed once per client and month.
pub fn finance_charge_idempotent_per_period(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    set_up(h, s, false);
    let g = golden6(h, s);
    let all = [g.prairie, g.low_water, g.cedar];
    // October: golden 6's rows.
    let p = preview(h, s, oct31(), &all);
    assert!(p.problems.is_empty(), "set up");
    assert_eq!(p.period, "2026-10");
    let pf = row(&p, g.prairie).expect("Prairie Fork");
    let sources: Vec<_> = pf.sources.iter().map(|x| (x.days, x.amount)).collect();
    assert_eq!(sources, vec![(31, m("15.29")), (16, m("0.39"))]);
    assert_eq!(pf.charge, m("15.68"));
    assert_eq!(pf.ticked, Some(true));
    assert_eq!(charge_of(&p, g.low_water), m("2.00"), "the minimum");
    let cg = row(&p, g.cedar).expect("Cedar Gap");
    assert_eq!(cg.has_open_credits, Some(m("400.00")));
    assert_eq!(cg.ticked, Some(false), "open credits: unticked");
    assert_eq!(cg.charge, m("9.17"));
    // Assess Prairie Fork; the same key replays the same invoice.
    let k = cl_api::IdempotencyKey::new();
    let one = assess(h, s, oct31(), g.prairie, pf.charge, k.clone()).expect("assess");
    assert_eq!(one.period, "2026-10");
    assert_eq!(one.invoices.len(), 1);
    let october = one.invoices[0].invoice_id;
    assert_eq!(one.invoices[0].total, m("15.68"));
    let two = assess(h, s, oct31(), g.prairie, pf.charge, k).expect("replayed");
    assert_eq!(two.run_id, one.run_id);
    assert_eq!(
        two.invoices
            .iter()
            .map(|i| i.invoice_id)
            .collect::<Vec<_>>(),
        vec![october],
        "the same invoice"
    );
    assert_eq!(invoice_balance(h, s, october), m("15.68"));
    // Another key for the same month: 409 ASSESSED, nothing posts.
    let e = assess(h, s, oct31(), g.prairie, m("15.68"), key()).expect_err("the month is done");
    assert_eq!(
        finance_charge_refusal(&e),
        Some(FinanceChargeRefusal::Assessed),
        "{e:?}"
    );
    let p = preview(h, s, oct31(), &[g.prairie]);
    let again = row(&p, g.prairie).expect("row");
    assert_eq!(
        again.already_assessed.as_ref().map(|a| a.invoice_id),
        Some(october)
    );
    let listed = h
        .run(s.finance_charges().list("2026-10"))
        .expect("assessments");
    assert!(
        listed
            .iter()
            .any(|a| a.invoice_id == october && a.contact_id == g.prairie && !a.voided)
    );
    // The charge invoice can be edited (plan §2.7); the month stays done.
    let read = h.run(s.invoices().get(october)).expect("invoice").record;
    let mut edit = NewInvoice::new(
        g.prairie,
        read.txn.txn_date,
        read.lines
            .iter()
            .map(|l| InvoiceLine {
                is_taxable: Some(false),
                description: l.description.clone(),
                ..InvoiceLine::account(
                    l.account,
                    l.quantity
                        .unwrap_or_else(|| Quantity::parse_wire("1").expect("qty")),
                    l.unit_price.unwrap_or(l.amount),
                )
            })
            .collect(),
    );
    edit.due_date = read.due_date;
    edit.memo = Some("Late fee, see statement".into());
    let saved = h
        .run(
            s.invoices()
                .update(october, &read, &edit, &PostingOptions::default(), key()),
        )
        .expect("a finance-charge invoice can be edited");
    assert_eq!(saved.invoice.record.txn.total, Some(m("15.68")));
    let p = preview(h, s, oct31(), &[g.prairie]);
    assert!(
        row(&p, g.prairie).is_some_and(|r| r.already_assessed.is_some()),
        "still assessed"
    );
    // November, compounding off: from October's charged-through date,
    // the October charge itself not charged.
    let p = preview(h, s, nov30(), &[g.prairie]);
    let pf = row(&p, g.prairie).expect("row");
    let sources: Vec<_> = pf.sources.iter().map(|x| (x.days, x.amount)).collect();
    assert_eq!(sources, vec![(30, m("14.79")), (30, m("0.74"))]);
    assert_eq!(pf.charge, m("15.53"));
    let nov = assess(h, s, nov30(), g.prairie, pf.charge, key()).expect("November");
    let november = nov.invoices[0].invoice_id;
    // Backdated (B2): void October and re-assess 10/31 with November
    // assessed: no days left, nothing charged.
    void_invoice(h, s, october);
    let p = preview(h, s, oct31(), &[g.prairie]);
    assert_eq!(charge_of(&p, g.prairie), Money::ZERO, "no days left");
    // Void November too: 11/30 runs from the due dates again (61 and 46
    // days): 30.08 + 1.13.
    void_invoice(h, s, november);
    let p = preview(h, s, nov30(), &[g.prairie]);
    let pf = row(&p, g.prairie).expect("row");
    let sources: Vec<_> = pf.sources.iter().map(|x| (x.days, x.amount)).collect();
    assert_eq!(sources, vec![(61, m("30.08")), (46, m("1.13"))]);
    assert_eq!(pf.charge, m("31.21"));
    let again = assess(h, s, nov30(), g.prairie, pf.charge, key()).expect("a void frees the month");
    assert_eq!(again.invoices[0].total, m("31.21"));
}

/// A date in a later month is refused, naming the field.
pub fn finance_charge_later_month_400(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let c = customer(h, s, "Too early");
    let e = h
        .run(s.finance_charges().preview(date(2099, 1, 31), Some(&[c])))
        .expect_err("a later month");
    assert_eq!(
        finance_charge_refusal(&e),
        Some(FinanceChargeRefusal::FutureDate),
        "{e:?}"
    );
    match &e {
        ApiError::Validation(v) => assert_eq!(
            v.field().map(|f| f.as_str().to_string()),
            Some("assessmentDate".into())
        ),
        other => panic!("a 400: {other:?}"),
    }
    let e = assess(h, s, date(2099, 1, 31), c, m("1.00"), key()).expect_err("assess too");
    assert_eq!(
        finance_charge_refusal(&e),
        Some(FinanceChargeRefusal::FutureDate),
        "{e:?}"
    );
}

/// Another company's client can't be charged.
pub fn finance_charge_cross_company_404(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let c = customer(h, s, "Charge scope");
    let b = h.s3();
    // PR-3 item 9: company B is set up, so its settings can't answer first;
    // the client is another company's: exactly 404 ("Customer not found",
    // finance-charge.service.ts assess), nothing posts.
    set_up(h, &b, false);
    let e = assess(h, &b, oct31(), c, m("2.00"), key()).expect_err("another company's client");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    let listed = h
        .run(b.finance_charges().list("2026-10"))
        .expect("assessments");
    assert!(listed.iter().all(|a| a.contact_id != c));
}

/// U-145 (Q-P22v, `invoice-client-guard-v1`): a finance-charge invoice
/// keeps its client: an edit that changes it is 409
/// `FINANCE_CHARGE_CLIENT_CHANGE` whatever is applied; nothing changes.
pub fn finance_charge_client_change_409(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::INVOICE_CLIENT_GUARD, "U-145") {
        return;
    }
    let s = &h.s1;
    let fc_income = set_up(h, s, false);
    let income = account(h, s, AccountType::Revenue, "Kennel services").id;
    let late = customer(h, s, "Late Payer");
    let other = customer(h, s, "Someone Else");
    dated_invoice(
        h,
        s,
        late,
        income,
        date(2026, 8, 31),
        date(2026, 9, 30),
        "1000.00",
    );
    let charge = charge_of(&preview(h, s, oct31(), &[late]), late);
    let run = assess(h, s, oct31(), late, charge, key()).expect("assess");
    let fc = run.invoices[0].invoice_id;
    let read = h.run(s.invoices().get(fc)).expect("the charge").record;
    assert!(read.is_finance_charge, "the engine marks it");
    let one = Quantity::parse_wire("1").expect("qty");
    let moved = NewInvoice::new(
        other,
        read.txn.txn_date,
        vec![InvoiceLine {
            is_taxable: Some(false),
            ..InvoiceLine::account(fc_income, one, charge)
        }],
    );
    let e = h
        .run(
            s.invoices()
                .update(fc, &read, &moved, &PostingOptions::default(), key()),
        )
        .expect_err("a finance charge keeps its client");
    assert_eq!(
        cl_api::invoice_refusal(&e),
        Some(cl_api::InvoiceRefusal::FinanceChargeClientChange),
        "{e:?}"
    );
    let after = h.run(s.invoices().get(fc)).expect("unchanged").record;
    assert_eq!(after.txn.contact_id, Some(late));
    assert_eq!(after.txn.updated_at, read.txn.updated_at);
}
