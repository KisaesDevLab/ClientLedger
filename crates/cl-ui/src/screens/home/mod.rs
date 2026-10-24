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
//!
//! Every read goes through the request queue and is polled each frame; the
//! egui thread never waits. Home reads on open, when another window saves
//! something (the data bus, Q-262), and on Refresh: there is no timer, so
//! nothing runs in the background. The setup checklist link, the chart and
//! the Ko-fi button arrive with the next work package (D10); the layout
//! leaves their places marked below.

mod balances;
mod lanes;

use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, Capabilities, ContactFilter, ContactType, DashboardSummary,
    PageRequest, RECONCILE_ACCOUNTS_MAX, ReconcileState, RequestGroup,
};
use cl_core::CalendarDate;
use cl_core::ids::{AccountId, TenantId};
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
                let (clients, vendors) = demo::contact_counts(s);
                Ok(Counts {
                    feed: Ok(demo::feed_waiting()),
                    clients: Ok(clients),
                    vendors: Ok(vendors),
                })
            },
            move |s| async move {
                let home = s.home();
                Ok(Counts {
                    feed: home.feed_pending_count(today).await,
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
        self.poll();
    }

    /// Reconciliations of the bank and card accounts, once the chart is in.
    fn read_reconciliations(&mut self) {
        let Some(accounts) = self.data.accounts.value.as_ref() else {
            return;
        };
        let ids: Vec<AccountId> = money_accounts(accounts)
            .into_iter()
            .map(|a| a.id)
            .take(RECONCILE_ACCOUNTS_MAX)
            .collect();
        let demo_ids = ids.clone();
        self.data.reconcile.start(run(
            &self.link,
            self.group.as_ref(),
            None,
            move |_| Ok(demo::reconcile(&demo_ids)),
            move |s| async move { s.home().reconcile_state(&ids).await },
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

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let mut frame = FixedFrame::begin(ui, KIND, FrameOpts::default());
        let refresh = frame.top(|ui| self.top(ui, &tokens));
        frame.body(Body::Scroll, |ui| {
            ui.add_space(4.0);
            self.lane_columns = lanes::show(ui, self, &tokens);
            ui.add_space(GAP + 6.0);
            self.cards(ui, &tokens);
            // WP8 (D10): the Ko-fi button sits here, at the bottom right
            // of the body.
            ui.add_space(12.0);
        });
        self.layout = frame.end();
        if refresh {
            self.reload();
        }
    }

    /// The title row. Returns true when Refresh was chosen.
    fn top(&self, ui: &mut Ui, tokens: &Tokens) -> bool {
        let mut refresh = false;
        let title = RichText::new(t("cmd.nav.home"))
            .size(22.0)
            .strong()
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
        let title_w = measure(t("cmd.nav.home"), egui::FontId::proportional(22.0));
        let subtitle_w = measure(t("home.subtitle"), body);
        // The subtitle sits beside the title when it fits there, else on
        // its own line under it (narrow windows, 200% zoom).
        let beside = title_w + subtitle_w + button_w + 3.0 * 16.0 <= ui.available_width();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            ui.add(egui::Label::new(title).selectable(false));
            if beside {
                ui.add(egui::Label::new(subtitle.clone()).selectable(false));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // WP8 (D10): the setup checklist link ("Setup checklist 6 of
                // 7 done · …") goes at the right end of this row.
                let busy = self.data.loading();
                if ui
                    .add_enabled(!busy, egui::Button::new(t("home.refresh")))
                    .on_hover_text(t("home.refresh.tip"))
                    .clicked()
                {
                    refresh = true;
                }
            });
        });
        if !beside {
            ui.add(egui::Label::new(subtitle).wrap());
        }
        refresh
    }

    /// The cards under the lanes, side by side when there is room (the
    /// mockup's wrapping row).
    fn cards(&mut self, ui: &mut Ui, tokens: &Tokens) {
        // WP8 (D10): the "Income and expenses" chart card (min width 300)
        // joins this row.
        let mins = [340.0_f32];
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
        let mut retry = false;
        tiles(ui, id, mins.len(), cols, |ui, _i, min_h| {
            card(ui, tokens, min_h, |ui| {
                if balances::show(ui, self, tokens) {
                    retry = true;
                }
            })
        });
        if retry {
            self.reload();
        }
    }

    fn state_json(&self) -> Value {
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
            "bills_due_days": self.local.bills_due_days,
            "balances_rows": rows.as_ref().map(Vec::len),
            "balances_source": balances::source(&self.data, self.exact),
            "loading": self.data.loading(),
            "errors": {
                "balances": rows.is_none() && self.data.accounts.error.is_some(),
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

/// The bills window from this computer's record of the company.
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
    Local { bills_due_days }
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
        if !self.link.bus().take(&mut self.sub).is_empty() {
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
