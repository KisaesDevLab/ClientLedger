//! The detail panel (P26 DI-2, mockup right panel): the cursor's row and
//! what to do with it.
//!
//! - **Categorize**: the account (the proposal preselected), Accept.
//! - **Split** (S): an inline grid (Account · Amount · Memo, Ctrl+Insert /
//!   Ctrl+Delete) that must place the bank amount exactly; disabled for an
//!   amount with more than 2 decimals.
//! - **Match**: the bank lines already in the books it may be, one per
//!   line (payroll lines each on their own, Q-P26r).
//! - **Transfer**: the other account's bank line within the transfer
//!   window (Preferences, Q-P26l), or an account with no bank line yet.
//! - **Deposit** (money in): the exact groupings of waiting payments, or a
//!   manual pick (±10 days) with fee lines; Accept at a 0.00 difference
//!   (Q-P26t). Display only: the engine reads the live payment amounts.
//! - A possible duplicate is shown beside the row it resembles with Keep
//!   both / Remove this one (Q-P26b, Q-P26g).
//! - Accepted rows have Undo; rows whose entry was voided have Review
//!   again (Q-P26u).
//!
//! Amounts typed here are parsed with at most 2 decimals and never
//! rounded ([`UserInput::CENTS`]); sums are display-only.

use std::collections::BTreeSet;

use cl_api::{
    DuplicateDecision, FeeLine, FeedChoice, FeedItemId, FeedRow, FeedSplitLine, MatchCandidate,
    PayrollOverlap, TransferCandidate, TransferTarget, UndoTarget,
};
use cl_core::ids::{AccountId, JournalLineId, TransactionId};
use cl_core::money::UserInput;
use cl_core::{CalendarDate, Money};
use cl_model::bank_feed::{self as model, DepositDraft, Show, SplitDraft, Why};
use eframe::egui::{self, RichText, Ui};

use super::BankFeedScreen;
use super::send::{self, Send};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::templates::engine::{Job, finished, run};
use crate::theme::Tokens;

/// What the panel is doing with a waiting row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Choose an account.
    #[default]
    Categorize,
    /// Split across accounts.
    Split,
    /// Match an entry in the books.
    Match,
    /// A transfer between own accounts.
    Transfer,
    /// A deposit of waiting payments.
    Deposit,
}

impl Mode {
    const ALL: [Mode; 5] = [
        Mode::Categorize,
        Mode::Split,
        Mode::Match,
        Mode::Transfer,
        Mode::Deposit,
    ];

    fn key(self) -> &'static str {
        match self {
            Mode::Categorize => "bank_feed.mode.categorize",
            Mode::Split => "bank_feed.mode.split",
            Mode::Match => "bank_feed.mode.match",
            Mode::Transfer => "bank_feed.mode.transfer",
            Mode::Deposit => "bank_feed.mode.deposit",
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Mode::Categorize => "categorize",
            Mode::Split => "split",
            Mode::Match => "match",
            Mode::Transfer => "transfer",
            Mode::Deposit => "deposit",
        }
    }
}

/// A waiting payment a deposit may take (its live amount).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Pay {
    pub id: TransactionId,
    pub amount: Money,
    pub date: CalendarDate,
    /// "Received from · ref" (owner data, never logged).
    pub label: String,
}

/// A waiting payment that can't be deposited here (its amount is display
/// only).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Held {
    pub date: CalendarDate,
    /// "Received from · ref" (owner data, never logged).
    pub label: String,
    /// The amount as the engine sent it, for display only.
    pub amount: String,
}

