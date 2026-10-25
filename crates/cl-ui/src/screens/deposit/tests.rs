//! Make Deposit without an engine: the demo engine and a test double that
//! plays the P14 engine (pending payments, groupings, the 409).

use std::sync::{Arc, Mutex};

use cl_api::{
    ApiError, BankPosting, DepositCaps, DepositDraft, ErrorCode, FeedDeposit, FeedGrouping,
    FeedGroupings, FeedItemId, PendingDeposits, PendingPayment, RecordVersion, RegisterAmount,
    Transaction, UserText, Versioned,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use cl_model::forms::SaveIntent;
use cl_model::forms::deposit::{CashBackForm, check};
use cl_model::forms::lines::FormLine;

use super::*;
use crate::company::CompanyLink;

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn account(n: u32) -> AccountId {
    crate::company::demo::account_id(n)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn txn(n: u8) -> TransactionId {
    TransactionId::parse(&format!("5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02}")).expect("id")
}

fn payment(n: u8, name: &str, amount: &str) -> PendingPayment {
    PendingPayment {
        source: txn(n),
        txn_type: "customer_payment".into(),
        date: today(),
        received_from: Some(name.into()),
        contact: None,
        ref_no: None,
        method: Some("check".into()),
        memo: None,
        amount: RegisterAmount::Exact(money(amount)),
    }
}

#[derive(Default)]
struct Log {
    saved: Vec<DepositDraft>,
    /// The next save answers 409 ALREADY_DEPOSITED.
    conflict_next: bool,
    /// The next save is posted but its reply is lost (B2).
    lost_next: bool,
    /// What pending answers.
    pending: Vec<PendingPayment>,
    record: Option<Transaction>,
}

/// The P14 engine as a test double: everything the shapes file promises.
struct Fake(Arc<Mutex<Log>>, DepositCaps);

impl Fake {
    fn new(caps: DepositCaps) -> (Fake, Arc<Mutex<Log>>) {
        let log = Arc::new(Mutex::new(Log {
            pending: vec![
                payment(1, "Hollister Family Trust", "1250.00"),
                payment(2, "Maple Street HOA", "842.50"),
            ],
            ..Log::default()
        }));
        (Fake(Arc::clone(&log), caps), log)
    }
}

fn all_caps() -> DepositCaps {
    DepositCaps {
        grouping: true,
        cashback: true,
        vendor_refund: true,
    }
}

impl LocalDeposits for Fake {
    fn caps(&self) -> DepositCaps {
        self.1
    }
    fn feed_groupings(&self) -> bool {
        true
    }
    fn pending(&mut self, _link: &CompanyLink) -> Result<PendingDeposits, ApiError> {
        let log = self.0.lock().expect("log");
        Ok(PendingDeposits {
            exact: true,
            clearing_balance: RegisterAmount::Exact(money("2092.50")),
            items: log.pending.clone(),
        })
    }
    fn waiting(&mut self, _link: &CompanyLink) -> Vec<FeedDeposit> {
        vec![FeedDeposit {
            id: FeedItemId::parse("7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"),
            date: today(),
            amount: money("2092.50"),
            description: None,
            account_name: None,
        }]
    }
    fn groupings(
        &mut self,
        _link: &CompanyLink,
        _id: FeedItemId,
    ) -> Result<FeedGroupings, ApiError> {
        let log = self.0.lock().expect("log");
        Ok(FeedGroupings {
            target: money("2092.50"),
            candidates: vec![FeedGrouping {
                items: log.pending.clone(),
            }],
            truncated: false,
        })
    }
    fn save(
        &mut self,
        _link: &CompanyLink,
        draft: &DepositDraft,
        _edit: Option<(TransactionId, RecordVersion)>,
        _password: Option<&Secret>,
    ) -> Result<Saved, ApiError> {
        let mut log = self.0.lock().expect("log");
        if std::mem::take(&mut log.conflict_next) {
            // Another window deposited Maple Street HOA meanwhile.
            log.pending.retain(|p| p.source != txn(2));
            return Err(ApiError::Conflict {
                reason: cl_api::ConflictReason::Unspecified,
                current_version: None,
                code: ErrorCode::parse(cl_api::ALREADY_DEPOSITED),
                message: UserText::default(),
            });
        }
        log.saved.push(draft.clone());
        if std::mem::take(&mut log.lost_next) {
            return Err(ApiError::OutcomeUnknown {
                request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000004")
                    .expect("id"),
                idempotency_key: Some(cl_api::IdempotencyKey::new()),
            });
        }
        let picked: Vec<_> = draft.payments.iter().map(|p| p.source).collect();
        log.pending.retain(|p| !picked.contains(&p.source));
        Ok(Saved {
            id: txn(99),
            version: RecordVersion::Fingerprint([0; 32]),
            date: draft.date,
            clearing_negative: false,
        })
    }
    fn get(
        &mut self,
        _link: &CompanyLink,
        _id: TransactionId,
    ) -> Result<Versioned<Transaction>, ApiError> {
        let log = self.0.lock().expect("log");
        let record = log
            .record
            .clone()
            .ok_or(ApiError::NotFound { code: None })?;
        Ok(Versioned {
            record,
            version: RecordVersion::Fingerprint([1; 32]),
        })
    }
    fn list(&mut self, _link: &CompanyLink, _account: AccountId) -> Vec<DepositRow> {
        Vec::new()
    }
    fn void(
        &mut self,
        _link: &CompanyLink,
        _id: TransactionId,
        _password: Option<&Secret>,
    ) -> Result<(), ApiError> {
        Ok(())
    }
    fn balance(&mut self, _link: &CompanyLink, _account: AccountId) -> Option<Money> {
        Some(money("10000.00"))
    }
}

fn screen(caps: DepositCaps) -> (DepositScreen, Arc<Mutex<Log>>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let (fake, log) = Fake::new(caps);
    let s = DepositScreen::with_local(link, None, today(), Some(Box::new(fake)));
    (s, log, dir)
}

fn set_lines(s: &mut DepositScreen, lines: &[(u32, &str)]) {
    let keys = &mut s.lines.form_mut().adapter_mut().keys;
    let mut seeded: Vec<FormLine> = lines
        .iter()
        .map(|(n, amount)| FormLine {
            account: Some(account(*n)),
            amount: Some(money(amount)),
            ..FormLine::blank(keys.next_key())
        })
        .collect();
    seeded.push(FormLine::blank(keys.next_key()));
    s.lines.form_mut().edit_step(|v| v.lines = seeded);
}

fn set_cash_back(s: &mut DepositScreen, n: u32, amount: &str) {
    s.header.edit_step(|h| {
        h.cash_back = CashBackForm {
            account: Some(account(n)),
            amount: Some(money(amount)),
            memo: String::new(),
        };
    });
    s.sync_inputs();
}

#[test]
fn the_form_opens_on_the_operating_account_with_the_payments_waiting() {
    let (s, _log, _dir) = screen(all_caps());
    assert!(s.loaded);
    assert_eq!(s.account(), Some(account(1010)));
    assert_eq!(
        s.panel.balance,
        Some(money("10000.00")),
        "the engine's balance"
    );
    let v = s.state_json();
    assert_eq!(v["screen"], "deposit");
    assert_eq!(v["pending"]["count"], 2);
    assert_eq!(v["pending"]["clearing_balance"], "2092.50");
    assert_eq!(v["waiting"], 1);
    let text = v.to_string();
    assert!(
        !text.contains("Hollister") && !text.contains("Maple"),
        "{text}"
    );
}

#[test]
fn a_bank_deposit_click_ticks_the_engines_grouping_and_the_save_publishes() {
    let (mut s, log, _dir) = screen(all_caps());
    let fd = s.panel.waiting[0].clone();
    s.choose_feed(fd);
    assert_eq!(s.header.current().picked.len(), 2, "both payments ticked");
    assert!(!s.is_dirty() || s.save.count == 0);
    let v = s.state_json();
    assert_eq!(v["totals"]["total"], "2,092.50");
    assert_eq!(v["bank_match"], "matches");
    let mut bus = s.link.bus().subscribe();
    assert!(s.start_save(SaveIntent::Save, None));
    let saved = log.lock().expect("log").saved.clone();
    assert_eq!(saved.len(), 1);
    let d = &saved[0];
    assert_eq!(d.total(), Some(money("2092.50")));
    assert_eq!(d.payments.len(), 2);
    let clearing = s.clearing().expect("payments clearing");
    assert!(d.payments.iter().all(|p| p.account == clearing));
    assert_eq!(
        d.payments.iter().map(|p| p.amount).collect::<Vec<_>>(),
        [money("1250.00"), money("842.50")],
        "the engine's exact amounts"
    );
    // Q-262: the bank and Payments Clearing reload elsewhere.
    let heard = s.link.bus().take(&mut bus);
    assert!(heard.ledger_touches(account(1010)));
    assert!(heard.ledger_touches(clearing));
    assert_eq!(s.save.count, 1);
    // Nothing left waiting after the save.
    assert_eq!(s.state_json()["pending"]["count"], 0);
}

#[test]
fn a_payment_deposited_elsewhere_is_dropped_and_nothing_posts() {
    let (mut s, log, _dir) = screen(all_caps());
    s.select_all();
    log.lock().expect("log").conflict_next = true;
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(log.lock().expect("log").saved.len(), 0, "nothing posted");
    assert_eq!(s.save.notice, Some("deposit.already_deposited"));
    assert_eq!(
        s.header.current().picked.len(),
        1,
        "the stale pick is dropped"
    );
    assert_eq!(s.header.current().picked[0].source, txn(1));
    // The owner saves again: only Hollister goes.
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(
        log.lock().expect("log").saved[0].total(),
        Some(money("1250.00"))
    );
}

#[test]
fn fees_and_cash_back_post_exactly_and_too_much_cash_back_sends_nothing() {
    let (mut s, log, _dir) = screen(all_caps());
    set_lines(&mut s, &[(4000, "300.00"), (6250, "-12.50")]);
    set_cash_back(&mut s, 1020, "50.00");
    assert_eq!(s.state_json()["totals"]["total"], "237.50");
    set_cash_back(&mut s, 1020, "400.00");
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state_json()["save"]["refused"], "cash_back_too_large");
    assert_eq!(log.lock().expect("log").saved.len(), 0);
    set_cash_back(&mut s, 1020, "50.00");
    assert!(s.start_save(SaveIntent::Save, None));
    let d = log.lock().expect("log").saved[0].clone();
    assert_eq!(d.total(), Some(money("237.50")));
    assert_eq!(d.deductions.len(), 1);
    assert_eq!(d.deductions[0].amount, money("12.50"));
    assert!(BankPosting::deposit_with(d, all_caps()).is_ok());
    let published = s.save.published.clone();
    for n in [1010, 4000, 6250, 1020] {
        assert!(published.contains(&account(n)), "{n}");
    }
}

#[test]
fn a_fee_without_the_capability_is_refused_locally() {
    let (mut s, log, _dir) = screen(DepositCaps::default());
    set_lines(&mut s, &[(4000, "100.00"), (6250, "-5.00")]);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state_json()["save"]["refused"], "negative_needs_update");
    assert_eq!(log.lock().expect("log").saved.len(), 0);
    // Plain lines still save.
    set_lines(&mut s, &[(4000, "100.00")]);
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(log.lock().expect("log").saved.len(), 1);
}

