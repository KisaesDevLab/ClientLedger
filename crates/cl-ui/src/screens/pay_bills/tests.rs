//! Pay Bills against a test engine with the mockup's bills (P18).

use std::sync::{Arc, Mutex};

use cl_api::{
    Account, ApiError, CreatedPayment, IdempotencyKey, PayBillsRequest, PayMethod, PayableFilter,
    PayableList,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};

use super::engine::{LocalPayBills, PayCaps, Setup, Viewed};
use super::*;
use crate::company::CompanyLink;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

/// What the test engine saw.
#[derive(Default)]
struct Seen {
    pays: Vec<PayBillsRequest>,
    keys: Vec<IdempotencyKey>,
    passwords: usize,
    discount_default: Option<AccountId>,
    /// The list it answers (tests change it between reads).
    list: Option<PayableList>,
}

/// The mockup's list: five bills, 7781 with the engine's 4.37, the Table
/// Rock credit CM-114. Bank 84,210.55, card owing 1,250.00.
struct Fake {
    seen: Arc<Mutex<Seen>>,
    refuse: Option<ApiError>,
    discount_default: Option<AccountId>,
}

fn chart() -> Vec<Account> {
    crate::company::demo::chart()
}

fn by_number(n: &str) -> AccountId {
    chart()
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some(n))
        .map_or_else(|| panic!("{n}"), |a| a.id)
}

fn card_account() -> Option<AccountId> {
    chart()
        .iter()
        .find(|a| crate::screens::register::class_of(a) == Some(AccountClass::CreditCard))
        .map(|a| a.id)
}

fn list() -> PayableList {
    let v = |n: u8| format!("c0000000-0000-4000-8000-0000000000{n:02x}");
    let b = |n: u8| format!("d0000000-0000-4000-8000-0000000000{n:02x}");
    let row = |n: u8, name: &str, r: &str, due: &str, amt: &str, dd: Option<&str>, da: &str| {
        serde_json::json!({
            "id": b(n), "contactId": v(n), "contactName": name, "txnDate": "2026-10-20",
            "dueDate": due, "discountDate": dd, "vendorInvoiceNumber": r,
            "balanceDue": amt, "discountAvailable": da, "paymentTerms": "Net 30",
        })
    };
    serde_json::from_value(serde_json::json!({
        "bills": [
            row(1, "Ozark Feed & Seed", "20931", "2026-11-05", "1286.40", None, "0.00"),
            row(2, "Riverbend Fuel", "STMT-1031", "2026-11-06", "1412.06", None, "0.00"),
            row(3, "Monett Tire & Lube", "2214", "2026-11-08", "640.00", None, "0.00"),
            row(4, "Ozark Office Supply", "7781", "2026-11-10", "218.44", Some("2026-11-06"), "4.37"),
            row(5, "Table Rock Nursery", "8813", "2026-11-11", "3915.00", None, "0.00"),
        ],
        "credits": [{ "id": b(9), "contactId": v(5), "txnDate": "2026-10-20",
            "balanceDue": "250.00", "vendorInvoiceNumber": "CM-114" }],
    }))
    .expect("list")
}

