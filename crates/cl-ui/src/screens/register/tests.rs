//! Register screen tests (no engine): the demo stand-in feeds the screen.

use super::*;
use cl_grid::RowModel;

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn screen(link: &Arc<CompanyLink>) -> RegisterScreen {
    RegisterScreen::new(Arc::clone(link), None, today(), None)
}

/// Fetch and accept pages until nothing is in flight (the demo answers at
/// once).
fn settle(s: &mut RegisterScreen, rows: Option<&std::ops::Range<u64>>) {
    for _ in 0..8 {
        if let Some(r) = rows.cloned() {
            let account = s.account.expect("account");
            let q = s.query.clone().expect("query");
            for req in s.model.wanted(Some(r)) {
                if let Some(t) = s.fetch_page(account, &q, req) {
                    s.pages.push((req, t));
                }
            }
        } else {
            s.request_pages();
        }
        s.poll();
    }
}

fn operating(link: &Arc<CompanyLink>) -> AccountId {
    link.demo_state()
        .and_then(|mut d| {
            d.chart
                .get_or_insert_with(crate::company::demo::chart)
                .iter()
                .find(|a| a.fields.account_number.as_deref() == Some("1010"))
                .map(|a| a.id)
        })
        .expect("1010")
}

#[test]
fn opens_on_the_first_bank_account_with_its_title() {
    let (_d, link) = link();
    let s = screen(&link);
    assert_eq!(s.account(), Some(operating(&link)));
    assert_eq!(s.title(), "Register \u{b7} Operating Checking");
    assert_eq!(s.class, AccountClass::Bank);
    let q = s.query.as_ref().expect("query");
    assert_eq!(q.end, today(), "last 90 days by default");
    assert_eq!(q.sort, RegisterSort::Date);
    assert!(s.jump_to_end, "newest at the bottom");
    assert_eq!(s.state()["screen"], "register");
    assert_eq!(s.state()["account_class"], "bank");
}

#[test]
fn rows_are_engine_lines_with_their_balance_as_sent() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    let total = s.model.total().expect("total");
    assert!(total > 10);
    let last = usize::try_from(total - 1).expect("idx");
    settle(&mut s, Some(&((total - 1)..total)));
    // The last row's balance is the demo engine's figure for that line.
    let q = s.query.clone().expect("q");
    let chart = link
        .demo_state()
        .and_then(|d| d.chart.clone())
        .expect("chart");
    let page_no = u32::try_from(last / 200 + 1).expect("page");
    let page = crate::company::demo::register::page(
        &crate::company::demo::register::DemoLedger::default(),
        &chart,
        operating(&link),
        &q,
        page_no,
        200,
    )
    .expect("page");
    let engine_line = page.lines.last().expect("line");
    let shown = match s.model.cell(last, view::COL_BALANCE) {
        cl_grid::CellValue::Text(t) => t.into_owned(),
        _ => panic!("balance text"),
    };
    let want = crate::screens::lists::center::model::layout_plain(
        &engine_line.balance.display(false).expect("display"),
        s.model.style,
    );
    assert_eq!(shown, want);
    assert!(s.model.row_key(last).is_some());
    assert!(!s.model.display_only());
}

#[test]
fn row_keys_follow_lines_across_reloads() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, Some(&(0..5)));
    let key = s.model.row_key(3).expect("key");
    let journal = s.model.line_of(key).expect("line");
    s.model.invalidate();
    settle(&mut s, Some(&(0..5)));
    assert_eq!(s.model.key_of(journal), Some(key));
    assert_eq!(s.model.index_of(key), Some(3));
    // A newest-first sort moves the line but keeps its key.
    s.set_sort(SortChoice::DateNewest);
    let total = s.model.total();
    assert!(total.is_none(), "a new query starts over");
    settle(&mut s, Some(&(0..400)));
    let at = s.model.index_of(key).expect("still there");
    assert_eq!(s.model.row_key(at), Some(key));
}

#[test]
fn hidden_balances_show_a_dash() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, Some(&(0..3)));
    s.model.balance = BalanceMode::HiddenFiltered;
    assert!(matches!(
        s.model.cell(0, view::COL_BALANCE),
        cl_grid::CellValue::Text(t) if t == "\u{2014}"
    ));
    let sp = view::spec(false, AccountClass::Bank, BalanceMode::HiddenFiltered);
    let bal = sp
        .columns
        .iter()
        .find(|c| c.id == view::COL_BALANCE)
        .expect("col");
    assert_eq!(
        bal.help,
        Some(cl_grid::TextKey("register.balance.hidden_filtered"))
    );
    assert!(!bal.editable);
}

#[test]
fn one_line_folds_the_second_line_into_payee() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, Some(&(0..20)));
    let key = s.model.row_key(7).expect("key");
    let env_parts = EnvParts::default();
    let label = String::new();
    let env = env_parts.env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
    s.grid.focus(
        CellPos {
            row: key,
            col: COL_DATE,
        },
        ModelRef::Browse(&mut s.model),
        &env,
    );
    s.set_one_line(true);
    assert_eq!(
        s.grid.core().cursor().map(|c| c.row),
        Some(key),
        "the cursor stays on its line"
    );
    assert!(
        s.grid
            .spec()
            .columns
            .iter()
            .all(|c| c.line == cl_grid::Line::First)
    );
    assert!(
        !s.grid
            .spec()
            .columns
            .iter()
            .any(|c| c.id == view::COL_ACCOUNT)
    );
    settle(&mut s, Some(&(0..20)));
    let folded = (0..20).any(|i| {
        matches!(s.model.cell(i, view::COL_PAYEE), cl_grid::CellValue::Text(t) if t.contains(" \u{2014} "))
    });
    assert!(folded);
    s.command(CommandId::new("register.one_line"));
    assert!(!s.prefs.one_line);
    assert!(
        view::spec(false, AccountClass::Bank, BalanceMode::Engine)
            .validate()
            .is_ok()
    );
    assert!(
        view::spec(true, AccountClass::CreditCard, BalanceMode::Engine)
            .validate()
            .is_ok()
    );
}

#[test]
fn locked_and_reconciled_rows_carry_their_states() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.set_range(cl_model::register::RangePreset::AllDates);
    settle(&mut s, Some(&(0..200)));
    let closing = s.model.closing.expect("demo closing date");
    let mut seen_locked = false;
    for i in 0..200 {
        let Some(r) = s.model.row_at(i) else { continue };
        let st = s.model.row_state(i);
        assert_eq!(st.contains(cl_grid::RowState::LOCKED), r.date <= closing);
        if st.contains(cl_grid::RowState::LOCKED) {
            seen_locked = true;
            assert!(s.model.row_label(i).contains("Locked"));
        }
        if r.cleared == cl_api::ClearState::Reconciled {
            assert!(st.contains(cl_grid::RowState::RECONCILED));
        }
    }
    assert!(seen_locked);
}

#[test]
fn display_only_lines_are_text_and_flag_the_page() {
    let body = include_str!("../../../../../fixtures/engine/register-legacy.json");
    let page = cl_api::resources::register::page_from_json(body.as_bytes()).expect("page");
    assert!(!page.exact);
    let mut model = RegisterModel::new(1);
    let req = model.wanted(None)[0];
    let mapping = Mapping {
        class: Some(AccountClass::Bank),
        ..Mapping::default()
    };
    assert_eq!(
        model.accept(req, &page, &mapping),
        cl_model::register::Reply::Taken
    );
    assert!(model.display_only());
    assert!(matches!(
        model.cell(0, view::COL_PAYMENT),
        cl_grid::CellValue::Text(t) if t == "1,084.66"
    ));
    assert!(!model.row_at(0).expect("row").exact);
    // A credit card shows the engine's balance with its normal sign.
    let card = Mapping {
        class: Some(AccountClass::CreditCard),
        ..Mapping::default()
    };
    let row = card.row(&page.lines[0]);
    assert_eq!(row.balance.as_deref(), Some("0.00"));
    // An expense that debits the card (sent as `payment`) pays it down.
    assert_eq!(row.kind, "PMT");
}

#[test]
fn custom_ranges_are_checked() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.custom.open = true;
    s.custom.from = "10/31/2026".into();
    s.custom.to = "10/01/2026".into();
    s.apply_custom();
    assert_eq!(
        s.custom.problem,
        Some("register.range.problem.from_after_to")
    );
    s.custom.from = "nonsense".into();
    s.apply_custom();
    assert_eq!(s.custom.problem, Some("register.range.problem.bad_date"));
    s.custom.from = "10/01/2026".into();
    s.custom.to = "10/31/2026".into();
    s.apply_custom();
    assert!(!s.custom.open);
    let q = s.query.as_ref().expect("q");
    assert_eq!(q.start.to_wire(), "2026-10-01");
    assert_eq!(q.end.to_wire(), "2026-10-31");
}

#[test]
fn picking_another_account_opens_its_own_tab() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let savings = s
        .accounts
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some("1020"))
        .map(|a| a.id)
        .expect("1020");
    let _ = link.drain();
    s.pick_account(savings);
    assert_eq!(link.drain(), vec![CompanyEvent::OpenRegister(savings)]);
    assert_eq!(s.account(), Some(operating(&link)));
    let other = RegisterScreen::new(Arc::clone(&link), None, today(), Some(savings));
    assert_eq!(other.account(), Some(savings));
    // An income account has no register: the default account opens.
    let income = s
        .accounts
        .iter()
        .find(|a| class_of(a).is_none())
        .map(|a| a.id)
        .expect("income");
    let fallback = RegisterScreen::new(Arc::clone(&link), None, today(), Some(income));
    assert_eq!(fallback.account(), Some(operating(&link)));
}

#[test]
fn void_and_delete_need_a_row_and_esc_closes_the_panel() {
    let (_d, link) = link();
    let mut s = screen(&link);
    for c in ["form.delete_item", "edit.void_transaction"] {
        assert!(s.command(CommandId::new(c)), "{c}");
        assert_eq!(s.state()["row_action"]["panel"], "notice");
        assert_eq!(
            s.state()["row_action"]["notice"],
            "register.row_action.pick_row"
        );
        assert!(s.command(CommandId::new("form.revert")), "Esc closes it");
        assert_eq!(s.state()["row_action"]["panel"], "none");
    }
    assert!(s.command(CommandId::new(KIND)), "Ctrl+G stays");
}

#[test]
fn void_rows_are_muted_with_a_void_prefix() {
    let body = include_str!("../../../../../fixtures/engine/register-legacy.json");
    let mut page = cl_api::resources::register::page_from_json(body.as_bytes()).expect("page");
    page.lines[0].status = cl_api::TxnStatus::Void;
    let mut model = RegisterModel::new(1);
    let req = model.wanted(None)[0];
    let mapping = Mapping {
        class: Some(AccountClass::Bank),
        ..Mapping::default()
    };
    model.accept(req, &page, &mapping);
    assert!(model.row_state(0).contains(cl_grid::RowState::MUTED));
    let row = model.row_at(0).expect("row");
    assert!(row.account_memo.starts_with("VOID"), "{}", row.account_memo);
    assert!(!row.editable);
    assert!(model.row_label(0).contains("Void"));
    if page.lines.len() > 1 {
        assert!(!model.row_state(1).contains(cl_grid::RowState::MUTED));
    }
}

