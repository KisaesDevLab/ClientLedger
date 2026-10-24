//! Daily Sales Summary screen tests (no engine): a test double of the
//! engine's daily sales answers at once, computes the preview with the
//! exact twin, and counts what was sent.

use std::sync::{Arc, Mutex};

use cl_api::{
    ApiError, DailyEntry, DailyEntryDraft, DailyEntryQuery, DailyEntrySummary, DailyLine,
    DailyLineInput, DailyPreview, DailyTemplate, DailyTotals, DailyValue, EntryStatus, NormalSide,
    Preset, Section,
};
use cl_core::ids::{AccountId, DailyEntryId, DailyLineId, DailyTemplateId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use cl_model::forms::SaveIntent;
use cl_model::forms::daily_sales::{DailySalesForm, OverShort, totals};

use super::*;
use crate::company::CompanyLink;
use crate::company::demo::register::DEMO_CLOSING_PASSWORD;
use crate::screens::company::form::SecretText;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn acct(n: u32) -> AccountId {
    crate::company::demo::account_id(n)
}

/// A fresh id text (no uuid crate here).
fn fresh() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("7a0c4e8e-6f6e-4c1d-8a37-{n:012x}")
}

fn line(
    section: Section,
    label: &str,
    account: Option<AccountId>,
    side: NormalSide,
    sort: u32,
) -> DailyLine {
    DailyLine {
        id: DailyLineId::parse(&fresh()).expect("id"),
        section,
        label: label.into(),
        account,
        tag: None,
        side,
        required: false,
        allow_tag: false,
        active: true,
        sort,
    }
}

/// The plan's "Plant sale counter".
fn counter() -> DailyTemplate {
    DailyTemplate {
        id: DailyTemplateId::parse(&fresh()).expect("id"),
        name: "Plant sale counter".into(),
        preset: Preset::Retail,
        is_active: true,
        default_tag: None,
        lines: vec![
            line(
                Section::Sales,
                "Plant sales",
                Some(acct(4000)),
                NormalSide::Credit,
                0,
            ),
            line(
                Section::Tax,
                "Sales Tax Collected",
                Some(acct(20900)),
                NormalSide::Credit,
                1,
            ),
            line(
                Section::Payment,
                "Cash",
                Some(acct(10150)),
                NormalSide::Debit,
                2,
            ),
            line(
                Section::Payment,
                "Cards",
                Some(acct(10150)),
                NormalSide::Debit,
                3,
            ),
            line(
                Section::Payout,
                "Paid-outs",
                Some(acct(6250)),
                NormalSide::Debit,
                4,
            ),
            line(Section::Sales, "Dept 2", None, NormalSide::Credit, 5),
        ],
    }
}

/// The test double: the engine's rules, answered at once.
#[derive(Default)]
struct Fake {
    templates: Vec<DailyTemplate>,
    entries: Vec<DailyEntry>,
    closing: Option<CalendarDate>,
    /// Added to the preview's over/short (a layout changed elsewhere).
    skew: Option<Money>,
    /// The next post fails with this.
    fail_post: Option<ApiError>,
    drafts_made: u32,
    posts: u32,
    voids: u32,
}

impl Fake {
    fn template_of(&self, id: DailyTemplateId) -> Result<&DailyTemplate, ApiError> {
        self.templates
            .iter()
            .find(|t| t.id == id && t.is_active)
            .ok_or(ApiError::NotFound { code: None })
    }

    fn compute(t: &DailyTemplate, values: &[DailyValue]) -> DailyPreview {
        let layout = layout_of(t);
        let form = DailySalesForm {
            template: Some(t.id),
            date: Some(today()),
            values: values.iter().map(|v| (v.line, Some(v.amount))).collect(),
            ..DailySalesForm::default()
        };
        let tot = totals(&form, &layout).expect("range");
        DailyPreview {
            debits: tot.debits,
            credits: tot.credits,
            over_short: tot.signed_over_short,
            balanced: tot.signed_over_short.is_zero(),
            sales: tot.sales,
            tax: tot.tax,
            payments: tot.payments,
            unmapped: tot
                .unmapped
                .iter()
                .map(|i| layout.lines[*i].label.clone())
                .collect(),
        }
    }

    fn locked(&self, id: DailyEntryId, password: Option<&Secret>) -> Result<(), ApiError> {
        let e = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .ok_or(ApiError::NotFound { code: None })?;
        crate::company::demo::register::check_closing(self.closing, &[e.date], password)?;
        Ok(())
    }
}

