//! Create Invoice against the real engine (P20 review B4, plan §3 A6/§6).
//! Each test is `PENDING(U-xx)` until the pinned engine advertises the
//! capability it needs (the E1-E5 engine PRs land with the A7 pin bump).
//!
//! - `mockup_invoice_totals_3234_87`: the mockup's lines total 3,080.15 +
//!   154.72 = 3,234.87; preview = post = GET.
//! - `rounding_golden_cases`: half-away-from-zero line amounts and the
//!   document tax rounded once.
//! - `price_level_and_discount`: a level price and a percent discount under
//!   a subtotal survive create → GET → PUT unchanged (same total).
//! - `unsent_filter`, `invoice_closed_period`, `invoice_stale_409`,
//!   `invoice_put_idempotent`, `email_not_configured`, `layouts`.

use cl_api::{
    AccountType, ApiError, Capabilities, ClosedPeriodAuthorization, ConflictReason, ContactFields,
    ContactType, EmailMessage, IdempotencyKey, InvoiceLine, InvoiceListQuery, InvoiceRecord,
    InvoiceRefusal, InvoiceStatus, ItemFields, LayoutDraft, Method, NewContact, NewInvoice,
    PageRequest, PostingOptions, SendMethod, SendRecord, Session, SignedPercent, Versioned,
    invoice_refusal,
};
use cl_core::ids::{AccountId, ContactId};
use cl_core::{CalendarDate, Money, Quantity, Secret};
use serde_json::json;

use crate::common::{account, date, key, m};
use crate::support::{Harness, unique};

fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

pub(crate) fn customer(h: &Harness, s: &Session, name: &str) -> ContactId {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(ContactType::Customer, unique(name)),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("create customer: {e}"))
    .record
    .id
}

/// The mockup's 8.1 % company rate.
pub(crate) fn tax_rate_8_1(h: &Harness, s: &Session) {
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"defaultSalesTaxRate": "0.0810"})),
    ))
    .expect("set the tax rate");
}

fn line(a: AccountId, qty: &str, rate: &str, taxable: bool) -> InvoiceLine {
    InvoiceLine {
        is_taxable: Some(taxable),
        ..InvoiceLine::account(a, q(qty), m(rate))
    }
}

/// The mockup invoice (Q-P20 AC1).
pub(crate) fn mockup(c: ContactId, a: AccountId, on: CalendarDate) -> NewInvoice {
    NewInvoice::new(
        c,
        on,
        vec![
            line(a, "18", "65.00", false),
            line(a, "24", "32.50", true),
            line(a, "6", "48.00", true),
            line(a, "1", "842.15", true),
        ],
    )
}

fn create(h: &Harness, s: &Session, new: &NewInvoice) -> Versioned<InvoiceRecord> {
    h.run(s.invoices().create(new, &PostingOptions::default(), key()))
        .unwrap_or_else(|e| panic!("create invoice: {e}"))
        .invoice
}

fn get(h: &Harness, s: &Session, r: &InvoiceRecord) -> InvoiceRecord {
    h.run(s.invoices().get(r.txn.id)).expect("get").record
}

pub fn mockup_invoice_totals_3234_87(h: &Harness) {
    if !h.require_cap(Capabilities::INVOICE_PRICING, "U-129") {
        return;
    }
    let s = &h.s1;
    tax_rate_8_1(h, s);
    let c = customer(h, s, "Hartley Residence");
    let a = account(h, s, AccountType::Revenue, "Landscaping").id;
    let new = mockup(c, a, date(2026, 11, 4));
    let p = h.run(s.invoices().preview(&new, None)).expect("preview");
    assert_eq!(p.subtotal, m("3080.15"));
    assert_eq!(p.tax_total, m("154.72"));
    assert_eq!(p.total, m("3234.87"));
    let saved = create(h, s, &new);
    let r = get(h, s, &saved.record);
    assert_eq!(r.txn.total, Some(m("3234.87")));
    assert_eq!(r.tax_amount, Some(m("154.72")));
    let previewed: Vec<Money> = p.lines.iter().map(|l| l.amount).collect();
    let posted: Vec<Money> = r.lines.iter().map(|l| l.amount).collect();
    assert_eq!(previewed, posted, "preview = post = GET");
}

