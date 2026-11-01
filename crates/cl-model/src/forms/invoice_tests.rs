//! Unit tests of the Create Invoice form model.

use super::*;
use crate::forms::FormState;
use crate::keymap::{Key, Keymap, Mods, Scope, Shortcut};

fn client() -> ContactId {
    ContactId::parse("3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id")
}

fn item() -> ItemId {
    ItemId::parse("6b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("item")
}

fn income() -> AccountId {
    AccountId::parse("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("account")
}

fn source() -> JournalLineId {
    JournalLineId::parse("9a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("line")
}

fn day(d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, d).expect("date")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

const ALL: InvoiceCaps = InvoiceCaps {
    fields: true,
    pricing: true,
    billable: true,
    closing: true,
};

fn ctx() -> InvoiceContext {
    InvoiceContext {
        caps: ALL,
        ..InvoiceContext::default()
    }
}

/// The mockup's first lines (synthetic): an item priced by the engine and
/// labor typed by hand.
fn form(keys: &mut RowKeyGen) -> InvoiceForm {
    let mut f = InvoiceForm::new(Some(day(3)), keys);
    f.client = Some(client());
    f.number = " 1042 ".into();
    let k = f.lines[0].key;
    let l = f.line_mut(k).expect("line");
    l.item = Some(item());
    l.description = "Shrubs, 3 gal\nplanted  ".into();
    l.quantity = Some(q("24"));
    l.service_date = Some(day(1));
    f.ensure_entry_line(keys);
    let k = f.lines[1].key;
    let l = f.line_mut(k).expect("line");
    l.account = Some(income());
    l.description = "Install labor".into();
    l.quantity = Some(q("18"));
    l.rate = Some(m("65.00"));
    l.taxable = Some(false);
    f.ensure_entry_line(keys);
    f
}

#[test]
fn the_entry_line_stays_at_the_end() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    assert_eq!(f.lines.len(), 3);
    assert!(f.lines[2].is_blank());
    f.ensure_entry_line(&mut keys);
    assert_eq!(f.lines.len(), 3, "one entry line only");
    let first = f.lines[0].key;
    let inserted = f.insert_line(first, &mut keys);
    assert_eq!(f.lines[0].key, inserted);
    assert!(f.delete_line(inserted, &mut keys));
    let entry = f.lines[2].key;
    assert!(f.delete_line(entry, &mut keys));
    assert!(f.lines.last().is_some_and(InvoiceLine::is_blank));
    assert_eq!(f.lines.len(), 3);
}

#[test]
fn a_valid_invoice_becomes_its_request_without_any_sum() {
    let mut keys = RowKeyGen::new();
    let f = form(&mut keys);
    let r = f.to_new_invoice(&ctx()).expect("valid");
    assert_eq!(r.client, client());
    assert_eq!(r.date, day(3));
    assert_eq!(r.number.as_deref(), Some("1042"));
    assert_eq!(r.lines.len(), 2, "the entry line is left out");
    let a = &r.lines[0];
    assert_eq!(a.item, Some(item()));
    assert_eq!(a.unit_price, None, "the engine prices it");
    assert_eq!(a.quantity, Some(q("24")));
    assert_eq!(a.description.as_deref(), Some("Shrubs, 3 gal\nplanted"));
    assert_eq!(a.service_date, Some(day(1)));
    assert_eq!(a.taxable, None, "the item's flag");
    let b = &r.lines[1];
    assert_eq!(b.unit_price, Some(m("65.00")));
    assert_eq!(b.taxable, Some(false));
    // Header fields always go out with invoice-fields-v1 so an edit clears.
    assert_eq!(r.po_number.as_deref(), Some(""));
    assert_eq!(r.customer_message.as_deref(), Some(""));
    assert_eq!(r.header_tags, Some(vec![]));
    assert_eq!(r.hide_zero_lines, Some(false));
    // A blank quantity is left out (the engine takes 1); before
    // invoice-pricing-v1 it goes out as 1.
    let mut f = f;
    f.lines[1].quantity = None;
    let r = f.to_new_invoice(&ctx()).expect("valid");
    assert_eq!(r.lines[1].quantity, None);
    let old = InvoiceContext {
        caps: InvoiceCaps {
            pricing: false,
            ..ALL
        },
        ..InvoiceContext::default()
    };
    f.lines[0].rate = Some(m("32.50"));
    let r = f.to_new_invoice(&old).expect("valid");
    assert_eq!(r.lines[1].quantity, Some(q("1")));
}

#[test]
fn changing_the_item_clears_the_old_price_and_discount() {
    let mut keys = RowKeyGen::new();
    let mut l = InvoiceLine {
        item: Some(item()),
        rate: Some(m("45.00")),
        discount_percent: Some(q("10")),
        ..InvoiceLine::blank(keys.next_key())
    };
    l.item_changed(Some(item()));
    assert_eq!(l.rate, Some(m("45.00")), "same item keeps its price");
    l.item_changed(None);
    assert_eq!(l.rate, None);
    assert_eq!(l.discount_percent, None);
}

#[test]
fn problems_are_reported_in_form_order() {
    let mut keys = RowKeyGen::new();
    let blank = InvoiceForm::new(None, &mut keys);
    assert_eq!(
        check_invoice(&blank, &ctx()),
        Err(vec![
            InvoiceProblem::ClientMissing,
            InvoiceProblem::DateMissing,
            InvoiceProblem::NoLines
        ])
    );
    let mut f = form(&mut keys);
    let k0 = f.lines[0].key;
    let k1 = f.lines[1].key;
    f.lines[0].item = None;
    f.lines[1].description = "x".repeat(TEXT_MAX + 1);
    f.po_number = "p".repeat(PO_MAX + 1);
    f.terms = Some(CUSTOM_TERMS.into());
    f.due_date = Some(day(2));
    let p = check_invoice(&f, &ctx()).expect_err("problems");
    assert_eq!(
        p,
        vec![
            InvoiceProblem::DueBeforeDate,
            InvoiceProblem::PoTooLong,
            InvoiceProblem::TermsDaysInvalid,
            InvoiceProblem::LineItemMissing(k0),
            InvoiceProblem::LineDescriptionTooLong(k1),
        ]
    );
    assert_eq!(
        InvoiceProblem::LineItemMissing(k0).field(),
        InvoiceField::LineItem(k0)
    );
    assert_eq!(
        InvoiceProblem::DueBeforeDate.key().as_str(),
        "invoice.problem.due_before_date"
    );
}

#[test]
fn what_the_engine_cannot_take_yet_is_named() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    let k0 = f.lines[0].key;
    let e1_only = InvoiceContext {
        caps: InvoiceCaps {
            fields: true,
            closing: true,
            ..InvoiceCaps::default()
        },
        ..InvoiceContext::default()
    };
    assert_eq!(
        check_invoice(&f, &e1_only),
        Err(vec![InvoiceProblem::LineRateMissing(k0)])
    );
    f.lines[0].rate = Some(m("32.50"));
    assert!(check_invoice(&f, &e1_only).is_ok());
    let pin = InvoiceContext::default();
    assert_eq!(
        check_invoice(&f, &pin),
        Err(vec![InvoiceProblem::FieldsNeedUpdate])
    );
    let closed = InvoiceContext {
        closing_date: Some(day(30)),
        ..pin
    };
    let p = check_invoice(&f, &closed).expect_err("closed");
    assert_eq!(p[0], InvoiceProblem::ClosedPeriodNeedsUpdate);
    // Moving an invoice out of a closed period needs the password too.
    let moved = InvoiceContext {
        closing_date: Some(day(2)),
        original_date: Some(day(1)),
        caps: InvoiceCaps {
            fields: true,
            ..InvoiceCaps::default()
        },
    };
    assert_eq!(
        check_invoice(&f, &moved),
        Err(vec![InvoiceProblem::ClosedPeriodNeedsUpdate])
    );
    // Without invoice-fields-v1 nothing of it is sent.
    f.lines[0].item = None;
    f.lines[0].account = Some(income());
    f.lines[0].service_date = None;
    let r = check_invoice(&f, &pin).expect("plain");
    assert_eq!(r.po_number, None);
    assert_eq!(r.header_tags, None);
}

#[test]
fn billable_costs_become_locked_taxable_lines_once() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    let seed = BillableSeed {
        source_line: source(),
        account: Some(income()),
        description: "Stump grinder".into(),
        quantity: Some(q("2")),
        markup_percent: Some(q("10")),
    };
    assert_eq!(f.add_billable(std::slice::from_ref(&seed), &mut keys), 1);
    assert_eq!(
        f.add_billable(&[seed], &mut keys),
        0,
        "already on the invoice"
    );
    assert_eq!(f.lines.len(), 4);
    let b = &f.lines[2];
    assert!(b.amounts_locked());
    assert_eq!(b.taxable, Some(true));
    assert_eq!(b.rate, None, "the engine's display rate fills it");
    assert!(f.lines[3].is_blank());
    assert_eq!(f.billable_sources(), vec![source()]);
    let r = f.to_new_invoice(&ctx()).expect("valid");
    let bl = &r.lines[2];
    assert_eq!(bl.billable_source_line, Some(source()));
    assert_eq!(bl.markup_percent, Some(q("10")));
    assert_eq!(bl.account, Some(income()));
    // Item, quantity and price are the source's: never sent (U-128).
    assert_eq!(
        (bl.item, bl.quantity, bl.unit_price, bl.discount_percent),
        (None, None, None, None)
    );
    let k = f.lines[2].key;
    let mut no_account = f.clone();
    if let Some(l) = no_account.line_mut(k) {
        l.account = None;
    }
    assert_eq!(
        check_invoice(&no_account, &ctx()),
        Err(vec![InvoiceProblem::BillableAccountMissing(k)])
    );
    let no_e3 = InvoiceContext {
        caps: InvoiceCaps {
            billable: false,
            ..ALL
        },
        ..InvoiceContext::default()
    };
    assert_eq!(
        check_invoice(&f, &no_e3),
        Err(vec![InvoiceProblem::BillableNeedsUpdate(k)])
    );
    if let Some(b) = f.line_mut(k).and_then(|l| l.billable.as_mut()) {
        b.markup_percent = Some(q("-1"));
    }
    assert_eq!(
        check_invoice(&f, &ctx()),
        Err(vec![InvoiceProblem::MarkupNegative(k)])
    );
}