fn grouped_record() -> Transaction {
    #[derive(serde::Deserialize)]
    struct W {
        transaction: Transaction,
    }
    let body = include_str!("../../../../../fixtures/engine/deposit-grouped-fees.json");
    let mut t = serde_json::from_str::<W>(body).expect("txn").transaction;
    // The fixture's bank account is the demo's 1010 here.
    let bank = t.lines[0].account_id;
    for l in &mut t.lines {
        if l.account_id == bank {
            l.account_id = account(1010);
        }
    }
    t
}

#[test]
fn a_grouped_deposit_is_read_only_and_its_summary_is_the_engines_total() {
    let (mut s, log, _dir) = screen(all_caps());
    let record = grouped_record();
    let id = record.id;
    log.lock().expect("log").record = Some(record);
    assert!(s.open_record(id));
    let v = s.state_json();
    assert_eq!(v["read_only"], true);
    assert_eq!(v["grouped"], true);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.save.notice, Some("deposit.read_only.grouped"));
    let summary = s.summary().expect("summary");
    assert_eq!(summary.total, money("1487.50"));
    assert_eq!(Some(summary.total), s.record.as_ref().map(|r| r.total));
    assert!(summary.parts_agree());
    // Void stays open to it.
    assert!(s.start_void());
    assert!(s.void.open);
    s.send_void("returned to waiting", None);
    assert_eq!(s.save.last_action, Some("void"));
}