fn shared_prefs() -> (egui::Context, crate::grid_env::WidthsHandle) {
    let ctx = egui::Context::default();
    let handle = crate::grid_env::WidthsHandle::default();
    crate::grid_env::publish_widths(&ctx, &handle);
    (ctx, handle)
}

fn demo_tenant(link: &Arc<CompanyLink>) -> Option<(TenantId, String)> {
    link.demo_state()
        .and_then(|d| d.companies.first().map(|c| (c.tenant_id, c.name.clone())))
}

#[test]
fn range_sort_and_one_line_are_remembered_per_account() {
    let (_d, link) = link();
    let (ctx, handle) = shared_prefs();
    let mut s = RegisterScreen::new(Arc::clone(&link), demo_tenant(&link), today(), None);
    s.sync_prefs(&ctx);
    s.set_range(cl_model::register::RangePreset::AllDates);
    s.set_sort(SortChoice::Number);
    s.set_one_line(true);
    s.sync_prefs(&ctx);
    let changed = handle
        .lock()
        .map(|w| (w.registers.len(), w.registers_changed_at.is_some()))
        .expect("lock");
    assert_eq!(changed, (1, true));

    // A new window on the same account starts from the remembered choices.
    let mut again = RegisterScreen::new(Arc::clone(&link), demo_tenant(&link), today(), None);
    again.sync_prefs(&ctx);
    assert_eq!(again.prefs.range, cl_model::register::RangePreset::AllDates);
    assert_eq!(again.prefs.sort, SortChoice::Number);
    assert!(again.prefs.one_line);
    assert_eq!(again.state()["range"], "all_dates");
    assert_eq!(
        again.query.as_ref().map(|q| q.sort),
        Some(RegisterSort::Number)
    );

    // Without a company nothing is remembered.
    let mut none = screen(&link);
    none.sync_prefs(&ctx);
    assert_eq!(none.prefs, RegisterPrefs::default());
}

#[test]
fn an_own_write_reloads_and_rereads_the_stamp() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, Some(&(0..5)));
    let rev = cl_grid::RowModel::revision(&s.model);
    s.after_own_write();
    assert!(cl_grid::RowModel::revision(&s.model) > rev);
    assert!(s.stamp.now && s.stamp.quiet);
    let ctx = egui::Context::default();
    s.watch_stamp(&ctx);
    // The demo has no stamp to read; the request is consumed.
    assert!(!s.stamp.now);
    assert!(s.stamp.read_at.is_some());
}

/// Runs `finish_jump` in a frame where `field` (a text field) has the
/// keyboard, or nobody does.
fn jump_with_focus_on(s: &mut RegisterScreen, field: Option<egui::Id>) {
    let ctx = egui::Context::default();
    if let Some(id) = field {
        egui::text_edit::TextEditState::default().store(&ctx, id);
        ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.ctx().memory_mut(|m| m.request_focus(id));
        })
        .textures_delta
        .clear();
    }
    ctx.run_ui(egui::RawInput::default(), |ui| {
        s.finish_jump(ui.ctx());
    })
    .textures_delta
    .clear();
}

#[test]
fn the_newest_line_takes_the_keyboard_when_nobody_is_typing() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    assert!(s.jump_to_end);
    jump_with_focus_on(&mut s, None);
    assert!(!s.jump_to_end);
    assert_eq!(s.entry.focus, Some(entry_row::E_DATE));
}

#[test]
fn a_reload_leaves_the_keyboard_in_a_quick_filter_field() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    assert!(s.jump_to_end);
    jump_with_focus_on(&mut s, Some(egui::Id::unique("amount-to-filter")));
    assert!(!s.jump_to_end, "the jump is done");
    assert_eq!(s.entry.focus, None, "the keyboard stays in the filter");
}

#[test]
fn the_footer_shows_the_engines_summary_as_sent() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    s.footer_load();
    let chart = link
        .demo_state()
        .and_then(|d| d.chart.clone())
        .expect("chart");
    let q = s.query.clone().expect("q");
    let sum = crate::company::demo::register::summary(
        &crate::company::demo::register::DemoLedger::default(),
        &chart,
        operating(&link),
        Some(q.end),
    )
    .expect("summary");
    let text = |a: &cl_api::RegisterAmount| {
        crate::screens::lists::center::model::layout_plain(
            &a.display(false).expect("display"),
            s.model.style,
        )
    };
    let shown = s.footer_shown();
    assert_eq!(shown.ending, text(sum.ending_balance.as_ref().expect("R4")));
    assert_eq!(
        shown.cleared.as_deref(),
        Some(text(sum.cleared_balance.as_ref().expect("R4")).as_str())
    );
    assert_eq!(
        shown.last_reconciled.as_deref(),
        Some("Last reconciled 09/30/2026")
    );
    assert_eq!(
        shown.uncleared,
        Some(format!("{} uncleared", sum.uncleared_count))
    );
    assert!(!shown.failed);
    assert_eq!(s.state()["footer"]["ending"], shown.ending.as_str());
    // A reload keeps the figures until the new ones arrive.
    s.ledger_changed();
    assert!(s.footer.key.is_none());
    assert_eq!(s.footer_shown().ending, shown.ending);
    s.footer_load();
    assert_eq!(s.footer_shown(), shown);
}

#[test]
fn the_footer_falls_back_only_to_engine_figures() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    s.footer_load();
    let mut sum = s.footer.summary.clone().expect("summary");
    // An engine without R1/R4: no ending or cleared balance anywhere.
    sum.ending_balance = None;
    sum.cleared_balance = None;
    s.footer.summary = Some(sum.clone());
    s.footer.page_ending = None;
    let shown = s.footer_shown();
    assert!(shown.cleared.is_none());
    // The default range reaches today, but the stored balance also counts
    // future-dated lines: no engine figure for this range, a dash.
    assert_eq!(shown.ending, "\u{2014}", "last 90 days: no fallback");
    // A range that ends before today: a dash too.
    if let Some(q) = s.query.as_mut() {
        q.end = CalendarDate::from_ymd(2026, 10, 1).expect("date");
    }
    assert_eq!(s.footer_shown().ending, "\u{2014}");
    // "All dates": the stored balance is the ending balance.
    if let Some(q) = s.query.as_mut() {
        q.end = CalendarDate::max();
    }
    let current = crate::screens::lists::center::model::layout_plain(
        &sum.current_balance.display(false).expect("display"),
        s.model.style,
    );
    assert_eq!(
        s.footer_shown().ending,
        current,
        "all dates: stored balance"
    );
    // A failed summary shows a dash and never blocks the register.
    s.footer.summary = None;
    s.footer.summary_failed = true;
    let shown = s.footer_shown();
    assert_eq!(shown.ending, "\u{2014}");
    assert!(shown.failed && shown.last_reconciled.is_none());
}

fn demo_account(link: &Arc<CompanyLink>, number: &str) -> AccountId {
    link.demo_state()
        .and_then(|mut d| {
            d.chart
                .get_or_insert_with(crate::company::demo::chart)
                .iter()
                .find(|a| a.fields.account_number.as_deref() == Some(number))
                .map(|a| a.id)
        })
        .expect("account")
}

fn demo_contact(link: &Arc<CompanyLink>, name: &str) -> cl_core::ids::ContactId {
    link.demo_state()
        .and_then(|mut d| {
            d.contacts_mut()
                .iter()
                .find(|c| c.fields.display_name == name)
                .map(|c| c.id)
        })
        .expect("contact")
}

fn money(s: &str) -> cl_core::Money {
    cl_core::Money::parse_wire(s).expect("money")
}

#[test]
fn the_entry_row_records_a_check_then_a_deposit() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    let before = s.model.total().expect("total");
    assert_eq!(s.entry.next_check, Some(4418), "highest check number + 1");
    let fuel = demo_account(&link, "6100");
    let vendor = demo_contact(&link, "Hollister Fuel Stop");
    s.entry.form.edit_step(|d| {
        d.payee = Some(vendor);
        d.account = Some(fuel);
        d.payment = Some(money("125.50"));
    });
    assert_eq!(s.draft_kind(), Some(cl_model::register::EntryKind::Check));
    s.sync_proposal();
    assert_eq!(s.entry.draft().number, "4418");
    assert_eq!(s.state()["entry"]["number_proposed"], true);
    assert!(s.entry_dirty());
    assert!(s.start_record(), "the demo saves at once");
    assert_eq!(s.entry.last_saved, Some("check"));
    assert!(s.entry.draft().is_empty(), "cleared after the save");
    assert_eq!(s.entry.draft().date, Some(today()), "the date stays");
    assert!(!s.entry_dirty());
    settle(&mut s, None);
    assert_eq!(s.model.total(), Some(before + 1));
    assert_eq!(s.entry.next_check, Some(4419));

    // A deposit: the second column, a customer and an income account.
    let income = demo_account(&link, "4000");
    let customer = demo_contact(&link, "Riverbend HOA");
    s.entry.form.edit_step(|d| {
        d.payee = Some(customer);
        d.account = Some(income);
        d.deposit = Some(money("300.00"));
    });
    assert_eq!(s.draft_kind(), Some(cl_model::register::EntryKind::Deposit));
    s.sync_proposal();
    assert!(s.entry.draft().number.is_empty(), "no number for a deposit");
    assert!(s.start_record());
    assert_eq!(s.entry.last_saved, Some("deposit"));
    settle(&mut s, None);
    assert_eq!(s.model.total(), Some(before + 2));
    assert_eq!(s.state()["entry"]["saved_count"], 2);
}

#[test]
fn the_entry_row_refuses_plainly_and_enter_on_an_empty_row_does_nothing() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    assert!(s.start_record(), "nothing to record");
    assert_eq!(s.entry.saved, 0);
    s.entry.form.edit_step(|d| d.payment = Some(money("10.00")));
    assert!(!s.start_record());
    assert_eq!(
        s.entry.problems,
        vec![cl_grid::TextKey("register.entry.problem.account_missing")]
    );
    assert_eq!(s.entry.focus, Some(entry_row::E_ACCOUNT));
    // A check needs a payee; a deposit takes no number.
    let fuel = demo_account(&link, "6100");
    s.entry.form.edit_step(|d| d.account = Some(fuel));
    assert!(!s.start_record());
    assert_eq!(
        s.entry.problems,
        vec![cl_grid::TextKey("register.entry.problem.check_payee")]
    );
    let income = demo_account(&link, "4000");
    s.entry.form.edit_step(|d| {
        d.payment = None;
        d.deposit = Some(money("10.00"));
        d.account = Some(income);
        d.number = "77".into();
    });
    assert!(!s.start_record());
    assert_eq!(
        s.entry.problems,
        vec![cl_grid::TextKey("register.entry.problem.no_number")]
    );
    assert_eq!(s.entry.saved, 0);
    // A transfer to savings: no name, no number.
    let savings = demo_account(&link, "1020");
    s.entry.form.edit_step(|d| {
        d.number.clear();
        d.account = Some(savings);
    });
    assert_eq!(
        s.draft_kind(),
        Some(cl_model::register::EntryKind::Transfer)
    );
    assert!(s.start_record());
    assert_eq!(s.entry.last_saved, Some("transfer"));
}

fn last_key(s: &mut RegisterScreen) -> cl_grid::RowKey {
    let total = s.model.total().expect("total");
    settle(s, Some(&((total - 1)..total)));
    let last = usize::try_from(total - 1).expect("idx");
    s.model.row_key(last).expect("key")
}