fn entry_of(id: DailyEntryId, d: &DailyEntryDraft) -> DailyEntry {
    DailyEntry {
        id,
        template: d.template,
        date: d.date,
        status: EntryStatus::Draft,
        transaction: None,
        tag: d.tag,
        notes: d.notes.clone(),
        values: d.values.clone(),
        over_short: Money::ZERO,
        totals: DailyTotals::default(),
        layout: None,
    }
}

impl DailyBooks for Fake {
    fn templates(&mut self) -> Result<Vec<DailyTemplate>, ApiError> {
        Ok(self
            .templates
            .iter()
            .filter(|t| t.is_active)
            .map(|t| DailyTemplate {
                lines: Vec::new(),
                ..t.clone()
            })
            .collect())
    }
    fn template(&mut self, id: DailyTemplateId) -> Result<DailyTemplate, ApiError> {
        self.template_of(id).cloned()
    }
    fn create_template(
        &mut self,
        name: &str,
        preset: Option<Preset>,
    ) -> Result<DailyTemplate, ApiError> {
        let mut t = match preset {
            Some(_) => counter(),
            None => DailyTemplate {
                lines: Vec::new(),
                ..counter()
            },
        };
        t.name = name.into();
        self.templates.push(t.clone());
        Ok(t)
    }
    fn rename(&mut self, id: DailyTemplateId, name: &str) -> Result<DailyTemplate, ApiError> {
        let t = self
            .templates
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(ApiError::NotFound { code: None })?;
        t.name = name.into();
        Ok(t.clone())
    }
    fn replace_lines(
        &mut self,
        id: DailyTemplateId,
        lines: &[DailyLineInput],
    ) -> Result<DailyTemplate, ApiError> {
        let t = self
            .templates
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(ApiError::NotFound { code: None })?;
        let mut out: Vec<DailyLine> = t
            .lines
            .iter()
            .map(|l| DailyLine {
                active: false,
                ..l.clone()
            })
            .collect();
        for (i, l) in lines.iter().enumerate() {
            let sort = u32::try_from(i).expect("sort");
            let new = DailyLine {
                id: l
                    .id
                    .unwrap_or_else(|| DailyLineId::parse(&fresh()).expect("id")),
                section: l.section.clone(),
                label: l.label.clone(),
                account: l.account,
                tag: l.tag,
                side: l.side,
                required: l.required,
                allow_tag: false,
                active: l.active,
                sort,
            };
            match out.iter_mut().find(|o| o.id == new.id) {
                Some(o) => *o = new,
                None => out.push(new),
            }
        }
        t.lines = out;
        Ok(t.clone())
    }
    fn delete_template(&mut self, id: DailyTemplateId) -> Result<(), ApiError> {
        if let Some(t) = self.templates.iter_mut().find(|t| t.id == id) {
            t.is_active = false;
        }
        Ok(())
    }
    fn list(&mut self, q: &DailyEntryQuery) -> Result<Vec<DailyEntrySummary>, ApiError> {
        Ok(self
            .entries
            .iter()
            .rev()
            .filter(|e| q.status.as_ref().is_none_or(|s| *s == e.status))
            .filter(|e| q.template.is_none_or(|t| t == e.template))
            .filter(|e| q.date.is_none_or(|d| d == e.date))
            .map(|e| DailyEntrySummary {
                id: e.id,
                template: e.template,
                template_name: None,
                date: e.date,
                status: e.status.clone(),
                transaction: e.transaction,
                over_short: e.over_short,
                sales: e.totals.sales,
            })
            .collect())
    }
    fn entry(&mut self, id: DailyEntryId) -> Result<DailyEntry, ApiError> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or(ApiError::NotFound { code: None })
    }
    fn create_draft(&mut self, d: &DailyEntryDraft) -> Result<DailyEntry, ApiError> {
        self.template_of(d.template)?;
        assert!(d.values.iter().all(|v| !v.amount.is_negative()));
        self.drafts_made += 1;
        let e = entry_of(DailyEntryId::parse(&fresh()).expect("id"), d);
        self.entries.push(e.clone());
        Ok(e)
    }
    fn update_draft(
        &mut self,
        id: DailyEntryId,
        d: &DailyEntryDraft,
    ) -> Result<DailyEntry, ApiError> {
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or(ApiError::NotFound { code: None })?;
        let mut new = entry_of(id, d);
        new.template = e.template;
        *e = new.clone();
        Ok(new)
    }
    fn preview(
        &mut self,
        template: DailyTemplateId,
        values: &[DailyValue],
    ) -> Result<DailyPreview, ApiError> {
        let t = self.template_of(template)?.clone();
        let mut p = Fake::compute(&t, values);
        if let Some(s) = self.skew {
            p.over_short = p.over_short.checked_add(s).expect("skew");
        }
        Ok(p)
    }
    fn post(
        &mut self,
        id: DailyEntryId,
        password: Option<&Secret>,
    ) -> Result<DailyEntry, ApiError> {
        if let Some(e) = self.fail_post.take() {
            return Err(e);
        }
        self.locked(id, password)?;
        let e = self.entry(id)?;
        let t = self.template_of(e.template)?.clone();
        let p = Fake::compute(&t, &e.values);
        if !p.unmapped.is_empty() {
            return Err(ApiError::Local(cl_api::LocalValidation::InvalidValue));
        }
        self.posts += 1;
        let e = self.entries.iter_mut().find(|x| x.id == id).expect("entry");
        e.status = EntryStatus::Posted;
        e.transaction = Some(TransactionId::parse(&fresh()).expect("id"));
        e.over_short = p.over_short;
        Ok(e.clone())
    }
    fn void(
        &mut self,
        id: DailyEntryId,
        password: Option<&Secret>,
    ) -> Result<DailyEntry, ApiError> {
        self.locked(id, password)?;
        self.voids += 1;
        let e = self
            .entries
            .iter_mut()
            .find(|x| x.id == id)
            .ok_or(ApiError::NotFound { code: None })?;
        e.status = EntryStatus::Void;
        Ok(e.clone())
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    link: Arc<CompanyLink>,
    fake: Arc<Mutex<Fake>>,
    s: DailySalesScreen,
}

