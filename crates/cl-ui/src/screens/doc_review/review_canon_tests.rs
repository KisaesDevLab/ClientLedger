//! Review S2: form → body → wire for each draft form. The form side reads
//! the checked values; the posted side reads the request body cl-api puts
//! on the wire, built by the form's **own** builder (the one Accept uses).
//! Equal digests for the same values; any changed field in the body shows.

use cl_api::{Capabilities, CardChargeDraft, CheckDraft, ConvertBody, DraftKind};
use cl_core::ids::{AccountId, ContactId, ItemId, TagId};
use cl_core::{CalendarDate, Money, Quantity};
use cl_model::forms::bill::{BillKind, CheckedBill, CheckedItemLine, CheckedTerms, Percent};
use cl_model::forms::card::{CardMode, CheckedCard};
use cl_model::forms::check::CheckedCheck;
use cl_model::forms::lines::PostingLine;

use super::super::{PostBody, posted_digest};
use super::Canon;

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn uuid(prefix: &str, n: u8) -> String {
    format!("{prefix}-0000-4000-8000-0000000000{n:02}")
}

fn account(n: u8) -> AccountId {
    AccountId::parse(&uuid("a0000000", n)).expect("id")
}

fn contact(n: u8) -> ContactId {
    ContactId::parse(&uuid("c0000000", n)).expect("id")
}

fn tag(n: u8) -> TagId {
    TagId::parse(&uuid("70000000", n)).expect("id")
}

fn item(n: u8) -> ItemId {
    ItemId::parse(&uuid("10000000", n)).expect("id")
}

fn day(d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, d).expect("date")
}

fn caps() -> Capabilities {
    serde_json::from_value(serde_json::json!([
        Capabilities::BILL_LINES,
        Capabilities::BILL_TERMS,
        Capabilities::BILL_RECEIVED,
    ]))
    .expect("caps")
}

fn line(acct: u8, amount: &str, memo: Option<&str>) -> PostingLine {
    PostingLine {
        account: account(acct),
        amount: money(amount),
        memo: memo.map(str::to_string),
        contact: None,
        billable: false,
        tag: None,
    }
}

/// Two lines that use every line field: a memo, a customer billed, a tag.
fn lines() -> Vec<PostingLine> {
    let mut billed = line(11, "40.10", Some("Paint for the Ruiz job"));
    billed.contact = Some(contact(7));
    billed.billable = true;
    billed.tag = Some(tag(1));
    let mut tagged = line(12, "9.90", None);
    tagged.tag = Some(tag(2));
    vec![billed, tagged]
}

/// The posted digest; `None` when cl-api refuses the body (nothing would
/// be sent).
fn posted(kind: DraftKind, body: &PostBody) -> Option<String> {
    let live: ConvertBody = body.live(true).ok()?;
    posted_digest(kind, &live, &caps())
}

// ------------------------------------------------------------------ card

fn card() -> CheckedCard {
    CheckedCard {
        card: account(1),
        mode: CardMode::Charge,
        payee: Some(contact(2)),
        date: day(14),
        ref_no: Some("R-1007".into()),
        amount: money("50.00"),
        memo: Some("November supplies".into()),
        header_tags: vec![tag(1)],
        lines: lines(),
    }
}

fn card_body(c: &CheckedCard) -> CardChargeDraft {
    crate::screens::card_charge::CardChargeScreen::charge_draft(c)
}

#[test]
fn a_card_charge_posts_every_reviewed_value() {
    let c = card();
    for kind in [DraftKind::CardCharge, DraftKind::CashExpense] {
        let body = match kind {
            DraftKind::CardCharge => PostBody::CardCharge(card_body(&c)),
            _ => PostBody::CashExpense(card_body(&c)),
        };
        assert_eq!(Some(Canon::of_card(kind, &c).digest()), posted(kind, &body));
    }
    let form = Some(Canon::of_card(DraftKind::CardCharge, &c).digest());
    // Each value the body could lose or change shows in the digest.
    let changes: Vec<fn(&mut CardChargeDraft)> = vec![
        |d| d.lines[0].amount = money("40.11"),
        |d| d.lines[0].memo = None,
        |d| d.lines[0].contact = None,
        |d| d.lines[0].billable = false,
        |d| d.lines[1].tag = None,
        |d| d.lines[1].account = account(13),
        |d| d.memo = None,
        |d| d.number = None,
        |d| d.payee = Some(contact(3)),
        |d| d.pay_from = account(2),
        |d| d.date = day(15),
        |d| {
            d.lines.swap(0, 1);
        },
    ];
    for (i, change) in changes.into_iter().enumerate() {
        let mut d = card_body(&c);
        change(&mut d);
        assert_ne!(
            form,
            posted(DraftKind::CardCharge, &PostBody::CardCharge(d)),
            "change {i}"
        );
    }
    // The kind is part of it: a charge is not a cash expense.
    assert_ne!(
        form,
        posted(
            DraftKind::CashExpense,
            &PostBody::CashExpense(card_body(&c))
        )
    );
}

// ----------------------------------------------------------------- check