#[test]
fn an_edit_starts_from_the_transaction_and_updates_it() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    let fuel = demo_account(&link, "6100");
    let vendor = demo_contact(&link, "Hollister Fuel Stop");
    s.entry.form.edit_step(|d| {
        d.payee = Some(vendor);
        d.account = Some(fuel);
        d.payment = Some(money("125.50"));
    });
    assert!(s.start_record());
    settle(&mut s, None);
    let total = s.model.total().expect("total");
    let key = last_key(&mut s);
    s.begin_edit(key, false);
    let e = s.entry.editing.clone().expect("editing");
    assert_eq!(e.kind, cl_model::register::EntryKind::Check);
    assert_eq!(s.entry.draft().payment, Some(money("125.50")));
    assert_eq!(s.entry.draft().account, Some(fuel));
    assert_eq!(s.state()["entry"]["mode"], "edit");
    assert!(!s.entry_dirty(), "a fresh edit has nothing unsaved");
    // Moving the amount to the other column is refused.
    s.entry.form.edit_step(|d| {
        d.payment = None;
        d.deposit = Some(money("1.00"));
    });
    assert!(!s.start_record());
    assert_eq!(
        s.entry.problems,
        vec![cl_grid::TextKey("register.entry.problem.side_changed")]
    );
    s.entry.form.edit_step(|d| {
        d.deposit = None;
        d.payment = Some(money("130.00"));
    });
    assert!(s.start_record());
    assert_eq!(s.entry.last_saved, Some("update"));
    assert!(s.entry.editing.is_none());
    settle(&mut s, None);
    assert_eq!(s.model.total(), Some(total), "changed, not added");
    let key = last_key(&mut s);
    s.begin_edit(key, false);
    assert_eq!(s.entry.draft().payment, Some(money("130.00")));
    s.finish_edit();
    assert!(s.entry.editing.is_none());
}

#[test]
fn splits_record_a_check_once_the_lines_add_up() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    let before = s.model.total().expect("total");
    let fuel = demo_account(&link, "6100");
    let repairs = demo_account(&link, "6350");
    let vendor = demo_contact(&link, "Hollister Fuel Stop");
    s.entry.form.edit_step(|d| {
        d.payee = Some(vendor);
        d.account = Some(fuel);
        d.payment = Some(money("200.00"));
    });
    s.toggle_splits();
    assert_eq!(s.state()["entry"]["splits"]["open"], true);
    assert_eq!(s.state()["entry"]["splits"]["lines"], 1);
    assert_eq!(s.state()["entry"]["splits"]["remaining_zero"], true);
    s.splits.as_mut().expect("panel").form_mut().edit_step(|v| {
        v.lines[0].amount = Some(money("150.00"));
        let mut extra = splits::SplitRow::blank(v.lines[1].key);
        extra.account = Some(repairs);
        extra.amount = Some(money("40.00"));
        v.lines[1] = extra;
    });
    assert_eq!(s.state()["entry"]["splits"]["remaining_zero"], false);
    assert!(!s.start_record());
    assert_eq!(
        s.entry.problems,
        vec![cl_grid::TextKey("register.splits.not_assigned")]
    );
    // More than one line: the panel stays open.
    s.toggle_splits();
    assert!(s.splits.is_some());
    s.splits
        .as_mut()
        .expect("panel")
        .form_mut()
        .edit_step(|v| v.lines[1].amount = Some(money("50.00")));
    assert!(s.start_record());
    assert_eq!(s.entry.last_saved, Some("check"));
    assert!(s.splits.is_none());
    settle(&mut s, None);
    assert_eq!(s.model.total(), Some(before + 1));
    // The split check opens again with Splits.
    let key = last_key(&mut s);
    s.begin_edit(key, false);
    assert_eq!(s.state()["entry"]["splits"]["lines"], 2);
    assert_eq!(s.state()["entry"]["splits"]["remaining_zero"], true);
}

/// The register's row key of `id` (loads every row).
fn key_of(s: &mut RegisterScreen, id: cl_core::ids::TransactionId) -> cl_grid::RowKey {
    let total = s.model.total().expect("total");
    settle(s, Some(&(0..total)));
    (0..usize::try_from(total).expect("n"))
        .find(|i| s.model.row_at(*i).is_some_and(|r| r.txn_id == id))
        .and_then(|i| s.model.row_key(i))
        .expect("the row")
}

/// The form an `OpenRecord` asked for, if one was pushed.
fn opened(link: &CompanyLink) -> Option<(String, cl_core::ids::TransactionId)> {
    link.drain().into_iter().find_map(|e| match e {
        crate::company::CompanyEvent::OpenRecord(k, id) => Some((k.as_str().to_string(), id)),
        _ => None,
    })
}

#[test]
fn edit_transaction_opens_the_form_that_owns_the_row() {
    let (_d, link) = link();
    let bank = demo_account(&link, "1010");
    let loan = demo_account(&link, "2200");
    let fuel = demo_account(&link, "6250");
    let id = {
        let mut demo = link.demo_state().expect("demo");
        let chart = demo
            .chart
            .get_or_insert_with(crate::company::demo::chart)
            .clone();
        let p = crate::company::demo::register::DemoPosting {
            kind: "check",
            date: CalendarDate::from_ymd(2026, 11, 3).expect("date"),
            number: String::new(),
            contact: None,
            memo: String::new(),
            own: bank,
            own_debit: false,
            lines: vec![(loan, money("400.00")), (fuel, money("25.00"))],
        };
        demo.ledger.save(&chart, &p, None).expect("saved")
    };
    let mut s = screen(&link);
    settle(&mut s, None);
    let key = key_of(&mut s, id);
    let _ = link.drain();
    // Enter keeps the register's own edit (P12: Splits in place).
    s.begin_edit(key, false);
    assert!(s.entry.editing.is_some());
    assert_eq!(opened(&link), None);
    s.finish_edit();
    // Edit Transaction (Ctrl+E): the loan payment opens in Loan Payment.
    s.begin_edit_routed(key, true, edit::Route::Form);
    assert!(s.entry.editing.is_none());
    assert_eq!(
        opened(&link),
        Some((crate::screens::loan_payment::KIND.to_string(), id))
    );
}

#[test]
fn form_for_names_each_p14_form_by_the_shape_of_the_lines() {
    use cl_model::register::AccountClass as C;
    #[derive(serde::Deserialize)]
    struct W {
        transaction: cl_api::Transaction,
    }
    let body = include_str!("../../../../../fixtures/engine/deposit-grouped-fees.json");
    let mut t = serde_json::from_str::<W>(body).expect("txn").transaction;
    let bank = t.lines[0].account_id;
    let is = |c: C| move |id: AccountId| Some(if id == bank { C::Bank } else { c });
    // A grouped deposit: Make Deposit.
    assert_eq!(
        edit::form_for(&t, &is(C::OtherAsset)),
        Some(crate::screens::deposit::KIND)
    );
    // The same lines all from equity: Put Money In.
    assert_eq!(
        edit::form_for(&t, &is(C::Equity)),
        Some(crate::screens::owner_money::KIND)
    );
    t.txn_type = cl_api::TxnType::DailySales;
    assert_eq!(
        edit::form_for(&t, &is(C::OtherAsset)),
        Some(crate::screens::daily_sales::KIND)
    );
    // Any journal entry: Make Journal Entry (P15, Ctrl+E).
    t.txn_type = cl_api::TxnType::JournalEntry;
    assert_eq!(
        edit::form_for(&t, &is(C::OtherAsset)),
        Some(crate::screens::journal_entry::KIND)
    );
}

#[test]
fn journal_entries_the_row_cant_show_go_to_make_journal_entry() {
    use cl_model::register::AccountClass as C;
    #[derive(serde::Deserialize)]
    struct W {
        transaction: cl_api::Transaction,
    }
    let body = include_str!("../../../../../fixtures/engine/je-record-reversing.json");
    let full = serde_json::from_str::<W>(body).expect("txn").transaction;
    let own = full.lines[0].account_id;
    let mut plain = full.clone();
    plain.is_adjusting = false;
    plain.reverse_on = None;
    plain.reversal_transaction_id = None;
    plain.lines[1].contact_id = None;
    plain.txn_number = None;
    let seed = |t: &cl_api::Transaction| {
        let v = cl_api::Versioned {
            record: t.clone(),
            version: cl_api::RecordVersion::Server(1),
        };
        matches!(edit::seed(&v, own, C::OtherAsset), edit::Seed::ReadOnly(_))
    };
    // A plain two-line entry stays in the register's row (Enter).
    assert!(!seed(&plain));
    // Adjusting, a reversing pair or a name: the row can't show it, so
    // Enter and double-click open Make Journal Entry (Q-P14aa).
    let mut t = plain.clone();
    t.is_adjusting = true;
    assert!(seed(&t));
    let mut t = plain.clone();
    t.reverse_on = full.reverse_on;
    assert!(seed(&t));
    let mut t = plain.clone();
    t.reverses_transaction_id = full.reversal_transaction_id;
    assert!(seed(&t));
    let mut t = plain.clone();
    t.lines[0].contact_id = full.lines[1].contact_id;
    assert!(seed(&t));
    assert!(seed(&full));
    // A numbered entry: the row sends no number, so Make Journal Entry
    // edits it and keeps its txnNumber (B3).
    let mut t = plain.clone();
    t.txn_number = Some("JE-1042".into());
    assert!(seed(&t));
    t.txn_number = Some("  ".into());
    assert!(!seed(&t), "a blank number is no number");
}

#[test]
fn rows_the_register_cant_change_show_read_only() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    let total = s.model.total().expect("total");
    settle(&mut s, Some(&(0..total)));
    // A customer payment (PMT) from the demo sample.
    let key = (0..usize::try_from(total).expect("n"))
        .filter_map(|i| {
            s.model
                .row_at(i)
                .map(|r| (i, r.kind.clone(), r.editable && !s.model.locked(r)))
        })
        .find(|(_, k, e)| k == "PMT" && *e)
        .and_then(|(i, _, _)| s.model.row_key(i))
        .expect("a payment row");
    s.begin_edit(key, false);
    assert!(s.entry.editing.is_none());
    assert_eq!(s.state()["entry"]["read_only"], true);
}

