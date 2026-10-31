//! Create Invoice against the `--demo` engine: the mockup totals come from
//! the engine's preview (AC1), a billable cost leaves the unbilled list
//! once invoiced (AC2), edits keep the send history, void, Prev/Next.

use std::sync::Arc;
use std::time::Duration;

use cl_core::ids::ContactId;
use cl_core::{CalendarDate, Money, Quantity};
use cl_model::forms::SaveIntent;
use cl_model::forms::invoice::{InvoiceLine, TotalsView};

use super::*;
use crate::company::CompanyLink;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn screen() -> (tempfile::TempDir, InvoiceScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = InvoiceScreen::new(link, None, today());
    (dir, s)
}

fn story_client(s: &InvoiceScreen) -> ContactId {
    s.books
        .contact(crate::company::demo::bills::STORY_CUSTOMER)
        .expect("the story client")
}

fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

/// Lets the 250 ms debounce pass and answers the preview.
fn settle(s: &mut InvoiceScreen) {
    s.poll_preview();
    s.clock = s
        .clock
        .checked_sub(Duration::from_millis(400))
        .unwrap_or(s.clock);
    s.poll_preview();
}

/// The mockup's lines (typed rates on demo items; the billable cost
/// arrives through Add Billable Costs).
fn mockup(s: &mut InvoiceScreen) {
    let c = story_client(s);
    s.pick_client(c);
    let items = s.items.clone();
    let line = |i: usize, qty: &str, rate: &str, tax: bool, key| InvoiceLine {
        item: Some(items[i].id),
        quantity: Some(q(qty)),
        rate: Some(m(rate)),
        taxable: Some(tax),
        ..InvoiceLine::blank(key)
    };
    let mut lines = Vec::new();
    for (i, qty, rate, tax) in [
        (3, "18", "65.00", false),
        (1, "24", "32.50", true),
        (2, "6", "48.00", true),
    ] {
        let key = s.grid.keys().next_key();
        lines.push(line(i, qty, rate, tax, key));
    }
    s.grid.edit(|v, keys| {
        v.lines = lines;
        v.ensure_entry_line(keys);
    });
    s.open_billable();
    s.billable.markup.clear();
    let line = s.billable.costs[0].line_id;
    s.billable
        .choose_income(line, crate::company::demo::account_id(4000));
    s.add_billable();
    settle(s);
}

#[test]
fn the_mockup_invoice_totals_3234_87_from_the_engine() {
    let (_d, mut s) = screen();
    assert!(s.books.loaded);
    mockup(&mut s);
    let TotalsView::Ready(t) = s.tracker.totals() else {
        panic!("the preview answered: {:?}", s.tracker.totals());
    };
    assert_eq!(t.subtotal, m("3080.15"));
    assert_eq!(t.tax_total, m("154.72"));
    assert_eq!(t.total, m("3234.87"));
    let v = s.state_json();
    assert_eq!(v["total"], "3,234.87");
    assert_eq!(v["lines"].as_array().map(Vec::len), Some(4));
}

#[test]
fn an_edit_shows_updating_never_an_old_figure() {
    let (_d, mut s) = screen();
    mockup(&mut s);
    let key = s.grid.value().lines[0].key;
    s.grid.edit(|v, _| {
        if let Some(l) = v.lines.iter_mut().find(|l| l.key == key) {
            l.quantity = Some(q("19"));
        }
    });
    s.poll_preview();
    assert_eq!(s.tracker.totals(), TotalsView::Updating);
    assert_eq!(s.grid.shown(key), grid::Shown::Updating);
    assert_eq!(s.state_json()["total"], serde_json::Value::Null);
    settle(&mut s);
    assert_eq!(s.state_json()["total"], "3,299.87");
}