/// Q-267: the summary is a condensed table on wide windows; the total sits
/// in the amounts' column.
#[test]
fn the_summary_draws_condensed_on_wide_windows() {
    use crate::forms::condensed::MAX_WIDTH;
    use eframe::egui::{self, Pos2, RawInput, Rect, vec2};
    let (mut s, log, _dir) = screen(all_caps());
    let record = grouped_record();
    let id = record.id;
    log.lock().expect("log").record = Some(record);
    assert!(s.open_record(id));
    for width in [1440.0, 1920.0] {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        let tokens = crate::theme::Tokens::of(&ctx);
        let mut report = None;
        for _ in 0..2 {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 900.0))),
                ..RawInput::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    report = s.summary_ui(ui, &tokens);
                });
            });
            out.textures_delta.clear();
        }
        let r = report.expect("a saved deposit has a summary");
        assert!(r.width <= MAX_WIDTH, "{width}: {}", r.width);
        assert!(r.rows.len() >= 2, "lines and the total: {r:?}");
        assert!(r.widest_gap() <= MAX_WIDTH, "{width}");
        let total = r.rows.last().expect("total");
        assert!(total.total);
        for row in &r.rows {
            assert!(
                (row.amount.right() - total.amount.right()).abs() < 1.5,
                "{row:?}"
            );
        }
    }
}