#[test]
fn quick_filters_go_to_the_engine_and_clear() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.prefs.range = cl_model::register::RangePreset::AllDates;
    s.apply_query();
    settle(&mut s, None);
    let all = s.model.total().expect("total");
    // The payee autofills to a known name as typed.
    s.quick.payee = "hollis".into();
    s.apply_typed_filters();
    assert!(s.quick.payee.starts_with("Hollister "), "autofilled");
    assert_eq!(s.filters.payee_text(), Some(s.quick.payee.as_str()));
    // A payee from the register goes to the engine as typed.
    settle(&mut s, Some(&(0..all)));
    let payee = (0..usize::try_from(all).expect("n"))
        .filter_map(|i| s.model.row_at(i))
        .map(|r| r.payee.clone())
        .find(|p| !p.is_empty())
        .expect("a payee");
    s.quick.payee.clone_from(&payee);
    s.apply_typed_filters();
    let q = s.query.clone().expect("q");
    assert_eq!(q.payee.as_deref(), Some(payee.as_str()));
    assert_eq!(s.state()["filters"]["payee"], true);
    // The demo engine keeps balances whatever the filters (R1).
    assert_eq!(s.state()["balance_mode"], "engine");
    settle(&mut s, None);
    let some = s.model.total().expect("total");
    assert!(some > 0 && some < all, "{some} of {all}");
    settle(&mut s, Some(&(0..some)));
    for i in 0..usize::try_from(some).expect("n") {
        assert!(s.model.row_at(i).expect("row").payee.contains(&payee));
    }
    // Exact amounts: from above to is refused and changes nothing.
    s.quick.min = "1,000".into();
    s.quick.max = "500".into();
    s.apply_typed_filters();
    assert_eq!(
        s.quick.problem,
        Some("register.filter.problem.min_above_max")
    );
    assert!(s.filters.min.is_none());
    // A later filter change keeps the last good amounts and the message.
    s.set_uncleared(true);
    assert_eq!(
        s.quick.problem,
        Some("register.filter.problem.min_above_max")
    );
    assert!(s.filters.min.is_none() && s.filters.max.is_none());
    assert_eq!(s.state()["filters"]["problem"], true);
    s.set_uncleared(false);
    s.quick.max = "abc".into();
    s.apply_typed_filters();
    assert_eq!(s.quick.problem, Some("grid.amount.invalid"));
    s.quick.max = "-5".into();
    s.apply_typed_filters();
    assert_eq!(s.quick.problem, Some("grid.amount.no_negative"));
    s.quick.max = "2000.10".into();
    s.apply_typed_filters();
    assert_eq!(s.quick.problem, None);
    assert_eq!(s.filters.min, Some(money("1000")));
    assert_eq!(s.filters.max, Some(money("2000.10")));
    assert_eq!(s.query.as_ref().and_then(|q| q.max), Some(money("2000.10")));
    // Uncleared and Tag go to the engine as their own fields.
    s.set_uncleared(true);
    assert_eq!(
        s.query.as_ref().and_then(|q| q.cleared),
        Some(cl_api::ClearedFilter::Uncleared)
    );
    // "Clear filters" brings every line back.
    s.clear_filters();
    assert!(!s.filters.is_active());
    assert!(s.quick.payee.is_empty() && s.quick.min.is_empty());
    let q = s.query.clone().expect("q");
    assert!(!q.filters_active());
    settle(&mut s, None);
    assert_eq!(s.model.total(), Some(all));
}

#[test]
fn filter_bounds_parse_exactly() {
    assert_eq!(filters::parse_bound("  "), Ok(None));
    assert_eq!(filters::parse_bound("1,084.66"), Ok(Some(money("1084.66"))));
    assert_eq!(filters::parse_bound("0.10"), Ok(Some(money("0.1"))));
    // Typed money rounds to the cent, half away from zero (Q-232).
    assert_eq!(filters::parse_bound("1.005"), Ok(Some(money("1.01"))));
    assert_eq!(filters::parse_bound("1.0049"), Ok(Some(money("1.00"))));
}

#[test]
fn go_to_finds_the_next_match_and_puts_the_cursor_on_it() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.prefs.range = cl_model::register::RangePreset::AllDates;
    s.apply_query();
    settle(&mut s, None);
    let total = s.model.total().expect("total");
    settle(&mut s, Some(&(0..total)));
    assert!(s.command(CommandId::new("edit.find")));
    assert_eq!(s.state()["goto"]["open"], true);
    // Nothing typed: a plain problem, no search.
    s.find(goto::Dir::Next);
    assert_eq!(s.state()["goto"]["status"], "empty");
    // The last row's payee, searched from the top: the first row with it.
    let rows: Vec<view::RegRow> = (0..usize::try_from(total).expect("n"))
        .map(|i| s.model.row_at(i).expect("row").clone())
        .collect();
    let payee = rows
        .iter()
        .rev()
        .map(|r| r.payee.clone())
        .find(|p| !p.is_empty())
        .expect("payee");
    let first = rows
        .iter()
        .position(|r| r.payee.contains(&payee))
        .expect("i");
    s.goto.field = goto::GotoField::Payee;
    s.goto.text = payee.to_lowercase();
    s.find(goto::Dir::Next);
    let (index, found) = s.goto.jump.expect("found");
    assert_eq!(index, first as u64);
    assert_eq!(found, rows[first].line_id);
    s.finish_goto();
    assert!(s.goto.jump.is_none());
    let key = s.grid.core().cursor().map(|c| c.row).expect("cursor");
    assert_eq!(s.model.line_of(key), Some(rows[first].line_id));
    assert_eq!(s.state()["goto"]["found"], 1);
    // Find Previous from there: nothing above it.
    s.find(goto::Dir::Previous);
    assert_eq!(s.state()["goto"]["status"], "not_found");
    // A new query stops a search and forgets its target.
    s.goto.jump = Some((0, rows[0].line_id));
    s.set_sort(SortChoice::DateNewest);
    assert!(s.goto.jump.is_none());
}

#[test]
fn quick_report_shows_the_account_report_for_the_shown_dates() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    assert!(s.command(CommandId::new("list.quick_report")));
    let st = s.state();
    let q = &st["quick_report"];
    assert_eq!(q["loaded"], true);
    assert_eq!(q["error"], false);
    assert_eq!(q["start"], s.query.as_ref().expect("q").start.to_wire());
    assert_eq!(q["end"], "2026-11-04");
    assert!(q["lines"].as_u64().expect("n") > 0);
    // Ctrl+Shift+Q again closes it.
    assert!(s.command(CommandId::new("list.quick_report")));
    assert!(s.state()["quick_report"].is_null());
}

#[test]
fn esc_closes_the_goto_bar_then_the_quick_report_not_the_tab() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    assert!(s.command(CommandId::new("list.quick_report")));
    assert!(s.command(CommandId::new("edit.find")));
    assert_eq!(s.state()["goto"]["open"], true);
    // First Esc: only the Go To bar goes.
    assert!(
        s.command(CommandId::new("form.revert")),
        "Esc stays in the tab"
    );
    assert_eq!(s.state()["goto"]["open"], false);
    assert!(!s.state()["quick_report"].is_null());
    // Second Esc: the QuickReport goes.
    assert!(
        s.command(CommandId::new("form.revert")),
        "Esc stays in the tab"
    );
    assert!(s.state()["quick_report"].is_null());
    // Nothing left open: Esc falls through so the shell closes the tab.
    assert!(!s.command(CommandId::new("form.revert")));
}

// ---------------------------------------------------------------- WP8

/// Records a check dated today (open period) and returns its row key.
fn recorded_check(link: &Arc<CompanyLink>, s: &mut RegisterScreen) -> cl_grid::RowKey {
    settle(s, None);
    let fuel = demo_account(link, "6100");
    let vendor = demo_contact(link, "Hollister Fuel Stop");
    s.entry.form.edit_step(|d| {
        d.payee = Some(vendor);
        d.account = Some(fuel);
        d.payment = Some(money("42.10"));
    });
    assert!(s.start_record());
    settle(s, None);
    last_key(s)
}

fn row_of(s: &RegisterScreen, key: cl_grid::RowKey) -> view::RegRow {
    s.model.row_for_key(key).cloned().expect("row")
}

#[test]
fn void_asks_first_then_voids_and_reloads() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let key = recorded_check(&link, &mut s);
    let total = s.model.total();
    s.actions.cursor = Some(key);
    assert!(s.command(CommandId::new("edit.void_transaction")));
    assert_eq!(s.state()["row_action"]["panel"], "confirm_void");
    // Cancel leaves it alone.
    assert!(s.command(CommandId::new("form.revert")));
    assert!(!row_of(&s, key).void);
    assert!(s.command(CommandId::new("edit.void_transaction")));
    s.confirm();
    let st = s.state()["row_action"].clone();
    assert_eq!(st["panel"], "none");
    assert_eq!(st["last"], "void");
    assert_eq!(st["count"], 1);
    assert_eq!(st["error"], false);
    settle(&mut s, None);
    assert_eq!(s.model.total(), total, "a void stays listed");
    let key = last_key(&mut s);
    let row = row_of(&s, key);
    assert!(row.void);
    assert!(row.payment.is_none(), "its amounts are zero");
    // Voiding it again explains.
    s.actions.cursor = Some(key);
    assert!(s.command(CommandId::new("edit.void_transaction")));
    assert_eq!(s.state()["row_action"]["notice"], "register.void.already");
}

#[test]
fn a_save_in_another_window_reloads_the_rows_of_its_account_only() {
    let (_d, link) = link();
    let mut shown = screen(&link);
    settle(&mut shown, None);
    let total = shown.model.total().expect("total");
    // Another account's transactions: nothing to read again.
    let elsewhere = link.bus().subscribe();
    link.bus().publish_from(
        &elsewhere,
        crate::data_bus::Change::ledger([demo_account(&link, "6100")]),
    );
    let rev = cl_grid::RowModel::revision(&shown.model);
    shown.hear_changes();
    assert_eq!(cl_grid::RowModel::revision(&shown.model), rev);
    // A second window on the same account records a check: the first
    // reads its rows again from the engine and lists it.
    let mut other = screen(&link);
    let _ = recorded_check(&link, &mut other);
    assert_eq!(other.model.total(), Some(total + 1));
    assert_eq!(shown.model.total(), Some(total), "not yet heard");
    shown.hear_changes();
    assert!(cl_grid::RowModel::revision(&shown.model) > rev);
    settle(&mut shown, None);
    assert_eq!(shown.model.total(), Some(total + 1));
    // Heard once; a window does not hear its own write back.
    let rev = cl_grid::RowModel::revision(&shown.model);
    shown.hear_changes();
    other.hear_changes();
    assert_eq!(cl_grid::RowModel::revision(&shown.model), rev);
    let rev_other = cl_grid::RowModel::revision(&other.model);
    other.hear_changes();
    assert_eq!(cl_grid::RowModel::revision(&other.model), rev_other);
}

#[test]
fn delete_asks_says_what_happens_and_removes_the_row() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let key = recorded_check(&link, &mut s);
    let total = s.model.total().expect("total");
    s.actions.cursor = Some(key);
    assert_eq!(s.state()["row_action"]["delete_supported"], true);
    assert!(s.command(CommandId::new("form.delete_item")));
    assert_eq!(s.state()["row_action"]["panel"], "confirm_delete");
    s.confirm();
    assert_eq!(s.state()["row_action"]["last"], "delete");
    settle(&mut s, None);
    assert_eq!(s.model.total(), Some(total - 1));
}

#[test]
fn delete_offers_void_without_the_capability_or_for_other_kinds() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let key = recorded_check(&link, &mut s);
    s.actions.caps = Some((true, false));
    s.actions.cursor = Some(key);
    assert_eq!(s.state()["row_action"]["delete_supported"], false);
    assert!(s.command(CommandId::new("form.delete_item")));
    assert_eq!(s.state()["row_action"]["panel"], "offer_void");
    assert_eq!(
        s.state()["row_action"]["notice"],
        "register.delete.unavailable"
    );
    // With the capability: kinds the engine does not delete (Q-245) and
    // reconciled rows.
    s.actions.caps = None;
    s.set_range(cl_model::register::RangePreset::AllDates);
    settle(&mut s, Some(&(0..200)));
    let mut seen = (false, false);
    for i in 0..200 {
        let Some(r) = s.model.row_at(i).cloned() else {
            continue;
        };
        let k = s.model.row_key(i).expect("key");
        s.actions.cursor = Some(k);
        s.actions.panel = dialogs::Panel::None;
        // A/P documents have their own refusal (P17 WP2b).
        if !r.txn_type.is_deletable() && !dialogs::VoidRoute::of(r.txn_type).is_ap() && !seen.0 {
            assert!(s.start_delete());
            assert_eq!(
                s.state()["row_action"]["notice"],
                "register.delete.unsupported"
            );
            seen.0 = true;
        } else if r.txn_type.is_deletable()
            && r.cleared == cl_api::ClearState::Reconciled
            && !seen.1
        {
            assert!(s.start_delete());
            assert_eq!(
                s.state()["row_action"]["notice"],
                "register.delete.reconciled"
            );
            seen.1 = true;
        }
    }
    assert!(seen.0 && seen.1, "{seen:?}");
}