impl LocalPayBills for Fake {
    fn caps(&self) -> PayCaps {
        PayCaps {
            discounts: true,
            numbers: true,
            closing: true,
        }
    }
    fn setup(&mut self, _link: &CompanyLink) -> Result<Setup, ApiError> {
        Ok(Setup {
            accounts: chart(),
            closing: CalendarDate::from_ymd(2026, 9, 30).ok(),
            discount_account: self.discount_default,
        })
    }
    fn payable(&mut self, _l: &CompanyLink, _f: &PayableFilter) -> Result<PayableList, ApiError> {
        Ok(self
            .seen
            .lock()
            .expect("seen")
            .list
            .clone()
            .unwrap_or_else(list))
    }
    fn balance(&mut self, _l: &CompanyLink, account: AccountId) -> Option<Money> {
        Some(if Some(account) == card_account() {
            m("-1250.00")
        } else {
            m("84210.55")
        })
    }
    fn numbers(&mut self, _l: &CompanyLink, _a: AccountId) -> (Option<u32>, Option<u32>) {
        (None, Some(4418))
    }
    fn pay(
        &mut self,
        _l: &CompanyLink,
        req: &PayBillsRequest,
        password: Option<&Secret>,
        key: &IdempotencyKey,
    ) -> Result<Vec<CreatedPayment>, ApiError> {
        let mut s = self.seen.lock().expect("seen");
        s.pays.push(req.clone());
        s.keys.push(key.clone());
        s.passwords += usize::from(password.is_some());
        if let Some(e) = self.refuse.take() {
            return Err(e);
        }
        Ok(req
            .bills
            .iter()
            .map(|b| CreatedPayment {
                id: b.bill,
                contact_id: None,
                txn_date: req.date,
                check_number: None,
                print_status: Some("queue".into()),
                net_payment: b.cash,
                discount_total: Some(b.discount),
                bills_paid: 1,
                credits_applied: 0,
            })
            .collect())
    }
    fn set_discount_account(&mut self, _l: &CompanyLink, a: AccountId) -> Result<(), ApiError> {
        self.seen.lock().expect("seen").discount_default = Some(a);
        Ok(())
    }
    fn get(&mut self, _l: &CompanyLink, _id: TransactionId) -> Result<Viewed, ApiError> {
        Err(ApiError::NotFound { code: None })
    }
    fn void(
        &mut self,
        _l: &CompanyLink,
        _id: TransactionId,
        _p: Option<&Secret>,
    ) -> Result<(), ApiError> {
        Ok(())
    }
}

fn screen(
    discount_default: Option<AccountId>,
) -> (tempfile::TempDir, PayBillsScreen, Arc<Mutex<Seen>>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fake = Fake {
        seen: Arc::clone(&seen),
        refuse: None,
        discount_default,
    };
    let s = PayBillsScreen::with_local(link, None, today(), Some(Box::new(fake)));
    (dir, s, seen)
}

fn bill(n: u8) -> TransactionId {
    TransactionId::parse(&format!("d0000000-0000-4000-8000-0000000000{n:02x}")).expect("id")
}

fn select_mockup(s: &mut PayBillsScreen) {
    for n in [1, 2, 4, 5] {
        s.form.toggle(bill(n));
    }
}

#[test]
fn opens_on_the_mockup_list_and_bank() {
    let (_d, s, _) = screen(None);
    let st = s.state();
    assert_eq!(st["screen"], "pay_bills");
    assert_eq!(st["listed"], 5);
    assert_eq!(st["selected"], 0);
    assert_eq!(st["method"], "check");
    assert_eq!(st["numbering"], "print_later", "To be printed by default");
    assert_eq!(st["first_number"], 4419);
    assert_eq!(st["balance_shown"], true);
    assert_eq!(s.pay_from(), Some(by_number("1010")));
    let text = st.to_string();
    for figure in ["84210", "218", "Ozark", "7781"] {
        assert!(!text.contains(figure), "{figure} in {text}");
    }
}

#[test]
fn the_mockup_run_totals_and_pays_to_the_penny() {
    let (_d, mut s, seen) = screen(Some(by_number("6210")));
    select_mockup(&mut s);
    let t = s.form.totals().expect("totals");
    assert_eq!(t.amount_due, m("6831.90"), "selected bills only");
    assert_eq!(t.discount, m("4.37"));
    assert_eq!(t.credits, m("250.00"));
    assert_eq!(t.cash, m("6577.53"));
    assert_eq!(s.preview(), Some((m("84210.55"), m("77633.02"))));
    assert!(s.start_pay(None));
    let pays = &seen.lock().expect("seen").pays;
    assert_eq!(pays.len(), 1);
    let r = &pays[0];
    assert_eq!(
        r.method,
        PayMethod::Check(cl_api::CheckNumbering::PrintLater)
    );
    assert_eq!(r.discount_account, Some(by_number("6210")));
    let office = r.bills.iter().find(|b| b.bill == bill(4)).expect("7781");
    assert_eq!((office.cash, office.discount), (m("214.07"), m("4.37")));
    assert_eq!(r.credits.len(), 1);
    assert_eq!(r.credits[0].amount, m("250.00"));
    assert_eq!(s.state()["paid_count"], 1);
    assert_eq!(s.state()["selected"], 0, "cleared after paying");
}

