//! Credit memo form: the Apply-or-Keep question, stale figures, and a save
//! that copies the engine's strings and tokens byte for byte (P22 plan
//! §6.2, §6.3).

use super::*;
use crate::forms::invoice::InvoiceLine;
use cl_core::Quantity;
use cl_core::ids::{AccountId, ContactId};
use proptest::prelude::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn tid(n: u32) -> TransactionId {
    TransactionId::parse(&format!("5a0c4e8e-6f6e-4c1d-8a37-{n:012}")).expect("id")
}

fn caps() -> InvoiceCaps {
    InvoiceCaps {
        fields: true,
        pricing: true,
        billable: true,
        closing: true,
    }
}

fn ctx() -> InvoiceContext {
    InvoiceContext {
        caps: caps(),
        ..InvoiceContext::default()
    }
}

fn form() -> CreditMemoForm {
    let mut keys = RowKeyGen::new();
    let mut f = CreditMemoForm::new(CalendarDate::from_ymd(2026, 11, 2).ok(), &mut keys);
    f.doc.client = ContactId::parse("3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").ok();
    let key = f.doc.lines[0].key;
    f.doc.lines[0] = InvoiceLine {
        account: AccountId::parse("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").ok(),
        quantity: Quantity::parse_wire("2").ok(),
        rate: Some(m("150.00")),
        ..InvoiceLine::blank(key)
    };
    f.doc.ensure_entry_line(&mut keys);
    f
}

fn figures(suggest: &[(u32, &str)], open: &[(u32, &str, &str)]) -> CreditMemoFigures {
    CreditMemoFigures {
        total: m("399.75"),
        open_invoices: open
            .iter()
            .map(|(n, bal, token)| OpenInvoiceFigure {
                invoice: tid(*n),
                number: n.to_string(),
                date: CalendarDate::from_ymd(2026, 9, 15).expect("d"),
                due: None,
                balance_due: m(bal),
                token: BalanceToken::from_wire(token),
            })
            .collect(),
        suggested: suggest
            .iter()
            .map(|(n, a)| SuggestedFigure {
                invoice: tid(*n),
                amount: m(a),
            })
            .collect(),
        remaining_credit: m("0.00"),
        problems: Vec::new(),
    }
}

fn golden2() -> CreditMemoFigures {
    figures(
        &[(1, "250.00"), (2, "149.75")],
        &[(1, "250.00", "250.0000"), (2, "500.00", "500.0049")],
    )
}

#[test]
fn the_kind_gates_terms_po_and_billable() {
    let mut f = form();
    f.doc.terms = Some("net_30".into());
    f.doc.po_number = "PO-1".into();
    let r = f.doc.to_new_invoice(&ctx()).expect("request");
    assert_eq!(r.kind, SalesDocKind::CreditMemo);
    assert!(r.terms.is_none() && r.due_date.is_none() && r.po_number.is_none());
    assert!(!SalesDocKind::CreditMemo.has_terms() && SalesDocKind::Invoice.has_terms());
    assert!(SalesDocKind::SalesReceipt.has_payment() && !SalesDocKind::CreditMemo.has_payment());
}

#[test]
fn unanswered_blocks_save_while_the_client_has_open_invoices() {
    let mut f = form();
    let req = f.preview_request(caps()).expect("request");
    assert!(f.apply_preview(&req, golden2(), caps()));
    assert!(f.must_ask(caps()));
    assert_eq!(
        f.save_plan(&ctx()),
        Err(vec![CreditMemoProblem::Unanswered])
    );
    f.choose_keep();
    let plan = f.save_plan(&ctx()).expect("kept");
    assert_eq!(plan.applications.len(), 0);
    // No open invoices: no question, kept as credit.
    let mut g = form();
    let req = g.preview_request(caps()).expect("request");
    g.apply_preview(&req, figures(&[], &[]), caps());
    assert!(!g.must_ask(caps()));
    assert_eq!(g.save_plan(&ctx()).expect("plan").applications.len(), 0);
}