fn rig_with(fake: Fake) -> Rig {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let fake = Arc::new(Mutex::new(fake));
    let books: SharedBooks = fake.clone();
    let s = DailySalesScreen::with_books(Arc::clone(&link), None, today(), Some(books));
    Rig {
        _dir: dir,
        link,
        fake,
        s,
    }
}

fn rig() -> Rig {
    rig_with(Fake {
        templates: vec![counter()],
        ..Fake::default()
    })
}

impl Rig {
    fn fake(&self) -> std::sync::MutexGuard<'_, Fake> {
        self.fake.lock().expect("fake")
    }

    /// Types `amount` on the line labelled `label`.
    fn type_amount(&mut self, label: &str, amount: &str) {
        let (index, id) = self
            .s
            .layout
            .lines
            .iter()
            .enumerate()
            .find(|(_, l)| l.label == label && l.active)
            .map_or_else(
                || panic!("{label}"),
                |(i, l)| (i, l.id.expect("saved line")),
            );
        self.s.inputs.amounts.insert(id, amount.into());
        self.s.amount_left(index, id);
    }

    fn plan_day(&mut self, cash: &str) {
        self.type_amount("Plant sales", "1,000.00");
        self.type_amount("Sales Tax Collected", "80");
        self.type_amount("Cash", cash);
        self.type_amount("Cards", "577.65");
    }
}

#[test]
fn opens_on_the_first_layout_with_its_lines() {
    let r = rig();
    let st = r.s.state();
    assert_eq!(st["screen"], "daily_sales");
    assert_eq!(st["loaded"], true);
    assert_eq!(st["layouts"], 1);
    assert_eq!(st["layout_set"], true);
    assert_eq!(st["lines"], 6);
    assert_eq!(st["mode"], "new");
    assert_eq!(st["dirty"], false);
    assert_eq!(r.s.inputs.layout, "Plant sale counter");
    assert_eq!(r.s.title(), "Daily Sales Summary");
    assert_eq!(st["empty_state"], false);
}