#[test]
fn a_discount_without_an_account_asks_for_one_and_remembers_it() {
    let (_d, mut s, seen) = screen(None);
    s.form.toggle(bill(4));
    assert!(!s.start_pay(None));
    assert_eq!(seen.lock().expect("seen").pays.len(), 0, "nothing sent");
    assert_eq!(s.state()["dialog"], "discount");
    if let Some(Dialog::Discount { account, .. }) = s.dialog.as_mut() {
        *account = Some(by_number("6210"));
    }
    assert!(s.apply_discount());
    s.poll();
    assert_eq!(
        seen.lock().expect("seen").discount_default,
        Some(by_number("6210")),
        "saved as the company default (Q-P18a)"
    );
    assert!(s.start_pay(None));
}

#[test]
fn a_card_payment_raises_the_amount_owed() {
    let (_d, mut s, seen) = screen(None);
    s.method = Method::Card;
    s.reference = "AUTH-77".into();
    s.fetch_account_facts();
    s.poll();
    s.form.toggle(bill(3));
    assert_eq!(s.preview(), Some((m("1250.00"), m("1890.00"))));
    assert!(s.start_pay(None));
    let r = seen.lock().expect("seen").pays[0].clone();
    assert_eq!(Some(r.bank_account), card_account());
    assert_eq!(
        r.method,
        PayMethod::CreditCard {
            reference: Some("AUTH-77".into())
        }
    );
}

#[test]
fn a_closed_period_date_takes_the_password() {
    let (_d, mut s, seen) = screen(None);
    s.date = crate::screens::company::form::DateInput::new(
        CalendarDate::from_ymd(2026, 9, 15).expect("d"),
    );
    s.form.toggle(bill(1));
    assert!(!s.start_pay(None));
    assert_eq!(seen.lock().expect("seen").pays.len(), 0);
    assert!(s.closing_prompt.dialog_open());
    assert!(s.start_pay(Some(Secret::new("demo-close"))));
    assert_eq!(seen.lock().expect("seen").passwords, 1);
}

#[test]
fn changed_bills_read_the_list_again() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let seen = Arc::new(Mutex::new(Seen::default()));
    let refused = cl_api::engine_refusal(
        400,
        &serde_json::json!({"error": {"message": "Payment amount exceeds balance due"}}),
    );
    let fake = Fake {
        seen,
        refuse: Some(refused),
        discount_default: None,
    };
    let mut s = PayBillsScreen::with_local(link, None, today(), Some(Box::new(fake)));
    s.form.toggle(bill(1));
    assert!(!s.start_pay(None));
    assert_eq!(s.state()["notice"], "paybills.changed");
    assert!(s.form.is_selected(bill(1)), "the selection stays");
}

#[test]
fn nothing_selected_is_refused_locally() {
    let (_d, mut s, seen) = screen(None);
    assert!(!s.start_pay(None));
    assert_eq!(seen.lock().expect("seen").pays.len(), 0);
    assert_eq!(
        s.state()["problems"][0],
        "paybills.problem.nothing_selected"
    );
}

#[test]
fn the_grid_spec_is_valid_and_space_selects() {
    use cl_grid::{EditableModel, RowModel};
    assert_eq!(grid::spec().validate(), Ok(()));
    let (_d, mut s, _) = screen(None);
    let mut rev = 0;
    let mut rows = grid::Rows::new(&mut s.form, &mut rev);
    let key = rows.row_key(3).expect("7781");
    rows.set_cell(
        cl_grid::CellPos {
            row: key,
            col: grid::SELECT,
        },
        cl_grid::CellInput::Bool(true),
        cl_grid::Source::Typed,
    )
    .expect("tick");
    assert!(matches!(rows.cell(3, grid::TO_PAY), cl_grid::CellValue::Money(x) if x == m("214.07")));
    assert!(matches!(rows.cell(3, grid::DISCOUNT), cl_grid::CellValue::Money(x) if x == m("4.37")));
    assert_eq!(rows.row_label(3), "Ozark Office Supply 7781");
}

