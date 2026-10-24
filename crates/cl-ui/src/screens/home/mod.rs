//! Home (P16, `nav.home`, Alt+H): the four task lanes, the account
//! balances and Needs attention (`docs/design/mockups/Home.html`, D10).
//!
//! - **Lanes** come from `cl_model::home::LANES`; every action is a
//!   registry command, so its label, shortcut chip and "arrives later"
//!   come from the command table, and a click opens its form or the
//!   standard placeholder window (`lanes.rs`).
//! - **Account balances** are the engine's own figures, never summed or
//!   worked out here (Non-negotiables 6 and 7): the cash position of
//!   `dashboard-exact-v1` when the engine has it, else each account's
//!   stored balance. The bank-file column is a dash until U-77
//!   (`balances.rs`).
//! - **Needs attention** shows counts from engine queries (bank items,
//!   reconciliations, bills, payroll imports; overdue invoices with U-76),
//!   a dash for what arrives later (to-dos U-37, reorder alerts U-78, the
//!   health check U-12) and the app's own backup reminder (Q-217)
//!   (`attention.rs`).
//! - **Income and expenses** draws six months of the engine's Profit &
//!   Loss, each month drilling into the report (`charts.rs`, D1).
//!
//! Every read goes through the request queue and is polled each frame; the
//! egui thread never waits. Home reads on open, when another window saves
//! something (the data bus, Q-262), and on Refresh: there is no timer, so
//! nothing runs in the background.

mod attention;
mod balances;
pub(crate) mod charts;
pub(crate) mod checklist;
mod lanes;
mod support;

use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, BillQuery, Capabilities, ContactFilter, ContactType,
    DashboardSummary, PageRequest, PayrollStatus, RECONCILE_ACCOUNTS_MAX, ReconcileState,
    RequestGroup,
};
use cl_core::ids::{AccountId, TenantId};
use cl_core::{CalendarDate, Timestamp};
use cl_model::CommandId;
use cl_model::home::LANES;
use cl_model::register::AccountClass;
use eframe::egui::{self, Align, Id, Layout, RichText, Ui, Vec2};
use serde_json::{Value, json};

use crate::company::demo::home as demo;
use crate::company::{CompanyEvent, CompanyLink};
use crate::data_bus::Subscription;
use crate::i18n::t;
use crate::layout::{Body, FixedFrame, FrameOpts, FrameReport};
use crate::screens::templates::engine::{Job, finished, run};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The window kind.
pub const KIND: &str = "nav.home";

/// Space between lanes and cards (the mockup's 14 px).
const GAP: f32 = 14.0;

/// Room kept free at the right of the body (the borders and the scroll
/// bar's edge).
const EDGE: f32 = 6.0;

/// Home for `kind`, if it is Home.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(HomeScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// One read: its last answer, its last failure, and the call on its way.
pub(crate) struct Slot<T> {
    /// The last answer (cleared by a failure).
    pub value: Option<T>,
    /// The last failure.
    pub error: Option<ApiError>,
    job: Option<Job<T>>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Slot {
            value: None,
            error: None,
            job: None,
        }
    }
}

impl<T> Slot<T> {
    /// A slot holding `value` (tests).
    #[cfg(test)]
    pub(crate) fn ready(value: T) -> Slot<T> {
        Slot {
            value: Some(value),
            error: None,
            job: None,
        }
    }

    /// A slot that failed with `error` (tests).
    #[cfg(test)]
    pub(crate) fn failed(error: ApiError) -> Slot<T> {
        Slot {
            value: None,
            error: Some(error),
            job: None,
        }
    }

    fn start(&mut self, job: Job<T>) {
        self.job = Some(job);
    }

    /// Takes a finished answer; true when one arrived this call.
    fn poll(&mut self) -> bool {
        let Some(r) = finished(&mut self.job) else {
            return false;
        };
        match r {
            Ok(v) => {
                self.value = Some(v);
                self.error = None;
            }
            Err(e) => {
                self.value = None;
                self.error = Some(e);
            }
        }
        true
    }

