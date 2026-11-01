//! Receive Payment's form model: the Auto/Explicit state machine, stale
//! previews, Save waiting for the current preview, and properties that the
//! model never invents money.

use super::*;
use proptest::prelude::*;

fn id(n: u8) -> TransactionId {
    TransactionId::parse(&format!("00000000-0000-4000-8000-0000000000{n:02x}")).unwrap()
}

fn client(n: u8) -> ContactId {
    ContactId::parse(&format!("00000000-0000-4000-8000-0000000002{n:02x}")).unwrap()
}

fn acct(n: u8) -> AccountId {
    AccountId::parse(&format!("00000000-0000-4000-8000-0000000001{n:02x}")).unwrap()
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap()
}

fn day() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).unwrap()
}

fn row(n: u8, number: &str, balance: &str, payment: &str, remaining: &str) -> FigureRow {
    FigureRow {
        invoice: id(n),
        number: number.into(),
        date: day(),
        due: None,
        original: m(balance),
        balance: m(balance),
        token: BalanceToken::from_wire(&format!("{balance}00")),
        payment: m(payment),
        remaining: m(remaining),
        discount: Money::ZERO,
        write_off: Money::ZERO,
        credit: Money::ZERO,
        discount_available: Money::ZERO,
        discount_date: None,
        discount_percent: None,
        suggest_discount: false,
    }
}

/// The mockup's preview (golden case 1).
fn mockup() -> PaymentFigures {
    PaymentFigures {
        balance_before: m("5645.00"),
        balance_after: m("1645.00"),
        amount: m("4000.00"),
        amount_from_rows: false,
        rows: vec![
            row(1, "1091", "4385.00", "4000.00", "385.00"),
            row(2, "1095", "1260.00", "0.00", "1260.00"),
        ],
        amount_due: m("5645.00"),
        applied: m("4000.00"),
        discounts_and_credits: Money::ZERO,
        unapplied: Money::ZERO,
        write_offs: Money::ZERO,
        shortfalls: vec![FigureShortfall {
            invoice: id(1),
            amount: m("385.00"),
            within_limit: false,
        }],
        overpayment: None,
        credits: Vec::new(),
        credit_uses: Vec::new(),
        hold_offered: false,
        pending: Some((2, m("10210.00"))),
        average_days: Some(24),
        problems: Vec::new(),
    }
}

fn form() -> PaymentForm {
    let mut f = PaymentForm::new(Some(day()), Some(acct(1)));
    f.set_client(Some(client(1)));
    f.set_amount(AmountInput::Typed(m("4000")));
    f
}

/// Sends the current request and answers it with `figures`.
fn answer(f: &mut PaymentForm, figures: PaymentFigures) {
    let mut now = 10_000;
    let rev = loop {
        if let Some((rev, _)) = f.poll_preview(now) {
            break rev;
        }
        now += PREVIEW_DEBOUNCE_MS;
    };
    assert!(f.apply_preview(rev, figures));
}

#[test]
fn auto_sends_no_applications_and_save_copies_strings_and_tokens() {
    let mut f = form();
    let req = f.to_preview_request().unwrap();
    assert_eq!(req.rows, None);
    assert_eq!(req.amount, Some(m("4000")));
    assert!(matches!(f.figures(), FiguresView::Updating));
    assert_eq!(
        f.save_plan(PaymentDefaults::default()),
        Err(vec![PaymentProblem::Updating])
    );
    answer(&mut f, mockup());
    let FiguresView::Ready(fig) = f.figures() else {
        panic!("ready")
    };
    assert_eq!(fig.balance_after, m("1645.00"));
    let c = f.callouts();
    assert_eq!(c.underpaid.len(), 1);
    assert!(!c.ask_deposit && c.overpayment.is_none());
    let save = f.save_plan(PaymentDefaults::default()).unwrap();
    assert_eq!(save.amount, m("4000.00"));
    assert_eq!(save.rows.len(), 1);
    assert_eq!(save.rows[0].invoice, id(1));
    assert_eq!(save.rows[0].amount, m("4000.00"));
    assert_eq!(save.rows[0].token.as_wire(), "4385.0000");
    assert_eq!(save.method.as_deref(), Some("check"));
    assert!(!save.hold && save.write_off_account.is_none());
}

