//! A document's sales tax against the real engine (P23 plan §2.2, goldens
//! 1-4 of `fixtures/sales_tax/golden.json`). Every figure is the engine's.
//!
//! - `rounded_once_per_document_goldens_1_to_4`: 8.875 % on 1,000.00 is
//!   88.75 (not 88.80) with the rate "0.088750"; a group is rounded once
//!   (0.49 = State 0.26 + City 0.23, not 0.48); the mockup invoice's
//!   154.72 splits 63.18 / 23.33 / 68.21 over the lines and 80.70 / 74.02
//!   over the members, the Install labor line's code is Non; a credit memo
//!   gives back −0.26 / −0.23 (Dr income 6.00, Dr STP 0.49, Cr A/R 6.49).
//!   Preview = post = GET, and the STP lines equal the document tax.
//! - `rate_changed_and_use_current_rates`: after an item's rate changes an
//!   edit's preview says `RATE_CHANGED` and keeps the saved rate; with
//!   `useCurrentRates` it takes the new one.

use cl_api::{InvoicePreview, InvoiceRecord, NewInvoice, SalesTaxItemDraft, SalesTaxKind};
use cl_core::Money;

use crate::common::{date, key, m};
use crate::contract_sales_tax_setup::{Monett, caps, monett, rate};
use crate::support::Harness;

fn members(r: &InvoiceRecord) -> Vec<(String, Option<Money>)> {
    r.sales_tax
        .as_ref()
        .expect("salesTax")
        .members
        .iter()
        .map(|mb| (mb.rate.to_wire6(), mb.tax))
        .collect()
}

fn preview(h: &Harness, x: &Monett, new: &NewInvoice) -> InvoicePreview {
    h.run(x.s.invoices().preview(new, None))
        .unwrap_or_else(|e| panic!("preview: {e}"))
}

/// Goldens 1-4.
#[allow(clippy::too_many_lines, reason = "four golden cases in one company")]
pub fn rounded_once_per_document_goldens_1_to_4(h: &Harness) {
    if !caps(h) {
        return;
    }
    let x = monett(h, "Sales tax documents", true);

    // 1. INV-E: Springfield 8.875 % on 1,000.00.
    let new = NewInvoice::new(
        x.ozark,
        date(2026, 11, 2),
        vec![Monett::line(x.plants, "1", "1000.00")],
    );
    let p = preview(h, &x, &new);
    assert_eq!(p.tax_total, m("88.75"), "not 88.80");
    assert_eq!(p.total, m("1088.75"));
    let ptax = p.sales_tax.as_ref().expect("preview salesTax");
    assert_eq!(ptax.id, x.springfield, "the client's default");
    assert_eq!(ptax.rate.to_wire6(), "0.088750");
    let e = h
        .run(
            x.s.invoices()
                .create(&new, &cl_api::PostingOptions::default(), key()),
        )
        .expect("INV-E")
        .invoice
        .record;
    assert_eq!(e.tax_amount, Some(m("88.75")));
    assert_eq!(members(&e), vec![("0.088750".into(), Some(m("88.75")))]);
    assert_eq!(e.lines[0].tax_amount, Some(m("88.75")));
    assert_eq!(
        e.lines[0].tax_code.as_ref().map(|c| c.code.as_str()),
        Some("Tax")
    );
    let got = h.run(x.s.invoices().get(e.txn.id)).expect("GET").record;
    assert_eq!(got.tax_amount, e.tax_amount, "post = GET");
    assert_eq!(members(&got), members(&e));
    assert_eq!(
        Monett::net(&x.txn(h, e.txn.id), x.stp),
        m("-88.75"),
        "Cr STP 88.75"
    );

    // 2. INV-B: the Monett group (company default) rounded once.
    let new = NewInvoice::new(
        x.hollister,
        date(2026, 11, 3),
        vec![Monett::line(x.plants, "1", "6.00")],
    );
    let p = preview(h, &x, &new);
    assert_eq!(p.tax_total, m("0.49"));
    let b = x.plants(h, x.hollister, date(2026, 11, 3), "6.00");
    assert_eq!(b.tax_amount, Some(m("0.49")));
    let tax = b.sales_tax.as_ref().expect("salesTax");
    assert_eq!(tax.id, x.monett, "the company default");
    assert_eq!(tax.rate.to_wire6(), "0.081000");
    let by_member: Vec<_> = tax.members.iter().map(|mb| (mb.id, mb.tax)).collect();
    assert!(by_member.contains(&(x.state, Some(m("0.26")))));
    assert!(by_member.contains(&(x.city, Some(m("0.23")))));
    assert_eq!(Monett::net(&x.txn(h, b.txn.id), x.stp), m("-0.49"));

    // 3. INV-A: the mockup invoice.
    let new = NewInvoice::new(
        x.hollister,
        date(2026, 11, 4),
        vec![
            Monett::line(x.install_labor, "18", "65.00"),
            Monett::line(x.shrubs, "24", "32.50"),
            Monett::line(x.mulch, "6", "48.00"),
            Monett::line(x.perennials, "1", "842.15"),
        ],
    );
    let p = preview(h, &x, &new);
    let a = x.invoice(h, x.hollister, date(2026, 11, 4), new.lines.clone());
    for (label, lines, tax, total) in [
        (
            "preview",
            p.lines.iter().map(|l| l.tax_amount).collect::<Vec<_>>(),
            Some(p.tax_total),
            Some(p.total),
        ),
        (
            "post",
            a.lines.iter().map(|l| l.tax_amount).collect(),
            a.tax_amount,
            a.subtotal
                .zip(a.tax_amount)
                .and_then(|(s, t)| s.checked_add(t).ok()),
        ),
    ] {
        assert_eq!(
            lines,
            vec![
                Some(m("0.00")),
                Some(m("63.18")),
                Some(m("23.33")),
                Some(m("68.21"))
            ],
            "{label} line taxes"
        );
        assert_eq!(tax, Some(m("154.72")), "{label}");
        assert_eq!(total, Some(m("3234.87")), "{label}");
    }
    let codes: Vec<_> = a
        .lines
        .iter()
        .map(|l| l.tax_code.as_ref().map(|c| c.code.clone()))
        .collect();
    assert_eq!(
        codes,
        vec![
            Some("Non".into()),
            Some("Tax".into()),
            Some("Tax".into()),
            Some("Tax".into())
        ]
    );
    let by_member: Vec<_> = a
        .sales_tax
        .as_ref()
        .expect("salesTax")
        .members
        .iter()
        .map(|mb| (mb.id, mb.tax))
        .collect();
    assert!(by_member.contains(&(x.state, Some(m("80.70")))));
    assert!(by_member.contains(&(x.city, Some(m("74.02")))));
    assert_eq!(Monett::net(&x.txn(h, a.txn.id), x.stp), m("-154.72"));

    // 4. CM-C: a credit memo kept as a credit gives the tax back.
    let memo = cl_api::NewCreditMemo::new(
        x.hollister,
        date(2026, 11, 10),
        vec![Monett::line(x.plants, "1", "6.00")],
    );
    let cm = crate::contract_credit_memos::create(h, &x.s, &memo);
    assert_eq!(cm.tax_amount, m("0.49"));
    assert_eq!(cm.total, m("6.49"));
    let tax = cm.sales_tax.as_ref().expect("memo salesTax");
    let by_member: Vec<_> = tax.members.iter().map(|mb| (mb.id, mb.tax)).collect();
    assert!(by_member.contains(&(x.state, Some(m("-0.26")))));
    assert!(by_member.contains(&(x.city, Some(m("-0.23")))));
    let t = x.txn(h, cm.id);
    assert_eq!(Monett::net(&t, x.income), m("6.00"), "Dr income");
    assert_eq!(Monett::net(&t, x.stp), m("0.49"), "Dr STP");
    assert_eq!(Monett::net(&t, x.ar), m("-6.49"), "Cr A/R");
}