/// A locked (closed-period), not reconciled, single-line check: the demo's
/// closed period is all reconciled, so one is recorded there first (with
/// the closing-date password).
fn locked_check(s: &mut RegisterScreen) -> cl_grid::RowKey {
    let link = Arc::clone(&s.link);
    settle(s, None);
    let closing = s.model.closing.expect("demo closing date");
    let fuel = demo_account(&link, "6100");
    let vendor = demo_contact(&link, "Hollister Fuel Stop");
    s.entry.form.edit_step(|d| {
        d.date = Some(closing);
        d.payee = Some(vendor);
        d.account = Some(fuel);
        d.payment = Some(money("12.00"));
    });
    assert!(!s.start_record(), "a closed-period date asks first");
    assert_eq!(s.state()["row_action"]["closing"], "asking");
    s.actions.closing.password = crate::screens::company::form::SecretText::from(
        crate::company::demo::register::DEMO_CLOSING_PASSWORD,
    );
    s.submit_closing();
    assert_eq!(s.state()["row_action"]["closing"], "done");
    assert_eq!(s.entry.last_saved, Some("check"));
    s.set_range(cl_model::register::RangePreset::AllDates);
    settle(s, None);
    let total = s.model.total().expect("total");
    let all = 0..total;
    settle(s, Some(&all));
    for i in 0..usize::try_from(total).expect("rows") {
        let Some(r) = s.model.row_at(i).cloned() else {
            continue;
        };
        if s.model.locked(&r)
            && r.editable
            && r.txn_type == cl_api::TxnType::Expense
            && r.cleared == cl_api::ClearState::None
        {
            let key = s.model.row_key(i).expect("key");
            s.begin_edit(key, false);
            let ok = s.entry.editing.is_some();
            s.finish_edit();
            if ok {
                return key;
            }
        }
    }
    panic!("no locked open check in the demo");
}

#[test]
fn a_locked_void_needs_the_closing_password_which_is_never_kept() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let key = locked_check(&mut s);
    s.actions.cursor = Some(key);
    assert!(s.command(CommandId::new("edit.void_transaction")));
    s.confirm();
    let st = s.state()["row_action"].clone();
    assert_eq!(st["closing"], "asking");
    assert_eq!(st["pending"], "void");
    // Cancel: nothing changed.
    assert!(s.command(CommandId::new("form.revert")));
    assert_eq!(s.state()["row_action"]["closing"], "cancelled");
    assert!(!row_of(&s, key).void);
    // A wrong password keeps the dialog for another try.
    assert!(s.command(CommandId::new("edit.void_transaction")));
    s.confirm();
    s.actions.closing.password = crate::screens::company::form::SecretText::from("wrong-canary");
    s.submit_closing();
    let st = s.state();
    assert_eq!(st["row_action"]["closing"], "rejected");
    assert!(
        s.actions.closing.password.is_empty(),
        "the typed password is wiped"
    );
    assert!(!st.to_string().contains("canary"));
    assert!(!format!("{:?}", s.actions).contains("canary"));
    // The right one voids it, and is dropped.
    s.actions.closing.password = crate::screens::company::form::SecretText::from(
        crate::company::demo::register::DEMO_CLOSING_PASSWORD,
    );
    s.submit_closing();
    let st = s.state()["row_action"].clone();
    assert_eq!(st["closing"], "done");
    assert_eq!(st["last"], "void");
    assert!(s.actions.closing.flow.password().is_none());
    assert!(s.actions.closing.pending.is_none());
    settle(&mut s, None);
    let total = s.model.total().expect("total");
    settle(&mut s, Some(&(0..total)));
    assert!(row_of(&s, key).void);
}

#[test]
fn a_locked_edit_asks_for_the_password_when_recorded() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let key = locked_check(&mut s);
    s.begin_edit(key, false);
    assert!(s.entry.editing.is_some(), "the edit opens");
    s.entry.form.edit_step(|d| d.memo = "changed".into());
    assert!(!s.start_record());
    let st = s.state()["row_action"].clone();
    assert_eq!(st["closing"], "asking");
    assert_eq!(st["pending"], "save");
    assert!(s.entry.error.is_none(), "the dialog shows it, not the row");
    s.actions.closing.password = crate::screens::company::form::SecretText::from(
        crate::company::demo::register::DEMO_CLOSING_PASSWORD,
    );
    s.submit_closing();
    assert_eq!(s.state()["row_action"]["closing"], "done");
    assert_eq!(s.entry.last_saved, Some("update"));
    assert!(s.entry.editing.is_none());
}

#[test]
fn without_the_engine_password_closed_rows_are_explained() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let key = locked_check(&mut s);
    s.actions.caps = Some((false, true));
    s.begin_edit(key, false);
    assert!(s.entry.editing.is_none(), "no edit opens");
    assert_eq!(s.state()["row_action"]["closing"], "unavailable");
    assert!(s.command(CommandId::new("form.revert")), "OK / Esc");
    assert_eq!(s.state()["row_action"]["closing"], "cancelled");
    // An engine refusal without a password path is explained the same way.
    let closed = cl_api::ApiError::ClosedPeriod(cl_api::ClosedPeriod {
        lock_date: s.model.closing,
        can_override: false,
        password_required: None,
        message: cl_api::UserText::default(),
    });
    assert!(s.closing_failed(&closed, dialogs::Pending::Save));
    assert_eq!(s.state()["row_action"]["closing"], "unavailable");
}

#[test]
fn rate_limits_and_conflicts_are_plain_messages() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let key = recorded_check(&link, &mut s);
    s.ask_closing(dialogs::Pending::Save);
    s.actions
        .closing
        .flow
        .handle(cl_model::closing::ClosingEvent::Submit(
            cl_core::Secret::new("x"),
        ));
    let limited = cl_api::ApiError::ClosingPasswordRateLimited {
        retry_after: Some(std::time::Duration::from_secs(150)),
    };
    // The dialog keeps the change and turns Change off for the wait.
    assert!(s.closing_failed(&limited, dialogs::Pending::Save));
    assert_eq!(s.state()["row_action"]["closing"], "rejected");
    assert_eq!(s.state()["row_action"]["pending"], "save");
    assert_eq!(s.state()["row_action"]["closing_wait_minutes"], 3);
    s.actions.closing.password = "y".into();
    s.submit_closing();
    assert_eq!(
        s.state()["row_action"]["closing"],
        "rejected",
        "nothing sent while limited"
    );
    // Once the wait is over, another try may go.
    s.actions.closing.limited_until = Some(std::time::Instant::now());
    assert!(s.closing_wait_minutes().is_none());
    s.cancel_closing();
    assert!(s.actions.closing.limited_until.is_none());
    assert_eq!(
        cl_core::OwnerFacing::owner_message(&limited).key,
        "error.api.closing_password_attempts"
    );
    // A void refused because the row changed elsewhere offers Reload,
    // which reads it again and asks again.
    let row = row_of(&s, key);
    s.row_action_failed(
        &dialogs::Pending::Row {
            kind: dialogs::ActionKind::Void,
            route: dialogs::VoidRoute::Generic,
            id: row.txn_id,
            date: row.date,
            version: None,
            reason: None,
        },
        cl_api::ApiError::Conflict {
            reason: cl_api::ConflictReason::Stale,
            current_version: None,
            code: None,
            message: cl_api::UserText::default(),
        },
    );
    assert_eq!(s.state()["row_action"]["stale"], true);
    assert_eq!(s.state()["row_action"]["error"], true);
    s.reload_row_action();
    assert_eq!(s.state()["row_action"]["panel"], "confirm_void");
    assert_eq!(s.state()["row_action"]["stale"], false);
}

/// B1 (Q-P12h, Q-241): a card register in the engine's convention. The
/// engine sends a charge (a credit) as `deposit` and a payment (a debit) as
/// `payment`, with running balances in the natural sign (credit − debit);
/// only the stored account balance is debit-positive.
#[test]
fn a_card_register_shows_charges_under_charge_and_the_amount_owed_positive() {
    let (_d, link) = link();
    let card = demo_account(&link, "2050");
    let mut s = RegisterScreen::new(Arc::clone(&link), None, today(), Some(card));
    s.set_range(cl_model::register::RangePreset::AllDates);
    settle(&mut s, None);
    assert_eq!(s.account(), Some(card));
    assert_eq!(s.class, AccountClass::CreditCard);
    let total = s.model.total().expect("total");
    assert!(total > 2, "the sample card has activity");
    settle(&mut s, Some(&(0..total)));

    let chart = link
        .demo_state()
        .and_then(|d| d.chart.clone())
        .expect("chart");
    let q = s.query.clone().expect("q");
    let per = u16::try_from(total).expect("rows");
    let page = crate::company::demo::register::page(
        &crate::company::demo::register::DemoLedger::default(),
        &chart,
        card,
        &q,
        1,
        per,
    )
    .expect("page");
    let text = |a: Option<&cl_api::RegisterAmount>| a.and_then(|a| a.display(false));
    let (mut charges, mut payments) = (0, 0);
    for (i, l) in page.lines.iter().enumerate() {
        let row = s.model.row_at(i).expect("row");
        assert_eq!(row.line_id, l.line_id);
        // The first column (Charge) holds the credit, sent as `deposit`.
        assert_eq!(row.payment, text(l.deposit.as_ref()), "row {i}: Charge");
        assert_eq!(row.deposit, text(l.payment.as_ref()), "row {i}: Payment");
        // The running balance is shown exactly as sent.
        assert_eq!(row.balance, l.balance.display(false), "row {i}: balance");
        // A charge credits the card: its amount sits under Charge only.
        if row.payment.is_some() {
            assert!(row.deposit.is_none(), "row {i}: one column");
            assert_eq!(row.kind, "CHARGE", "row {i}");
            charges += 1;
        }
        // A payment (here a check from the bank) debits it, labelled as one.
        if row.deposit.is_some() {
            assert_eq!(row.kind, "PMT", "row {i}");
            payments += 1;
        }
    }
    assert!(
        charges > 0 && payments > 0,
        "{charges} charges, {payments} payments"
    );
    let owed = page
        .lines
        .last()
        .and_then(|l| l.balance.exact())
        .expect("last balance");
    assert!(owed.is_positive(), "a card owing money reads positive");
    let first_header = s
        .grid
        .spec()
        .columns
        .iter()
        .find(|c| c.id == view::COL_PAYMENT)
        .map(|c| c.header.as_str());
    assert_eq!(first_header, Some("register.col.charge"));

    // The footer: the engine's ending balance as sent, positive.
    s.footer_load();
    let layout = |m: cl_core::Money| {
        crate::screens::lists::center::model::layout_plain(
            &cl_api::RegisterAmount::Exact(m)
                .display(false)
                .expect("display"),
            s.model.style,
        )
    };
    let shown = s.footer_shown();
    assert_eq!(shown.ending, layout(owed));
    // The stored balance is debit-positive (negative while money is owed);
    // as a fallback it is flipped, so it reads the same.
    let mut sum = s.footer.summary.clone().expect("summary");
    let stored = sum.current_balance.exact().expect("exact");
    assert_eq!(stored, owed.checked_neg().expect("neg"));
    sum.ending_balance = None;
    s.footer.summary = Some(sum);
    s.footer.page_ending = None;
    assert_eq!(s.footer_shown().ending, layout(owed));
}