/// Who a waiting payment came from, so same-size payments can be told
/// apart (P26 QA-7): the payer and its reference, else the reference, else
/// the kind of entry ("Payment", "Sales receipt").
pub(crate) fn payer_label(
    received_from: Option<&str>,
    ref_no: Option<&str>,
    txn_type: Option<&str>,
) -> String {
    let clean = |s: Option<&str>| {
        s.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let kind = || {
        txn_type
            .map(|k| format!("bank_feed.txn.{k}"))
            .filter(|k| crate::i18n::has(k))
            .map_or_else(|| t("bank_feed.txn.payment"), |k| t(&k))
    };
    match (clean(received_from), clean(ref_no)) {
        (Some(w), Some(r)) => t_with("bank_feed.deposit.payer_ref", &[("who", &w), ("ref", &r)]),
        (Some(w), None) => w,
        (None, Some(r)) => t_with(
            "bank_feed.deposit.payer_ref",
            &[("who", &kind()), ("ref", &r)],
        ),
        (None, None) => kind(),
    }
}

/// What the panel reads for a row.
#[derive(Clone, Default)]
pub(crate) struct DetailData {
    pub row: Option<FeedItemId>,
    pub candidates: Vec<MatchCandidate>,
    pub payroll: Vec<PayrollOverlap>,
    pub transfers: Vec<TransferCandidate>,
    pub groupings: Vec<Vec<Pay>>,
    pub payments: Vec<Pay>,
    /// Waiting payments whose amount the engine didn't send exactly: shown
    /// as "can't be deposited here", never picked.
    pub held: Vec<Held>,
    /// 403: candidates aren't available for this role.
    pub forbidden: bool,
}

/// One typed line (split or fee).
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Line {
    pub account: Option<AccountId>,
    pub amount: String,
    pub memo: String,
}

impl Line {
    fn money(&self) -> Option<Money> {
        let s = self.amount.trim();
        if s.is_empty() {
            return None;
        }
        Money::parse_user(s, UserInput::CENTS)
            .ok()
            .filter(|m| m.is_positive())
    }

    fn memo(&self) -> Option<String> {
        Some(self.memo.trim().to_string()).filter(|m| !m.is_empty())
    }
}

/// The panel's state for the cursor's row.
#[derive(Default)]
pub(crate) struct Detail {
    pub row: Option<FeedItemId>,
    pub mode: Mode,
    pub account: Option<AccountId>,
    pub split: Vec<Line>,
    /// The chosen match: entry and line.
    pub pick_match: Option<(TransactionId, Option<JournalLineId>)>,
    pub pick_transfer: Option<FeedItemId>,
    pub transfer_to: Option<AccountId>,
    pub picks: BTreeSet<TransactionId>,
    pub fees: Vec<Line>,
    pub data: DetailData,
    load: Option<Job<DetailData>>,
    /// What Accept (A) sends now, when the panel's draft is complete.
    pub ready: Option<Send>,
    /// C: the type-to-find account box (P26 QA-18) and its highlighted
    /// match.
    pub find: Option<String>,
    pub find_pick: usize,
    find_focus: bool,
    /// A05: the receipt for the row.
    pub receipt: super::receipts::Panel,
}

impl Detail {
    /// The row moved: a fresh draft and a fresh read.
    fn reset(&mut self, r: &FeedRow, why: Why) {
        *self = Detail {
            row: Some(r.id),
            mode: match why {
                // Money in with no entry to match: waiting payments to
                // deposit. With a candidate the row opens on Match, as the
                // list says (P26 QA-24).
                Why::Match if !r.is_money_out() && r.match_candidate_count == 0 => Mode::Deposit,
                Why::Match => Mode::Match,
                Why::Transfer => Mode::Transfer,
                _ => Mode::Categorize,
            },
            account: r
                .assigned_account
                .as_ref()
                .or(r.suggested_account.as_ref())
                .map(|a| a.id),
            ..Detail::default()
        };
        self.transfer_to = r.suggested_account.as_ref().map(|a| a.id);
        // The engine's one counterpart within 3 days (Q-P26y) is chosen,
        // and "Transfer to" shows its account (P26 QA-14).
        if let Some(h) = &r.transfer_candidate {
            self.pick_transfer = Some(h.feed_item);
            self.transfer_to = h.account.or(self.transfer_to);
        }
    }

    /// The partners arrived: a pre-picked partner outside the owner's
    /// transfer window (Preferences) is dropped (P26 QA-16).
    pub(crate) fn take_data(&mut self, d: DetailData) {
        if let Some(p) = self.pick_transfer
            && !d.transfers.iter().any(|c| c.feed_item == p)
        {
            self.pick_transfer = None;
        }
        // Money in that the list counted as a match but has no entry to
        // match, only waiting payments: the panel opens on Deposit.
        if self.mode == Mode::Match
            && d.candidates.is_empty()
            && d.payroll.is_empty()
            && (!d.groupings.is_empty() || !d.payments.is_empty())
        {
            self.mode = Mode::Deposit;
        }
        self.data = d;
    }

    /// The split draft for the row's amount (`None`: more than 2 decimals).
    pub(crate) fn split_draft(&self, r: &FeedRow) -> Option<SplitDraft<AccountId>> {
        let mut d = SplitDraft::new(r.amount)?;
        d.lines = self.split.iter().map(|l| (l.account, l.money())).collect();
        Some(d)
    }

    /// The deposit draft: the bank amount, the picked payments, the fees.
    pub(crate) fn deposit_draft(&self, r: &FeedRow) -> DepositDraft<TransactionId, AccountId> {
        let pool = self
            .data
            .payments
            .iter()
            .chain(self.data.groupings.iter().flatten());
        let mut payments: Vec<(TransactionId, Money)> = Vec::new();
        for p in pool {
            if self.picks.contains(&p.id) && !payments.iter().any(|(id, _)| *id == p.id) {
                payments.push((p.id, p.amount));
            }
        }
        DepositDraft {
            bank: r.amount.abs(),
            payments,
            fees: self.fees.iter().map(|l| (l.account, l.money())).collect(),
        }
    }

    /// A typed line went wrong (not a positive amount with at most 2
    /// decimals).
    fn bad_amount(lines: &[Line]) -> bool {
        lines
            .iter()
            .any(|l| !l.amount.trim().is_empty() && l.money().is_none())
    }
}

/// S: the split grid with two lines (the first on the proposed account).
pub(crate) fn enter_split(s: &mut BankFeedScreen) {
    s.detail.mode = Mode::Split;
    if s.detail.split.is_empty() {
        s.detail.split = vec![
            Line {
                account: s.detail.account,
                ..Line::default()
            },
            Line::default(),
        ];
    }
}

/// C: opens the account box of the cursor's row and puts the typing
/// cursor in it (P26 QA-18). Typing narrows the chart, Up/Down move,
/// Enter picks and gives the keys back to the list (then A accepts), Esc
/// closes it.
pub(crate) fn choose_account(s: &mut BankFeedScreen) -> bool {
    let Some(id) = s.list.current() else {
        return true;
    };
    let waiting = s
        .row(id)
        .is_some_and(|r| r.status.is_waiting() && !r.possible_duplicate);
    if s.view_only || !waiting || s.show != Show::Waiting {
        return true;
    }
    s.rules.open = false;
    if s.detail.row != Some(id) {
        follow(s);
    }
    s.detail.mode = Mode::Categorize;
    s.detail.find = Some(String::new());
    s.detail.find_pick = 0;
    s.detail.find_focus = true;
    true
}

/// The chart entries whose name holds every word typed (any case).
pub(crate) fn find_accounts<'a>(
    chart: &'a [(AccountId, String)],
    typed: &str,
) -> Vec<&'a (AccountId, String)> {
    let words: Vec<String> = typed.split_whitespace().map(str::to_lowercase).collect();
    chart
        .iter()
        .filter(|(_, n)| {
            let n = n.to_lowercase();
            words.iter().all(|w| n.contains(w.as_str()))
        })
        .collect()
}

