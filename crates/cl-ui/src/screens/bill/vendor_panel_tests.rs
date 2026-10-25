//! Vendor panel and duplicate-ref tests (P17 WP8): the demo engine answers,
//! so every check runs without a network.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_api::{Account, BillDraft, BillLineDraft, BillSummary, Contact, VendorCreditDraft};
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle};
use cl_model::forms::SaveIntent;
use cl_model::forms::bill::BillKind;

use super::*;
use crate::company::CompanyLink;
use crate::company::demo::bills as demo_bills;
use crate::data_bus::Changes;
use crate::screens::bill::EnterBillScreen;

const REF: &str = "INV-55871";

struct Env {
    _dir: tempfile::TempDir,
    link: Arc<CompanyLink>,
    contacts: Vec<Contact>,
    accounts: Vec<Account>,
}

fn env() -> Env {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let (contacts, accounts) = {
        let mut demo = link.demo_state().expect("demo");
        let contacts = demo.contacts_mut().clone();
        let accounts = demo
            .chart
            .get_or_insert_with(crate::company::demo::chart)
            .clone();
        (contacts, accounts)
    };
    Env {
        _dir: dir,
        link,
        contacts,
        accounts,
    }
}

impl Env {
    fn vendor(&self, name: &str) -> ContactId {
        self.contacts
            .iter()
            .find(|c| c.fields.display_name == name)
            .map_or_else(|| panic!("{name}"), |c| c.id)
    }

    fn story(&self) -> ContactId {
        self.vendor(demo_bills::STORY_VENDOR)
    }

    /// Another vendor of the sample.
    fn other(&self) -> ContactId {
        let story = self.story();
        self.contacts
            .iter()
            .find(|c| {
                c.id != story
                    && matches!(
                        c.fields.contact_type,
                        cl_api::ContactType::Vendor | cl_api::ContactType::Both
                    )
            })
            .map(|c| c.id)
            .expect("another vendor")
    }

    fn input<'a>(
        &'a self,
        kind: BillKind,
        vendor: Option<ContactId>,
        ref_no: &'a str,
        date: CalendarDate,
        editing: Option<TransactionId>,
    ) -> PanelInput<'a> {
        PanelInput {
            link: &self.link,
            kind,
            vendor,
            ref_no,
            date: Some(date),
            editing,
            contact: vendor.and_then(|v| self.contacts.iter().find(|c| c.id == v)),
            accounts: &self.accounts,
            money_style: MoneyStyle::default(),
        }
    }

    fn rental(&self) -> cl_core::ids::AccountId {
        self.accounts
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("6210"))
            .map(|a| a.id)
            .expect("6210")
    }

    fn bill(&self, vendor: ContactId, reference: &str, date: CalendarDate) -> TransactionId {
        let d = BillDraft {
            vendor,
            date,
            vendor_invoice_number: Some(reference.into()),
            terms: None,
            due_date: None,
            memo: None,
            amount_due: money("1480.00"),
            lines: vec![
                BillLineDraft::expense(self.rental(), money("1330.00")),
                BillLineDraft::expense(self.rental(), money("150.00")),
            ],
            received: true,
            draft_attachment: None,
        };
        let mut demo = self.link.demo_state().expect("demo");
        demo_bills::create_bill(&mut demo, &d, None).expect("bill")
    }

    fn credit(&self, vendor: ContactId, reference: &str, date: CalendarDate) -> TransactionId {
        let d = VendorCreditDraft {
            vendor,
            date,
            vendor_invoice_number: Some(reference.into()),
            memo: None,
            amount: money("150.00"),
            lines: vec![BillLineDraft::expense(self.rental(), money("150.00"))],
            draft_attachment: None,
        };
        let mut demo = self.link.demo_state().expect("demo");
        demo_bills::create_credit(&mut demo, &d, None).expect("credit")
    }

    fn void(&self, id: TransactionId) {
        let mut demo = self.link.demo_state().expect("demo");
        demo_bills::void(&mut demo, id, None).expect("void");
    }
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn day(y: i32, m: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, d).expect("date")
}