#[test]
fn the_draft_total_is_the_checked_total() {
    let (mut s, _log, _dir) = screen(all_caps());
    s.select_all();
    set_lines(&mut s, &[(4000, "10.01"), (6250, "-0.01")]);
    let control = s.control_accounts();
    let c = check(&s.compose(), &s.context(&control)).expect("ok");
    let d = to_draft(&c, s.clearing()).expect("draft");
    assert_eq!(d.total(), Some(c.total));
    assert_eq!(c.total, money("2102.50"));
    // Cash back to Payments Clearing is refused before sending.
    let clearing = s.clearing().expect("clearing");
    s.header.edit_step(|h| {
        h.cash_back = CashBackForm {
            account: Some(clearing),
            amount: Some(money("1")),
            memo: String::new(),
        };
    });
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(
        s.state_json()["save"]["refused"],
        "cash_back_to_control_account"
    );
}

#[test]
fn the_demo_saves_a_plain_deposit_and_reads_it_back() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let mut s = DepositScreen::new(Arc::clone(&link), None, today());
    assert_eq!(
        s.state_json()["pending"]["count"],
        2,
        "the sample's two payments"
    );
    // Their method comes with them (numbered payments are checks).
    let pending = s.panel.pending.clone().expect("pending");
    assert!(
        pending
            .items
            .iter()
            .all(|p| p.method.as_deref() == Some("check")),
        "{:?}",
        pending
            .items
            .iter()
            .map(|p| p.method.clone())
            .collect::<Vec<_>>()
    );
    set_lines(&mut s, &[(4000, "300.00"), (6250, "-12.50")]);
    set_cash_back(&mut s, 1020, "50.00");
    assert!(s.start_save(SaveIntent::Save, None), "{:?}", s.save.error);
    assert_eq!(s.save.count, 1);
    let rows = DemoDeposits.list(&link, account(1010));
    let row = rows.first().expect("the deposit");
    assert_eq!(row.total, Some(money("237.50")));
    let got = DemoDeposits.get(&link, row.id).expect("get");
    let r = cl_api::DepositRecord::from_transaction(got.record, account(1010)).expect("record");
    assert_eq!(r.total, money("237.50"));
    assert_eq!(r.lines.len(), 2);
    assert!(r.cash_back.is_some(), "the twin keeps the cash back");
    assert!(r.lines.iter().any(|l| l.amount == money("-12.50")));
    // Prev opens it, plain and editable.
    s.navigate(true);
    assert_eq!(s.state_json()["mode"], "edit");
    assert_eq!(s.state_json()["read_only"], false);
    // The edit drops the fee and the cash back: their accounts still hear
    // about it (M2).
    set_lines(&mut s, &[(4000, "300.00")]);
    s.header
        .edit_step(|h| h.cash_back = CashBackForm::default());
    s.sync_inputs();
    let mut bus = link.bus().subscribe();
    assert!(s.start_save(SaveIntent::Save, None), "{:?}", s.save.error);
    let heard = link.bus().take(&mut bus);
    for n in [1010, 4000, 6250, 1020] {
        assert!(heard.ledger_touches(account(n)), "{n}");
    }
}