/// The account box C opened.
fn find_ui(s: &mut BankFeedScreen, ui: &mut Ui, chart: &[(AccountId, String)]) {
    let Some(mut typed) = s.detail.find.take() else {
        return;
    };
    let label = t("bank_feed.detail.find");
    let id = egui::Id::unique("bank_feed.detail.account_find");
    let r = ui.add(
        egui::TextEdit::singleline(&mut typed)
            .id(id)
            .hint_text(t("bank_feed.detail.find.hint"))
            .desired_width(ui.available_width().min(280.0)),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &label));
    if s.detail.find_focus {
        r.request_focus();
        s.detail.find_focus = false;
    }
    if r.changed() {
        s.detail.find_pick = 0;
    }
    let found = find_accounts(chart, &typed);
    let n = found.len();
    let focused = r.has_focus();
    let (down, up, enter, esc) = ui.input_mut(|i| {
        if !focused {
            return (false, false, false, false);
        }
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    if down && n > 0 {
        s.detail.find_pick = (s.detail.find_pick + 1).min(n - 1);
    }
    if up {
        s.detail.find_pick = s.detail.find_pick.saturating_sub(1);
    }
    let mut picked = None;
    for (i, (aid, name)) in found.iter().take(8).enumerate() {
        if ui.selectable_label(i == s.detail.find_pick, name).clicked() {
            picked = Some(*aid);
        }
    }
    if n == 0 {
        ui.label(RichText::new(t("bank_feed.detail.find.none")).size(12.0));
    }
    // Enter right after the box lost focus (egui moves focus on Enter).
    let entered = enter || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
    if entered && let Some((aid, _)) = found.get(s.detail.find_pick) {
        picked = Some(*aid);
    }
    if let Some(a) = picked {
        s.detail.account = Some(a);
        // The keys go back to the list: A accepts now.
        r.surrender_focus();
        return;
    }
    if esc || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape))) {
        r.surrender_focus();
        return;
    }
    s.detail.find = Some(typed);
}

/// Ctrl+Insert / Ctrl+Delete in the split or fee grid.
pub(crate) fn line_command(s: &mut BankFeedScreen, id: &str) -> bool {
    let lines = match s.detail.mode {
        Mode::Split => &mut s.detail.split,
        Mode::Deposit => &mut s.detail.fees,
        _ => return false,
    };
    match id {
        "form.insert_line" | "feed.insert_line" => {
            if s.detail.mode == Mode::Deposit && lines.len() >= cl_api::FEE_LINES_MAX {
                return true;
            }
            lines.push(Line::default());
            true
        }
        "form.delete_line" | "feed.delete_line" => {
            lines.pop();
            true
        }
        _ => false,
    }
}

/// Keeps the panel on the cursor's row and its data read.
pub(crate) fn follow(s: &mut BankFeedScreen) {
    if let Some(r) = finished(&mut s.detail.load) {
        match r {
            Ok(d) if d.row == s.detail.row => s.detail.take_data(d),
            Ok(_) => {}
            Err(e) => s.detail.data.forbidden = cl_api::is_view_only(&e),
        }
    }
    let Some(id) = s.list.current() else {
        s.detail.row = None;
        return;
    };
    if s.detail.row == Some(id) {
        return;
    }
    let Some(r) = s.row(id).cloned() else {
        return;
    };
    let why = s.why(&r);
    s.detail.reset(&r, why);
    super::receipts::reset(s, id);
    if s.detail_request == Some(model::FeedAction::Split(id)) {
        enter_split(s);
    }
    if r.status.is_waiting() {
        read(s, &r);
    }
}

/// Reads the candidates, transfer partners and payments for `r`.
fn read(s: &mut BankFeedScreen, r: &FeedRow) {
    let id = r.id;
    let money_in = !r.is_money_out();
    let window = s.transfer_window;
    s.detail.load = Some(run(
        &s.link,
        s.group.as_ref(),
        None,
        move |d| {
            use crate::company::demo::feed_review as demo;
            let pay = |p: &crate::company::demo::deposits::DemoPending| Pay {
                id: p.source,
                amount: p.amount,
                date: p.date,
                label: payer_label(
                    p.received_from.as_deref(),
                    p.ref_no.as_deref(),
                    serde_json::to_value(p.txn_type)
                        .ok()
                        .as_ref()
                        .and_then(serde_json::Value::as_str),
                ),
            };
            Ok(DetailData {
                row: Some(id),
                candidates: demo::match_candidates(d, id),
                payroll: Vec::new(),
                transfers: demo::transfer_candidates(d, id, window),
                groupings: demo::deposit_groupings(d, id)
                    .iter()
                    .map(|g| g.iter().map(pay).collect())
                    .collect(),
                payments: demo::deposit_payments(d, id).iter().map(pay).collect(),
                held: Vec::new(),
                forbidden: d.feed_review.view_only,
            })
        },
        move |sess| async move { send::live_detail(sess, id, money_in, window).await },
    ));
}

