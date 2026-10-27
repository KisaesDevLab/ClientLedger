//! Needs attention (P16 step 3): counts, never amounts, each from an
//! engine query (`cl_api::HomeQueries`, the dashboard with U-76) or the
//! app's own backup record (Q-217). In-app only (Q-71).
//!
//! - Bank items waiting, unfinished reconciliations, bills overdue or due
//!   within the company's window (Q-169) and unposted payroll imports read
//!   on every engine. A line shows when its count is above zero.
//! - Invoices past due need `dashboard-exact-v1` (U-76, Q-P16g): a dash
//!   until then.
//! - To-dos (Q-130, U-37), reorder alerts (Q-149, U-78) and the weekly
//!   health check (Q-171, U-12) share one muted line (Q-P16e).
//! - The backup reminder: "Back up now" after 7 days without a backup,
//!   else when the last one was made.

use cl_api::ApiError;
use cl_core::{CalendarDate, Timestamp};
use cl_model::home::{AttentionItem, AttentionKind};
use eframe::egui::{self, Align, Layout, Margin, RichText, Ui, Vec2};

use super::balances::{failed, heading, note, rule};
use super::{HomeData, HomeScreen, Zone};
use crate::i18n::{t, t_count, t_with};
use crate::screens::company::form::activated;
use crate::theme::Tokens;

/// What [`show`] returns for Try again.
pub(super) const RETRY: &str = "home.retry";

fn count_of(slot: Option<&Result<u64, ApiError>>, kind: AttentionKind) -> AttentionItem {
    match slot {
        Some(Ok(n)) => AttentionItem::counted(kind, *n),
        _ => AttentionItem::unknown(kind, true),
    }
}

/// Bills overdue or due within `days` from the payables panel
/// (`dashboard-exact-v1`): its overdue count plus its due-soon count, the
/// same bills as Home's line (posted, open, a balance left, due on or before
/// today + `days`; voided bills out). Only when the panel was read for this
/// very window (the engine takes 1 to 90 days); else `None` and Home counts
/// from the bills list.
#[must_use]
pub(crate) fn payables_due(data: &HomeData, days: u16, exact: bool) -> Option<u64> {
    if !exact || !(1..=cl_api::DUE_WITHIN_DAYS_MAX).contains(&days) {
        return None;
    }
    let p = data.summary.value.as_ref()?.payables.as_ref()?;
    (p.due_within_days == days).then(|| p.overdue_count.saturating_add(p.due_soon_count))
}

/// Every Needs attention item, in display order: each count is exactly the
/// engine's answer (`None` while unknown).
#[must_use]
pub(crate) fn items(
    data: &HomeData,
    bills_due_days: u16,
    exact: bool,
    backup_due: bool,
) -> Vec<AttentionItem> {
    let counts = data.counts.value.as_ref();
    AttentionKind::all(bills_due_days)
        .into_iter()
        .map(|kind| match kind {
            AttentionKind::BankWaiting => count_of(counts.map(|c| &c.feed), kind),
            AttentionKind::BillsDue { days } => match payables_due(data, days, exact) {
                Some(n) => AttentionItem::counted(kind, n),
                None => count_of(counts.map(|c| &c.bills), kind),
            },
            AttentionKind::PayrollUnposted => count_of(counts.map(|c| &c.payroll), kind),
            // Intake folders: counted on this computer by the inbox
            // service; the card puts in its count each frame
            // ([`with_intake`]).
            AttentionKind::IntakeFolders => AttentionItem::counted(kind, 0),
            // Documents to review (A04 D6): the engine's total of live
            // drafts ([`documents_count`]); 0 on an engine without them.
            AttentionKind::DocumentsToReview => match data.documents.value {
                Some(n) => AttentionItem::counted(kind, n),
                None => AttentionItem::unknown(kind, true),
            },
            AttentionKind::ReconcileUnfinished => match &data.reconcile.value {
                Some(r) => AttentionItem::counted(
                    kind,
                    u64::try_from(r.in_progress.len()).unwrap_or(u64::MAX),
                ),
                None => AttentionItem::unknown(kind, true),
            },
            AttentionKind::InvoicesOverdue => {
                if !exact {
                    return AttentionItem::unknown(kind, false);
                }
                match data
                    .summary
                    .value
                    .as_ref()
                    .and_then(|s| s.receivables.as_ref())
                {
                    Some(r) => AttentionItem::counted(kind, r.overdue_count),
                    None => AttentionItem::unknown(kind, true),
                }
            }
            AttentionKind::ToDos | AttentionKind::Reorder | AttentionKind::HealthCheck => {
                AttentionItem::unknown(kind, false)
            }
            AttentionKind::Backup => AttentionItem::backup(backup_due),
        })
        .collect()
}

/// `items` with the intake folders that need attention on this computer
/// (A01 D6: `receipt_inbox::intake_problems`).
#[must_use]
pub(crate) fn with_intake(mut items: Vec<AttentionItem>, problems: usize) -> Vec<AttentionItem> {
    for item in &mut items {
        if item.kind == AttentionKind::IntakeFolders {
            *item = AttentionItem::counted(
                AttentionKind::IntakeFolders,
                u64::try_from(problems).unwrap_or(u64::MAX),
            );
        }
    }
    items
}