fn figures(n: usize, total: &str) -> PreviewFigures {
    PreviewFigures {
        lines: (0..n)
            .map(|_| PreviewLineFigures {
                quantity: None,
                unit_price: Some(m("32.50")),
                source: RateSource::Item,
                amount: m("780.00"),
                tax_amount: Some(m("63.18")),
                display_rate: None,
                billed_elsewhere: false,
            })
            .collect(),
        totals: PreviewTotals {
            subtotal: m("1950.00"),
            tax_total: m("63.18"),
            tax_rate: Some(Rate::parse_wire("0.081").expect("rate")),
            total: m(total),
            due_date: Some(day(18)),
            balance_after: Some(m("3013.18")),
        },
        billed_on: Vec::new(),
    }
}

#[test]
fn previews_wait_250_ms_and_stale_replies_are_dropped() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    let mut t = PreviewTracker::new();
    assert_eq!(t.totals(), TotalsView::Nothing);
    assert!(t.poll(f.preview_request(ALL), 1_000).is_none(), "debounce");
    assert_eq!(t.totals(), TotalsView::Updating);
    assert!(t.poll(f.preview_request(ALL), 1_249).is_none());
    let (first, req) = t.poll(f.preview_request(ALL), 1_250).expect("send");
    assert_eq!(req.lines.len(), 2);
    assert!(t.in_flight());
    assert!(t.poll(f.preview_request(ALL), 1_400).is_none(), "sent once");
    // The owner edits a quantity while the first preview is in flight.
    f.lines[0].quantity = Some(q("25"));
    assert!(t.poll(f.preview_request(ALL), 1_500).is_none());
    assert!(
        !t.apply_preview(first, figures(2, "2013.18")),
        "a reply for an older request is dropped"
    );
    assert_eq!(t.totals(), TotalsView::Updating, "never an old figure");
    assert_eq!(t.line(f.lines[0].key), LineView::Updating);
    let (second, _) = t.poll(f.preview_request(ALL), 1_750).expect("send");
    assert!(
        !t.apply_preview(second, figures(1, "1.00")),
        "another line count is not this invoice"
    );
    assert!(t.apply_preview(second, figures(2, "2013.18")));
    match t.totals() {
        TotalsView::Ready(tot) => assert_eq!(tot.total, m("2013.18")),
        other => panic!("{other:?}"),
    }
    assert!(matches!(t.line(f.lines[1].key), LineView::Ready(_)));
    assert_eq!(t.line(f.lines[2].key), LineView::Nothing, "entry line");
    assert_eq!(t.shown_due_date(&f), Some(day(18)));
    f.due_date = Some(day(20));
    assert_eq!(t.shown_due_date(&f), Some(day(20)), "the owner's wins");
    // Editing what the engine doesn't price keeps the figures.
    f.memo = "internal".into();
    f.lines[0].description = "Shrubs".into();
    f.due_date = None;
    assert!(t.poll(f.preview_request(ALL), 2_000).is_none());
    assert!(matches!(t.totals(), TotalsView::Ready(_)));
}