#[test]
fn toggle_typing_and_unapply_go_explicit_and_client_change_resets() {
    let mut f = form();
    answer(&mut f, mockup());
    f.toggle(id(2));
    assert_eq!(f.mode(), Mode::Explicit);
    // The engine's allocation is kept as typed rows; 1095 is a fill.
    let rows = f.to_preview_request().unwrap().rows.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].amount, Some(m("4000.00")));
    assert_eq!(rows[1].amount, None);
    f.type_payment(id(2), m("100"));
    assert_eq!(f.row_choice(id(2)), RowChoice::Typed(m("100")));
    f.toggle(id(2));
    assert_eq!(f.row_choice(id(2)), RowChoice::Off);
    f.unapply_all();
    assert_eq!(f.to_preview_request().unwrap().rows, Some(Vec::new()));
    f.auto_apply();
    assert_eq!(f.to_preview_request().unwrap().rows, None);
    f.unapply_all();
    f.set_client(Some(client(2)));
    assert_eq!(f.mode(), Mode::Auto);
    assert_eq!(f.to_preview_request().unwrap().client, client(2));
}

#[test]
fn a_stale_preview_is_dropped_and_save_waits() {
    let mut f = form();
    let (rev, _) = f.poll_preview(0).unwrap();
    assert!(f.in_flight());
    f.set_amount(AmountInput::Typed(m("4001")));
    assert!(!f.apply_preview(rev, mockup()));
    assert!(matches!(f.figures(), FiguresView::Updating));
    // Debounced: nothing goes before the request stood 250 ms.
    assert!(f.poll_preview(100).is_none());
    let (rev2, req) = f.poll_preview(100 + PREVIEW_DEBOUNCE_MS).unwrap();
    assert_eq!(req.amount, Some(m("4001")));
    f.fail_preview(rev2);
    assert!(matches!(f.figures(), FiguresView::CantCalculate));
    assert_eq!(
        f.save_plan(PaymentDefaults::default()),
        Err(vec![PaymentProblem::CantCalculate])
    );
    // Memo and reference don't ask the engine again.
    f.memo = "thanks".into();
    assert!(matches!(f.figures(), FiguresView::CantCalculate));
}

#[test]
fn write_off_copies_the_shortfall_and_needs_an_account() {
    let mut f = form();
    let mut fig = mockup();
    fig.shortfalls[0].within_limit = true;
    answer(&mut f, fig);
    assert!(f.write_off(None));
    let req = f.to_preview_request().unwrap();
    let rows = req.rows.unwrap();
    assert_eq!(rows[0].write_off, Some(m("385.00")));
    assert_eq!(rows[0].amount, Some(m("4000.00")));
    // The engine answers with the write-off on the row.
    let mut after = mockup();
    after.rows[0].write_off = m("385.00");
    after.rows[0].remaining = Money::ZERO;
    after.shortfalls.clear();
    answer(&mut f, after);
    assert!(f.callouts().writing_off);
    assert_eq!(
        f.save_plan(PaymentDefaults::default()),
        Err(vec![PaymentProblem::WriteOffAccount])
    );
    let save = f
        .save_plan(PaymentDefaults {
            write_off_account: Some(acct(9)),
            discount_account: None,
        })
        .unwrap();
    assert_eq!(save.rows[0].write_off, Some(m("385.00")));
    f.leave_open();
    assert!(
        f.to_preview_request().unwrap().rows.unwrap()[0]
            .write_off
            .is_none()
    );
}