fn amount_text(ui: &Ui, m: Money) -> String {
    m.format(crate::grid_env::shell_frame(ui.ctx()).money_style)
}

/// Draws the panel.
pub(crate) fn draw(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    let Some(id) = s.list.current() else {
        form::hint(ui, tokens, &t("bank_feed.detail.none"));
        return;
    };
    let Some(r) = s.row(id).cloned() else {
        return;
    };
    let why = s.why(&r);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let date = crate::shell::us_date(r.date);
        ui.label(
            RichText::new(format!(
                "{} \u{b7} {}",
                date.get(..5).unwrap_or(&date),
                amount_text(ui, model::display_amount(r.amount))
            ))
            .monospace()
            .strong(),
        );
    });
    ui.add(
        egui::Label::new(
            RichText::new(super::bank_text(&r))
                .monospace()
                .size(12.0)
                .color(tokens.text_secondary),
        )
        .wrap(),
    );
    ui.add_space(6.0);
    let can = !s.view_only && s.act.is_none();
    let tip = if s.view_only {
        t("bank_feed.view_only.tip")
    } else {
        String::new()
    };
    // A waiting line's Accept and Exclude stay below the scrolling part.
    let pinned = !r.entry_voided
        && !r.possible_duplicate
        && r.status.is_waiting()
        && s.show == Show::Waiting;
    let reserve = if pinned { ACTIONS_H } else { 0.0 };
    egui::ScrollArea::vertical()
        .id_salt("bank_feed.detail")
        .auto_shrink([false, true])
        .max_height((ui.available_height() - reserve).max(80.0))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            if r.entry_voided {
                voided(s, ui, tokens, &r, can, &tip);
            } else if r.status.is_accepted() {
                accepted(s, ui, tokens, &r, can, &tip);
            } else if r.possible_duplicate && r.status.is_waiting() {
                duplicate(s, ui, tokens, &r, can, &tip);
            } else if r.status.is_waiting() && s.show == Show::Waiting {
                waiting(s, ui, tokens, &r, why);
            } else {
                form::hint(ui, tokens, &t("bank_feed.detail.excluded"));
            }
        });
    if pinned {
        actions(s, ui, tokens, &r, can, &tip);
    }
}

/// The height kept under the panel's scrolling part for Accept / Exclude.
const ACTIONS_H: f32 = 44.0;

fn voided(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens, r: &FeedRow, can: bool, tip: &str) {
    form::inline_warning(ui, tokens, &t("bank_feed.detail.entry_voided"));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(can, egui::Button::new(t("bank_feed.detail.review_again")))
            .on_hover_text(t("bank_feed.detail.review_again.tip"))
            .on_disabled_hover_text(tip)
            .clicked()
        {
            send::start(s, Send::ReviewAgain(r.id), None);
        }
        if ui
            .add_enabled(can, egui::Button::new(t("bank_feed.detail.undo")))
            .on_hover_text(t("bank_feed.detail.undo.voided_tip"))
            .on_disabled_hover_text(tip)
            .clicked()
        {
            send::start(s, Send::Undo(UndoTarget::Items(vec![r.id]), false), None);
        }
    });
}

fn accepted(
    s: &mut BankFeedScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    r: &FeedRow,
    can: bool,
    tip: &str,
) {
    let key = accepted_text(r.status, s.accepted_how(r));
    form::hint(ui, tokens, &t(key));
    ui.add_space(8.0);
    if ui
        .add_enabled(can, egui::Button::new(t("bank_feed.detail.undo")))
        .on_hover_text(t("bank_feed.detail.undo.tip"))
        .on_disabled_hover_text(tip)
        .clicked()
    {
        send::start(s, Send::Undo(UndoTarget::Items(vec![r.id]), false), None);
    }
}

/// What an accepted row's panel says Undo will do (P26 QA-9, QA-15): a
/// deposit or transfer made here is removed; a match only unlinks. A
/// matched row accepted before this window opened says both.
pub(crate) fn accepted_text(status: cl_api::FeedStatus, how: Option<&str>) -> &'static str {
    match (status, how) {
        (_, Some("deposit")) => "bank_feed.detail.deposited",
        (_, Some("transfer")) => "bank_feed.detail.transferred",
        (cl_api::FeedStatus::Matched, Some("match")) => "bank_feed.detail.matched",
        (cl_api::FeedStatus::Matched, _) => "bank_feed.detail.linked",
        _ => "bank_feed.detail.posted",
    }
}