#[test]
fn the_demo_story_groups_the_bank_deposit_and_void_returns_the_payments() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let mut s = DepositScreen::new(Arc::clone(&link), None, today());
    let w = s
        .panel
        .waiting
        .iter()
        .find(|w| w.amount == money("2092.50"))
        .cloned()
        .expect("the 11/04 bank deposit");
    s.choose_feed(w);
    let v = s.state_json();
    assert_eq!(v["picked"], 2);
    assert_eq!(v["totals"]["total"], "2,092.50");
    assert_eq!(v["bank_match"], "matches");
    assert!(s.start_save(SaveIntent::Save, None), "{:?}", s.save.error);
    assert_eq!(s.state_json()["pending"]["count"], 0);
    assert_eq!(s.state_json()["pending"]["clearing_balance"], "0.00");
    // Prev opens the grouped deposit read-only; Void returns the payments.
    s.navigate(true);
    assert_eq!(s.state_json()["grouped"], true, "{}", s.state_json());
    s.start_void();
    s.send_void("scenario cleanup", None);
    assert_eq!(s.save.last_action, Some("void"));
    assert_eq!(s.state_json()["pending"]["count"], 2);
}

#[test]
fn prev_on_unsaved_work_asks_save_discard_or_keep_editing() {
    use crate::windows::PromptAnswer;
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let mut s = DepositScreen::new(Arc::clone(&link), None, today());
    set_lines(&mut s, &[(4000, "20.00")]);
    s.navigate(true);
    assert_eq!(s.state_json()["nav_prompt"], true);
    assert_eq!(s.save.notice, None, "a question, not a notice");
    // Keep editing (Esc too): nothing moves, nothing is lost.
    s.answer_nav(PromptAnswer::Keep);
    assert!(s.is_dirty() && s.nav_prompt.is_none());
    s.navigate(true);
    assert!(s.command(CommandId::new("form.revert")));
    assert!(s.nav_prompt.is_none() && s.is_dirty());
    // Save: saved, then Prev opens the newest deposit (it).
    s.navigate(true);
    s.answer_nav(PromptAnswer::Save);
    assert_eq!(s.save.count, 1);
    assert_eq!(s.state_json()["mode"], "edit");
    // Discard: the change goes, then Prev moves on.
    set_lines(&mut s, &[(4000, "21.00")]);
    s.navigate(false);
    s.answer_nav(PromptAnswer::Discard);
    assert!(!s.is_dirty());
    assert_eq!(s.save.count, 1);
    assert_eq!(
        s.state_json()["mode"],
        "new",
        "Next past the newest: a new one"
    );
}

#[test]
fn a_lost_refund_reply_resends_under_the_same_key() {
    let (mut s, _log, _dir) = screen(all_caps());
    let draft = cl_api::VendorRefundDraft {
        vendor: cl_core::ids::ContactId::parse("6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"),
        deposit_to: account(1010),
        payables: account(2000),
        date: today(),
        amount: money("75.00"),
        applies_to: txn(7),
        memo: None,
    };
    let first = s.refund_key(&draft, false);
    // The reply is lost: the outcome is unknown, so the key stays.
    s.refund_failed(ApiError::Timeout);
    assert!(s.refund.key.is_some());
    assert_eq!(
        s.refund_key(&draft, false),
        first,
        "the resend reuses the key"
    );
    // With the password it is another request.
    let with_password = s.refund_key(&draft, true);
    assert_ne!(with_password, first);
    // A definitive refusal drops the key: the next try is fresh.
    s.refund_failed(ApiError::Local(cl_api::LocalValidation::InvalidValue));
    assert!(s.refund.key.is_none());
    assert_ne!(s.refund_key(&draft, true), with_password);
}