pub fn rounding_golden_cases(h: &Harness) {
    if !h.require_cap(Capabilities::SALES_CENTS, "U-122") {
        return;
    }
    let s = &h.s1;
    tax_rate_8_1(h, s);
    let c = customer(h, s, "Rounding");
    let a = account(h, s, AccountType::Revenue, "Rounding income").id;
    // 3 × 0.335 = 1.005 → 1.01 (half away from zero); tax 8.1 % of 1.01
    // = 0.08181 → 0.08.
    let new = NewInvoice::new(c, date(2026, 11, 4), vec![line(a, "3", "0.335", true)]);
    let r = create(h, s, &new).record;
    assert_eq!(r.lines[0].amount, m("1.01"));
    assert_eq!(r.tax_amount, Some(m("0.08")));
    assert_eq!(r.txn.total, Some(m("1.09")));
    // A negative line (a return): -1.005 → -1.01.
    let back = NewInvoice::new(
        c,
        date(2026, 11, 4),
        vec![line(a, "1", "5.00", false), line(a, "-3", "0.335", false)],
    );
    let r = create(h, s, &back).record;
    assert_eq!(r.lines[1].amount, m("-1.01"));
}

pub fn price_level_and_discount(h: &Harness) {
    if !h.require_cap(Capabilities::PRICE_LEVELS, "U-40")
        || !h.require_cap(Capabilities::ITEM_KINDS_SALES, "U-39a")
        || !h.require_cap(Capabilities::INVOICE_EDIT, "U-124")
    {
        return;
    }
    let s = &h.s1;
    tax_rate_8_1(h, s);
    let c = customer(h, s, "Builders");
    let income = account(h, s, AccountType::Revenue, "Plants").id;
    let item = |kind, name: &str, price: Option<Money>, pct: Option<&str>| {
        h.run(s.items().create(
            &ItemFields {
                name: unique(name),
                description: None,
                unit_price: price,
                income_account_id: income,
                is_taxable: true,
                item_type: kind,
                discount_percent: pct.map(|p| SignedPercent::parse(p).expect("pct")),
            },
            key(),
        ))
        .expect("item")
        .record
        .id
    };
    let mulch = item(cl_api::ItemType::Service, "Mulch", Some(m("45.00")), None);
    let sub = item(cl_api::ItemType::Subtotal, "Subtotal", None, None);
    let disc = item(cl_api::ItemType::Discount, "Loyalty", None, Some("10"));
    let level = h
        .run(
            s.invoices()
                .price_levels()
                .create(&cl_api::PriceLevelDraft {
                    name: unique("Commercial"),
                    percent: SignedPercent::parse("-7.5").expect("pct"),
                    is_active: true,
                    item_prices: Vec::new(),
                }),
        )
        .expect("level");
    h.run(s.invoices().set_client_sales(c, Some(level.id), None))
        .expect("client level");
    let new = NewInvoice::new(
        c,
        date(2026, 11, 4),
        vec![
            InvoiceLine::item(mulch, q("2")),
            InvoiceLine::item(sub, q("1")),
            InvoiceLine::item(disc, q("1")),
        ],
    );
    let saved = create(h, s, &new);
    let r = get(h, s, &saved.record);
    assert_eq!(r.lines.len(), 3, "subtotal and discount read back");
    // 45.00 − 7.5 % = 41.63 (half away); 2 × 41.63 = 83.26; −8.33.
    assert_eq!(r.lines[0].amount, m("83.26"));
    assert_eq!(r.lines[2].amount, m("-8.33"));
    // PUT as read (stored prices, the discount's percent): same total.
    let resend = NewInvoice::new(
        c,
        date(2026, 11, 4),
        r.lines
            .iter()
            .map(|l| InvoiceLine {
                account_id: None,
                item_id: l.item,
                description: l.description.clone(),
                quantity: l.quantity,
                unit_price: l.unit_price,
                discount_percent: l.discount_percent,
                is_taxable: Some(l.is_taxable),
                ..InvoiceLine::item(l.item.expect("item"), q("1"))
            })
            .collect(),
    );
    let again = h
        .run(
            s.invoices()
                .update(r.txn.id, &r, &resend, &PostingOptions::default(), key()),
        )
        .expect("put unchanged");
    assert_eq!(again.invoice.record.txn.total, r.txn.total);
}