/// Home's count of documents to review (A04 D6) on the demo engine: the
/// total of `list(status=new,ready,failed, view=summary, limit=1)`.
///
/// # Errors
/// Any [`ApiError`] but `Unsupported` (no drafts: 0).
pub(crate) fn documents_demo(
    s: &crate::company::demo::DemoState,
    tenant: Option<cl_core::ids::TenantId>,
) -> Result<u64, ApiError> {
    let Some(t) = tenant else {
        return Ok(0);
    };
    crate::screens::doc_review::queue::count_of(crate::company::demo::drafts::list(
        s,
        t,
        &crate::screens::doc_review::queue::count_query(),
    ))
}

/// [`documents_demo`] on the engine.
///
/// # Errors
/// As [`documents_demo`].
pub(crate) async fn documents_count(s: cl_api::Session) -> Result<u64, ApiError> {
    let q = crate::screens::doc_review::queue::count_query();
    crate::screens::doc_review::queue::count_of(s.document_drafts().list(&q).await)
}

/// `days` as words: "1 day", "14 days".
#[must_use]
pub(crate) fn days_text(days: u16) -> String {
    t_count("home.days", usize::from(days), &[])
}

/// When `at` was, for the backup line: "today 8:12 AM", else the date.
#[must_use]
pub(crate) fn when_text(at: Timestamp, zone: Zone, today: CalendarDate) -> String {
    use chrono::Datelike as _;
    let local = zone.at(at);
    let day = CalendarDate::from_ymd(local.year(), local.month(), local.day()).ok();
    if day == Some(today) {
        t_with(
            "home.when.today",
            &[("time", &local.format("%-I:%M %p").to_string())],
        )
    } else {
        day.map_or_else(String::new, crate::shell::us_date)
    }
}

/// One line of the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    /// What it says.
    pub text: String,
    /// The link at its right: its text and the command it opens.
    pub link: Option<(String, &'static str)>,
    /// The link's tooltip (a plain word for a bookkeeping term).
    pub link_tip: Option<String>,
    /// A dash at the right with this tooltip (a count not known).
    pub dash: Option<String>,
    /// Muted (information, not a task).
    pub muted: bool,
}

impl Line {
    fn task(text: String, link_key: &str, opens: Option<&'static str>) -> Line {
        Line {
            text,
            link: opens.map(|o| (t(link_key), o)),
            link_tip: crate::i18n::has(&format!("{link_key}.tip"))
                .then(|| t(&format!("{link_key}.tip"))),
            dash: None,
            muted: false,
        }
    }

    fn dash(text: String, tip: String) -> Line {
        Line {
            text,
            link: None,
            link_tip: None,
            dash: Some(tip),
            muted: false,
        }
    }

    fn muted(text: String) -> Line {
        Line {
            text,
            link: None,
            link_tip: None,
            dash: None,
            muted: true,
        }
    }
}