#[test]
fn the_deposit_question_blocks_save_until_answered() {
    let mut f = form();
    let mut fig = mockup();
    fig.rows.clear();
    fig.shortfalls.clear();
    fig.applied = Money::ZERO;
    fig.unapplied = m("4000.00");
    fig.overpayment = Some(m("4000.00"));
    fig.hold_offered = true;
    answer(&mut f, fig.clone());
    assert!(f.callouts().ask_deposit);
    assert_eq!(
        f.save_plan(PaymentDefaults::default()),
        Err(vec![PaymentProblem::AskDeposit])
    );
    f.answer_deposit(NothingApplied::Hold);
    assert!(f.to_preview_request().unwrap().hold);
    fig.overpayment = None;
    answer(&mut f, fig);
    let save = f.save_plan(PaymentDefaults::default()).unwrap();
    assert!(save.hold && save.rows.is_empty());
}

#[test]
fn apply_discount_copies_the_engines_figure() {
    let mut f = form();
    let mut fig = mockup();
    fig.rows[0].discount_available = m("87.70");
    fig.rows[0].suggest_discount = true;
    answer(&mut f, fig);
    assert_eq!(f.callouts().discounts, vec![id(1)]);
    assert!(f.apply_discount(id(1)));
    assert_eq!(f.discount(id(1)), Some(m("87.70")));
    let rows = f.to_preview_request().unwrap().rows.unwrap();
    assert_eq!(rows[0].discount, Some(m("87.70")));
    // Nothing to apply without a ready preview.
    assert!(!f.apply_discount(id(2)));
}

#[test]
fn engine_problems_and_missing_fields_block_save() {
    let mut f = PaymentForm::new(None, None);
    assert_eq!(
        f.save_plan(PaymentDefaults::default()),
        Err(vec![
            PaymentProblem::NoClient,
            PaymentProblem::NoDate,
            PaymentProblem::NoDepositTo
        ])
    );
    f = form();
    let mut fig = mockup();
    fig.problems.push(("OVERAPPLIED".into(), Some(id(1))));
    answer(&mut f, fig);
    assert_eq!(
        f.save_plan(PaymentDefaults::default()),
        Err(vec![PaymentProblem::Engine("OVERAPPLIED".into())])
    );
    let mut zero = mockup();
    zero.amount = Money::ZERO;
    f.set_amount(AmountInput::Typed(Money::ZERO));
    answer(&mut f, zero);
    assert!(
        f.save_plan(PaymentDefaults::default())
            .unwrap_err()
            .contains(&PaymentProblem::NoAmount)
    );
}

#[test]
fn credits_go_in_the_request_and_save_copies_the_engines_amount() {
    let mut f = form();
    f.use_credit(id(7), id(2), None);
    let req = f.to_preview_request().unwrap();
    assert_eq!(req.credits.len(), 1);
    let mut fig = mockup();
    fig.credit_uses.push(FigureCreditUse {
        source: id(7),
        invoice: id(2),
        amount: m("55.00"),
    });
    answer(&mut f, fig);
    let save = f.save_plan(PaymentDefaults::default()).unwrap();
    assert_eq!(save.credits[0].amount, m("55.00"));
    assert_eq!(save.credits[0].token.as_wire(), "1260.0000");
    f.drop_credit(id(7));
    assert_eq!(f.to_preview_request().unwrap().credits, Vec::new());
}

/// A saved payment of 4,000.00: 3,615.00 cash and a 385.00 write-off on
/// 1091, a 30.00 discount, and 55.00 of an older credit on 1095.
fn saved() -> LoadedPayment {
    LoadedPayment {
        client: client(1),
        date: day(),
        amount: m("4000.00"),
        deposit_to: Some(acct(1)),
        method: "check".into(),
        ref_no: "5521".into(),
        memo: "thanks".into(),
        rows: vec![(id(1), m("3585.00"), Some(m("30.00")), Some(m("385.00")))],
        credits: vec![(id(7), id(2), m("55.00"))],
        write_off_account: Some(acct(8)),
        discount_account: Some(acct(9)),
        hold: false,
    }
}