fn duplicate(
    s: &mut BankFeedScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    r: &FeedRow,
    can: bool,
    tip: &str,
) {
    form::inline_warning(ui, tokens, &t("bank_feed.detail.duplicate"));
    ui.add_space(6.0);
    let other = r.duplicate_of.clone();
    ui.columns(2, |cols| {
        cols[0].label(RichText::new(t("bank_feed.dup.this")).strong());
        side(
            &mut cols[0],
            tokens,
            Some(r.date),
            Some(r.amount),
            Some(super::bank_text(r)),
        );
        cols[1].label(RichText::new(t("bank_feed.dup.other")).strong());
        match &other {
            Some(o) => side(
                &mut cols[1],
                tokens,
                o.date,
                o.amount,
                o.description.as_deref(),
            ),
            None => {
                cols[1].label(t("bank_feed.dup.unknown"));
            }
        }
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(can, egui::Button::new(t("bank_feed.dup.keep")))
            .on_disabled_hover_text(tip)
            .clicked()
        {
            send::start(s, Send::Duplicate(r.id, DuplicateDecision::Keep), None);
        }
        if ui
            .add_enabled(can, egui::Button::new(t("bank_feed.dup.remove")))
            .on_hover_text(t("bank_feed.dup.remove.tip"))
            .on_disabled_hover_text(tip)
            .clicked()
        {
            send::start(s, Send::Duplicate(r.id, DuplicateDecision::Remove), None);
        }
    });
}

fn side(
    ui: &mut Ui,
    tokens: &Tokens,
    date: Option<CalendarDate>,
    amount: Option<Money>,
    text: Option<&str>,
) {
    if let Some(d) = date {
        ui.label(RichText::new(crate::shell::us_date(d)).monospace());
    }
    if let Some(a) = amount {
        let shown = amount_text(ui, model::display_amount(a));
        ui.label(RichText::new(shown).monospace());
    }
    ui.add(
        egui::Label::new(
            RichText::new(text.unwrap_or_default())
                .size(12.0)
                .color(tokens.text_secondary),
        )
        .wrap(),
    );
}

fn account_combo(
    ui: &mut Ui,
    salt: egui::IdSalt,
    chart: &[(AccountId, String)],
    value: &mut Option<AccountId>,
    label: &str,
    width: f32,
) {
    let shown = value
        .and_then(|a| chart.iter().find(|(id, _)| *id == a))
        .map_or_else(|| t("bank_feed.detail.choose_account"), |(_, n)| n.clone());
    let r = egui::ComboBox::from_id_salt(salt)
        .selected_text(shown)
        .truncate()
        .width(ui.available_width().min(width))
        .show_ui(ui, |ui| {
            for (id, name) in chart {
                ui.selectable_value(value, Some(*id), name);
            }
        })
        .response;
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, label));
}

fn waiting(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens, r: &FeedRow, why: Why) {
    match why {
        Why::Ai { high: false } => {
            form::inline_warning(ui, tokens, &t("bank_feed.detail.ai_unsure"));
        }
        _ => {
            if let Some(key) = missing_key(s, r, why) {
                form::inline_warning(ui, tokens, &t(key));
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        for m in Mode::ALL {
            if m == Mode::Deposit && r.is_money_out() {
                continue;
            }
            let label = match m {
                Mode::Match => format!(
                    "{} ({})",
                    t(m.key()),
                    s.detail.data.candidates.len() + s.detail.data.payroll.len()
                ),
                _ => t(m.key()),
            };
            if ui.selectable_label(s.detail.mode == m, label).clicked() {
                if m == Mode::Split {
                    enter_split(s);
                } else {
                    s.detail.mode = m;
                }
            }
        }
    });
    ui.add_space(6.0);
    super::receipts::section(s, ui, tokens, r);
    let chart = s.chart.clone();
    let send = match s.detail.mode {
        Mode::Categorize => categorize_ui(s, ui, &chart, r),
        Mode::Split => split_ui(s, ui, tokens, &chart, r),
        Mode::Match => match_ui(s, ui, tokens, r),
        Mode::Transfer => transfer_ui(s, ui, tokens, &chart, r),
        Mode::Deposit => deposit_ui(s, ui, tokens, &chart, r),
    };
    let send = super::receipts::with_receipt(s, r, send);
    s.detail.ready = send;
}

/// Accept and Exclude under the panel's scrolling part (QA-6): a long
/// receipt section or split grid at 1024x768 scrolls; the buttons stay in
/// the window.
fn actions(
    s: &mut BankFeedScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    r: &FeedRow,
    can: bool,
    tip: &str,
) {
    let send = s.detail.ready.clone();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let accept = egui::Button::new(
            RichText::new(format!("{}  A", t("bank_feed.detail.accept"))).color(tokens.on_accent),
        )
        .fill(tokens.accent);
        let why_off = if send.is_none() {
            t("bank_feed.detail.accept.incomplete")
        } else {
            tip.to_string()
        };
        if ui
            .add_enabled(can && send.is_some(), accept)
            .on_disabled_hover_text(&why_off)
            .clicked()
            && let Some(x) = send
        {
            send::start(s, x, None);
        }
        if ui
            .add_enabled(can, egui::Button::new(t("bank_feed.detail.exclude")))
            .on_disabled_hover_text(tip)
            .clicked()
        {
            s.list.clear_selection();
            s.list.toggle(r.id);
            super::bulk::exclude_selected(s);
        }
    });
}