#[test]
fn a_failed_preview_says_so_and_a_new_edit_tries_again() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    let mut t = PreviewTracker::new();
    t.poll(f.preview_request(ALL), 0);
    let (ticket, _) = t.poll(f.preview_request(ALL), 300).expect("send");
    t.fail(ticket);
    assert_eq!(t.totals(), TotalsView::CantCalculate);
    assert_eq!(t.line(f.lines[0].key), LineView::CantCalculate);
    assert!(!t.in_flight());
    assert!(
        t.poll(f.preview_request(ALL), 900).is_none(),
        "no retry storm"
    );
    f.lines[1].rate = Some(m("70.00"));
    t.poll(f.preview_request(ALL), 1_000);
    assert_eq!(t.totals(), TotalsView::Updating);
    assert!(t.poll(f.preview_request(ALL), 1_250).is_some());
    // Clearing the client leaves nothing to price.
    f.client = None;
    assert!(t.poll(f.preview_request(ALL), 1_300).is_none());
    assert_eq!(t.totals(), TotalsView::Nothing);
}

#[test]
fn form_state_tracks_revisions_and_undo_for_the_invoice() {
    let mut keys = RowKeyGen::new();
    let mut s = FormState::new(form(&mut keys));
    let r0 = s.revision();
    assert!(s.edit(1, |f| f.memo = "x".into()));
    assert!(s.revision() > r0);
    assert!(s.is_dirty());
    assert!(s.undo());
    assert!(!s.is_dirty());
}