/// The engine's preview of [`saved`] as it stands.
fn saved_figures() -> PaymentFigures {
    let mut fig = mockup();
    fig.rows[0].payment = m("3585.00");
    fig.rows[0].discount = m("30.00");
    fig.rows[0].write_off = m("385.00");
    fig.rows[0].remaining = Money::ZERO;
    fig.rows[1].credit = m("55.00");
    fig.rows[1].remaining = m("1205.00");
    fig.amount = m("4000.00");
    fig.applied = m("3585.00");
    fig.unapplied = m("415.00");
    fig.shortfalls.clear();
    fig.credit_uses.push(FigureCreditUse {
        source: id(7),
        invoice: id(2),
        amount: m("55.00"),
    });
    fig
}

#[test]
fn a_saved_payment_saves_back_unchanged_with_its_credits_and_accounts() {
    let mut f = PaymentForm::default();
    let editing = Editing {
        id: id(40),
        updated_at: Timestamp::from_unix_millis(1_700_000_000_000).unwrap(),
    };
    f.load_saved(editing, saved());
    assert_eq!(f.editing(), Some(editing));
    // The preview asks with the credit the payment used (D-B1).
    let req = f.to_preview_request().unwrap();
    assert_eq!(req.editing, Some(id(40)));
    assert_eq!(
        req.credits,
        vec![CreditChoice {
            source: id(7),
            invoice: id(2),
            amount: Some(m("55.00")),
        }]
    );
    let rows = req.rows.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].amount, Some(m("3585.00")));
    assert_eq!(rows[0].discount, Some(m("30.00")));
    assert_eq!(rows[0].write_off, Some(m("385.00")));
    answer(&mut f, saved_figures());
    // Save unchanged: the same body, the same accounts (not the company
    // defaults), the same credit.
    let defaults = PaymentDefaults {
        write_off_account: Some(acct(3)),
        discount_account: Some(acct(4)),
    };
    let save = f.save_plan(defaults).unwrap();
    assert_eq!(save.amount, m("4000.00"));
    assert_eq!(save.deposit_to, acct(1));
    assert_eq!(save.write_off_account, Some(acct(8)));
    assert_eq!(save.discount_account, Some(acct(9)));
    assert_eq!(save.rows.len(), 1);
    assert_eq!(save.rows[0].amount, m("3585.00"));
    assert_eq!(save.rows[0].discount, Some(m("30.00")));
    assert_eq!(save.rows[0].write_off, Some(m("385.00")));
    assert_eq!(save.credits.len(), 1);
    assert_eq!(
        (
            save.credits[0].source,
            save.credits[0].invoice,
            save.credits[0].amount
        ),
        (id(7), id(2), m("55.00"))
    );
    assert_eq!(save.credits[0].token.as_wire(), "1260.0000");
    assert_eq!(
        (save.ref_no.as_deref(), save.memo.as_deref()),
        (Some("5521"), Some("thanks"))
    );
    // A memo edit changes nothing the engine computes: the same plan.
    f.memo = "thank you".into();
    let again = f.save_plan(defaults).unwrap();
    assert_eq!(again.credits, save.credits);
    assert_eq!(again.rows, save.rows);
    // With no account saved, the company default still applies.
    let mut plain = saved();
    plain.write_off_account = None;
    let mut g = PaymentForm::default();
    g.load_saved(editing, plain);
    assert_eq!(g.under(), Under::WriteOff { account: None });
}