#[test]
fn the_plan_day_is_2_35_short_and_posts_with_the_engine_agreeing() {
    let mut r = rig();
    let mut other = r.link.bus().subscribe();
    r.plan_day("500.00");
    let tot = r.s.totals().expect("totals");
    assert_eq!(tot.over_short, OverShort::Short(m("2.35")));
    assert_eq!(
        form::over_short_text(tot.over_short, r.s.money_style),
        "Cash over/short: 2.35 short \u{2014} posts to Cash Over/Short"
    );
    assert_eq!(r.s.state()["over_short"], "short");
    let st = r.s.state().to_string();
    assert!(
        !st.contains("2.35") && !st.contains("1077"),
        "no amounts in the state"
    );
    r.s.command(CommandId::new("form.save_new"));
    assert_eq!(r.fake().posts, 1);
    assert_eq!(r.s.save.count, 1);
    assert_eq!(r.s.save.last_action, Some("post"));
    // Save & New: the next day, blank, same layout.
    assert_eq!(r.s.header.current().date, today().add_days(1).ok());
    assert!(r.s.header.current().values.is_empty());
    assert_eq!(r.s.state()["mode"], "new");
    // Q-262: the other windows hear the accounts the day touched.
    let heard = r.link.bus().take(&mut other);
    for a in [acct(4000), acct(20900), acct(10150)] {
        assert!(heard.ledger_touches(a), "{a:?}");
    }
    let over_short = system_account(&r.s.accounts, "cash_over_short");
    match over_short {
        Some(a) => assert!(heard.ledger_touches(a)),
        None => assert!(heard.ledger_any()),
    }
    // The over day: 0.65 over.
    r.plan_day("503.00");
    let tot = r.s.totals().expect("totals");
    assert_eq!(tot.over_short, OverShort::Over(m("0.65")));
    let posted = r
        .fake()
        .entries
        .iter()
        .find(|e| e.status == EntryStatus::Posted)
        .map(|e| e.over_short);
    assert_eq!(posted, Some(m("-2.35")));
}

#[test]
fn a_preview_mismatch_posts_nothing_and_the_draft_is_reused() {
    let mut r = rig();
    r.fake().skew = Some(m("0.01"));
    r.plan_day("500.00");
    r.s.command(CommandId::new("form.save_close"));
    assert_eq!(r.fake().posts, 0, "nothing posts on a mismatch");
    assert_eq!(r.fake().drafts_made, 1);
    assert_eq!(r.s.save.notice, Some("daily.layout_changed"));
    assert_eq!(r.s.state()["mode"], "draft");
    assert!(r.link.drain().is_empty(), "the window stays open");
    // Fixed elsewhere: the next save updates the same draft and posts it.
    r.fake().skew = None;
    r.s.command(CommandId::new("form.save_close"));
    assert_eq!(r.fake().posts, 1);
    assert_eq!(r.fake().drafts_made, 1, "the kept draft was posted");
    assert_eq!(r.fake().entries.len(), 1);
}

#[test]
fn a_failed_post_keeps_the_draft_and_it_shows_on_reopen() {
    let mut r = rig();
    r.fake().fail_post = Some(ApiError::Server {
        status: 500,
        code: None,
    });
    r.plan_day("500.00");
    r.s.command(CommandId::new("form.save_new"));
    assert_eq!(r.fake().posts, 0);
    assert_eq!(r.s.save.notice, Some("daily.not_posted"));
    r.s.poll();
    assert_eq!(r.s.state()["mode"], "draft");
    assert_eq!(r.s.state()["drafts"], 1);
    // A new window lists the unposted day and opens it.
    let books: SharedBooks = r.fake.clone();
    let mut again = DailySalesScreen::with_books(Arc::clone(&r.link), None, today(), Some(books));
    assert_eq!(again.state()["drafts"], 1);
    let id = again.panel.drafts[0].id;
    again.open_entry(id);
    again.poll();
    assert_eq!(again.state()["mode"], "draft");
    assert_eq!(again.state()["values"], 4);
    // Try again posts that same draft.
    again.command(CommandId::new("form.save_close"));
    assert_eq!(r.fake().posts, 1);
    assert_eq!(r.fake().drafts_made, 1);
}

#[test]
fn a_negative_or_unmapped_amount_is_refused_with_nothing_sent() {
    let mut r = rig();
    r.plan_day("500.00");
    r.type_amount("Cards", "-577.65");
    r.s.command(CommandId::new("form.save_new"));
    assert_eq!(r.s.state()["save"]["refused"], "negative_amount");
    assert_eq!(r.fake().drafts_made, 0);
    r.type_amount("Cards", "577.65");
    // The fixed amount takes the refusal away before the next save.
    r.s.recheck_problems();
    assert!(r.s.state()["save"]["refused"].is_null());
    r.type_amount("Dept 2", "10");
    assert_eq!(r.s.state()["unmapped"], 1);
    r.s.command(CommandId::new("form.save_new"));
    assert_eq!(r.s.state()["save"]["refused"], "unmapped_with_amount");
    assert_eq!(r.fake().drafts_made, 0);
    // A typo is held in the field, not taken.
    r.type_amount("Cash", "12..5");
    assert!(!r.s.inputs.amount_problems.is_empty());
}