/// Q-P18f: a demo bill payment opens read-only; Void takes it off the bill.
#[test]
fn the_bill_payment_view_shows_and_voids_a_payment() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let mut pay = PayBillsScreen::new(Arc::clone(&link), None, today());
    let first = pay.form.visible().first().map(|r| (r.bill, r.balance_due));
    let (b, due) = first.expect("an open sample bill");
    pay.form.toggle(b);
    assert!(pay.start_pay(None), "{:?}", pay.state());
    let id = {
        let s = link.demo_state().expect("demo");
        s.ledger
            .transactions()
            .last()
            .map(crate::company::demo::register::DemoLedger::id_of)
    }
    .expect("posted");
    pay.poll();
    assert!(
        !pay.form.all_rows().iter().any(|r| r.bill == b),
        "paid in full"
    );
    let mut view = view::BillPaymentView::new(Arc::clone(&link));
    assert!(view.open_record(id));
    let st = view.state();
    assert_eq!(st["loaded"], true, "{st}");
    assert_eq!(st["bills"], 1);
    assert_eq!(st["status"], "posted");
    assert!(view.command(CommandId::new("edit.void_transaction")));
    view.set_void_reason("paid twice");
    view.confirm_void();
    let st = view.state();
    assert_eq!(st["voided"], true, "{st}");
    assert_eq!(st["status"], "void");
    pay.load_list();
    pay.poll();
    let again = pay
        .form
        .all_rows()
        .iter()
        .find(|r| r.bill == b)
        .map(|r| r.balance_due);
    assert_eq!(again, Some(due), "the bill is open again");
}

fn lost_reply() -> ApiError {
    ApiError::OutcomeUnknown {
        request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000003")
            .expect("id"),
        idempotency_key: Some(IdempotencyKey::new()),
        keyed: false,
    }
}

/// 8813 with 1,000.00 cash and CM-114, sent with the reply lost.
fn lost_partial_pay() -> (tempfile::TempDir, PayBillsScreen, Arc<Mutex<Seen>>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fake = Fake {
        seen: Arc::clone(&seen),
        refuse: Some(lost_reply()),
        discount_default: None,
    };
    let mut s = PayBillsScreen::with_local(link, None, today(), Some(Box::new(fake)));
    s.form.set_cash(bill(5), m("1000.00"));
    assert!(!s.start_pay(None));
    (dir, s, seen)
}

/// Review 6: an unknown outcome blocks paying and closing quietly.
#[test]
fn an_unknown_outcome_blocks_paying() {
    let (_d, mut s, seen) = lost_partial_pay();
    let st = s.state();
    assert_eq!(st["unknown"], true);
    assert_eq!(st["pending"], true);
    assert!(s.save_unconfirmed(), "closing asks first");
    s.poll();
    assert!(!s.start_pay(None), "blocked until Look again");
    assert_eq!(seen.lock().expect("seen").pays.len(), 1);
}

/// Review 6: the list as it was (the pay did not go through): Look again
/// and the identical pay goes under the same key.
#[test]
fn an_identical_retry_after_look_again_reuses_the_key() {
    let (_d, mut s, seen) = lost_partial_pay();
    s.poll();
    assert_eq!(s.state()["pending"], true, "the list did not move");
    s.recheck();
    s.poll();
    assert!(s.form.is_selected(bill(5)), "the entries stay");
    assert_eq!(s.form.cash(bill(5)), m("1000.00"));
    assert!(s.start_pay(None));
    let seen = seen.lock().expect("seen");
    assert_eq!(seen.pays.len(), 2);
    assert_eq!(seen.pays[0], seen.pays[1], "the same pay");
    assert_eq!(seen.keys[0], seen.keys[1], "under the same key");
}