#[test]
fn apply_copies_the_suggestion_and_tokens_byte_for_byte() {
    let mut f = form();
    let req = f.preview_request(caps()).expect("request");
    f.apply_preview(&req, golden2(), caps());
    f.choose_apply(caps());
    let plan = f.save_plan(&ctx()).expect("plan");
    let got: Vec<(TransactionId, String, String)> = plan
        .applications
        .iter()
        .map(|a| (a.invoice, a.amount.to_wire(), a.token.as_wire().to_string()))
        .collect();
    assert_eq!(
        got,
        vec![
            (tid(1), m("250.00").to_wire(), "250.0000".into()),
            (tid(2), m("149.75").to_wire(), "500.0049".into()),
        ]
    );
    // The owner's amount on B; A taken off.
    f.set_row(tid(2), Some(ApplyAmount::Typed(m("100.00"))));
    f.set_row(tid(1), None);
    let plan = f.save_plan(&ctx()).expect("plan");
    assert_eq!(plan.applications.len(), 1);
    assert_eq!(plan.applications[0].amount, m("100.00"));
    f.set_row(tid(2), None);
    assert_eq!(
        f.save_plan(&ctx()),
        Err(vec![CreditMemoProblem::NothingToApply])
    );
}

#[test]
fn a_stale_preview_is_dropped_and_save_waits() {
    let mut f = form();
    let old = f.preview_request(caps()).expect("request");
    f.doc.lines[0].rate = Some(m("151.00"));
    assert!(!f.apply_preview(&old, golden2(), caps()), "an older answer");
    assert_eq!(
        f.save_plan(&ctx()),
        Err(vec![CreditMemoProblem::PreviewPending])
    );
    let now = f.preview_request(caps()).expect("request");
    f.fail_preview(&now, caps());
    assert_eq!(
        f.save_plan(&ctx()),
        Err(vec![CreditMemoProblem::PreviewFailed])
    );
    let mut neg = golden2();
    neg.problems = vec!["CREDIT_MEMO_TOTAL_NOT_POSITIVE".into()];
    f.apply_preview(&now, neg, caps());
    assert_eq!(
        f.save_plan(&ctx()),
        Err(vec![CreditMemoProblem::TotalNotPositive])
    );
}

#[test]
fn a_billable_line_is_refused_on_a_credit_memo() {
    let mut f = form();
    let key = f.doc.lines[0].key;
    f.doc.lines[0].billable = Some(crate::forms::invoice::BillableRef {
        source_line: cl_core::ids::JournalLineId::parse("7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01")
            .expect("id"),
        markup_percent: None,
    });
    assert_eq!(
        f.save_plan(&ctx()),
        Err(vec![CreditMemoProblem::Form(
            InvoiceProblem::BillableNotAllowed(key)
        )])
    );
    assert!(
        f.preview_request(caps()).is_none(),
        "billable lines aren't priced"
    );
}

#[test]
fn a_rows_invoice_that_left_the_list_is_dropped_on_the_next_preview() {
    let mut f = form();
    let req = f.preview_request(caps()).expect("request");
    f.apply_preview(&req, golden2(), caps());
    f.choose_apply(caps());
    f.apply_preview(
        &req,
        figures(&[(2, "399.75")], &[(2, "500.00", "500.0000")]),
        caps(),
    );
    let AfterSave::Apply(rows) = f.after_save() else {
        panic!("apply");
    };
    assert_eq!(rows.len(), 1);
}

proptest! {
    /// Every money string in the plan is one the preview wrote (or the
    /// owner's typed amount); every token is the preview's.
    #[test]
    fn plan_strings_come_from_the_preview(cents in proptest::collection::vec(1i64..10_000_000, 1..6)) {
        let mut f = form();
        let req = f.preview_request(caps()).expect("request");
        let sug: Vec<(u32, String)> = cents.iter().enumerate().map(|(i, c)| (u32::try_from(i).expect("i"), Money::from_cents(*c).expect("m").to_wire())).collect();
        let open: Vec<(u32, String, String)> = sug.iter().map(|(i, a)| (*i, a.clone(), format!("{a}9"))).collect();
        let fig = CreditMemoFigures {
            total: m("1.00"),
            open_invoices: open.iter().map(|(n, bal, t)| OpenInvoiceFigure { invoice: tid(*n), number: String::new(), date: CalendarDate::from_ymd(2026, 9, 1).expect("d"), due: None, balance_due: m(bal), token: BalanceToken::from_wire(t) }).collect(),
            suggested: sug.iter().map(|(n, a)| SuggestedFigure { invoice: tid(*n), amount: m(a) }).collect(),
            remaining_credit: m("0.00"),
            problems: Vec::new(),
        };
        f.apply_preview(&req, fig, caps());
        f.choose_apply(caps());
        let plan = f.save_plan(&ctx()).expect("plan");
        for (a, (_, s)) in plan.applications.iter().zip(&sug) {
            prop_assert_eq!(a.amount.to_wire(), m(s).to_wire());
            prop_assert_eq!(a.token.as_wire(), format!("{s}9"));
        }
    }
}