/// Ticks past the debounce and returns the check's state name.
fn settle(panel: &mut VendorPanel, input: &PanelInput<'_>, start: Instant) -> String {
    panel.tick(input, start);
    panel.tick(input, start + DEBOUNCE);
    panel.state_json()["duplicate_check"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn the_check_waits_for_the_typing_to_stop_then_finds_the_bill() {
    let e = env();
    let v = e.story();
    let other = e.bill(v, REF, day(2026, 11, 3));
    let mut p = VendorPanel::default();
    let t0 = Instant::now();
    // Trimmed and case-folded: " inv-55871 " is INV-55871.
    let input = e.input(
        BillKind::Bill,
        Some(v),
        " inv-55871 ",
        day(2026, 11, 20),
        None,
    );
    p.tick(&input, t0);
    assert_eq!(p.state_json()["duplicate_check"], "waiting");
    assert!(p.busy());
    p.tick(&input, t0 + Duration::from_millis(399));
    assert_eq!(p.state_json()["duplicate_check"], "waiting");
    assert!(p.duplicate().is_none());
    p.tick(&input, t0 + DEBOUNCE);
    assert_eq!(p.state_json()["duplicate_check"], "found");
    let d = p.duplicate().expect("duplicate");
    assert_eq!(d.id, other);
    assert_eq!(d.ref_no, REF);
    assert_eq!(d.date, day(2026, 11, 3));
    assert!(!p.duplicate_unchecked());
    // Nothing owner-typed in Debug or the control channel.
    assert!(!format!("{d:?} {p:?}").contains("55871"));
    assert!(!p.state_json().to_string().contains("55871"));
}

#[test]
fn typing_again_restarts_the_wait() {
    let e = env();
    let v = e.story();
    e.bill(v, REF, day(2026, 11, 3));
    let mut p = VendorPanel::default();
    let t0 = Instant::now();
    p.tick(
        &e.input(BillKind::Bill, Some(v), "INV-5587", day(2026, 11, 3), None),
        t0,
    );
    let typed = e.input(BillKind::Bill, Some(v), REF, day(2026, 11, 3), None);
    let t1 = t0 + Duration::from_millis(300);
    p.tick(&typed, t1);
    p.tick(&typed, t0 + Duration::from_millis(500));
    assert_eq!(p.state_json()["duplicate_check"], "waiting");
    p.tick(&typed, t1 + DEBOUNCE);
    assert_eq!(p.state_json()["duplicate_check"], "found");
    // A blank ref has nothing to check.
    p.tick(
        &e.input(BillKind::Bill, Some(v), "  ", day(2026, 11, 3), None),
        t1 + DEBOUNCE,
    );
    assert_eq!(p.state_json()["duplicate_check"], "off");
    assert!(p.duplicate().is_none());
}

#[test]
fn void_other_vendor_own_and_out_of_window_bills_are_not_duplicates() {
    let e = env();
    let v = e.story();
    let t0 = Instant::now();
    let voided = e.bill(v, REF, day(2026, 11, 3));
    e.void(voided);
    e.bill(e.other(), REF, day(2026, 11, 3));
    let mut p = VendorPanel::default();
    let at = |date| e.input(BillKind::Bill, Some(v), REF, date, None);
    assert_eq!(settle(&mut p, &at(day(2026, 11, 3)), t0), "clear");
    let live = e.bill(v, REF, day(2026, 11, 3));
    // The bill being edited is never its own duplicate.
    let mut p = VendorPanel::default();
    let own = e.input(BillKind::Bill, Some(v), REF, day(2026, 11, 3), Some(live));
    assert_eq!(settle(&mut p, &own, t0), "clear");
    // Exactly 18 months later is inside the window; a day more is not.
    let mut p = VendorPanel::default();
    assert_eq!(settle(&mut p, &at(day(2028, 5, 3)), t0), "found");
    let mut p = VendorPanel::default();
    assert_eq!(settle(&mut p, &at(day(2028, 5, 4)), t0), "clear");
    let mut p = VendorPanel::default();
    assert_eq!(settle(&mut p, &at(day(2025, 5, 3)), t0), "found");
    let mut p = VendorPanel::default();
    assert_eq!(settle(&mut p, &at(day(2025, 5, 2)), t0), "clear");
    // "INV 55871" is another ref (nothing but trim and case fold).
    let mut p = VendorPanel::default();
    let spaced = e.input(BillKind::Bill, Some(v), "INV 55871", day(2026, 11, 3), None);
    assert_eq!(settle(&mut p, &spaced, t0), "clear");
}

#[test]
fn a_credit_is_checked_against_credits_only() {
    let e = env();
    let v = e.story();
    let t0 = Instant::now();
    e.bill(v, REF, day(2026, 11, 3));
    let mut p = VendorPanel::default();
    let credit = e.input(BillKind::Credit, Some(v), REF, day(2026, 11, 3), None);
    assert_eq!(settle(&mut p, &credit, t0), "clear");
    let c = e.credit(v, REF, day(2026, 10, 1));
    let mut p = VendorPanel::default();
    assert_eq!(settle(&mut p, &credit, t0), "found");
    assert_eq!(p.duplicate().map(|d| d.id), Some(c));
}

#[test]
fn save_with_a_duplicate_asks_and_save_anyway_sends_it() {
    let e = env();
    let v = e.story();
    e.bill(v, REF, day(2026, 11, 3));
    let mut p = VendorPanel::default();
    let input = e.input(BillKind::Bill, Some(v), REF, day(2026, 11, 3), None);
    // Saved before the typing pause: the check runs at once.
    p.tick(&input, Instant::now());
    assert!(!p.clear_to_save(&input, SaveIntent::SaveAndNew));
    assert!(p.asking());
    assert_eq!(p.state_json()["duplicate_prompt"], true);
    assert_eq!(p.take_resume(), None);
    // Go back: nothing is sent; asking again asks again.
    assert_eq!(p.answer(false), PanelEvent::GoBack);
    assert!(!p.asking());
    assert_eq!(p.take_resume(), None);
    assert!(!p.clear_to_save(&input, SaveIntent::SaveAndClose));
    // Save anyway: the save goes, and the next check lets it through.
    assert_eq!(p.answer(true), PanelEvent::None);
    assert_eq!(p.take_resume(), Some(SaveIntent::SaveAndClose));
    assert!(p.clear_to_save(&input, SaveIntent::SaveAndClose));
    // Esc closes the question as Go back.
    let mut q = VendorPanel::default();
    q.tick(&input, Instant::now());
    assert!(!q.clear_to_save(&input, SaveIntent::Save));
    assert!(q.go_back());
    assert!(!q.go_back());
}

#[test]
fn no_duplicate_or_no_ref_saves_without_asking() {
    let e = env();
    let v = e.story();
    let mut p = VendorPanel::default();
    let input = e.input(BillKind::Bill, Some(v), REF, day(2026, 11, 3), None);
    p.tick(&input, Instant::now());
    assert!(p.clear_to_save(&input, SaveIntent::Save));
    assert!(!p.asking());
    let blank = e.input(BillKind::Bill, Some(v), "", day(2026, 11, 3), None);
    assert!(p.clear_to_save(&blank, SaveIntent::Save));
}

#[test]
fn the_panel_reads_the_summary_again_when_the_vendors_bills_change() {
    let e = env();
    let v = e.story();
    let mut p = VendorPanel::default();
    let input = e.input(BillKind::Bill, Some(v), "", day(2026, 11, 3), None);
    let t0 = Instant::now();
    p.tick(&input, t0);
    let owed = |p: &VendorPanel| {
        p.cards
            .as_ref()
            .and_then(|c| c.summary.ready())
            .map(|s| s.total_owed)
    };
    assert_eq!(owed(&p), Some(Money::ZERO));
    assert_eq!(p.state_json()["recent_bills"], 0);
    let id = e.bill(v, REF, day(2026, 11, 3));
    // Another vendor's change: nothing to read.
    let mut other = Changes::default();
    other.bill_vendors.insert(e.other());
    p.bills_changed(&other);
    p.tick(&input, t0);
    assert_eq!(owed(&p), Some(Money::ZERO));
    let mut mine = Changes::default();
    mine.bill_vendors.insert(v);
    p.bills_changed(&mine);
    p.tick(&input, t0);
    // The engine's figure, as sent.
    assert_eq!(owed(&p), Some(money("1480.00")));
    assert_eq!(p.state_json()["open_bills_count"], 1);
    assert_eq!(p.state_json()["open_bills_shown"], 1);
    assert_eq!(p.state_json()["recent_bills"], 1);
    let open = p.cards.as_ref().and_then(|c| c.open.ready()).expect("open");
    assert_eq!(open[0].id, id);
    assert_eq!(open[0].amount, Some(money("1480.00")));
    // Voided: out of both lists.
    e.void(id);
    p.bills_changed(&mine);
    p.tick(&input, t0);
    assert_eq!(owed(&p), Some(Money::ZERO));
    assert_eq!(p.state_json()["open_bills_shown"], 0);
    assert_eq!(p.state_json()["recent_bills"], 0);
}

fn summary(json: &str) -> BillSummary {
    serde_json::from_str(json).expect("row")
}

fn row(n: u32, status: &str, balance: &str) -> BillSummary {
    summary(&format!(
        r#"{{"id":"00000000-0000-4000-8000-{n:012}","txnDate":"2026-11-0{d}","status":"{status}",
        "vendorInvoiceNumber":"R{n}","total":"10.0000","balanceDue":"{balance}"}}"#,
        d = n % 9 + 1
    ))
}