    /// A call is on its way.
    pub(crate) fn loading(&self) -> bool {
        self.job.is_some()
    }
}

/// The counts Home reads on every engine (integers, never amounts). Each
/// fails on its own, so one bad query shows one dash.
pub(crate) struct Counts {
    /// Bank-feed items waiting (`pendingFeedCount`).
    pub feed: Result<u64, ApiError>,
    /// Open bills overdue or due within the company's window (Q-169).
    pub bills: Result<u64, ApiError>,
    /// Payroll imports not posted.
    pub payroll: Result<u64, ApiError>,
    /// Active clients (the rail).
    pub clients: Result<u64, ApiError>,
    /// Active vendors (the rail).
    pub vendors: Result<u64, ApiError>,
}

/// What Home has read.
#[derive(Default)]
pub(crate) struct HomeData {
    /// The chart: names, numbers, kinds and stored balances.
    pub accounts: Slot<Vec<Account>>,
    /// `dashboard-exact-v1` panels (only read when the engine has it).
    pub summary: Slot<DashboardSummary>,
    /// Counts from list queries.
    pub counts: Slot<Counts>,
    /// Reconciliations of the bank and card accounts.
    pub reconcile: Slot<ReconcileState>,
    /// The closing date (the Set Closing Date chip).
    pub closing: Slot<Option<CalendarDate>>,
    /// Six months of Profit & Loss (the chart).
    pub chart: Slot<Vec<charts::MonthPl>>,
    /// What the setup checklist reads from the engine.
    pub checklist: Slot<checklist::Facts>,
}

impl HomeData {
    /// The chart account with `id`, if it was read.
    pub(crate) fn account(&self, id: AccountId) -> Option<&Account> {
        self.accounts.value.as_ref()?.iter().find(|a| a.id == id)
    }

    fn loading(&self) -> bool {
        self.accounts.loading()
            || self.summary.loading()
            || self.counts.loading()
            || self.reconcile.loading()
            || self.closing.loading()
            || self.chart.loading()
            || self.checklist.loading()
    }
}

/// The active bank and card accounts of the chart, banks first, each kind
/// in account-number order (the balances' rows; the accounts whose
/// reconciliations Home reads).
#[must_use]
pub(crate) fn money_accounts(accounts: &[Account]) -> Vec<&Account> {
    let mut out: Vec<(&Account, AccountClass)> = accounts
        .iter()
        .filter(|a| a.is_active)
        .filter_map(|a| {
            crate::screens::register::class_of(a)
                .filter(|c| c.is_money_account())
                .map(|c| (a, c))
        })
        .collect();
    out.sort_by_key(|(a, c)| {
        let number = a.fields.account_number.clone().unwrap_or_default();
        (
            u8::from(*c != AccountClass::Bank),
            number.parse::<u64>().unwrap_or(u64::MAX),
            number,
            a.fields.name.clone(),
        )
    });
    out.into_iter().map(|(a, _)| a).collect()
}

/// The company-local facts Home reads from this computer's records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Local {
    /// Bills overdue or due within this many days (Q-169).
    pub bills_due_days: u16,
    /// The last backup this app made of the company.
    pub last_backup: Option<Timestamp>,
    /// Now.
    pub now: Timestamp,
    /// The clock times are shown in (UTC in the demo, so screenshots stay
    /// the same on every machine).
    pub zone: Zone,
    /// Standard months or a retail calendar (kept on this computer, U-25).
    pub calendar: cl_settings::FiscalCalendar,
    /// The owner hid the setup checklist (kept with the company's prefs).
    pub checklist_dismissed: bool,
}

/// The clock Home shows a time in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Zone {
    /// This computer's time zone, with the offset of each instant (a
    /// backup made before a daylight-saving change shows its own offset).
    Local,
    /// A fixed offset (the demo: UTC).
    Fixed(chrono::FixedOffset),
}

impl Zone {
    /// `at` on this clock.
    #[must_use]
    pub(crate) fn at(self, at: Timestamp) -> chrono::DateTime<chrono::FixedOffset> {
        match self {
            Zone::Local => at.as_utc().with_timezone(&chrono::Local).fixed_offset(),
            Zone::Fixed(o) => at.as_utc().with_timezone(&o),
        }
    }
}