/// What the panel says is still missing on a line that needs the owner
/// (QA-2): nothing once an account is picked (Accept is enabled; the payee
/// is optional), "an account" when the payee is known or a receipt supplies
/// the vendor, else "a payee and an account".
pub(crate) fn missing_key(s: &BankFeedScreen, r: &FeedRow, why: Why) -> Option<&'static str> {
    if !matches!(why, Why::NeedsYou | Why::Receipt) || s.detail.mode != Mode::Categorize {
        return None;
    }
    let has_account = r.assigned_account.is_some() || r.suggested_account.is_some();
    if has_account || s.detail.account.is_some() {
        return None;
    }
    let payee = r.assigned_contact.is_some() || r.suggested_contact.is_some();
    let receipt = super::receipts::takes_receipt(s.detail.mode)
        && s.detail.receipt.pick.is_some()
        && s.receipts.of(r.id).contains(&s.detail.receipt.pick?);
    Some(if payee || receipt {
        "bank_feed.detail.needs_account"
    } else {
        "bank_feed.detail.needs_you"
    })
}

fn categorize_ui(
    s: &mut BankFeedScreen,
    ui: &mut Ui,
    chart: &[(AccountId, String)],
    r: &FeedRow,
) -> Option<Send> {
    ui.label(t("bank_feed.detail.account"));
    let label = t("bank_feed.detail.account");
    account_combo(
        ui,
        egui::IdSalt::new("bank_feed.detail.account"),
        chart,
        &mut s.detail.account,
        &label,
        280.0,
    );
    if s.detail.find.is_some() {
        find_ui(s, ui, chart);
    } else if !s.view_only {
        ui.label(
            RichText::new(t("bank_feed.detail.find.key"))
                .size(12.0)
                .weak(),
        );
    }
    let account = s.detail.account?;
    let proposed = r
        .assigned_account
        .as_ref()
        .or(r.suggested_account.as_ref())
        .map(|a| a.id);
    if proposed == Some(account) && r.status == cl_api::FeedStatus::Assigned {
        return Some(Send::Categorize(r.id, None));
    }
    Some(Send::Categorize(
        r.id,
        Some(FeedChoice {
            account: Some(account),
            contact: r
                .assigned_contact
                .as_ref()
                .or(r.suggested_contact.as_ref())
                .map(|c| c.id),
            ..FeedChoice::default()
        }),
    ))
}

/// Typed lines (split or fee), two rows each so they fit the panel:
/// the account and a delete button, then the amount and the memo.
fn lines_grid(
    ui: &mut Ui,
    salt: &str,
    chart: &[(AccountId, String)],
    lines: &mut [Line],
) -> Option<usize> {
    let mut remove = None;
    for (i, l) in lines.iter_mut().enumerate() {
        let n = (i + 1).to_string();
        let label = t_with("bank_feed.grid.line_account", &[("n", &n)]);
        ui.horizontal(|ui| {
            account_combo(
                ui,
                egui::IdSalt::new((salt, "a", i)),
                chart,
                &mut l.account,
                &label,
                250.0,
            );
            let del_label = t_with("bank_feed.grid.delete_line", &[("n", &n)]);
            let d = ui.small_button("\u{d7}").on_hover_text(&del_label);
            d.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &del_label));
            if d.clicked() {
                remove = Some(i);
            }
        });
        ui.horizontal(|ui| {
            let amount_label = t_with("bank_feed.grid.line_amount", &[("n", &n)]);
            let r = ui.add_sized(
                [100.0, 22.0],
                egui::TextEdit::singleline(&mut l.amount)
                    .font(egui::TextStyle::Monospace)
                    .hint_text(t("bank_feed.grid.amount")),
            );
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &amount_label));
            let memo_label = t_with("bank_feed.grid.line_memo", &[("n", &n)]);
            let m = ui.add_sized(
                [ui.available_width().min(170.0), 22.0],
                egui::TextEdit::singleline(&mut l.memo).hint_text(t("bank_feed.grid.memo")),
            );
            m.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &memo_label));
        });
        ui.add_space(4.0);
    }
    remove
}

fn split_ui(
    s: &mut BankFeedScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    chart: &[(AccountId, String)],
    r: &FeedRow,
) -> Option<Send> {
    if !model::whole_cents(r.amount) {
        form::hint(ui, tokens, &t("bank_feed.split.not_cents"));
        return None;
    }
    if let Some(i) = lines_grid(ui, "bank_feed.split", chart, &mut s.detail.split) {
        s.detail.split.remove(i);
    }
    if ui
        .small_button(t("bank_feed.grid.add_line"))
        .on_hover_text(t("bank_feed.grid.add_line.tip"))
        .clicked()
    {
        s.detail.split.push(Line::default());
    }
    let draft = s.detail.split_draft(r)?;
    if Detail::bad_amount(&s.detail.split) {
        form::inline_warning(ui, tokens, &t("bank_feed.grid.bad_amount"));
    }
    let left = draft
        .remainder()
        .map_or_else(String::new, |m| amount_text(ui, m));
    ui.label(
        RichText::new(t_with("bank_feed.split.left", &[("amount", &left)]))
            .monospace()
            .color(if draft.is_complete() {
                tokens.text_secondary
            } else {
                tokens.warn_text
            }),
    );
    if !draft.is_complete() {
        return None;
    }
    let splits = s
        .detail
        .split
        .iter()
        .filter_map(|l| {
            Some(FeedSplitLine {
                account: l.account?,
                amount: l.money()?,
                tag: None,
                memo: l.memo(),
            })
        })
        .collect::<Vec<_>>();
    Some(Send::Categorize(
        r.id,
        Some(FeedChoice {
            account: splits.first().map(|l| l.account),
            splits,
            ..FeedChoice::default()
        }),
    ))
}