#[test]
fn a_closed_period_day_asks_for_the_password_then_posts_and_voids() {
    let mut r = rig();
    let closing = today();
    r.fake().closing = Some(closing);
    r.s.closing = Some(closing);
    r.plan_day("500.00");
    r.s.command(CommandId::new("form.save_new"));
    assert_eq!(r.fake().drafts_made, 0, "asked before anything is sent");
    assert_eq!(r.s.state()["closing"], "asking");
    r.s.closing_prompt.password = SecretText::from(DEMO_CLOSING_PASSWORD);
    r.s.submit_closing();
    assert_eq!(r.fake().posts, 1);
    assert_eq!(r.s.state()["mode"], "new");
    // Void the posted day (Save & New moved on: open it again).
    let id = r.fake().entries[0].id;
    r.s.open_entry(id);
    r.s.poll();
    assert_eq!(r.s.state()["mode"], "posted");
    assert_eq!(r.s.state()["read_only"], true);
    r.s.command(CommandId::new("edit.void_transaction"));
    assert_eq!(r.s.state()["void_open"], true);
    r.s.send_void(None);
    assert_eq!(r.fake().voids, 0);
    assert_eq!(r.s.state()["closing"], "asking");
    r.s.closing_prompt.password = SecretText::from(DEMO_CLOSING_PASSWORD);
    r.s.submit_closing();
    assert_eq!(r.fake().voids, 1);
    assert_eq!(r.s.state()["mode"], "void");
    assert_eq!(r.s.save.last_action, Some("void"));
}

#[test]
fn without_the_lock_capability_a_closed_day_is_refused_plainly() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    // No books: the live path, which needs `daily-sales-lock-v1`.
    let mut s = DailySalesScreen::with_books(Arc::clone(&link), None, today(), None);
    assert!(!s.lock_supported());
    assert_eq!(s.state()["books"], false);
    let tpl = counter();
    s.templates = vec![tpl.clone()];
    s.header.load(DailySalesForm {
        template: Some(tpl.id),
        date: Some(today()),
        ..DailySalesForm::default()
    });
    s.apply_template(tpl.clone());
    s.closing = Some(today());
    let cash = tpl.lines[2].id;
    s.amount_typed(2, cash, Some(m("10")));
    s.start_save(SaveIntent::Save, None);
    assert_eq!(s.state()["closing"], "unavailable");
    assert_eq!(s.state()["saving"], false);
}

#[test]
fn the_empty_state_starts_from_a_preset_or_blank() {
    let mut r = rig_with(Fake::default());
    assert_eq!(r.s.state()["empty_state"], true);
    r.s.start_from(Some(Preset::Retail));
    assert_eq!(r.s.state()["layouts"], 1);
    assert_eq!(r.s.state()["layout_set"], true);
    assert_eq!(r.fake().templates[0].name, "Retail counter");

    let mut r = rig_with(Fake::default());
    r.s.start_from(None);
    let setup = r.s.state()["setup"].clone();
    assert_eq!(setup["saved_layout"], false);
    assert_eq!(
        setup["lines"], 4,
        "sales, the manual tax line (Q-260), cash, cards"
    );
    let tax =
        r.s.setup
            .as_ref()
            .and_then(|s| {
                s.layout
                    .lines
                    .iter()
                    .find(|l| l.section == Some(cl_model::forms::daily_sales::DailySection::Tax))
                    .cloned()
            })
            .expect("tax line");
    assert_eq!(tax.label, cl_model::forms::daily_sales::TAX_LINE_LABEL);
    assert_eq!(
        tax.account,
        system_account(&r.s.accounts, "sales_tax_payable")
    );
    r.s.save_setup(true);
    assert_eq!(r.s.state()["setup_open"], false, "{}", r.s.state());
    assert_eq!(r.fake().templates.len(), 1);
    assert_eq!(r.s.state()["layout_set"], true);
}

