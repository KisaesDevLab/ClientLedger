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
    let page = crate::company::demo::register::page(&chart, operating(&link), &q, page_no, 200)
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
    assert_eq!(row.kind, "CHARGE");
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
fn later_lane_commands_are_routed_but_not_handled_yet() {
    let (_d, link) = link();
    let mut s = screen(&link);
    for c in [
        "edit.find",
        "edit.edit_transaction",
        "form.delete_item",
        "edit.void_transaction",
        "list.quick_report",
        "form.save_new",
    ] {
        assert!(!s.command(CommandId::new(c)), "{c}");
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
    let sum = crate::company::demo::register::summary(&chart, operating(&link), Some(q.end))
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
    let current = crate::screens::lists::center::model::layout_plain(
        &sum.current_balance.display(false).expect("display"),
        s.model.style,
    );
    assert_eq!(shown.ending, current, "range reaches today: stored balance");
    // A range that ends before today has no engine figure: a dash.
    if let Some(q) = s.query.as_mut() {
        q.end = CalendarDate::from_ymd(2026, 10, 1).expect("date");
    }
    assert_eq!(s.footer_shown().ending, "\u{2014}");
    // A failed summary shows a dash and never blocks the register.
    s.footer.summary = None;
    s.footer.summary_failed = true;
    let shown = s.footer_shown();
    assert_eq!(shown.ending, "\u{2014}");
    assert!(shown.failed && shown.last_reconciled.is_none());
}