#[test]
fn an_invoiced_billable_cost_leaves_the_unbilled_list() {
    let (_d, mut s) = screen();
    let c = story_client(&s);
    s.pick_client(c);
    assert_eq!(s.billable.costs.len(), 1, "the story cost is unbilled");
    mockup(&mut s);
    assert_eq!(s.billable.unbilled(&s.compose()).len(), 0, "on the invoice");
    s.start_save(SaveIntent::Save, None);
    assert!(s.editing.is_some(), "saved: {:?}", s.problems);
    s.new_form(false);
    s.pick_client(c);
    assert!(s.billable.costs.is_empty(), "billed: gone from the list");
}

#[test]
fn a_saved_invoice_reads_back_and_void_unbills_its_cost() {
    let (_d, mut s) = screen();
    mockup(&mut s);
    s.start_save(SaveIntent::Save, None);
    let id = s.editing.as_ref().map(|e| e.id).expect("saved");
    assert_eq!(s.saved_count, 1);
    assert!(!s.is_dirty());
    // The stored rates come back (E2 note 1: an edit resends them).
    assert!(
        s.grid
            .value()
            .lines
            .iter()
            .filter(|l| !l.is_blank())
            .all(|l| l.rate.is_some())
    );
    s.void_reason = "Wrong client".into();
    s.send_void(None);
    assert!(s.editing.as_ref().is_some_and(Editing::void));
    assert_eq!(s.read_only(), Some("invoice.read_only.void"));
    let c = story_client(&s);
    s.client_changed();
    assert_eq!(s.billable.costs.len(), 1, "void brings the cost back");
    let _ = (id, c);
}

#[test]
fn an_edit_of_a_sent_invoice_keeps_its_history_and_offers_to_send() {
    let (_d, mut s) = screen();
    mockup(&mut s);
    s.start_save(SaveIntent::Save, None);
    s.open_email(None);
    assert!(s.email.is_open());
    s.email.to = "billing@example.com".into();
    s.send_email();
    assert_eq!(s.state_json()["sent_events"], 1);
    s.header.edit(field::PO, |h| h.po_number = "PO-7".into());
    s.start_save(SaveIntent::Save, None);
    assert!(s.offer_send, "Q-P20f");
    assert_eq!(s.state_json()["sent_events"], 1, "history kept");
}

#[test]
fn email_without_a_server_offers_the_outbox() {
    let (_d, mut s) = screen();
    mockup(&mut s);
    s.start_save(SaveIntent::Save, None);
    if let Some(mut d) = s.link.demo_state() {
        d.invoices.no_smtp = true;
    }
    s.open_email(None);
    s.email.to = "billing@example.com".into();
    s.send_email();
    let v = s.state_json();
    assert_eq!(v["email"]["failed"], true);
    assert_eq!(v["email"]["problem"], "invoice.email.no_smtp");
    assert!(s.editing.is_some(), "the invoice stays saved");
    s.queue_email();
    let n = s
        .link
        .demo_state()
        .map(|d| crate::company::demo::invoices::outbox(&d).len());
    assert_eq!(n, Some(1));
}

#[test]
fn nothing_is_sent_without_a_client() {
    let (_d, mut s) = screen();
    s.grid.edit(|v, keys| {
        v.lines[0].rate = Some(m("10"));
        v.lines[0].account = Some(crate::company::demo::account_id(4000));
        v.ensure_entry_line(keys);
    });
    s.start_save(SaveIntent::Save, None);
    assert!(s.editing.is_none());
    assert_eq!(
        s.state_json()["save"]["refused"],
        "invoice.problem.client_missing"
    );
}

#[test]
fn every_screen_key_is_in_the_catalog() {
    for k in [
        "invoice.updating",
        "invoice.cant_calculate",
        "invoice.terms.default",
        "invoice.sent.banner",
        "invoice.sent.banner_to",
        "orphan.invoice_save",
    ] {
        assert!(crate::i18n::has(k), "{k}");
    }
    for c in TERMS {
        assert!(crate::i18n::has(&format!("invoice.terms.{c}")), "{c}");
    }
}