/// The Home window.
pub struct HomeScreen {
    link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    sub: Subscription,
    /// The engine reports dashboard money exactly (U-76; simulated in the
    /// demo).
    pub(crate) exact: bool,
    pub(crate) local: Local,
    pub(crate) data: HomeData,
    /// The command a lane action or a Needs attention link opened last.
    pub(crate) last_opened: Option<CommandId>,
    /// The month the chart drilled into last (Profit & Loss takes the range
    /// when it arrives in P28).
    pub(crate) chart_drill: Option<charts::Month>,
    /// The owner opened or closed the full setup checklist (none: it
    /// opens by itself while fewer than half the steps are done).
    pub(crate) checklist_open: Option<bool>,
    /// Lanes per row last frame.
    lane_columns: usize,
    /// The fixed bars' sizes last frame (Q-265).
    layout: FrameReport,
}

impl HomeScreen {
    /// Home over the open company; starts reading at once.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> HomeScreen {
        let group = link.group().and_then(Result::ok);
        let sub = link.bus().subscribe();
        let exact = link.is_demo() || link.capabilities().has(Capabilities::DASHBOARD_EXACT);
        let mut s = HomeScreen {
            local: read_local(&link, tenant.as_ref()),
            link,
            tenant,
            group,
            today,
            sub,
            exact,
            data: HomeData::default(),
            last_opened: None,
            chart_drill: None,
            checklist_open: None,
            lane_columns: 4,
            layout: FrameReport::default(),
        };
        s.reload();
        s
    }

    /// Reads everything again (open, Refresh, a change elsewhere).
    pub(crate) fn reload(&mut self) {
        self.local = read_local(&self.link, self.tenant.as_ref());
        let (today, days) = (self.today, self.local.bills_due_days);
        let group = self.group.as_ref();
        self.data.accounts.start(run(
            &self.link,
            group,
            None,
            |s| Ok(demo::accounts(s)),
            |s| async move { s.accounts().list_all(&AccountFilter::default()).await },
        ));
        if self.exact {
            // The engine takes a window of 1 to 90 days; a longer one shows
            // a dash for the bills figure (its count still reads below).
            let window = days.clamp(1, cl_api::DUE_WITHIN_DAYS_MAX);
            self.data.summary.start(run(
                &self.link,
                group,
                None,
                move |s| Ok(demo::summary(s, today, days)),
                move |s| async move { s.dashboard().summary(today, window).await },
            ));
        }
        self.data.counts.start(run(
            &self.link,
            group,
            None,
            move |s| {
                let through = today.add_days(i64::from(days)).unwrap_or(today);
                let (clients, vendors) = demo::contact_counts(s);
                Ok(Counts {
                    feed: Ok(demo::feed_waiting()),
                    bills: Ok(demo::bills_due(through).0),
                    payroll: Ok(demo::PAYROLL_UNPOSTED),
                    clients: Ok(clients),
                    vendors: Ok(vendors),
                })
            },
            move |s| async move {
                let home = s.home();
                let bills = match BillQuery::due_within(today, days) {
                    Ok(q) => home.bills_count(&q).await,
                    Err(e) => Err(e),
                };
                Ok(Counts {
                    feed: home.feed_pending_count(today).await,
                    bills,
                    payroll: home.payroll_sessions_count(&PayrollStatus::UNPOSTED).await,
                    clients: contact_count(&s, ContactType::Customer).await,
                    vendors: contact_count(&s, ContactType::Vendor).await,
                })
            },
        ));
        self.data.closing.start(run(
            &self.link,
            group,
            None,
            |s| Ok(demo::closing_date(s)),
            |s| async move { Ok(s.company_settings().await?.lock_date) },
        ));
        let calendar = self.local.calendar;
        self.data.chart.start(run(
            &self.link,
            group,
            None,
            move |_| Ok(demo::profit_loss_months(today, calendar)),
            move |s| async move { charts::read(&s, today, calendar).await },
        ));
        self.data.checklist.start(run(
            &self.link,
            group,
            None,
            |_| Ok(demo::checklist_facts()),
            |s| async move { checklist::read(&s).await },
        ));
        self.poll();
    }

    /// Takes the shell's date when it published one; true when it moved.
    pub(crate) fn follow_shell_date(&mut self, ctx: &egui::Context) -> bool {
        match crate::grid_env::published(ctx) {
            Some(f) if f.today != self.today => {
                self.today = f.today;
                true
            }
            _ => false,
        }
    }

    /// Reconciliations of the bank and card accounts, once the chart is in.
    fn read_reconciliations(&mut self) {
        let Some(accounts) = self.data.accounts.value.as_ref() else {
            return;
        };
        // Every bank and card account, read in batches of
        // `RECONCILE_ACCOUNTS_MAX` (none is left out).
        let ids: Vec<AccountId> = money_accounts(accounts).into_iter().map(|a| a.id).collect();
        let demo_ids = ids.clone();
        self.data.reconcile.start(run(
            &self.link,
            self.group.as_ref(),
            None,
            move |_| Ok(demo::reconcile(&demo_ids)),
            move |s| async move {
                let mut state = ReconcileState::default();
                for batch in ids.chunks(RECONCILE_ACCOUNTS_MAX) {
                    state.merge(s.home().reconcile_state(batch).await?);
                }
                Ok(state)
            },
        ));
    }

    /// Takes the answers that arrived.
    pub(crate) fn poll(&mut self) {
        if self.data.accounts.poll() {
            self.read_reconciliations();
        }
        self.data.summary.poll();
        if self.data.counts.poll()
            && let Some(c) = &self.data.counts.value
        {
            let n = |r: &Result<u64, ApiError>| {
                r.as_ref()
                    .ok()
                    .map(|n| u32::try_from(*n).unwrap_or(u32::MAX))
            };
            self.link
                .push(CompanyEvent::RailCounts(crate::shell::RailCounts {
                    clients: n(&c.clients),
                    vendors: n(&c.vendors),
                    bank_feed: n(&c.feed),
                }));
        }
        self.data.reconcile.poll();
        self.data.closing.poll();
        self.data.chart.poll();
        self.data.checklist.poll();
    }

    /// Opens the Profit & Loss of a chart month (a placeholder until P28,
    /// which takes the range; STATE tracks it).
    pub(crate) fn drill(&mut self, month: charts::Month) {
        self.chart_drill = Some(month);
        self.open("reports.profit_loss");
    }

    /// Opens a command's window (its screen, or the placeholder that says
    /// it arrives later).
    pub(crate) fn open(&mut self, id: &str) {
        let Some(cmd) = cl_model::commands::find(id) else {
            return;
        };
        self.last_opened = Some(cmd.id);
        self.link.push(CompanyEvent::OpenWindow(cmd.id));
    }

    /// Whether a backup is due (Q-217: none in the last 7 days).
    pub(crate) fn backup_due(&self) -> bool {
        cl_model::home::backup_due(
            self.local.last_backup,
            self.local.now,
            cl_model::home::BACKUP_DAYS_DEFAULT,
        )
    }

    /// The Needs attention items, in display order.
    pub(crate) fn attention(&self) -> Vec<cl_model::home::AttentionItem> {
        attention::items(
            &self.data,
            self.local.bills_due_days,
            self.exact,
            self.backup_due(),
        )
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let list = self.checklist_shown();
        let mut frame = FixedFrame::begin(ui, KIND, FrameOpts::default());
        let (refresh, mut todo) = frame.top(|ui| self.top(ui, &tokens, list.as_ref()));
        frame.body(Body::Scroll, |ui| {
            ui.add_space(4.0);
            if let Some(c) = list.as_ref().filter(|_| self.checklist_expanded()) {
                let mut picked = None;
                card(ui, &tokens, 0.0, |ui| {
                    picked = checklist::card(ui, &tokens, c);
                });
                todo = todo.or(picked);
                ui.add_space(GAP + 6.0);
            }
            self.lane_columns = lanes::show(ui, self, &tokens);
            ui.add_space(GAP + 6.0);
            self.cards(ui, &tokens);
            // The Ko-fi support button, at the bottom right (D10, Q-278).
            ui.add_space(GAP);
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                ui.add_space(EDGE);
                support::show(ui, &tokens);
            });
            ui.add_space(12.0);
        });
        self.layout = frame.end();
        if let Some(a) = todo {
            self.checklist_action(a);
        }
        if refresh {
            self.reload();
        }
    }

    /// The title row: Refresh, and the setup checklist line when it shows.
    /// Returns whether Refresh was chosen and what the checklist asked.
    fn top(
        &self,
        ui: &mut Ui,
        tokens: &Tokens,
        list: Option<&cl_model::home::Checklist>,
    ) -> (bool, Option<checklist::Action>) {
        let mut refresh = false;
        let mut todo = None;
        // The window title, 22/600 (DESIGN.md typography: the Heading
        // style is IBM Plex Sans SemiBold 22).
        let title_font = egui::TextStyle::Heading.resolve(ui.style());
        let title = RichText::new(t("cmd.nav.home"))
            .font(title_font.clone())
            .color(tokens.text_primary);
        let subtitle = RichText::new(t("home.subtitle")).color(tokens.text_secondary);
        let measure = |s: String, font: egui::FontId| {
            ui.painter()
                .layout_no_wrap(s, font, tokens.text_primary)
                .size()
                .x
        };
        let body = egui::TextStyle::Body.resolve(ui.style());
        let button_w =
            measure(t("home.refresh"), body.clone()) + 2.0 * ui.spacing().button_padding.x;
        let title_w = measure(t("cmd.nav.home"), title_font);
        let subtitle_w = measure(t("home.subtitle"), body);
        let line_w = list.map_or(0.0, |c| {
            measure(checklist::line_text(c), egui::FontId::proportional(12.0)) + 16.0
        });
        let room = ui.available_width();
        // The checklist line sits in the title row when it fits there, else
        // on its own line under it; the subtitle likewise.
        let line_beside = title_w + line_w + button_w + 3.0 * 16.0 <= room;
        let beside_w = if line_beside { line_w } else { 0.0 };
        let beside = title_w + subtitle_w + beside_w + button_w + 4.0 * 16.0 <= room;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            ui.add(egui::Label::new(title).selectable(false));
            if beside {
                ui.add(egui::Label::new(subtitle.clone()).selectable(false));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let busy = self.data.loading();
                if ui
                    .add_enabled(!busy, egui::Button::new(t("home.refresh")))
                    .on_hover_text(t("home.refresh.tip"))
                    .clicked()
                {
                    refresh = true;
                }
                if let Some(c) = list.filter(|_| line_beside) {
                    ui.add_space(4.0);
                    todo = checklist::top_line(ui, tokens, c);
                }
            });
        });
        if let Some(c) = list.filter(|_| !line_beside) {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                todo = checklist::top_line(ui, tokens, c);
            });
        }
        if !beside {
            ui.add(egui::Label::new(subtitle).wrap());
        }
        (refresh, todo)
    }

    /// The setup checklist when Home shows it: read, not hidden, not done.
    pub(crate) fn checklist_shown(&self) -> Option<cl_model::home::Checklist> {
        checklist::checklist(self).filter(|c| c.visible(self.local.checklist_dismissed))
    }

    /// Whether the full list is open (by itself while fewer than half the
    /// steps are done, else when the owner opened it).
    pub(crate) fn checklist_expanded(&self) -> bool {
        self.checklist_shown().is_some_and(|c| {
            self.checklist_open
                .unwrap_or_else(|| checklist::opens_by_itself(&c))
        })
    }

    /// Does what the checklist asked.
    pub(crate) fn checklist_action(&mut self, a: checklist::Action) {
        match a {
            checklist::Action::Open(id) => self.open(id),
            checklist::Action::Toggle => self.checklist_open = Some(!self.checklist_expanded()),
            checklist::Action::Dismiss => self.dismiss_checklist(),
        }
    }

    /// Hides the checklist for this company (`setup_checklist_dismissed`,
    /// kept with the company's preferences).
    pub(crate) fn dismiss_checklist(&mut self) {
        self.local.checklist_dismissed = true;
        if let Some((id, name)) = &self.tenant
            && let Err(e) = self.link.store().update(|l| {
                l.upsert(*id, name).prefs.setup_checklist_dismissed = true;
            })
        {
            tracing::warn!("could not save the hidden setup checklist: {e}");
        }
    }
    /// The cards under the lanes: Account balances, Income and expenses and
    /// Needs attention, side by side when there is room (the mockup's
    /// wrapping row).
    fn cards(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mins = [340.0_f32, 300.0, 340.0];
        let width = ui.available_width();
        let (mut cols, mut need) = (1, mins[0]);
        while cols < mins.len() {
            need += GAP + mins[cols];
            if need > width {
                break;
            }
            cols += 1;
        }
        let id = ui.scope_id().with("home-cards");
        let mut clicked: Option<&'static str> = None;
        let mut retry = false;
        let mut drill = None;
        tiles(ui, id, mins.len(), cols, |ui, i, min_h| {
            card(ui, tokens, min_h, |ui| match i {
                0 => {
                    if balances::show(ui, self, tokens) {
                        retry = true;
                    }
                }
                1 => match charts::show(ui, self, tokens) {
                    Some(charts::Action::Retry) => retry = true,
                    Some(charts::Action::Drill(m)) => drill = Some(m),
                    None => {}
                },
                _ => {
                    if let Some(c) = attention::show(ui, self, tokens) {
                        if c == attention::RETRY {
                            retry = true;
                        } else {
                            clicked = Some(c);
                        }
                    }
                }
            })
        });
        if let Some(id) = clicked {
            self.open(id);
        }
        if let Some(m) = drill {
            self.drill(m);
        }
        if retry {
            self.reload();
        }
    }

    fn state_json(&self) -> Value {
        let items = self.attention();
        let attention: serde_json::Map<String, Value> = items
            .iter()
            .map(|i| (i.kind.key().to_string(), json!(i.count)))
            .collect();
        let available: Vec<&str> = items
            .iter()
            .filter(|i| i.available)
            .map(|i| i.kind.key())
            .collect();
        let rows = balances::rows(&self.data, self.exact);
        json!({
            "screen": "home",
            "layout": self.layout.json(),
            "company_open": self.tenant.is_some(),
            "dashboard_exact": self.exact,
            "lanes": LANES.iter().map(|l| l.id.key()).collect::<Vec<_>>(),
            "lane_columns": self.lane_columns,
            "actions": LANES
                .iter()
                .map(|l| (l.id.key().to_string(), json!(l.actions)))
                .collect::<serde_json::Map<String, Value>>(),
            "footers": lanes::footers_shown(self),
            "attention": attention,
            "attention_available": available,
            "needs_attention": items.iter().filter(|i| i.needs_attention()).count(),
            "bills_due_days": self.local.bills_due_days,
            "backup_due": self.backup_due(),
            "balances_rows": rows.as_ref().map(Vec::len),
            "balances_source": balances::source(&self.data, self.exact),
            "loading": self.data.loading(),
            "chart": charts::state(self),
            "checklist": checklist::state(self),
            "errors": {
                "balances": rows.is_none() && self.data.accounts.error.is_some(),
                "attention": self.data.counts.error.is_some(),
                "chart": self.data.chart.error.is_some(),
            },
            "last_opened": self.last_opened.map(CommandId::as_str),
        })
    }
}