#[test]
fn debug_shows_no_owner_data() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    f.memo = "secret memo".into();
    let r = f.to_new_invoice(&ctx()).expect("valid");
    let dbg = format!("{f:?} {r:?}");
    for s in ["secret", "Shrubs", "65.00", "1042", "labor"] {
        assert!(!dbg.contains(s), "{s} in {dbg}");
    }
}

/// Q-P20n: the invoice model never computes money. A scan of its source
/// for the exact-money operations proves it.
#[test]
fn the_invoice_model_does_no_money_arithmetic() {
    let src = include_str!("invoice.rs");
    for op in [
        "checked_add",
        "checked_sub",
        "checked_neg",
        "mul_round",
        "mul_rate_round",
        "Money::sum",
        "allocate(",
        "split(",
        "from_ten_thousandths",
        "as_decimal",
    ] {
        assert!(!src.contains(op), "invoice.rs uses {op}");
    }
}

#[test]
fn email_and_billable_shortcuts_resolve_in_forms_only() {
    let km = Keymap::new(crate::COMMANDS).expect("conflict free");
    let ctrl_e = Shortcut::new(Mods::CMD, Key::Letter('E'));
    let ctrl_shift_m = Shortcut::new(Mods::CMD_SHIFT, Key::Letter('M'));
    let ctrl_shift_b = Shortcut::new(Mods::CMD_SHIFT, Key::Letter('B'));
    assert_eq!(
        km.resolve(Scope::Form, ctrl_shift_m)
            .map(crate::CommandId::as_str),
        Some("invoice.email")
    );
    // Q-P20aa: Ctrl+E stays Edit Transaction, also in the register entry row.
    assert_eq!(
        km.resolve(Scope::Form, ctrl_e)
            .map(crate::CommandId::as_str),
        Some("edit.edit_transaction")
    );
    // Q-P20aa: Add Billable Costs is Ctrl+Shift+U; Pay Bills keeps
    // Ctrl+Shift+B in forms too.
    let ctrl_shift_u = Shortcut::new(Mods::CMD_SHIFT, Key::Letter('U'));
    assert_eq!(
        km.resolve(Scope::Form, ctrl_shift_u)
            .map(crate::CommandId::as_str),
        Some("invoice.add_billable")
    );
    assert_eq!(
        km.resolve(Scope::Form, ctrl_shift_b)
            .map(crate::CommandId::as_str),
        Some("vendors.pay_bills")
    );
    assert_eq!(
        km.resolve(Scope::Window, ctrl_e)
            .map(crate::CommandId::as_str),
        Some("edit.edit_transaction")
    );
    assert_eq!(
        km.resolve(Scope::Global, ctrl_shift_b)
            .map(crate::CommandId::as_str),
        Some("vendors.pay_bills")
    );
}