/// Q-P23i: saved rates stay on an edit unless `useCurrentRates`.
pub fn rate_changed_and_use_current_rates(h: &Harness) {
    if !caps(h) {
        return;
    }
    let x = monett(h, "Sales tax rate change", true);
    let e = x.plants(h, x.ozark, date(2026, 11, 2), "1000.00");
    assert_eq!(e.tax_amount, Some(m("88.75")));
    let st = x.s.sales_tax();
    let item = h
        .run(st.items(false))
        .expect("items")
        .into_iter()
        .find(|i| i.id == x.springfield)
        .expect("Springfield");
    h.run(st.update_item(
        x.springfield,
        &SalesTaxItemDraft {
            kind: SalesTaxKind::Item,
            name: item.name.clone(),
            description: item.description.clone(),
            rate: Some(rate("0.090000")),
            agency_id: item.agency_id,
            member_ids: Vec::new(),
        },
        item.updated_at,
        key(),
    ))
    .expect("new rate");
    let mut edit = NewInvoice::new(
        x.ozark,
        date(2026, 11, 2),
        vec![Monett::line(x.plants, "1", "1000.00")],
    );
    let kept = h
        .run(x.s.invoices().preview(&edit, Some(e.txn.id)))
        .expect("edit preview");
    assert!(
        kept.notice_codes().iter().any(|c| c == "RATE_CHANGED"),
        "{:?}",
        kept.notice_codes()
    );
    assert_eq!(kept.tax_total, m("88.75"), "the saved rate");
    edit.use_current_rates = true;
    let now = h
        .run(x.s.invoices().preview(&edit, Some(e.txn.id)))
        .expect("current rates");
    assert_eq!(now.tax_total, m("90.00"));
    // A new document takes the new rate.
    let n = x.plants(h, x.ozark, date(2026, 11, 3), "1000.00");
    assert_eq!(n.tax_amount, Some(m("90.00")));
}