/// Active contacts of `kind` (the list's `total`; one row read).
async fn contact_count(s: &cl_api::Session, kind: ContactType) -> Result<u64, ApiError> {
    let filter = ContactFilter {
        contact_type: Some(kind),
        is_active: Some(true),
        ..ContactFilter::default()
    };
    Ok(s.contacts()
        .list(&filter, PageRequest::new(1, 0)?)
        .await?
        .total)
}

/// The window, the last backup and the clock from this computer's record
/// of the company (the demo's own figures when it has none).
fn read_local(link: &CompanyLink, tenant: Option<&(TenantId, String)>) -> Local {
    let record = tenant.and_then(|(id, _)| {
        link.store()
            .load()
            .ok()
            .and_then(|list| list.get(*id).cloned())
    });
    let bills_due_days = record
        .as_ref()
        .map_or(cl_settings::CompanyPrefs::default().bills_due_days, |r| {
            r.prefs.bills_due_days
        });
    let calendar = record
        .as_ref()
        .map_or_else(cl_settings::FiscalCalendar::default, |r| {
            r.flags.fiscal_calendar
        });
    let checklist_dismissed = record
        .as_ref()
        .is_some_and(|r| r.prefs.setup_checklist_dismissed);
    let last = record.and_then(|r| r.last_backup);
    if link.is_demo() {
        return Local {
            bills_due_days,
            last_backup: last.or_else(|| Some(demo::last_backup())),
            now: demo::now(),
            zone: Zone::Fixed(utc()),
            calendar,
            checklist_dismissed,
        };
    }
    let now = chrono::Utc::now();
    Local {
        bills_due_days,
        last_backup: last,
        now: Timestamp::from_unix_millis(now.timestamp_millis())
            .unwrap_or_else(|_| crate::company::demo::DemoState::now()),
        zone: Zone::Local,
        calendar,
        checklist_dismissed,
    }
}