/// Review 1: the list read again shows the pay went through (8813 owes
/// 2,665.00, CM-114 is used up): the form and the key are cleared, nothing
/// is paid twice, and the screen says so.
#[test]
fn a_changed_list_after_an_unknown_outcome_does_not_pay_twice() {
    let (_d, mut s, seen) = lost_partial_pay();
    {
        let mut l = list();
        if let Some(b) = l.bills.iter_mut().find(|b| b.id == bill(5)) {
            b.balance_due = m("2665.00");
        }
        l.credits.clear();
        seen.lock().expect("seen").list = Some(l);
    }
    // The list read at once still had them; Look again reads it anew.
    s.poll();
    assert_eq!(s.state()["pending"], true);
    s.recheck();
    s.poll();
    let st = s.state();
    assert_eq!(st["pending"], false);
    assert_eq!(st["unknown"], false);
    assert_eq!(st["selected"], 0, "{st}");
    assert_eq!(st["notice"], "paybills.unknown.went_through");
    assert!(!s.form.is_selected(bill(5)));
    assert!(!s.start_pay(None), "nothing selected");
    assert_eq!(seen.lock().expect("seen").pays.len(), 1, "paid once");
}

/// Review 6: a failure that is not an unknown outcome forgets the pending
/// pay.
#[test]
fn a_refusal_is_not_an_unknown_outcome() {
    let (_d, mut s, _seen) = lost_partial_pay();
    s.pay_failed(ApiError::NotFound { code: None });
    assert_eq!(s.state()["pending"], false);
}

/// Review 4 (Q-P18a): the company's default discount account is set only
/// when it has none; another pick is for this run.
#[test]
fn another_discount_account_does_not_replace_the_default() {
    let (_d, mut s, seen) = screen(Some(by_number("6210")));
    s.form.toggle(bill(4));
    s.open_discount(bill(4), false);
    if let Some(Dialog::Discount { account, .. }) = s.dialog.as_mut() {
        *account = Some(by_number("5200"));
    }
    assert!(s.apply_discount());
    s.poll();
    assert_eq!(
        seen.lock().expect("seen").discount_default,
        None,
        "not saved"
    );
    assert_eq!(s.discount_account, Some(by_number("5200")));
    assert!(s.start_pay(None));
    assert_eq!(
        seen.lock().expect("seen").pays[0].discount_account,
        Some(by_number("5200"))
    );
}

/// Review (Low): a first check number the owner typed stays when the
/// numbers are read again; another account proposes its own.
#[test]
fn a_typed_first_number_is_kept() {
    let (_d, mut s, _) = screen(None);
    assert_eq!(s.first_number, "4419");
    s.first_number = "5000".into();
    s.fetch_account_facts();
    s.poll();
    assert_eq!(s.first_number, "5000", "kept");
    s.account_changed();
    s.poll();
    assert_eq!(s.first_number, "4419", "the new account's proposal");
}

/// Review (Low): Pay waits while the list is read again for a new date.
#[test]
fn pay_waits_for_the_list() {
    let (_d, mut s, seen) = screen(None);
    s.form.toggle(bill(1));
    s.date_changed();
    assert!(!s.start_pay(None), "the list is being read");
    s.poll();
    assert!(s.start_pay(None));
    assert_eq!(seen.lock().expect("seen").pays.len(), 1);
}

/// Review (Low): selected bills a filter hides are kept, not paid, and
/// stay selected after the others are paid.
#[test]
fn hidden_selected_bills_are_counted_and_kept() {
    let (_d, mut s, seen) = screen(None);
    s.form.toggle(bill(1));
    s.form.toggle(bill(2));
    s.form.vendor = s.form.all_rows()[0].vendor;
    let st = s.state();
    assert_eq!(
        (st["selected"].clone(), st["hidden_selected"].clone()),
        (1.into(), 1.into())
    );
    assert!(s.start_pay(None));
    assert_eq!(seen.lock().expect("seen").pays[0].bills.len(), 1);
    assert!(s.form.is_selected(bill(2)), "the hidden one stays");
}

#[test]
fn debug_shows_no_typed_amounts() {
    let d = Dialog::Discount {
        bill: bill(4),
        text: "4.37".into(),
        account: None,
        asked: false,
    };
    let c = Dialog::Credits {
        bill: bill(5),
        texts: vec![(bill(9), "250.00".into())],
    };
    let shown = format!("{d:?} {c:?}");
    assert!(!shown.contains("4.37") && !shown.contains("250"), "{shown}");
}