fn match_ui(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens, r: &FeedRow) -> Option<Send> {
    if s.detail.data.forbidden {
        form::hint(ui, tokens, &t("bank_feed.match.forbidden"));
        return None;
    }
    if s.detail.load.is_some() {
        form::progress_line(ui, tokens, &t("bank_feed.match.reading"));
    }
    let mut rows: Vec<(TransactionId, Option<JournalLineId>, String)> = Vec::new();
    for c in &s.detail.data.candidates {
        let date = c.date.map(crate::shell::us_date).unwrap_or_default();
        let kind = c
            .txn_type
            .as_deref()
            .map(|k| format!("bank_feed.txn.{k}"))
            .filter(|k| crate::i18n::has(k))
            .map_or_else(String::new, |k| t(&k));
        let who = c.contact_name.clone().unwrap_or_default();
        let num = c.txn_number.clone().unwrap_or_default();
        rows.push((
            c.transaction,
            Some(c.line),
            format!(
                "{date} \u{b7} {kind} {num} \u{b7} {who} \u{b7} {}",
                amount_text(ui, model::display_amount(c.amount))
            ),
        ));
    }
    for p in &s.detail.data.payroll {
        let date = p.date.map(crate::shell::us_date).unwrap_or_default();
        rows.push((
            p.transaction,
            Some(p.line),
            format!(
                "{date} \u{b7} {} \u{b7} {}",
                t("bank_feed.match.payroll"),
                amount_text(ui, model::display_amount(p.amount))
            ),
        ));
    }
    if rows.is_empty() {
        if s.detail.load.is_none() {
            form::hint(ui, tokens, &t("bank_feed.match.none"));
        }
        return None;
    }
    if rows.len() == 1 && s.detail.pick_match.is_none() {
        s.detail.pick_match = Some((rows[0].0, rows[0].1));
    }
    for (txn, line, text) in rows {
        let on = s.detail.pick_match == Some((txn, line));
        if ui.radio(on, text).clicked() {
            s.detail.pick_match = Some((txn, line));
        }
    }
    let (txn, line) = s.detail.pick_match?;
    Some(Send::Match(r.id, txn, line))
}

fn transfer_ui(
    s: &mut BankFeedScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    chart: &[(AccountId, String)],
    r: &FeedRow,
) -> Option<Send> {
    form::hint(
        ui,
        tokens,
        &t_with(
            "bank_feed.transfer.window",
            &[("n", &s.transfer_window.to_string())],
        ),
    );
    let mut chosen = None;
    for c in &s.detail.data.transfers {
        let on = s.detail.pick_transfer == Some(c.feed_item);
        let text = format!(
            "{} \u{b7} {} \u{b7} {}",
            crate::shell::us_date(c.date),
            c.account_name,
            amount_text(ui, model::display_amount(c.amount))
        );
        if ui.radio(on, text).clicked() {
            chosen = Some((c.feed_item, c.account));
        }
    }
    // The picked line's account shows in "Transfer to" (P26 QA-14).
    let picked_account = s
        .detail
        .pick_transfer
        .and_then(|p| s.detail.data.transfers.iter().find(|c| c.feed_item == p))
        .map(|c| c.account);
    if let Some((c, a)) = chosen {
        s.detail.pick_transfer = Some(c);
        s.detail.transfer_to = Some(a);
    } else if let Some(a) = picked_account {
        s.detail.transfer_to = Some(a);
    }
    let label = t(if r.is_money_out() {
        "bank_feed.transfer.to"
    } else {
        "bank_feed.transfer.from"
    });
    ui.label(&label);
    let before = s.detail.transfer_to;
    let own = s.current_account().and_then(|a| a.account);
    let others: Vec<(AccountId, String)> = chart
        .iter()
        .filter(|(id, _)| Some(*id) != own)
        .cloned()
        .collect();
    account_combo(
        ui,
        egui::IdSalt::new("bank_feed.transfer.account"),
        &others,
        &mut s.detail.transfer_to,
        &label,
        280.0,
    );
    if s.detail.transfer_to != before {
        s.detail.pick_transfer = None;
    }
    if let Some(o) = s.detail.pick_transfer {
        return Some(Send::Transfer(r.id, TransferTarget::OtherItem(o)));
    }
    s.detail
        .transfer_to
        .map(|a| Send::Transfer(r.id, TransferTarget::Account(a)))
}