#[test]
fn set_up_lines_saves_a_new_layout_without_amounts_and_deletes_it() {
    let mut r = rig();
    r.type_amount("Cash", "5");
    r.s.open_setup();
    assert!(r.s.command(CommandId::new("form.insert_line")));
    {
        let s = r.s.setup.as_mut().expect("setup");
        let at = s.row.expect("row");
        s.layout.lines[at].label = "Gift cards".into();
        s.layout.lines[at].account = Some(acct(10150));
        s.layout.name = "Counter 2".into();
    }
    assert_eq!(r.s.state()["setup"]["dirty"], true);
    r.s.save_setup(true);
    assert_eq!(r.fake().templates.len(), 2);
    let new = r.fake().templates[1].clone();
    assert_eq!(new.name, "Counter 2");
    assert_eq!(new.lines.iter().filter(|l| l.active).count(), 7);
    assert!(new.lines.iter().any(|l| l.label == "Gift cards"));
    // The form moved to the new layout; amounts are never part of it.
    assert_eq!(r.s.header.current().template, Some(new.id));
    assert!(r.s.header.current().values.is_empty());
    // An empty label is refused.
    r.s.poll();
    r.s.open_setup();
    r.s.setup.as_mut().expect("setup").layout.lines[0]
        .label
        .clear();
    r.s.save_setup(false);
    assert!(r.s.setup.is_some());
    assert!(
        r.s.state()["setup"]["problems"]
            .to_string()
            .contains("empty_label")
    );
    // Delete "Counter 2".
    r.s.setup.as_mut().expect("setup").confirm_delete = true;
    r.s.delete_setup();
    assert!(!r.fake().templates[1].is_active);
    assert_eq!(r.s.state()["layouts"], 1);
}

#[test]
fn esc_and_ctrl_l_and_undo_follow_the_form_rules() {
    let mut r = rig();
    r.s.focused = Some(Field::Layout);
    assert!(r.s.command(CommandId::new("edit.open_list")));
    assert_eq!(r.s.state()["layout_list"], true);
    assert!(r.s.command(CommandId::new("form.revert")));
    assert_eq!(r.s.state()["layout_list"], false);
    r.type_amount("Cash", "5");
    assert!(r.s.is_dirty());
    assert!(r.s.command(CommandId::new("edit.undo")));
    assert!(!r.s.is_dirty());
    r.type_amount("Cash", "5");
    r.s.command(CommandId::new("form.clear"));
    assert!(!r.s.is_dirty());
    assert_eq!(r.s.save.last_action, Some("clear"));
}

/// Draws the screen a few frames and returns every accessible (role,
/// name), a text input's name being the label it is labelled by.
fn tree(s: &mut DailySalesScreen, width: f32) -> Vec<(eframe::egui::accesskit::Role, String)> {
    use eframe::egui::{self, Pos2, RawInput, Rect, vec2};
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Dark);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut all = std::collections::HashMap::new();
    for _ in 0..3 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 940.0))),
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
        if let Some(t) = out.platform_output.accesskit_update {
            all.extend(t.nodes);
        }
    }
    let nodes: Vec<_> = all.into_iter().collect();
    let labels: std::collections::HashMap<_, _> = nodes
        .iter()
        .filter_map(|(id, n)| n.label().or(n.value()).map(|l| (*id, l.to_string())))
        .collect();
    nodes
        .iter()
        .map(|(_, n)| {
            let own = n.label().or(if n.role() == egui::accesskit::Role::Label {
                n.value()
            } else {
                None
            });
            let name = own.map_or_else(
                || {
                    n.labelled_by()
                        .iter()
                        .filter_map(|id| labels.get(id).cloned())
                        .collect::<Vec<_>>()
                        .join(" ")
                },
                str::to_string,
            );
            (n.role(), name)
        })
        .collect()
}

#[test]
fn every_input_is_named_and_the_form_draws_at_both_sizes() {
    use eframe::egui::accesskit::Role;
    for width in [1440.0, 960.0] {
        let mut r = rig();
        r.plan_day("500.00");
        let nodes = tree(&mut r.s, width);
        let has = |role: Role, name: &str| nodes.iter().any(|(ro, n)| *ro == role && n == name);
        for name in ["Layout", "Date", "Notes", "Plant sales", "Cash", "Cards"] {
            assert!(has(Role::TextInput, name), "{name}: {nodes:#?}");
        }
        for name in [
            "Clear",
            "Save & New",
            "Save & Close",
            "Set up lines",
            "Make Deposit",
        ] {
            assert!(
                nodes
                    .iter()
                    .any(|(ro, n)| *ro == Role::Button && n.starts_with(name)),
                "{name}"
            );
        }
        assert!(
            nodes
                .iter()
                .any(|(_, n)| n == "Cash over/short: 2.35 short \u{2014} posts to Cash Over/Short"),
            "the over/short line"
        );
        assert!(nodes.iter().any(|(_, n)| n == "Sales tax collected"));
        assert!(
            nodes
                .iter()
                .any(|(_, n)| n == "Type the sales tax from your register report.")
        );
    }
    // The set-up editor and the empty state draw too.
    let mut r = rig();
    r.s.open_setup();
    let nodes = tree(&mut r.s, 1440.0);
    assert!(nodes.iter().any(|(_, n)| n == "Save as new layout"));
    let mut r = rig_with(Fake::default());
    let nodes = tree(&mut r.s, 960.0);
    for name in ["Retail", "Restaurant", "Blank"] {
        assert!(nodes.iter().any(|(_, n)| n == name), "{name}");
    }
}