// ---- P22: the document kind -------------------------------------------------

#[test]
fn a_sales_receipt_sends_its_payment_fields_and_no_terms() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    f.kind = SalesDocKind::SalesReceipt;
    f.terms = Some(CUSTOM_TERMS.into());
    f.terms_days = None;
    f.due_date = Some(day(1));
    f.po_number = "PO-7".into();
    f.deposit_to = Some(income());
    f.payment_method = Some("credit_card".into());
    f.ref_no = " 5521 ".into();
    let r = check_invoice(&f, &ctx()).expect("terms are not checked on a receipt");
    assert_eq!(r.kind, SalesDocKind::SalesReceipt);
    assert!(r.terms.is_none() && r.terms_days.is_none() && r.due_date.is_none());
    assert!(r.po_number.is_none());
    assert_eq!(r.deposit_to, Some(income()));
    assert_eq!(r.payment_method.as_deref(), Some("credit_card"));
    assert_eq!(r.ref_no.as_deref(), Some("5521"));
    f.ref_no = "x".repeat(REF_MAX + 1);
    assert_eq!(
        check_invoice(&f, &ctx()).map(|_| ()),
        Err(vec![InvoiceProblem::RefTooLong])
    );
    assert_eq!(InvoiceProblem::RefTooLong.field(), InvoiceField::Reference);
    assert_eq!(
        InvoiceProblem::from_preview_code("NEGATIVE_RECEIPT_NEEDS_BANK").map(InvoiceProblem::field),
        Some(InvoiceField::DepositTo)
    );
    assert_eq!(InvoiceProblem::from_preview_code("OTHER"), None);
}

#[test]
fn an_invoice_keeps_its_p20_request_and_sends_no_payment_fields() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    f.deposit_to = Some(income());
    f.ref_no = "5521".into();
    let r = check_invoice(&f, &ctx()).expect("request");
    assert_eq!(r.kind, SalesDocKind::Invoice);
    assert!(r.deposit_to.is_none() && r.ref_no.is_none() && r.payment_method.is_none());
    assert_eq!(
        InvoiceForm::new_kind(SalesDocKind::CreditMemo, Some(day(2)), &mut keys).kind,
        SalesDocKind::CreditMemo
    );
}