#[test]
fn open_and_recent_rows_keep_posted_bills_as_the_engine_sent_them() {
    let rows = vec![
        row(1, "posted", "10.0000"),
        row(2, "void", "10.0000"),
        row(3, "posted", "0.0000"),
        row(4, "posted", "4.5000"),
        row(5, "posted", "1.0000"),
        row(6, "posted", "1.0000"),
        row(7, "posted", "1.0000"),
        row(8, "posted", "1.0000"),
    ];
    let open = open_bills(&rows);
    assert_eq!(open.len(), OPEN_SHOWN);
    assert_eq!(open[0].label(), "R1");
    assert_eq!(open[1].amount, Some(money("4.50")));
    assert!(open.iter().all(|b| b.label() != "R2" && b.label() != "R3"));
    let recent = recent_bills(&rows);
    assert_eq!(recent.len(), RECENT_SHOWN);
    assert!(recent.iter().all(|b| b.amount == Some(money("10.00"))));
    assert!(recent.iter().any(|b| b.label() == "R3"));
    assert!(!format!("{:?}", open[0]).contains("R1"));
}

#[test]
fn the_screen_asks_before_saving_a_duplicate_and_save_anyway_saves() {
    let e = env();
    let today = day(2026, 11, 4);
    let mut s = EnterBillScreen::new(Arc::clone(&e.link), None, today);
    s.poll();
    let fill = |s: &mut EnterBillScreen| {
        let v = e.story();
        s.pick_vendor(v);
        s.header.edit_step(|h| {
            h.date = Some(day(2026, 11, 3));
            h.reference = REF.into();
            h.set_amount_due(Some(money("150.00")));
        });
        let key = s.expenses.form_mut().adapter_mut().keys.next_key();
        let line = cl_model::forms::lines::FormLine {
            account: Some(e.rental()),
            amount: Some(money("150.00")),
            ..cl_model::forms::lines::FormLine::blank(key)
        };
        s.expenses.form_mut().edit_step(|v| v.lines = vec![line]);
        s.sync_inputs();
    };
    fill(&mut s);
    s.panel_tick();
    assert!(
        s.start_save(SaveIntent::SaveAndNew, None),
        "no duplicate yet"
    );
    assert_eq!(s.save.count, 1);
    s.panel_tick();
    fill(&mut s);
    s.panel_tick();
    let writes = e.link.demo_state().expect("demo").bills.writes;
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert!(s.panel.asking());
    assert_eq!(s.state_json()["panel"]["duplicate_prompt"], true);
    assert_eq!(s.save.count, 1);
    assert_eq!(e.link.demo_state().expect("demo").bills.writes, writes);
    // Save anyway: the next frame's tick sends it.
    s.panel.answer(true);
    s.panel_tick();
    assert_eq!(s.save.count, 2);
    // Both bills are open for the vendor now.
    let v = e.story();
    s.pick_vendor(v);
    s.panel_tick();
    assert_eq!(s.state_json()["panel"]["open_bills_count"], 2);
}

