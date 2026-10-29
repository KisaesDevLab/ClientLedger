//! Pay Bills against a test engine with the mockup's bills (P18).

use std::sync::{Arc, Mutex};

use cl_api::{
    Account, ApiError, CreatedPayment, PayBillsRequest, PayMethod, PayableFilter, PayableList,
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
    passwords: usize,
    discount_default: Option<AccountId>,
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
            keyed: true,
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
        Ok(list())
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
    ) -> Result<Vec<CreatedPayment>, ApiError> {
        let mut s = self.seen.lock().expect("seen");
        s.pays.push(req.clone());
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