fn utc() -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(0).unwrap_or_else(|| unreachable!("zero is a valid offset"))
}

/// A bordered white card, at least `min_h` tall inside. Returns the height
/// its content took (before the padding).
fn card(ui: &mut Ui, tokens: &Tokens, min_h: f32, add: impl FnOnce(&mut Ui)) -> f32 {
    egui::Frame::new()
        .fill(tokens.surface_window)
        .stroke(egui::Stroke::new(1.0, tokens.border_default))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let top = ui.cursor().top();
            add(ui);
            let natural = ui.cursor().top() - top;
            if min_h > natural {
                ui.add_space(min_h - natural);
            }
            natural
        })
        .inner
}

/// Draws `n` boxes in rows of `cols`, each the same width, and every box
/// in a row as tall as the row's tallest box was last frame (`draw` gets
/// that height and returns its own natural height).
pub(crate) fn tiles(
    ui: &mut Ui,
    id: Id,
    n: usize,
    cols: usize,
    mut draw: impl FnMut(&mut Ui, usize, f32) -> f32,
) {
    let cols = cols.clamp(1, n.max(1));
    // Geometry only (a count of boxes), never money.
    #[allow(clippy::cast_precision_loss)]
    let gaps = GAP * (cols - 1) as f32;
    // A few points short of the edge, so the last box's border is never
    // cut by the body's clip.
    let room = ui.available_width() - EDGE;
    #[allow(clippy::cast_precision_loss)]
    let w = ((room - gaps) / cols as f32).floor().max(120.0);
    let all: Vec<usize> = (0..n).collect();
    for (r, row) in all.chunks(cols).enumerate() {
        let row_id = id.with(("row", r, cols));
        let last: f32 = ui.data(|d| d.get_temp(row_id)).unwrap_or(0.0);
        let mut tallest = 0.0_f32;
        if r > 0 {
            ui.add_space(GAP);
        }
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for &i in row {
                ui.allocate_ui_with_layout(Vec2::new(w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(w);
                    tallest = tallest.max(draw(ui, i, last));
                });
            }
        });
        if (tallest - last).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(row_id, tallest));
            ui.ctx().request_discard("home tiles measured");
        }
    }
}

impl Screen for HomeScreen {
    fn title(&self) -> String {
        t("cmd.nav.home")
    }

    fn ui(&mut self, ui: &mut Ui) {
        let changed = !self.link.bus().take(&mut self.sub).is_empty();
        // The shell's date (it moves on at midnight): a new day reads
        // everything again for that day.
        let day_moved = self.follow_shell_date(ui.ctx());
        if changed || day_moved {
            self.reload();
        }
        self.poll();
        self.draw(ui);
        if self.data.loading() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        // Alt+H while Home is in front.
        id.as_str() == KIND
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