pub fn unsent_filter(h: &Harness) {
    if !h.require_cap(Capabilities::INVOICE_SENT_HISTORY, "U-126") {
        return;
    }
    let s = &h.s1;
    let c = customer(h, s, "Unsent");
    let a = account(h, s, AccountType::Revenue, "Unsent income").id;
    let r = create(h, s, &mockup(c, a, date(2026, 11, 5))).record;
    let unsent = |h: &Harness| {
        h.run(s.invoices().list(
            &InvoiceListQuery {
                contact: Some(c),
                sent: Some(false),
                ..InvoiceListQuery::default()
            },
            PageRequest::first_max(),
        ))
        .expect("list")
        .data
        .into_iter()
        .map(|x| x.id)
        .collect::<Vec<_>>()
    };
    assert_eq!(unsent(h), vec![r.txn.id]);
    let sent = h
        .run(s.invoices().record_sent(
            r.txn.id,
            &SendRecord {
                method: SendMethod::Printed,
                recipients: Vec::new(),
                sent_at: None,
            },
            key(),
        ))
        .expect("sent");
    assert_eq!(sent.record.invoice_status, Some(InvoiceStatus::Sent));
    assert_eq!(unsent(h), Vec::new());
}

pub fn invoice_closed_period(h: &Harness) {
    if !h.require_cap(Capabilities::INVOICE_EDIT, "U-124") {
        return;
    }
    let s = h.s3();
    let c = customer(h, &s, "Closed");
    let a = account(h, &s, AccountType::Revenue, "Closed income").id;
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": "2026-01-31"})),
    ))
    .expect("set the closing date");
    let e = h
        .run(s.invoices().create(
            &mockup(c, a, date(2026, 1, 15)),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("closed");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    if h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15") {
        let wrong = PostingOptions {
            closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
                "not the password",
            ))),
        };
        let e = h
            .run(
                s.invoices()
                    .create(&mockup(c, a, date(2026, 1, 15)), &wrong, key()),
            )
            .expect_err("wrong password");
        assert!(
            matches!(
                e,
                ApiError::ClosingPasswordRejected | ApiError::ClosedPeriod(_)
            ),
            "{e:?}"
        );
    }
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": null})),
    ))
    .expect("clear the closing date");
}

pub fn invoice_stale_409(h: &Harness) {
    if !h.require_cap(Capabilities::INVOICE_EDIT, "U-124") {
        return;
    }
    let s = &h.s1;
    let c = customer(h, s, "Stale");
    let a = account(h, s, AccountType::Revenue, "Stale income").id;
    let new = mockup(c, a, date(2026, 11, 6));
    let r = create(h, s, &new).record;
    let first = h
        .run(
            s.invoices()
                .update(r.txn.id, &r, &new, &PostingOptions::default(), key()),
        )
        .expect("first edit");
    assert_ne!(first.invoice.record.txn.updated_at, r.txn.updated_at);
    let e = h
        .run(
            s.invoices()
                .update(r.txn.id, &r, &new, &PostingOptions::default(), key()),
        )
        .expect_err("stale");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            }
        ),
        "{e:?}"
    );
    assert_eq!(invoice_refusal(&e), Some(InvoiceRefusal::Stale));
}