#[test]
fn a_charge_typed_under_charge_lands_under_charge() {
    let (_d, link) = link();
    let card = demo_account(&link, "2050");
    let mut s = RegisterScreen::new(Arc::clone(&link), None, today(), Some(card));
    s.set_range(cl_model::register::RangePreset::AllDates);
    settle(&mut s, None);
    let before = s.model.total().expect("total");
    let fuel = demo_account(&link, "6100");
    s.entry.form.edit_step(|d| {
        d.account = Some(fuel);
        d.payment = Some(money("42.17"));
    });
    assert_eq!(
        s.draft_kind(),
        Some(cl_model::register::EntryKind::CardCharge)
    );
    assert!(s.start_record(), "the demo saves at once");
    settle(&mut s, None);
    let total = s.model.total().expect("total");
    assert_eq!(total, before + 1);
    settle(&mut s, Some(&(0..total)));
    let rows: Vec<_> = (0..usize::try_from(total).expect("n"))
        .filter_map(|i| s.model.row_at(i))
        .filter(|r| r.date == today() && r.payment.as_deref() == Some("42.17"))
        .collect();
    assert_eq!(rows.len(), 1, "the charge sits in the Charge column");
    assert!(rows[0].deposit.is_none());
}

fn transfer_identity(amount: &str, edit: Option<u64>, password: bool) -> record::SaveIdentity {
    let a = AccountId::parse("d0000000-0000-4000-8000-a00000001010").expect("id");
    let b = AccountId::parse("d0000000-0000-4000-8000-a00000001020").expect("id");
    let built = entry_row::Built::Bank(
        cl_api::BankPosting::transfer(cl_api::TransferDraft {
            from: a,
            to: b,
            date: today(),
            amount: money(amount),
            memo: None,
        })
        .expect("transfer"),
    );
    let id =
        cl_core::ids::TransactionId::parse("d0000000-0000-4000-8000-b00000000001").expect("id");
    record::SaveIdentity {
        built,
        edit: edit.map(|v| (id, cl_api::RecordVersion::Server(v))),
        password,
    }
}

#[test]
fn the_idempotency_key_follows_the_request_sent() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let first = transfer_identity("10.00", Some(1), false);
    let k = s.entry.key_for(&first);
    assert_eq!(s.entry.key_for(&first), k, "the same request: the same key");
    // Anything the request sends changes the key.
    for other in [
        transfer_identity("10.01", Some(1), false),
        transfer_identity("10.00", Some(2), false),
        transfer_identity("10.00", None, false),
        transfer_identity("10.00", Some(1), true),
    ] {
        let mut s2 = screen(&link);
        let k1 = s2.entry.key_for(&first);
        assert_ne!(s2.entry.key_for(&other), k1);
    }
}

#[test]
fn a_definitive_refusal_rotates_the_key_and_an_unknown_outcome_keeps_it() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let id = transfer_identity("10.00", None, false);
    let k = s.entry.key_for(&id);
    // Transport trouble: the save may be recorded; a retry must replay.
    for e in [
        cl_api::ApiError::Timeout,
        cl_api::ApiError::EngineUnavailable { permanent: false },
        cl_api::ApiError::Server {
            status: 500,
            code: None,
        },
        cl_api::ApiError::SavedUnreadable { what: "test" },
    ] {
        s.record_failed(e);
        assert_eq!(s.entry.key_for(&id), k, "kept");
    }
    // A definitive answer: the next try is a new request.
    for e in [
        cl_api::ApiError::ClosingPasswordRejected,
        cl_api::ApiError::RateLimited { retry_after: None },
        cl_api::ApiError::NotFound { code: None },
    ] {
        let before = s.entry.key_for(&id);
        s.record_failed(e);
        assert!(s.entry.key.is_none(), "rotated");
        assert_ne!(s.entry.key_for(&id), before);
    }
}

#[test]
fn a_stamp_read_started_before_an_own_write_is_ignored() {
    let (mut last, mut quiet) = (Some(1_u64), false);
    // Someone else's change: reload.
    assert!(stamp_arrived(&mut last, &mut quiet, true, 2));
    assert_eq!(last, Some(2));
    // An own write: the next fresh read only resets the baseline.
    quiet = true;
    // A read that started before the write answers first: ignored, the
    // quiet re-read is kept for the read that follows the write.
    assert!(!stamp_arrived(&mut last, &mut quiet, false, 2));
    assert!(quiet && last == Some(2));
    assert!(!stamp_arrived(&mut last, &mut quiet, true, 3), "own write");
    assert!(!quiet && last == Some(3));
    // Unchanged: nothing.
    assert!(!stamp_arrived(&mut last, &mut quiet, true, 3));
}

#[test]
fn row_keys_of_lines_no_longer_loaded_are_dropped_past_the_cap() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, Some(&(0..5)));
    let key = s.model.row_key(3).expect("key");
    let journal = s.model.line_of(key).expect("line");
    // Another query: nothing loaded yet, so a prune drops every key.
    s.set_sort(SortChoice::DateNewest);
    s.model.prune_keys_for_test(0);
    assert!(s.model.key_of(journal).is_none());
    assert!(s.model.line_of(key).is_none());
    // Loaded lines get keys again and keep them through a prune.
    settle(&mut s, Some(&(0..5)));
    let k = s.model.row_key(0).expect("key");
    s.model.prune_keys_for_test(0);
    assert_eq!(s.model.row_key(0), Some(k));
}

#[test]
fn busy_keeps_the_key_and_other_conflicts_drop_it() {
    let conflict = |reason, code: &str| cl_api::ApiError::Conflict {
        reason,
        current_version: None,
        code: cl_api::ErrorCode::parse(code),
        message: cl_api::UserText::default(),
    };
    for code in ["BATCH_BUSY", "TENANT_BUSY", "IDEMPOTENCY_IN_PROGRESS"] {
        assert!(
            keeps_key(&conflict(cl_api::ConflictReason::Busy, code)),
            "{code}"
        );
    }
    assert!(!keeps_key(&conflict(
        cl_api::ConflictReason::Stale,
        "STALE"
    )));
    assert!(!keeps_key(&conflict(
        cl_api::ConflictReason::Unspecified,
        "IDEMPOTENCY_KEY_REUSED"
    )));
    assert!(keeps_key(&cl_api::ApiError::Timeout));
}

/// The grouped-deposit fixture cut down to the bank line and its first
/// payment, amounts made to agree.
fn one_payment_deposit() -> cl_api::Transaction {
    #[derive(serde::Deserialize)]
    struct W {
        transaction: cl_api::Transaction,
    }
    let body = include_str!("../../../../../fixtures/engine/deposit-grouped-fees.json");
    let mut t = serde_json::from_str::<W>(body).expect("txn").transaction;
    t.lines.truncate(2);
    t.lines[0].debit = t.lines[1].credit;
    t.total = Some(t.lines[1].credit);
    t
}

fn seeds_read_only(t: &cl_api::Transaction) -> bool {
    let v = cl_api::Versioned {
        record: t.clone(),
        version: cl_api::RecordVersion::Server(1),
    };
    let own = t.lines[0].account_id;
    matches!(
        edit::seed(&v, own, cl_model::register::AccountClass::Bank),
        edit::Seed::ReadOnly(_)
    )
}

#[test]
fn b1_a_one_payment_deposit_or_vendor_refund_opens_make_deposit_not_the_row() {
    use cl_model::register::AccountClass as C;
    let t = one_payment_deposit();
    assert!(t.lines[1].source_transaction_id.is_some());
    // One payment out of Payments Clearing: never edited in the row (the
    // row would send it without its link and release the payment).
    assert!(seeds_read_only(&t));
    let bank = t.lines[0].account_id;
    let is = |c: C| move |id: AccountId| Some(if id == bank { C::Bank } else { c });
    assert_eq!(
        edit::form_for(&t, &is(C::OtherAsset)),
        Some(crate::screens::deposit::KIND),
        "Make Deposit edits it"
    );
    // A one-line vendor refund: A/P, applied to a vendor credit.
    let mut refund = t.clone();
    refund.lines[1].contact_id = None;
    assert!(seeds_read_only(&refund));
    assert_eq!(
        edit::form_for(&refund, &is(C::Payable)),
        Some(crate::screens::deposit::KIND)
    );
    // A cash-back line has an engine role: Make Deposit too.
    let mut cash = t.clone();
    cash.lines[1].source_transaction_id = None;
    cash.lines[1].line_role = Some(cl_api::LINE_ROLE_CASH_BACK.into());
    assert!(seeds_read_only(&cash));
    // The same deposit without links stays in the row.
    let mut plain = t;
    plain.lines[1].source_transaction_id = None;
    plain.lines[1].contact_id = None;
    assert!(!seeds_read_only(&plain));
}

#[test]
fn b3_a_numbered_journal_entry_opens_make_journal_entry_and_keeps_its_number() {
    let (_d, link) = link();
    let bank = demo_account(&link, "1010");
    let fuel = demo_account(&link, "6100");
    let entry = cl_api::NewJournalEntry::new_with(
        CalendarDate::from_ymd(2026, 11, 3).expect("date"),
        Some("fuel".into()),
        cl_api::Basis::Both,
        vec![
            cl_api::JeLine {
                account: fuel,
                side: cl_api::Side::Debit(money("40.00")),
                description: None,
                tag: None,
                contact: None,
            },
            cl_api::JeLine {
                account: bank,
                side: cl_api::Side::Credit(money("40.00")),
                description: None,
                tag: None,
                contact: None,
            },
        ],
        cl_api::JeOptions {
            number: Some("JE-1042".into()),
            ..cl_api::JeOptions::default()
        },
        &cl_api::AccountRoles::default(),
    )
    .expect("entry");
    let id = {
        let mut demo = link.demo_state().expect("demo");
        crate::company::demo::journal::save(&mut demo, &entry, None, None)
            .expect("saved")
            .0
    };
    let mut s = screen(&link);
    settle(&mut s, None);
    let key = key_of(&mut s, id);
    let _ = link.drain();
    s.begin_edit(key, false);
    assert!(s.entry.editing.is_none(), "not in the row");
    assert_eq!(
        opened(&link),
        Some((crate::screens::journal_entry::KIND.to_string(), id))
    );
    let mut demo = link.demo_state().expect("demo");
    let saved = crate::company::demo::journal::get(&mut demo, id).expect("get");
    assert_eq!(saved.record.txn_number.as_deref(), Some("JE-1042"));
}