/// Q-267: on wide windows the lines sit in the condensed table (each
/// amount box within 760 of its label, stripes alternating within a
/// section); under 760 the columns are the old ones.
#[test]
fn wide_windows_draw_the_lines_condensed_and_striped() {
    use crate::forms::condensed::{CondensedTable, MAX_WIDTH};
    for width in [1440.0, 1920.0] {
        let mut r = rig();
        r.plan_day("500.00");
        let _ = tree(&mut r.s, width);
        let c = &r.s.condensed;
        let lines =
            r.s.layout
                .lines
                .iter()
                .filter(|l| l.active && l.id.is_some())
                .count();
        assert_eq!(c.rows.len(), lines, "{width}");
        assert!((c.width - MAX_WIDTH).abs() < 0.5, "{width}: {}", c.width);
        for row in &c.rows {
            let gap = row.amount.left() - row.label.left();
            assert!(gap > 0.0 && gap <= MAX_WIDTH, "{width}: {gap}");
        }
        assert!(c.widest_gap() <= MAX_WIDTH);
        // A section with two or more lines has a striped second line, and
        // no two neighbours in a section are both striped.
        assert!(c.rows.iter().any(|x| x.striped), "{width}: {c:?}");
        assert!(
            c.rows.windows(2).all(|w| !(w[0].striped && w[1].striped)),
            "{width}"
        );
        let state = r.s.state_json();
        assert_eq!(state["condensed"]["rows"], lines);
    }
    // A narrow window: the form is under 760 wide and the columns are the
    // ones Daily Sales always drew.
    let mut r = rig();
    let _ = tree(&mut r.s, 700.0);
    let c = r.s.condensed.clone();
    assert!(c.width < MAX_WIDTH, "{}", c.width);
    let expect = CondensedTable::default().label_width(c.width);
    assert!(
        c.rows
            .iter()
            .all(|x| (x.label.width() - expect).abs() < 1.0),
        "{c:?}"
    );
}

/// `--demo`: the screen answers through the demo twin. It opens on the
/// sample's layout and posts a 2.35-short day with the over/short line on
/// Cash Over/Short (60900).
#[test]
fn the_demo_books_load_the_layout_and_post_a_short_day() {
    use crate::company::demo::daily_sales::{DayStatus, days};
    use crate::company::demo::register::DemoLedger;

    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = DailySalesScreen::new(Arc::clone(&link), None, today());
    let mut r = Rig {
        _dir: dir,
        link,
        fake: Arc::new(Mutex::new(Fake::default())),
        s,
    };
    let st = r.s.state();
    assert_eq!(st["books"], true);
    assert_eq!(st["loaded"], true);
    assert_eq!(st["layouts"], 1);
    assert_eq!(st["lines"], 5);
    assert_eq!(st["empty_state"], false);
    assert_eq!(r.s.inputs.layout, "Plant sale counter");

    r.plan_day("500.00");
    let tot = r.s.totals().expect("totals");
    assert_eq!(tot.over_short, OverShort::Short(m("2.35")));
    r.s.command(CommandId::new("form.save_new"));
    assert_eq!(r.s.save.last_action, Some("post"));
    assert_eq!(r.s.save.count, 1);

    let demo = r.link.demo_state().expect("demo");
    let posted = days(&demo, Some(DayStatus::Posted), Some(today()));
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].over_short, Some(m("-2.35")));
    let txn = posted[0].transaction.expect("txn");
    let t = demo
        .ledger
        .transactions()
        .into_iter()
        .find(|t| DemoLedger::id_of(t) == txn)
        .expect("posted");
    let os: Vec<_> = t.lines.iter().filter(|x| x.account == "60900").collect();
    assert_eq!(os.len(), 1);
    assert_eq!((os[0].debit, os[0].credit), (m("2.35"), Money::ZERO));
}