fn check() -> CheckedCheck {
    CheckedCheck {
        bank: account(3),
        number: Some(4418),
        print_later: false,
        date: day(14),
        payee: contact(2),
        payee_name: String::new(),
        address: Some("12 Main St\nSpringfield".into()),
        amount: money("50.00"),
        memo: Some("Invoice 88".into()),
        header_tags: vec![tag(3)],
        lines: lines(),
        draft_attachment: None,
    }
}

fn check_body(c: &CheckedCheck) -> CheckDraft {
    crate::screens::write_check::to_draft(c, "Corner Hardware").expect("draft")
}

#[test]
fn a_check_posts_every_reviewed_value() {
    let c = check();
    let form = Some(Canon::of_check(&c).digest());
    assert_eq!(
        form,
        posted(DraftKind::Check, &PostBody::Check(check_body(&c)))
    );
    let changes: Vec<fn(&mut CheckDraft)> = vec![
        |d| d.check_number = Some(4419),
        |d| d.amount = money("50.01"),
        |d| d.payee_address = None,
        |d| d.memo = Some("Invoice 89".into()),
        |d| d.tag_ids = Vec::new(),
        |d| d.bank = account(4),
        |d| d.lines[0].billable = false,
        |d| d.lines[0].contact = Some(contact(8)),
        |d| d.lines[1].tag = Some(tag(1)),
    ];
    for (i, change) in changes.into_iter().enumerate() {
        let mut d = check_body(&c);
        change(&mut d);
        assert_ne!(
            form,
            posted(DraftKind::Check, &PostBody::Check(d)),
            "change {i}"
        );
    }
}

// ------------------------------------------------------------------ bill

fn bill() -> CheckedBill {
    let mut lines = lines();
    lines.truncate(1);
    CheckedBill {
        kind: BillKind::Bill,
        vendor: contact(2),
        date: day(14),
        reference: Some("INV-2291".into()),
        terms: Some(CheckedTerms {
            code: "custom".into(),
            days: Some(45),
            discount_percent: Percent::parse_user("1.5"),
            discount_days: Some(10),
        }),
        due_date: Some(day(29)),
        discount_date: None,
        amount: money("62.60"),
        memo: Some("Paint order".into()),
        header_tags: Vec::new(),
        expense_lines: lines,
        item_lines: vec![CheckedItemLine {
            item: Some(item(1)),
            description: Some("Rollers".into()),
            qty: Some(Quantity::parse_wire("2.5").expect("qty")),
            rate: Some(money("9.00")),
            amount: money("22.50"),
            account: account(14),
            contact: Some(contact(7)),
            billable: true,
            tag: Some(tag(2)),
        }],
        bill_received: None,
        attachments: Vec::new(),
        needs_closing_password: false,
    }
}

fn bill_body(c: &CheckedBill) -> cl_api::BillDraft {
    match crate::screens::bill::bill_body(c, None, true) {
        crate::screens::bill::BillBody::Bill(b) => b,
        crate::screens::bill::BillBody::Credit(_) => panic!("a bill"),
    }
}

#[test]
fn a_bill_posts_every_reviewed_value() {
    let c = bill();
    let form = Some(Canon::of_bill(&c).digest());
    assert_eq!(
        form,
        posted(DraftKind::Bill, &PostBody::Bill(bill_body(&c)))
    );
    let changes: Vec<fn(&mut cl_api::BillDraft)> = vec![
        |d| d.vendor_invoice_number = None,
        |d| d.memo = None,
        |d| d.due_date = Some(day(30)),
        |d| d.terms = None,
        |d| d.received = false,
        |d| d.lines[1].quantity = Some(Quantity::parse_wire("3").expect("qty")),
        |d| d.lines[1].unit_price = Some(money("9.01")),
        |d| d.lines[1].item = None,
        |d| d.lines[1].customer = None,
        |d| d.lines[1].billable = false,
        |d| d.lines[1].tag = None,
        |d| d.lines[0].description = Some("Paint".into()),
        |d| d.lines[0].amount = money("40.00"),
        |d| d.vendor = contact(9),
    ];
    for (i, change) in changes.into_iter().enumerate() {
        let mut d = bill_body(&c);
        change(&mut d);
        assert_ne!(
            form,
            posted(DraftKind::Bill, &PostBody::Bill(d)),
            "change {i}"
        );
    }
}

/// A blank memo or reference is none on both sides (the engine keeps a
/// blank as none); the canonical text never shows in `Debug`.
#[test]
fn blanks_are_none_and_debug_shows_no_values() {
    let mut c = card();
    c.memo = None;
    let mut d = card_body(&c);
    d.memo = Some("   ".into());
    assert_eq!(
        Some(Canon::of_card(DraftKind::CardCharge, &c).digest()),
        posted(DraftKind::CardCharge, &PostBody::CardCharge(d))
    );
    let dbg = format!("{:?}", Canon::of_card(DraftKind::CardCharge, &card()));
    assert!(!dbg.contains("40.1") && !dbg.contains("supplies"), "{dbg}");
}