#[test]
fn credits_alone_are_not_a_payment() {
    // The engine refuses a payment of 0 that only uses credits
    // (AMOUNT_REQUIRED); Save says nothing was received.
    let mut f = form();
    f.set_amount(AmountInput::Typed(Money::ZERO));
    f.use_credit(id(7), id(2), None);
    let mut fig = mockup();
    fig.amount = Money::ZERO;
    fig.applied = Money::ZERO;
    fig.rows[0].payment = Money::ZERO;
    fig.rows[1].credit = m("55.00");
    fig.shortfalls.clear();
    fig.credit_uses.push(FigureCreditUse {
        source: id(7),
        invoice: id(2),
        amount: m("55.00"),
    });
    answer(&mut f, fig.clone());
    assert_eq!(
        f.save_plan(PaymentDefaults::default()),
        Err(vec![PaymentProblem::NoAmount])
    );
    // A write-off of 0 cash is still a payment (no deposit line).
    fig.rows[0].write_off = m("385.00");
    f.refresh();
    answer(&mut f, fig);
    assert!(
        f.save_plan(PaymentDefaults {
            write_off_account: Some(acct(9)),
            discount_account: None,
        })
        .is_ok()
    );
}

#[derive(Debug, Clone)]
enum Edit {
    Amount(i64),
    Toggle(u8),
    Type(u8, i64),
    Unapply,
    Auto,
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (0i64..10_000_000).prop_map(Edit::Amount),
        (1u8..4).prop_map(Edit::Toggle),
        ((1u8..4), (0i64..10_000_000)).prop_map(|(r, c)| Edit::Type(r, c)),
        Just(Edit::Unapply),
        Just(Edit::Auto),
    ]
}

proptest! {
    /// For any sequence of edits the request carries exactly what was
    /// typed, byte for byte.
    #[test]
    fn the_request_carries_what_was_typed(edits in proptest::collection::vec(edit(), 0..20)) {
        let mut f = form();
        let mut amount = Some(m("4000"));
        let mut typed: BTreeMap<TransactionId, Money> = BTreeMap::new();
        for e in edits {
            match e {
                Edit::Amount(c) => {
                    let v = Money::from_cents(c).unwrap();
                    f.set_amount(AmountInput::Typed(v));
                    amount = Some(v);
                }
                Edit::Toggle(r) => { f.toggle(id(r)); typed.remove(&id(r)); }
                Edit::Type(r, c) => {
                    let v = Money::from_cents(c).unwrap();
                    f.type_payment(id(r), v);
                    typed.insert(id(r), v);
                }
                Edit::Unapply | Edit::Auto => {
                    if matches!(e, Edit::Unapply) { f.unapply_all() } else { f.auto_apply() }
                    typed.clear();
                }
            }
        }
        let req = f.to_preview_request().unwrap();
        prop_assert_eq!(req.amount.map(Money::to_wire), amount.map(Money::to_wire));
        if let Some(rows) = req.rows {
            for r in rows {
                if let Some(v) = typed.get(&r.invoice) {
                    prop_assert_eq!(r.amount.map(Money::to_wire), Some(v.to_wire()));
                }
            }
        }
    }

    /// Save never invents money: every amount it sends is one of the ready
    /// preview's figures.
    #[test]
    fn save_amounts_come_from_the_preview(
        pay in proptest::collection::vec(0i64..1_000_000, 1..5),
    ) {
        let mut f = form();
        let mut fig = mockup();
        fig.rows = pay
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let v = Money::from_cents(*c).unwrap().to_wire();
                let n = u8::try_from(i + 1).unwrap();
                row(n, "1", &v, &v, "0")
            })
            .collect();
        fig.shortfalls.clear();
        fig.amount = Money::from_cents(pay.iter().sum()).unwrap();
        answer(&mut f, fig.clone());
        let save = f.save_plan(PaymentDefaults::default()).unwrap();
        let figures: Vec<Money> = fig.rows.iter().map(|r| r.payment).chain([fig.amount]).collect();
        prop_assert!(figures.contains(&save.amount));
        for r in &save.rows {
            prop_assert!(figures.contains(&r.amount));
            prop_assert_eq!(&r.token, &fig.row(r.invoice).unwrap().token);
        }
    }
}