#[test]
fn b4_a_check_opens_write_check_with_its_bank() {
    let (_d, link) = link();
    let bank = demo_account(&link, "1010");
    let fuel = demo_account(&link, "6100");
    let id = {
        let mut demo = link.demo_state().expect("demo");
        let chart = demo
            .chart
            .get_or_insert_with(crate::company::demo::chart)
            .clone();
        let p = crate::company::demo::register::DemoPosting {
            kind: "check",
            date: CalendarDate::from_ymd(2026, 11, 3).expect("date"),
            number: "5120".into(),
            contact: None,
            memo: String::new(),
            own: bank,
            own_debit: false,
            lines: vec![(fuel, money("25.00"))],
        };
        demo.ledger.save(&chart, &p, None).expect("saved")
    };
    let mut s = screen(&link);
    settle(&mut s, None);
    let key = key_of(&mut s, id);
    let _ = link.drain();
    // Enter on the check: Write Check (update_check, the printed-check
    // question, header tags), never the row's plain expense update.
    s.begin_edit(key, false);
    assert!(s.entry.editing.is_none());
    assert_eq!(
        opened(&link),
        Some((crate::screens::write_check::KIND.to_string(), id))
    );
    let mut wc = crate::screens::write_check::WriteCheckScreen::new(
        Arc::clone(&link),
        None,
        CalendarDate::from_ymd(2026, 11, 4).expect("date"),
    );
    assert!(crate::windows::Screen::open_record(&mut wc, id));
    assert_eq!(wc.bank(), Some(bank), "the bank the check credits");
    assert_eq!(wc.header.current().number.text().as_deref(), Some("5120"));
}

#[test]
fn b2_the_entry_row_sends_nothing_after_an_unknown_outcome_until_the_recheck_answers() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    let before = s.model.total().expect("total");
    let fuel = demo_account(&link, "6100");
    let vendor = demo_contact(&link, "Hollister Fuel Stop");
    s.entry.form.edit_step(|d| {
        d.payee = Some(vendor);
        d.account = Some(fuel);
        d.payment = Some(money("125.50"));
    });
    let lists = Arc::clone(&s.lists);
    let _ = s
        .entry
        .form
        .begin_save(cl_model::forms::SaveIntent::SaveAndNew, &*lists);
    s.record_failed(cl_api::ApiError::OutcomeUnknown {
        request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000006")
            .expect("id"),
        idempotency_key: Some(cl_api::IdempotencyKey::new()),
        keyed: false,
    });
    assert!(s.entry.error.is_none(), "the banner speaks");
    assert!(!s.start_record());
    assert!(!s.start_record());
    settle(&mut s, None);
    assert_eq!(s.model.total(), Some(before), "nothing sent");
    s.entry
        .lost
        .recheck_answering(s.entry.form.form_mut(), cl_api::Recheck::Unknown);
    s.poll_entry();
    assert!(!s.start_record());
    s.entry
        .lost
        .recheck_answering(s.entry.form.form_mut(), cl_api::Recheck::NotCommitted);
    s.poll_entry();
    assert!(s.start_record());
    settle(&mut s, None);
    assert_eq!(s.model.total(), Some(before + 1), "sent once");
}

// ------------------------------------------------- P17 WP2b: the A/P guard

/// Every row of the register of `account` (all dates), with its key.
fn all_rows(s: &mut RegisterScreen) -> Vec<(cl_grid::RowKey, view::RegRow)> {
    s.set_range(cl_model::register::RangePreset::AllDates);
    // Read again: the test may have entered something in the demo.
    s.ledger_changed();
    settle(s, None);
    let total = s.model.total().expect("total");
    settle(s, Some(&(0..total)));
    (0..usize::try_from(total).expect("rows"))
        .filter_map(|i| Some((s.model.row_key(i)?, s.model.row_at(i)?.clone())))
        .collect()
}

/// Enters a bill (or, with `credit`, a vendor credit) of 12.00 on the
/// demo's Accounts Payable (2000) against fuel (6100), dated today.
fn demo_ap_document(link: &Arc<CompanyLink>, kind: &'static str, credit: bool) {
    let ap = demo_account(link, "2000");
    let fuel = demo_account(link, "6100");
    let mut demo = link.demo_state().expect("demo");
    let chart = demo
        .chart
        .get_or_insert_with(crate::company::demo::chart)
        .clone();
    let posting = crate::company::demo::register::DemoPosting {
        kind,
        date: today(),
        number: String::new(),
        contact: Some("Hollister Fuel Stop".into()),
        memo: String::new(),
        own: ap,
        own_debit: credit,
        lines: vec![(fuel, money("12.00"))],
    };
    demo.ledger.save(&chart, &posting, None).expect("saved");
}

/// The first open (not void) row of `kind`, in or out of the closed period.
fn ap_row(
    s: &mut RegisterScreen,
    kind: cl_api::TxnType,
    locked: bool,
) -> (cl_grid::RowKey, view::RegRow) {
    all_rows(s)
        .into_iter()
        .find(|(_, r)| r.txn_type == kind && !r.void && s.model.locked(r) == locked)
        .unwrap_or_else(|| panic!("no {kind:?} row (locked {locked}) in the demo"))
}

#[test]
fn void_routes_follow_the_kind() {
    use cl_api::TxnType as T;
    use dialogs::VoidRoute as R;
    let all = [
        T::Invoice,
        T::CustomerPayment,
        T::CashSale,
        T::Expense,
        T::Deposit,
        T::Transfer,
        T::JournalEntry,
        T::CreditMemo,
        T::CustomerRefund,
        T::Bill,
        T::VendorCredit,
        T::BillPayment,
        T::DailySales,
        T::Aje,
    ];
    for t in all {
        let r = R::of(t);
        let ap = matches!(t, T::Bill | T::VendorCredit | T::BillPayment);
        assert_eq!(r.is_ap(), ap, "{t:?}");
        assert_eq!(r.delete_refused().is_some(), ap, "{t:?}");
        assert_eq!(r.refused().is_some(), ap, "{t:?}");
        // The engine never soft-deletes an A/P document either (Q-245).
        assert!(!(ap && t.is_deletable()), "{t:?}");
    }
    assert_eq!(R::of(T::Bill), R::Bill);
    assert_eq!(R::of(T::VendorCredit), R::VendorCredit);
    assert_eq!(R::of(T::BillPayment), R::BillPayment);
    assert_eq!(R::of(T::Expense), R::Generic);
    assert_eq!(
        R::Bill.delete_refused(),
        Some("register.ap.delete_document")
    );
    assert_eq!(
        R::VendorCredit.delete_refused(),
        Some("register.ap.delete_document")
    );
    assert_eq!(
        R::BillPayment.delete_refused(),
        Some("register.ap.delete_payment")
    );
    for key in [
        "register.ap.delete_document",
        "register.ap.delete_payment",
        "register.ap.void_bill_paid",
        "register.ap.void_credit_applied",
        "register.ap.void_payment_refused",
        "register.ap.void_payment_body",
        "register.ap.closed",
    ] {
        assert_ne!(crate::i18n::t(key), key, "{key} has English text");
    }
}

#[test]
fn a_bill_row_is_never_deleted_and_voids_through_the_bill_route() {
    let (_d, link) = link();
    let ap = demo_account(&link, "2000");
    let mut s = RegisterScreen::new(Arc::clone(&link), None, today(), Some(ap));
    // The demo's sample bills post to an A/P account outside its chart, so
    // one is entered on Accounts Payable here.
    demo_ap_document(&link, "bill", false);
    let (key, row) = ap_row(&mut s, cl_api::TxnType::Bill, false);
    s.actions.cursor = Some(key);
    // Delete is refused in plain words and offers Void, with or without
    // the engine's delete; nothing is sent.
    for caps in [None, Some((true, false))] {
        s.actions.caps = caps;
        s.actions.panel = dialogs::Panel::None;
        assert!(s.command(CommandId::new("form.delete_item")));
        let st = s.state()["row_action"].clone();
        assert_eq!(st["panel"], "offer_void");
        assert_eq!(st["notice"], "register.ap.delete_document");
        assert_eq!(st["count"], 0, "nothing sent");
        assert_eq!(st["route"], serde_json::Value::Null);
    }
    s.actions.caps = None;
    s.actions.panel = dialogs::Panel::None;
    assert!(!row_of(&s, key).void);
    // Void asks first, then goes to the bill's own route.
    assert!(s.command(CommandId::new("edit.void_transaction")));
    assert_eq!(s.state()["row_action"]["panel"], "confirm_void");
    s.confirm();
    let st = s.state()["row_action"].clone();
    assert_eq!(st["last"], "void");
    assert_eq!(st["route"], "bill");
    assert_eq!(st["error"], false);
    let rows = all_rows(&mut s);
    assert!(
        rows.iter().any(|(_, r)| r.txn_id == row.txn_id && r.void),
        "the bill is void"
    );
    // A vendor credit: the same refusal, and its own route.
    demo_ap_document(&link, "vendor_credit", true);
    let (key, _) = ap_row(&mut s, cl_api::TxnType::VendorCredit, false);
    s.actions.cursor = Some(key);
    s.actions.panel = dialogs::Panel::None;
    assert!(s.command(CommandId::new("form.delete_item")));
    assert_eq!(
        s.state()["row_action"]["notice"],
        "register.ap.delete_document"
    );
    s.actions.panel = dialogs::Panel::None;
    assert!(s.command(CommandId::new("edit.void_transaction")));
    s.confirm();
    assert_eq!(s.state()["row_action"]["route"], "vendor_credit");
    assert_eq!(s.state()["row_action"]["count"], 2);
}

#[test]
fn a_bill_payment_row_voids_through_its_own_route_and_is_never_deleted() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let (key, row) = ap_row(&mut s, cl_api::TxnType::BillPayment, false);
    s.actions.cursor = Some(key);
    assert!(s.command(CommandId::new("form.delete_item")));
    assert_eq!(
        s.state()["row_action"]["notice"],
        "register.ap.delete_payment"
    );
    assert_eq!(s.state()["row_action"]["count"], 0);
    // The offer's Void: read, asked, then the bill-payment route.
    s.open_confirm(dialogs::ActionKind::Void, row.txn_id, row.date);
    assert_eq!(s.state()["row_action"]["panel"], "confirm_void");
    s.confirm();
    let st = s.state()["row_action"].clone();
    assert_eq!(
        (st["last"].clone(), st["route"].clone()),
        ("void".into(), "bill_payment".into())
    );
}

#[test]
fn a_closed_period_ap_row_is_explained_and_nothing_is_sent() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let (key, row) = ap_row(&mut s, cl_api::TxnType::BillPayment, true);
    s.actions.cursor = Some(key);
    assert!(s.command(CommandId::new("edit.void_transaction")));
    s.confirm();
    let st = s.state()["row_action"].clone();
    assert_eq!(st["panel"], "notice");
    assert_eq!(st["notice"], "register.ap.closed");
    assert_eq!(st["closing"], "idle", "no password is asked for: {st}");
    assert_eq!(
        (st["count"].clone(), st["working"].clone()),
        (0.into(), false.into())
    );
    let rows = all_rows(&mut s);
    assert!(rows.iter().any(|(_, r)| r.txn_id == row.txn_id && !r.void));
}

