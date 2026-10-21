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
        if !r.txn_type.is_deletable() && !seen.0 {
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
