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
    let line = s.billable.costs[0].source_line_id;
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

/// U-40, U-39a: an unpriced line takes the client's level price (45.00 −
/// 7.5 % = 41.63); a subtotal shows its group; a taxable 10 % discount
/// right under it covers that group and lowers the taxable base (Q-P20t).
#[test]
fn a_level_price_a_subtotal_and_a_discount_come_from_the_engine() {
    let (_d, mut s) = screen();
    mockup(&mut s);
    let find = |s: &InvoiceScreen, name: &str| {
        s.items
            .iter()
            .find(|i| i.fields.name == name)
            .map(|i| i.id)
            .expect(name)
    };
    let (mowing, sub, disc) = (
        find(&s, "Weekly Mowing"),
        find(&s, "Subtotal"),
        find(&s, "Loyalty discount"),
    );
    s.grid.edit(|v, keys| {
        let at = v.lines.len() - 1;
        let mut add = |item, qty: Option<Quantity>| InvoiceLine {
            item: Some(item),
            quantity: qty,
            ..InvoiceLine::blank(keys.next_key())
        };
        let lines = [add(mowing, Some(q("2"))), add(sub, None), add(disc, None)];
        for (i, l) in lines.into_iter().enumerate() {
            v.lines.insert(at + i, l);
        }
    });
    settle(&mut s);
    let TotalsView::Ready(t) = s.tracker.totals() else {
        panic!("{:?}", s.tracker.totals());
    };
    assert_eq!(t.subtotal, m("2847.07"));
    assert_eq!(t.tax_total, m("135.84"));
    assert_eq!(t.total, m("2982.91"));
    let lines = s.state_json()["lines"].clone();
    assert_eq!(lines[4]["amount"], "83.26");
    assert_eq!(lines[4]["rate_source"], "pricelevelpercent");
    assert_eq!(
        lines[5]["amount"], "3,163.41",
        "the subtotal shows its group"
    );
    assert_eq!(lines[6]["amount"], "-316.34");
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

/// B3 (review): create → GET → the form → PUT unchanged posts the same
/// total, with a level-priced line, a subtotal, a % discount and a
/// marked-up billable cost; the billable line goes back with only its
/// source, account and markup, and nothing is released.
#[test]
fn a_saved_invoice_round_trips_without_changing_the_money() {
    let (_d, mut s) = screen();
    mockup(&mut s);
    let find = |s: &InvoiceScreen, name: &str| {
        s.items
            .iter()
            .find(|i| i.fields.name == name)
            .map(|i| i.id)
            .expect(name)
    };
    let (sub, disc) = (find(&s, "Subtotal"), find(&s, "Loyalty discount"));
    s.grid.edit(|v, keys| {
        let at = v.lines.len() - 1;
        for (i, item) in [sub, disc].into_iter().enumerate() {
            v.lines.insert(
                at + i,
                InvoiceLine {
                    item: Some(item),
                    ..InvoiceLine::blank(keys.next_key())
                },
            );
        }
        for l in &mut v.lines {
            if let Some(b) = l.billable.as_mut() {
                b.markup_percent = Some(q("10"));
            }
        }
    });
    settle(&mut s);
    s.start_save(SaveIntent::Save, None);
    settle(&mut s);
    let saved = s.editing.clone().expect("saved").record;
    assert_eq!(saved.billable_sources().len(), 1);
    let mut keys = cl_grid::RowKeyGen::new();
    let (mut form, lines) = save::form_of(&saved, &mut keys);
    form.lines = lines;
    let req = form.to_new_invoice(&s.context()).expect("valid as read");
    let new = engine::new_invoice(&req).expect("maps");
    let body = serde_json::to_value(&new).expect("json");
    let billable: Vec<&serde_json::Value> = body["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter(|l| l.get("billableSourceLineId").is_some())
        .collect();
    assert_eq!(billable.len(), 1, "the markup line is not sent");
    let b = billable[0];
    assert!(b.get("accountId").is_some());
    assert_eq!(b["markupPercent"], "10");
    for k in ["itemId", "quantity", "unitPrice", "discountPercent"] {
        assert!(b.get(k).is_none(), "{k} is the cost's");
    }
    assert!(
        body["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .any(|l| l.get("discountPercent").is_some()),
        "the % discount keeps its stored percent"
    );
    assert_eq!(save::released(&saved, &req), Vec::new());
    let mut d = s.link.demo_state().expect("demo");
    let again = crate::company::demo::invoices::update(
        &mut d,
        saved.txn.id,
        saved.txn.updated_at,
        &new,
        None,
    )
    .expect("unchanged edit");
    assert_eq!(again.record.txn.total, saved.txn.total);
    assert_eq!(again.record.tax_amount, saved.tax_amount);
}

/// Follow-up 20: printed (or emailed) from another window, the open invoice
/// is read again so its send history and `updatedAt` are current.
#[test]
fn a_send_elsewhere_reloads_the_open_invoice() {
    let (_d, mut s) = screen();
    mockup(&mut s);
    s.start_save(SaveIntent::Save, None);
    settle(&mut s);
    let saved = s.editing.clone().expect("saved").record;
    let other = s.link.bus().subscribe();
    {
        let mut d = s.link.demo_state().expect("demo");
        crate::company::demo::invoices::record_sent(
            &mut d,
            saved.txn.id,
            &cl_api::SendRecord {
                method: cl_api::SendMethod::Printed,
                recipients: Vec::new(),
                sent_at: None,
            },
        )
        .expect("printed");
    }
    crate::forms::publish_saved(&s.link, &other, &[]);
    s.hear_changes();
    s.poll();
    let now = s.editing.clone().expect("open").record;
    assert_eq!(now.send_history.len(), 1);
    assert_ne!(now.txn.updated_at, None);
}

/// Q-P20ad: a new invoice shows the counter's next number and doesn't send
/// it; a typed number already in use saves with a warning (Q-94).
#[test]
fn the_next_number_shows_and_a_duplicate_warns() {
    let (_d, mut s) = screen();
    assert_eq!(s.next_number.as_deref(), Some("1042"));
    mockup(&mut s);
    s.start_save(SaveIntent::SaveAndNew, None);
    settle(&mut s);
    assert_eq!(s.next_number.as_deref(), Some("1043"));
    assert!(!s.duplicate_number);
    let c = story_client(&s);
    s.pick_client(c);
    let item = s.items[3].id;
    s.grid.edit(|v, keys| {
        v.lines = vec![InvoiceLine {
            item: Some(item),
            quantity: Some(q("1")),
            rate: Some(m("10.00")),
            ..InvoiceLine::blank(keys.next_key())
        }];
        v.ensure_entry_line(keys);
    });
    s.header.edit(field::NUMBER, |h| h.number = "1042".into());
    s.start_save(SaveIntent::Save, None);
    settle(&mut s);
    assert!(s.duplicate_number, "1042 is taken");
}

/// Q-P20v: a saved invoice takes a file; an unsaved one asks to save first.
#[test]
fn a_saved_invoice_takes_an_attachment() {
    let (_d, mut s) = screen();
    s.upload_attachment("note.pdf".into(), b"%PDF-1.4\n%%EOF\n".to_vec());
    assert_eq!(s.attach.problem, Some("invoice.attach.save_first"));
    mockup(&mut s);
    s.start_save(SaveIntent::Save, None);
    settle(&mut s);
    s.upload_attachment("note.pdf".into(), b"%PDF-1.4\n%%EOF\n".to_vec());
    s.poll();
    assert_eq!(s.attach.list.len(), 1);
    assert_eq!(s.state_json()["attachments"]["files"], 1);
    s.upload_attachment("x.txt".into(), b"hello".to_vec());
    assert_eq!(s.attach.problem, Some("check.attach.wrong_type"));
}

/// Q-P20i: an item set up from a line fills that line when it is saved.
#[test]
fn an_item_set_up_from_a_line_fills_it() {
    let (_d, mut s) = screen();
    let key = s.grid.value().lines[0].key;
    s.start_item_set_up(key, "Stump Grinding".into());
    let id = {
        let mut d = s.link.demo_state().expect("demo");
        let items = d
            .items
            .get_or_insert_with(crate::company::demo::lists::items);
        let mut new = items[0].clone();
        new.id = cl_core::ids::ItemId::parse("d6000000-0000-4000-8000-0000000000aa").expect("id");
        new.fields.name = "Stump Grinding".into();
        items.push(new.clone());
        new.id
    };
    s.items = s
        .link
        .demo_state()
        .expect("demo")
        .items
        .clone()
        .unwrap_or_default();
    s.item_set_up_back();
    assert_eq!(s.grid.value().lines[0].item, Some(id));
    assert!(s.setting_up_item.is_none());
}

/// Q-P20p: the income account picked for a cost is remembered per expense
/// account across windows (the company's local preferences).
#[test]
fn the_income_account_for_a_cost_is_remembered() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let tenant = cl_core::ids::TenantId::parse("d0000000-0000-4000-8000-0000000000f1").expect("id");
    let mut s = InvoiceScreen::new(Arc::clone(&link), Some((tenant, "Demo".into())), today());
    mockup(&mut s);
    let exp = s.billable.costs[0].expense_account_id;
    let saved = link
        .store()
        .load()
        .ok()
        .and_then(|l| l.get(tenant).map(|r| r.prefs.billable_income.clone()))
        .unwrap_or_default();
    assert_eq!(
        saved.get(&exp),
        Some(&crate::company::demo::account_id(4000))
    );
    let mut again = InvoiceScreen::new(link, Some((tenant, "Demo".into())), today());
    again.billable.remembered.clear();
    let c = story_client(&again);
    again.pick_client(c);
    again.open_billable();
    assert_eq!(
        again.billable.remembered.get(&exp),
        Some(&crate::company::demo::account_id(4000))
    );
}

/// Q-P20ac (U-130): the Template picked prints the invoice; Simple has no
/// quantity or rate column.
#[test]
fn the_template_picked_prints_the_invoice() {
    let (_d, mut s) = screen();
    let simple = s
        .print_books
        .layouts
        .iter()
        .find(|l| l.name == "Simple")
        .map(|l| l.id)
        .expect("demo layout");
    mockup(&mut s);
    s.header.edit_step(|h| {
        h.layout = cl_model::forms::invoice::LayoutPick::Layout(simple.to_string());
    });
    s.start_save(SaveIntent::Save, None);
    settle(&mut s);
    let r = s.editing.clone().expect("saved").record;
    assert_eq!(r.layout_id, Some(simple));
    let (data, _) = s.pdf_parts().expect("parts");
    let p = data.layout.expect("layout");
    assert!(!p.quantity && !p.rate);
}

/// UI-verifier MUST 1: Add Billable Costs is modal. Esc with a grid line
/// mid-edit closes the dialog only: the grid behind gets no key, and every
/// line stays (the typed figure is committed when the dialog opens).
#[test]
fn esc_closes_the_billable_dialog_and_keeps_every_line() {
    use eframe::egui::{self, Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};
    let tap = |k: Key, modifiers: Modifiers| {
        [true, false]
            .into_iter()
            .map(|pressed| Event::Key {
                key: k,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            })
            .collect::<Vec<_>>()
    };
    let ctx = egui::Context::default();
    crate::shell::Shell::install(&ctx);
    cl_grid::sequencer::install(&ctx);
    let mut shell = crate::shell::Shell::new(crate::shell::ShellOptions::default());
    let (_d, mut s) = screen();
    let c = story_client(&s);
    s.pick_client(c);
    let item = s.items[3].id;
    s.grid.edit(|v, keys| {
        v.lines = vec![InvoiceLine {
            item: Some(item),
            quantity: Some(q("18")),
            rate: Some(m("65.00")),
            ..InvoiceLine::blank(keys.next_key())
        }];
        v.ensure_entry_line(keys);
    });
    // The entry line, mid-edit: the case where a stray Esc deleted it.
    let entry = s.grid.value().lines[1].key;
    s.grid.focus(entry, grid::V_QTY);
    let w = shell.windows.open_new(CommandId::new(KIND), Box::new(s));
    let frame = |shell: &mut crate::shell::Shell, events: Vec<Event>| {
        for e in [events, Vec::new(), Vec::new(), Vec::new()] {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 940.0))),
                events: e,
                ..RawInput::default()
            };
            let mut out = ctx.run_ui(input, |ui| shell.ui(ui));
            out.textures_delta.clear();
        }
        shell.windows.get(w).expect("open").screen.state()
    };
    let before = frame(&mut shell, Vec::new());
    assert_eq!(before["line_count"], 1);
    // Mid-edit: a figure typed into Qty, not recorded.
    let typed = frame(&mut shell, vec![Event::Text("2".into())]);
    assert_eq!(typed["grid"]["focused"], true);
    // Ctrl+Shift+U (the keymap path is the scenario's).
    shell
        .windows
        .get_mut(w)
        .expect("open")
        .screen
        .command(CommandId::new("invoice.add_billable"));
    let open = frame(&mut shell, Vec::new());
    assert_eq!(open["billable_dialog"]["open"], true, "{open}");
    assert_eq!(open["line_count"], 2, "the typed figure is kept");
    assert_eq!(open["grid"]["focused"], false, "the grid gave up the keys");
    let after = frame(&mut shell, tap(Key::Escape, Modifiers::NONE));
    assert_eq!(after["billable_dialog"]["open"], false, "Esc closes it");
    assert_eq!(after["line_count"], 2, "every line stays");
    assert_eq!(after["lines"][0]["item"], true);
}