/// The lines Home shows for `items` (counts above zero, dashes for what
/// can't be counted yet, the later items' note and the backup line).
#[must_use]
pub(crate) fn lines(home: &HomeScreen, items: &[AttentionItem]) -> Vec<Line> {
    let mut out = Vec::new();
    let mut later = false;
    let failed_tip = || t("home.attention.failed.tip");
    for item in items {
        let kind = item.kind;
        let opens = kind.opens();
        let n = item.count.unwrap_or(0);
        let shown = usize::try_from(n).unwrap_or(usize::MAX);
        let name = |key: &str| t(&format!("home.attention.{key}.name"));
        match kind {
            AttentionKind::ToDos | AttentionKind::Reorder | AttentionKind::HealthCheck => {
                later = true;
            }
            AttentionKind::Backup => {}
            _ if !item.available => {
                out.push(Line::dash(name(kind.key()), t("home.footer.later.tip")));
            }
            _ if item.count.is_none() => {
                // Still loading: nothing yet; a failed read: a dash.
                let failed = match kind {
                    AttentionKind::ReconcileUnfinished => home.data.reconcile.error.is_some(),
                    AttentionKind::InvoicesOverdue => home.data.summary.error.is_some(),
                    AttentionKind::IntakeFolders => false,
                    AttentionKind::DocumentsToReview => home.data.documents.error.is_some(),
                    _ => home.data.counts.value.as_ref().is_some_and(|c| match kind {
                        AttentionKind::BankWaiting => c.feed.is_err(),
                        AttentionKind::BillsDue { .. } => c.bills.is_err(),
                        _ => c.payroll.is_err(),
                    }),
                };
                if failed {
                    out.push(Line::dash(name(kind.key()), failed_tip()));
                }
            }
            _ if n == 0 => {}
            AttentionKind::BankWaiting => out.push(Line::task(
                t_count("home.attention.bank", shown, &[]),
                "home.attention.review",
                opens,
            )),
            AttentionKind::ReconcileUnfinished => {
                let one = home
                    .data
                    .reconcile
                    .value
                    .as_ref()
                    .filter(|r| r.in_progress.len() == 1)
                    .and_then(|r| r.in_progress.first())
                    .and_then(|id| home.data.account(*id))
                    .map(crate::account_names::shown_name);
                let text = match one {
                    Some(account) => {
                        t_with("home.attention.reconcile_in", &[("account", &account)])
                    }
                    None => t_count("home.attention.reconcile", shown, &[]),
                };
                out.push(Line::task(text, "home.attention.resume", opens));
            }
            AttentionKind::InvoicesOverdue => out.push(Line::task(
                t_count("home.attention.invoices", shown, &[]),
                "home.attention.open_aging",
                opens,
            )),
            AttentionKind::BillsDue { days } => out.push(Line::task(
                t_count(
                    "home.attention.bills",
                    shown,
                    &[("within", &days_text(days))],
                ),
                "home.attention.pay_bills",
                opens,
            )),
            AttentionKind::PayrollUnposted => out.push(Line::task(
                t_count("home.attention.payroll", shown, &[]),
                "home.attention.import",
                opens,
            )),
            AttentionKind::IntakeFolders => out.push(Line::task(
                t_count("home.attention.intake", shown, &[]),
                "home.attention.intake.open",
                opens,
            )),
            AttentionKind::DocumentsToReview => out.push(Line::task(
                t_count("home.attention.documents", shown, &[]),
                "home.attention.review",
                opens,
            )),
        }
    }
    if later {
        out.push(Line::muted(t("home.attention.later")));
    }
    let local = home.local;
    let when = local
        .last_backup
        .map(|b| when_text(b, local.zone, home.today));
    if home.backup_due() {
        let text = match &when {
            Some(w) => t_with("home.attention.backup_due", &[("when", w)]),
            None => t("home.attention.backup_never"),
        };
        out.push(Line::task(
            text,
            "home.attention.back_up_now",
            AttentionKind::Backup.opens(),
        ));
    } else if let Some(w) = when {
        out.push(Line::muted(t_with(
            "home.attention.backed_up",
            &[("when", &w)],
        )));
    }
    out
}

/// The Needs attention card: the command a link opens, or [`RETRY`].
pub(super) fn show(ui: &mut Ui, home: &HomeScreen, tokens: &Tokens) -> Option<&'static str> {
    heading(ui, tokens, &t("home.attention.title"));
    if let Some(e) = &home.data.counts.error {
        return failed(ui, tokens, "home.attention.failed", e).then_some(RETRY);
    }
    let items = with_intake(
        home.attention(),
        crate::screens::receipt_inbox::intake_problems(ui.ctx()),
    );
    let lines = lines(home, &items);
    let waiting = home.data.counts.value.is_none();
    let mut clicked = None;
    let mut first = true;
    if waiting {
        note(ui, tokens, &t("home.attention.loading"));
        first = false;
    } else if !items.iter().any(AttentionItem::needs_attention) {
        line(
            ui,
            tokens,
            &Line::muted(t("home.attention.nothing")),
            &mut clicked,
        );
        first = false;
    }
    for l in &lines {
        if !first {
            rule(ui, tokens.border_row);
        }
        first = false;
        line(ui, tokens, l, &mut clicked);
    }
    if clicked == AttentionKind::IntakeFolders.opens() {
        // Preferences opens on Document intake.
        crate::screens::preferences::open_at(crate::screens::preferences::Category::ReceiptInbox);
    }
    clicked
}

fn line(ui: &mut Ui, tokens: &Tokens, l: &Line, clicked: &mut Option<&'static str>) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(14, 9))
        .show(ui, |ui| {
            let width = ui.available_width();
            ui.set_width(width);
            let right = match (&l.link, &l.dash) {
                (Some((text, _)), _) => {
                    let font = egui::TextStyle::Body.resolve(ui.style());
                    ui.painter()
                        .layout_no_wrap(text.clone(), font, tokens.accent)
                        .size()
                        .x
                        + 4.0
                }
                (None, Some(_)) => 24.0,
                _ => 0.0,
            };
            let text_w = (width - right - 12.0).max(80.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(text_w, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(text_w);
                        let color = if l.muted {
                            tokens.text_secondary
                        } else {
                            tokens.text_primary
                        };
                        ui.add(egui::Label::new(RichText::new(&l.text).color(color)).wrap());
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if let Some((text, opens)) = &l.link {
                        let mut r = ui.link(text);
                        if let Some(tip) = &l.link_tip {
                            r = r.on_hover_text(tip);
                        }
                        if activated(&r) {
                            *clicked = Some(*opens);
                        }
                    } else if let Some(tip) = &l.dash {
                        ui.add(
                            egui::Label::new(RichText::new("\u{2014}").color(tokens.text_tertiary))
                                .selectable(false),
                        )
                        .on_hover_text(tip);
                    }
                });
            });
        });
}