#[test]
fn this_windows_own_save_is_told_to_the_panel() {
    let e = env();
    let v = e.story();
    let mut s = EnterBillScreen::new(Arc::clone(&e.link), None, day(2026, 11, 4));
    s.poll();
    s.pick_vendor(v);
    s.panel_tick();
    assert_eq!(s.state_json()["panel"]["open_bills_count"], 0);
    // Saved by this window (the bus never echoes to the sender).
    e.bill(v, REF, day(2026, 11, 3));
    s.publish(Some(v));
    s.panel_tick();
    assert_eq!(s.state_json()["panel"]["open_bills_count"], 1);
}

#[test]
fn custom_terms_take_a_discount_and_preview_its_date() {
    let e = env();
    let mut s = EnterBillScreen::new(Arc::clone(&e.link), None, day(2026, 11, 4));
    s.poll();
    s.pick_vendor(e.story());
    let pct = cl_model::forms::bill::Percent::parse_user("2").expect("2%");
    s.header.edit_step(|h| {
        h.date = Some(day(2026, 11, 3));
        h.terms = Some(cl_model::forms::bill::CUSTOM_TERMS.into());
        h.terms_days = Some(30);
        h.discount_percent = Some(pct);
        h.discount_days = Some(10);
    });
    s.refresh_terms_dates();
    // The local preview (the demo plays the engine without a network).
    assert_eq!(s.header.current().discount_date, Some(day(2026, 11, 13)));
    assert_eq!(s.header.current().due_date, Some(day(2026, 12, 3)));
    s.sync_inputs();
    assert_eq!(s.inputs.discount_percent, "2");
    assert_eq!(s.inputs.discount_days, "10");
    // The owner's discount date wins until cleared.
    s.header
        .edit_step(|h| h.set_discount_by_owner(Some(day(2026, 11, 10))));
    s.refresh_terms_dates();
    assert_eq!(s.header.current().discount_date, Some(day(2026, 11, 10)));
    s.header.edit_step(|h| h.set_discount_by_owner(None));
    s.refresh_terms_dates();
    assert_eq!(s.header.current().discount_date, Some(day(2026, 11, 13)));
}