fn deposit_ui(
    s: &mut BankFeedScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    chart: &[(AccountId, String)],
    r: &FeedRow,
) -> Option<Send> {
    if s.detail.load.is_some() {
        form::progress_line(ui, tokens, &t("bank_feed.match.reading"));
    }
    if !s.detail.data.groupings.is_empty() {
        ui.label(RichText::new(t("bank_feed.deposit.groupings")).strong());
        let groupings = s.detail.data.groupings.clone();
        for (i, g) in groupings.iter().enumerate() {
            let ids: BTreeSet<TransactionId> = g.iter().map(|p| p.id).collect();
            let on = s.detail.picks == ids && s.detail.fees.is_empty();
            let names: Vec<String> = g
                .iter()
                .map(|p| format!("{} {}", p.label, amount_text(ui, p.amount)))
                .collect();
            let text = t_with(
                "bank_feed.deposit.grouping",
                &[("n", &(i + 1).to_string()), ("items", &names.join(" + "))],
            );
            if ui.radio(on, text).clicked() {
                s.detail.picks = ids;
                s.detail.fees.clear();
            }
        }
        ui.add_space(6.0);
    }
    ui.label(RichText::new(t("bank_feed.deposit.pick")).strong());
    let payments = s.detail.data.payments.clone();
    if payments.is_empty() && s.detail.data.held.is_empty() && s.detail.load.is_none() {
        form::hint(ui, tokens, &t("bank_feed.deposit.none"));
    }
    for p in &payments {
        let mut on = s.detail.picks.contains(&p.id);
        let text = format!(
            "{} \u{b7} {} \u{b7} {}",
            crate::shell::us_date(p.date),
            p.label,
            amount_text(ui, p.amount)
        );
        if ui.checkbox(&mut on, text).changed() {
            if on {
                s.detail.picks.insert(p.id);
            } else {
                s.detail.picks.remove(&p.id);
            }
        }
    }
    for p in &s.detail.data.held {
        let text = t_with(
            "bank_feed.deposit.held",
            &[
                ("date", &crate::shell::us_date(p.date)),
                ("from", &p.label),
                ("amount", &p.amount),
            ],
        );
        let mut off = false;
        ui.add_enabled(false, egui::Checkbox::new(&mut off, text))
            .on_disabled_hover_text(t("bank_feed.deposit.held.tip"));
    }
    ui.add_space(6.0);
    ui.label(RichText::new(t("bank_feed.deposit.fees")).strong());
    if let Some(i) = lines_grid(ui, "bank_feed.fees", chart, &mut s.detail.fees) {
        s.detail.fees.remove(i);
    }
    let full = s.detail.fees.len() >= cl_api::FEE_LINES_MAX;
    if ui
        .add_enabled(
            !full,
            egui::Button::new(t("bank_feed.deposit.add_fee")).small(),
        )
        .on_hover_text(t("bank_feed.grid.add_line.tip"))
        .on_disabled_hover_text(t("bank_feed.deposit.fees_max"))
        .clicked()
    {
        s.detail.fees.push(Line::default());
    }
    if Detail::bad_amount(&s.detail.fees) {
        form::inline_warning(ui, tokens, &t("bank_feed.grid.bad_amount"));
    }
    let d = s.detail.deposit_draft(r);
    let show = |m: Option<Money>| m.map_or_else(String::new, |m| amount_text(ui, m));
    ui.add(
        egui::Label::new(
            RichText::new(t_with(
                "bank_feed.deposit.sum",
                &[
                    ("payments", &show(d.payments_total())),
                    ("fees", &show(d.fees_total())),
                    ("bank", &show(Some(d.bank))),
                    ("difference", &show(d.difference())),
                ],
            ))
            .color(if d.is_balanced() {
                tokens.text_secondary
            } else {
                tokens.warn_text
            }),
        )
        .wrap(),
    );
    if !d.is_balanced() {
        return None;
    }
    let fees = s
        .detail
        .fees
        .iter()
        .filter_map(|l| {
            Some(FeeLine {
                account: l.account?,
                amount: l.money()?,
                memo: l.memo(),
            })
        })
        .collect();
    Some(Send::Deposit(
        r.id,
        d.payments.iter().map(|(id, _)| *id).collect(),
        fees,
    ))
}

/// The panel's control-channel state (flags and counts only).
pub(crate) fn state(s: &BankFeedScreen) -> serde_json::Value {
    let row = s.detail.row.and_then(|id| s.row(id));
    let deposit = row.map(|r| s.detail.deposit_draft(r));
    let split = row.and_then(|r| s.detail.split_draft(r));
    serde_json::json!({
        "mode": s.detail.mode.as_str(),
        "finding_account": s.detail.find.is_some(),
        "account_chosen": s.detail.account.is_some(),
        "transfer_to_chosen": s.detail.transfer_to.is_some(),
        "transfer_picked": s.detail.pick_transfer.is_some(),
        "candidates": s.detail.data.candidates.len(),
        "payroll": s.detail.data.payroll.len(),
        "transfers": s.detail.data.transfers.len(),
        "groupings": s.detail.data.groupings.len(),
        "payments": s.detail.data.payments.len(),
        "held_payments": s.detail.data.held.len(),
        "picked": s.detail.picks.len(),
        "fees": s.detail.fees.len(),
        "split_lines": s.detail.split.len(),
        "split_complete": split.as_ref().is_some_and(SplitDraft::is_complete),
        "deposit_difference": deposit.as_ref().and_then(DepositDraft::difference).map(Money::to_wire),
        "deposit_balanced": deposit.as_ref().is_some_and(DepositDraft::is_balanced),
        "forbidden": s.detail.data.forbidden,
        "duplicate": row.is_some_and(|r| r.possible_duplicate),
        "entry_voided": row.is_some_and(|r| r.entry_voided),
    })
}