#[test]
fn ap_rows_never_open_an_in_place_edit() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let own = s.account().expect("account");
    let (key, row) = ap_row(&mut s, cl_api::TxnType::BillPayment, false);
    s.begin_edit(key, false);
    assert!(s.entry.editing.is_none(), "no edit in the row");
    // Whatever its shape, an A/P document is shown read-only: the register's
    // save (an expense or journal update) never reaches it.
    let (_, check) = all_rows(&mut s)
        .into_iter()
        .find(|(_, r)| r.txn_type == cl_api::TxnType::Expense && r.editable && !r.void)
        .expect("a check");
    let read = |id| {
        let mut demo = link.demo_state().expect("demo");
        let chart = demo
            .chart
            .get_or_insert_with(crate::company::demo::chart)
            .clone();
        let contacts = demo.contacts_mut().clone();
        demo.ledger.get(&chart, &contacts, id).expect("read")
    };
    let paid = read(row.txn_id);
    assert!(matches!(
        edit::seed(&paid, own, AccountClass::Bank),
        edit::Seed::ReadOnly(_)
    ));
    let mut v = read(check.txn_id);
    assert!(matches!(
        edit::seed(&v, own, AccountClass::Bank),
        edit::Seed::Edit { .. }
    ));
    for t in [
        cl_api::TxnType::Bill,
        cl_api::TxnType::VendorCredit,
        cl_api::TxnType::BillPayment,
    ] {
        v.record.txn_type = t;
        assert!(
            matches!(
                edit::seed(&v, own, AccountClass::Bank),
                edit::Seed::ReadOnly(_)
            ),
            "{t:?}"
        );
        assert_eq!(edit::form_for(&v.record, &|_| None), None, "{t:?}");
    }
}

/// The stub engine's writes, as `METHOD /path`.
async fn posts(server: &wiremock::MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|r| r.method.as_str() != "GET")
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect()
}

/// The write the queue runs, against a stub engine: a bill's Void goes to
/// `POST /bills/{id}/void` and never to the generic route; an A/P Delete,
/// or a password for an A/P route, sends nothing at all; the engine's
/// refusal comes back as a plain notice on the screen.
#[test]
fn the_ap_write_never_reaches_the_generic_routes() {
    use dialogs::{ActionKind, RowWrite, VoidRoute, row_write};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    const ID: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    let id = cl_core::ids::TransactionId::parse(ID).expect("id");
    let w = |kind, route, password: Option<cl_core::Secret>| RowWrite {
        kind,
        route,
        id,
        version: None,
        reason: None,
        void_reason: "Voided in the register".into(),
        password,
        key: cl_api::IdempotencyKey::new(),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let refused = rt.block_on(async {
        let server = MockServer::start().await;
        let session = cl_api::testing::stub_session(server.address().port());
        // Refused before anything goes out.
        for (kind, route, pw) in [
            (ActionKind::Delete, VoidRoute::Bill, None),
            (ActionKind::Delete, VoidRoute::VendorCredit, None),
            (ActionKind::Delete, VoidRoute::BillPayment, None),
            (ActionKind::Void, VoidRoute::Bill, Some(cl_core::Secret::new("pw"))),
        ] {
            let e = row_write(session.clone(), w(kind, route, pw))
                .await
                .expect_err("refused");
            assert_eq!(e, ApiError::Local(cl_api::LocalValidation::InvalidValue));
        }
        assert_eq!(
            server.received_requests().await.map(|r| r.len()),
            Some(0),
            "nothing sent"
        );
        // Void on a bill: its own route only.
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/bills/{ID}/void")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"bill":{}}"#, "application/json"),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        row_write(session.clone(), w(ActionKind::Void, VoidRoute::Bill, None))
            .await
            .expect("voided");
        assert_eq!(
            posts(&server).await,
            vec![format!("POST /api/v1/bills/{ID}/void")]
        );
        // The engine refuses a paid bill: a validation error, still only the
        // bill route.
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/bills/{ID}/void")))
            .respond_with(ResponseTemplate::new(400).set_body_raw(
                r#"{"error":{"message":"Cannot void a bill that has payments applied. Void the payments first."}}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let e = row_write(session.clone(), w(ActionKind::Void, VoidRoute::Bill, None))
            .await
            .expect_err("paid");
        assert!(matches!(e, ApiError::Validation(_)), "{e:?}");
        let sent = posts(&server).await;
        assert_eq!(sent.len(), 2);
        assert!(
            sent.iter().all(|p| !p.contains("/transactions/")),
            "never the generic route: {sent:?}"
        );
        e
    });
    // The screen says it plainly, with no generic error and no reload.
    let (_d, link) = link();
    let mut s = screen(&link);
    let row = all_rows(&mut s).remove(0).1;
    let pending = |route| dialogs::Pending::Row {
        kind: dialogs::ActionKind::Void,
        route,
        id: row.txn_id,
        date: row.date,
        version: None,
        reason: None,
    };
    s.row_action_failed(&pending(VoidRoute::Bill), refused.clone());
    let st = s.state()["row_action"].clone();
    assert_eq!(st["notice"], "register.ap.void_bill_paid");
    assert_eq!(
        (st["error"].clone(), st["stale"].clone()),
        (false.into(), false.into())
    );
    s.row_action_failed(&pending(VoidRoute::VendorCredit), refused.clone());
    assert_eq!(
        s.state()["row_action"]["notice"],
        "register.ap.void_credit_applied"
    );
    s.row_action_failed(&pending(VoidRoute::BillPayment), refused);
    assert_eq!(
        s.state()["row_action"]["notice"],
        "register.ap.void_payment_refused"
    );
    // A closed period on an A/P route has no password path: explained.
    let closed = ApiError::ClosedPeriod(cl_api::ClosedPeriod {
        lock_date: s.model.closing,
        can_override: true,
        password_required: Some(true),
        message: cl_api::UserText::default(),
    });
    s.row_action_failed(&pending(VoidRoute::Bill), closed);
    let st = s.state()["row_action"].clone();
    assert_eq!(st["notice"], "register.ap.closed");
    assert_eq!(st["closing"], "idle");
}

/// A02 (Q-A02f): the selected transaction's files. A check with a file
/// turns Show document on; it fetches the first file and asks the shell to
/// pop the viewer out, which then follows the cursor. A row without files
/// leaves it off. No file name in the state.
#[test]
fn show_document_opens_the_selected_transactions_first_file() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    let fuel = demo_account(&link, "6100");
    let vendor = demo_contact(&link, "Hollister Fuel Stop");
    s.entry.form.edit_step(|d| {
        d.payee = Some(vendor);
        d.account = Some(fuel);
        d.payment = Some(money("12.00"));
    });
    assert!(s.start_record());
    settle(&mut s, None);
    let key = last_key(&mut s);
    let id = s.model.row_for_key(key).expect("row").txn_id;
    {
        let mut demo = link.demo_state().expect("demo");
        crate::company::demo::checks::upload(
            &mut demo,
            cl_api::AttachTo::Expense(id),
            "secret-receipt.pdf",
            b"%PDF-1.4\n%synthetic\n",
        )
        .expect("upload");
    }
    let ctx = egui::Context::default();
    s.actions.cursor = Some(key);
    s.sync_docs(&ctx);
    assert_eq!(s.state()["docs"]["listed"], false, "the cursor rests first");
    at_time(&ctx, 1.0);
    s.sync_docs(&ctx);
    assert_eq!(s.state()["docs"]["count"], 1);
    assert!(s.document().is_none(), "nothing fetched until asked");
    s.show_document(&ctx);
    assert!(s.document().is_some());
    assert!(!s.state().to_string().contains("secret"));
    // The pop-out request is waiting for the shell.
    let doc_viewer_state = ctx.data(|d| {
        d.get_temp::<crate::windows::doc_viewer::PaneRequest>(egui::Id::unique(
            "cl-doc-viewer-request",
        ))
    });
    assert_eq!(
        doc_viewer_state,
        Some(crate::windows::doc_viewer::PaneRequest::PopOut)
    );
    // Another row without files: nothing to show.
    let total = s.model.total().expect("total");
    settle(&mut s, Some(&(0..total)));
    let other = s.model.row_key(0).expect("first row");
    s.actions.cursor = Some(other);
    s.sync_docs(&ctx);
    at_time(&ctx, 2.0);
    s.sync_docs(&ctx);
    assert_eq!(s.state()["docs"]["listed"], true);
    assert_eq!(s.state()["docs"]["count"], 0);
    assert!(s.document().is_none());
}

/// Run one empty frame at `time` (egui seconds), for the rest delay.
fn at_time(ctx: &egui::Context, time: f64) {
    let raw = egui::RawInput {
        time: Some(time),
        ..egui::RawInput::default()
    };
    let mut out = ctx.run_ui(raw, |_| {});
    out.textures_delta.clear();
}

/// Review S3: moving the cursor over rows asks for nothing until it
/// rests; an answer for a row the cursor left is dropped; a list that
/// failed says so and is never taken for "no documents".
#[test]
fn row_documents_wait_for_the_cursor_and_say_when_the_list_failed() {
    let (_d, link) = link();
    let mut s = screen(&link);
    settle(&mut s, None);
    let total = s.model.total().expect("total");
    settle(&mut s, Some(&(0..total)));
    let a = s.model.row_key(0).expect("row");
    let b = s.model.row_key(1).expect("row");
    let id_a = s.model.row_for_key(a).expect("row").txn_id;
    let id_b = s.model.row_for_key(b).expect("row").txn_id;
    let ctx = egui::Context::default();
    at_time(&ctx, 10.0);
    // Down the register quickly: nothing is asked for on the way.
    s.actions.cursor = Some(a);
    s.sync_docs(&ctx);
    at_time(&ctx, 10.1);
    s.actions.cursor = Some(b);
    s.sync_docs(&ctx);
    assert_eq!(s.state()["docs"]["listed"], false);
    // A's answer arrives after the cursor left it: dropped.
    let queue = cl_api::ApiQueue::start(Arc::new(|| {}), cl_api::QueueConfig::default())
        .expect("queue");
    let group = queue.group().expect("group");
    let session = cl_api::testing::stub_session(9);
    let one = crate::company::demo::checks::attachments(
        &link.demo_state().expect("demo"),
        cl_api::AttachTo::Expense(id_a),
    );
    let late = group
        .read(&session, move |_s| async move { Ok(one) })
        .expect("read");
    s.docs_list_in_flight(id_a, crate::company::Task::Pending(late));
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while s.docs.busy() {
        assert!(std::time::Instant::now() < end, "no answer");
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.sync_docs(&ctx);
    }
    assert_eq!(s.state()["docs"]["listed"], false, "not B's list");
    // B's own list fails: it says so.
    let failed = group
        .read(&session, |_s| async move {
            Err::<Vec<cl_api::Attachment>, _>(ApiError::Internal)
        })
        .expect("read");
    s.docs_list_in_flight(id_b, crate::company::Task::Pending(failed));
    while s.docs.busy() {
        assert!(std::time::Instant::now() < end, "no answer");
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.sync_docs(&ctx);
    }
    assert_eq!(s.state()["docs"]["list_failed"], true);
    assert_eq!(s.state()["docs"]["listed"], false);
    assert!(crate::i18n::has("register.doc.list_failed"));
    // Resting on B again later asks once more (the demo answers).
    s.actions.cursor = Some(a);
    s.sync_docs(&ctx);
    s.actions.cursor = Some(b);
    s.sync_docs(&ctx);
    at_time(&ctx, 11.0);
    s.sync_docs(&ctx);
    assert_eq!(s.state()["docs"]["listed"], true);
    assert_eq!(s.state()["docs"]["list_failed"], false);
}