pub fn invoice_put_idempotent(h: &Harness) {
    if !h.require_cap(Capabilities::IDEMPOTENCY_INVOICE, "U-124") {
        return;
    }
    let s = &h.s1;
    let c = customer(h, s, "Replay");
    let a = account(h, s, AccountType::Revenue, "Replay income").id;
    let new = mockup(c, a, date(2026, 11, 7));
    let r = create(h, s, &new).record;
    let k = IdempotencyKey::new();
    let one = h
        .run(
            s.invoices()
                .update(r.txn.id, &r, &new, &PostingOptions::default(), k.clone()),
        )
        .expect("edit");
    // The same key again replays the first answer: nothing posts twice
    // (it would be STALE otherwise).
    let two = h
        .run(
            s.invoices()
                .update(r.txn.id, &r, &new, &PostingOptions::default(), k),
        )
        .expect("replayed");
    assert_eq!(
        one.invoice.record.txn.updated_at,
        two.invoice.record.txn.updated_at
    );
}

pub fn email_not_configured(h: &Harness) {
    if !h.require_cap(Capabilities::INVOICE_EMAIL, "U-127") {
        return;
    }
    let s = h.s3();
    let c = customer(h, &s, "No SMTP");
    let a = account(h, &s, AccountType::Revenue, "Email income").id;
    let r = create(h, &s, &mockup(c, a, date(2026, 11, 8))).record;
    let msg = EmailMessage {
        to: vec!["ap@example.test".into()],
        subject: "Invoice".into(),
        body: "Hello".into(),
        ..EmailMessage::default()
    };
    let e = h
        .run(s.invoices().email(
            r.txn.id,
            &msg,
            "Invoice-1.pdf",
            b"%PDF-1.4\n%%EOF\n",
            None,
            key(),
        ))
        .expect_err("no SMTP on the contract company");
    assert!(
        matches!(
            invoice_refusal(&e),
            Some(InvoiceRefusal::SmtpNotConfigured | InvoiceRefusal::Offline)
        ),
        "{e:?}"
    );
    let queued = h
        .run(s.invoices().email_later(r.txn.id, &msg, key()))
        .expect("outbox");
    h.run(s.invoices().remove_from_outbox(queued.id))
        .expect("removed");
    h.run(s.invoices().remove_from_outbox(queued.id))
        .expect("already gone is success");
}

pub fn layouts(h: &Harness) {
    if !h.require_cap(Capabilities::INVOICE_LAYOUTS, "U-130") {
        return;
    }
    let s = &h.s1;
    let all = h.run(s.invoice_layouts().list(false)).expect("list");
    let default = all
        .iter()
        .find(|l| l.is_default)
        .expect("a default")
        .clone();
    let draft = LayoutDraft {
        name: unique("Simple"),
        columns: cl_api::LayoutColumns {
            quantity: false,
            rate: false,
            ..cl_api::LayoutColumns::default()
        },
        ..LayoutDraft::of(&default)
    };
    let made = h.run(s.invoice_layouts().create(&draft)).expect("create");
    assert!(!made.is_default);
    let e = h
        .run(s.invoice_layouts().set_archived(&default, true))
        .expect_err("default can't be archived");
    assert!(matches!(e, ApiError::Conflict { .. }), "{e:?}");
    let archived = h
        .run(s.invoice_layouts().set_archived(&made, true))
        .expect("archive");
    assert!(archived.is_archived);
    let shown = h.run(s.invoice_layouts().list(false)).expect("list");
    assert!(shown.iter().all(|l| l.id != made.id), "archived is hidden");
    let raw = h
        .run(s.raw(
            Method::Delete,
            &format!("/api/v1/company/invoice-layouts/{}", made.id),
            &[],
            None,
        ))
        .expect("raw");
    assert_eq!(raw.status, 405, "layouts are never deleted");
    h.run(s.invoice_layouts().set_archived(&archived, false))
        .expect("unarchive");
}