/// Q-264: Enter in Date is Save & New by default and moves on to Memo
/// (Shift+Enter back) when "Enter moves between fields" is on.
#[test]
fn header_enter_saves_or_moves_by_the_preference() {
    use crate::forms::enter::testing::{context, enter, frame, settle};
    use cl_model::forms::EnterMode;
    for mode in [EnterMode::SaveAndNew, EnterMode::MoveBetweenFields] {
        let (mut s, log, _dir) = screen(DepositCaps::default());
        set_lines(&mut s, &[(4000, "100.00")]);
        let ctx = context();
        s.focus = Some(Field::Date);
        settle(&ctx, mode, &mut |ui| s.ui(ui));
        assert_eq!(s.focused, Some(Field::Date), "{mode:?}");
        frame(&ctx, mode, enter(false), &mut |ui| s.ui(ui));
        settle(&ctx, mode, &mut |ui| s.ui(ui));
        let saved = log.lock().expect("log").saved.len();
        match mode {
            EnterMode::SaveAndNew => assert_eq!(saved, 1, "Enter = Save & New (Q-190)"),
            EnterMode::MoveBetweenFields => {
                assert_eq!(saved, 0, "Enter never saves here");
                assert_eq!(s.focused, Some(Field::Memo));
                frame(&ctx, mode, enter(true), &mut |ui| s.ui(ui));
                settle(&ctx, mode, &mut |ui| s.ui(ui));
                assert_eq!(s.focused, Some(Field::Date), "Shift+Enter goes back");
            }
        }
    }
}

/// Review fix-PR-1 should-fix 3: a lost vendor refund that Check again
/// finds in the books is done; the card closes and nothing goes again.
#[test]
fn a_lost_refund_found_saved_is_never_sent_a_second_time() {
    let (mut s, log, _dir) = screen(all_caps());
    s.refund.open = true;
    s.refund.vendor =
        Some(cl_core::ids::ContactId::parse("6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"));
    s.refund.kind = Some(super::refund::RefundKind::VendorCredit);
    s.refund.credit = Some(txn(7));
    s.refund.amount = "75.00".into();
    log.lock().expect("log").lost_next = true;
    s.send_refund(None);
    assert_eq!(log.lock().expect("log").saved.len(), 1, "sent once");
    assert!(s.save.lost.pending());
    s.save.lost.recheck_answering(
        &mut s.header,
        cl_api::Recheck::Committed {
            transaction: Some(txn(99)),
        },
    );
    s.poll_save();
    assert!(!s.save.lost.pending());
    assert!(!s.refund.open && s.refund.vendor.is_none() && s.refund.amount.is_empty());
    assert_eq!(s.save.notice, Some("deposit.refund.saved"));
    s.send_refund(None);
    assert_eq!(
        log.lock().expect("log").saved.len(),
        1,
        "the refund is not sent again"
    );
}

#[test]
fn b2_after_an_unknown_outcome_a_second_save_sends_nothing_until_the_recheck_answers() {
    let (mut s, log, _dir) = screen(all_caps());
    set_lines(&mut s, &[(4100, "125.00")]);
    log.lock().expect("log").lost_next = true;
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(log.lock().expect("log").saved.len(), 1);
    assert!(s.save.lost.pending());
    for intent in [
        SaveIntent::Save,
        SaveIntent::SaveAndNew,
        SaveIntent::SaveAndClose,
    ] {
        assert!(!s.start_save(intent, None));
    }
    // A vendor refund from the same window waits too.
    s.send_refund(None);
    assert_eq!(log.lock().expect("log").saved.len(), 1, "nothing sent");
    s.save
        .lost
        .recheck_answering(&mut s.header, cl_api::Recheck::Unknown);
    s.poll_save();
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(log.lock().expect("log").saved.len(), 1);
    s.save
        .lost
        .recheck_answering(&mut s.header, cl_api::Recheck::NotCommitted);
    s.poll_save();
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(log.lock().expect("log").saved.len(), 2);
}