#[test]
fn prev_and_next_go_through_the_saved_days_with_the_standard_question() {
    use crate::windows::PromptAnswer;
    let mut r = rig();
    r.plan_day("500.00");
    r.s.command(CommandId::new("form.save_new"));
    r.plan_day("503.00");
    r.s.command(CommandId::new("form.save_new"));
    assert_eq!(r.fake().posts, 2);
    let day2 = today().add_days(1).expect("date");
    // Prev: the newest saved day (11/05), then 11/04, then the oldest note.
    r.s.command(CommandId::new("form.prev_record"));
    assert_eq!(r.s.header.current().date, Some(day2));
    assert_eq!(r.s.state()["mode"], "posted");
    r.s.command(CommandId::new("form.prev_record"));
    assert_eq!(r.s.header.current().date, Some(today()));
    r.s.command(CommandId::new("form.prev_record"));
    assert_eq!(r.s.save.notice, Some("daily.nav.oldest"));
    // Next: back to 11/05, then past the newest a new day.
    r.s.command(CommandId::new("form.next_record"));
    assert_eq!(r.s.header.current().date, Some(day2));
    r.s.command(CommandId::new("form.next_record"));
    assert_eq!(r.s.state()["mode"], "new");
    // Unsaved work: the question, not a silent move.
    r.type_amount("Plant sales", "10.00");
    assert!(r.s.is_dirty());
    r.s.command(CommandId::new("form.prev_record"));
    assert_eq!(r.s.state()["nav_prompt"], true);
    assert_eq!(r.s.state()["mode"], "new");
    // Esc is Keep editing.
    assert!(r.s.command(CommandId::new("form.revert")));
    assert!(r.s.nav_prompt.is_none() && r.s.is_dirty());
    // Discard: the typing goes, then Prev opens 11/05.
    r.s.command(CommandId::new("form.prev_record"));
    r.s.answer_nav(PromptAnswer::Discard);
    assert!(!r.s.is_dirty());
    assert_eq!(r.s.header.current().date, Some(day2));
    assert_eq!(r.fake().posts, 2, "nothing saved");
}

#[test]
fn a_list_change_heard_while_dirty_waits_until_the_form_is_clean() {
    let mut r = rig();
    let mut second = counter();
    second.name = "Farmers market".into();
    r.fake().templates.push(second);
    r.type_amount("Plant sales", "10.00");
    r.link.bus().publish(crate::data_bus::Change::Accounts);
    r.s.hear_changes();
    r.s.poll();
    assert_eq!(r.s.templates.len(), 1, "dirty: the layouts wait");
    assert!(r.s.lists_stale);
    r.s.command(CommandId::new("form.clear"));
    r.s.hear_changes();
    r.s.poll();
    assert_eq!(r.s.templates.len(), 2);
    assert!(!r.s.lists_stale);
}

#[test]
fn a_kept_draft_dated_in_the_closed_period_does_not_ask_for_the_password() {
    let mut r = rig();
    // A mismatch keeps a draft dated today.
    r.fake().skew = Some(m("0.01"));
    r.plan_day("500.00");
    r.s.start_save(SaveIntent::Save, None);
    assert_eq!(r.s.state()["mode"], "draft");
    // The books close through today; the owner moves the day after it.
    r.s.closing = Some(today());
    r.fake().closing = Some(today());
    r.fake().skew = None;
    let tomorrow = today().add_days(1).expect("date");
    r.s.header.edit_step(|h| h.date = Some(tomorrow));
    r.s.sync_inputs();
    r.s.start_save(SaveIntent::Save, None);
    assert!(
        !r.s.closing_prompt.dialog_open(),
        "a draft posted nothing: its old date doesn't ask"
    );
    assert_eq!(r.fake().posts, 1);
}

/// Q-264: Enter in Date posts the day (Save & New) by default and moves to
/// the next field when "Enter moves between fields" is on.
#[test]
fn header_enter_saves_or_moves_by_the_preference() {
    use crate::forms::enter::testing::{context, enter, frame, settle};
    use cl_model::forms::EnterMode;
    for mode in [EnterMode::SaveAndNew, EnterMode::MoveBetweenFields] {
        let mut r = rig();
        r.plan_day("500.00");
        let ctx = context();
        r.s.focus = Some(Field::Date);
        settle(&ctx, mode, &mut |ui| r.s.ui(ui));
        assert_eq!(r.s.focused, Some(Field::Date), "{mode:?}");
        frame(&ctx, mode, enter(false), &mut |ui| r.s.ui(ui));
        settle(&ctx, mode, &mut |ui| r.s.ui(ui));
        match mode {
            EnterMode::SaveAndNew => assert_eq!(r.fake().posts, 1, "Enter = Save & New"),
            EnterMode::MoveBetweenFields => {
                assert_eq!(r.fake().posts, 0, "Enter never saves here");
                assert!(
                    r.s.focused.is_some_and(|f| f != Field::Date),
                    "the next field: {:?}",
                    r.s.focused
                );
            }
        }
    }
}
