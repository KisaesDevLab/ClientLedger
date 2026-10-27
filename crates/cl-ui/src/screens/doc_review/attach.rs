//! Attach a document to a transaction already in the books (A04 D3,
//! Q-282, Q-A04e, review M5). Used by the draft forms' Accept
//! ([`super::review::DraftSession::accept`]):
//!
//! - **The search** ([`search`]): the register's posted records whose total
//!   is exactly the document's (`GET /transactions?amount=`, U-95 c), dated
//!   within ±7 days, paged to the end, **without** an account filter (it
//!   would hide expenses paid from another account); a second query with
//!   the document's account only marks `same_account`. The demo answers
//!   from [`crate::company::demo::drafts::search_amount`].
//! - **The rules** are `cl_docread::register_match` (WP-R): exact `Money`,
//!   an allowlist of row kinds by [`DocSide`] (always
//!   [`DocSide::Purchase`] here: Credit is hidden in draft mode), and the
//!   payee gate: a row of **another known payee is never offered**.
//! - **The dialog** ([`AttachDialog`]): the document's date, payee, amount
//!   and account beside each candidate's, status in words. Focus starts on
//!   the list with nothing selected and **Cancel is the default button**,
//!   so Enter never attaches. **Attach to selected** (Alt+T) is disabled
//!   until a pick and then asks once more ("This can't be undone", default
//!   Cancel); **Enter as new** (Alt+E) posts the document acknowledging the
//!   rows shown (`acknowledgeDuplicateOf`, audited by the engine).
//!
//! No undo in A04 (Q-A04e) and no learning after an attach (D5). Nothing
//! here logs; `Debug` shows counts only; the control channel gets counts,
//! labels and flags, never an amount, a date or a name.

use cl_api::{
    ApiError, Capabilities, DuplicateMatch, LocalValidation, MatchSource, PageRequest,
    RequestGroup, TransactionFilter, TransactionSummary, TxnStatus,
};
use cl_core::ids::{AccountId, ContactId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle};
use cl_docread::register_match::{
    self, Candidate, DocSide, MatchLabel, Probe, RegisterRow, RowKind, RowStatus,
};
use cl_model::CommandId;
use cl_model::doc_draft::REGISTER_PAGE;
use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// Register search pages read at most (200 rows each): a busy register
/// still answers, and the exact-amount filter keeps pages few.
const MAX_PAGES: u64 = 25;

/// What the document says at Accept: the form's own values (owner data;
/// `Debug` is redacted).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct MatchProbe {
    /// The total the form would post.
    pub amount: Money,
    /// The form's date.
    pub date: CalendarDate,
    /// The payee's name as shown.
    pub payee: Option<String>,
    /// The payee picked.
    pub contact: Option<ContactId>,
    /// The account it was paid from (card, bank or cash), if any.
    pub account: Option<AccountId>,
    /// That account's name as shown.
    pub account_name: Option<String>,
}

impl std::fmt::Debug for MatchProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchProbe")
            .field("contact", &self.contact.is_some())
            .field("account", &self.account.is_some())
            .finish_non_exhaustive()
    }
}

impl MatchProbe {
    /// The rules' probe: a purchase (Credit is hidden in draft mode).
    #[must_use]
    pub(crate) fn probe(&self) -> Probe {
        Probe {
            amount: self.amount,
            date: self.date,
            payee: self.payee.clone(),
            contact: self.contact,
            side: DocSide::Purchase,
        }
    }
}

/// The engine's snake-case name of a serde enum (`expense`, `posted`).
fn wire<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A register row as the rules take it; `None` for a row without a total.
#[must_use]
pub(crate) fn row_of(s: &TransactionSummary, same_account: bool) -> Option<RegisterRow> {
    let amount = s.total.or(s.display_total)?;
    Some(RegisterRow {
        id: s.id,
        date: s.txn_date,
        amount,
        payee: s.contact_name.clone(),
        contact: s.contact_id,
        kind: RowKind::from_txn_type(&wire(&s.txn_type)),
        status: RowStatus::from_wire(&wire(&s.status)),
        same_account,
    })
}

/// Marks the rows the account query found again.
fn mark_same(rows: &mut [RegisterRow], same: &[TransactionId]) {
    for r in rows {
        r.same_account = same.contains(&r.id);
    }
}

/// How the register search started (review B2: only an engine without the
/// capability posts without one; a search that couldn't run never posts).
pub(crate) enum Search {
    /// The engine can't attach (no `document-drafts-attach-v1`: no exact
    /// filter either); Accept posts as before, the engine's own duplicate
    /// check still runs.
    Unsupported,
    /// The search runs; its answer comes in the task.
    Started(Task<Vec<RegisterRow>>),
    /// The search couldn't be sent (no session, the request queue refused
    /// it, the demo failed): the owner chooses Try again or Enter as new.
    Failed,
}

/// The register search for `p` (see the module comment and [`Search`]).
pub(crate) fn search(
    link: &CompanyLink,
    group: Option<&RequestGroup>,
    tenant: Option<TenantId>,
    p: &MatchProbe,
) -> Search {
    let (start, end) = register_match::search_window(p.date);
    if let Some(mut demo) = link.demo_state() {
        let _ = tenant;
        let q = crate::company::demo::drafts::AmountSearch {
            amount: p.amount,
            start,
            end,
            account: None,
        };
        let all = crate::company::demo::drafts::search_amount(&mut demo, &q);
        let same = match (&all, p.account) {
            (Ok(_), Some(a)) => Some(crate::company::demo::drafts::search_amount(
                &mut demo,
                &crate::company::demo::drafts::AmountSearch {
                    account: Some(a),
                    ..q
                },
            )),
            _ => None,
        };
        drop(demo);
        return match (all, same) {
            // Without drafts the demo has no exact filter.
            (Err(ApiError::Local(LocalValidation::Unsupported)), _) => Search::Unsupported,
            (Err(_), _) | (Ok(_), Some(Err(_))) => Search::Failed,
            (Ok(rows), same) => {
                let same: Vec<TransactionId> = same
                    .and_then(Result::ok)
                    .map(|s| s.iter().map(|r| r.id).collect())
                    .unwrap_or_default();
                let mut rows: Vec<RegisterRow> =
                    rows.iter().filter_map(|r| row_of(r, false)).collect();
                mark_same(&mut rows, &same);
                Search::Started(Task::ready(rows))
            }
        };
    }
    let caps = link.capabilities();
    if !caps.has(Capabilities::DOCUMENT_DRAFTS_ATTACH) {
        return Search::Unsupported;
    }
    let (Some(session), Some(group)) = (link.session(), group) else {
        return Search::Failed;
    };
    let (amount, account) = (p.amount, p.account);
    let started = group
        .read(&session, move |s| async move {
            let filter = TransactionFilter {
                status: Some(TxnStatus::Posted),
                start_date: Some(start),
                end_date: Some(end),
                amount: Some(amount),
                ..TransactionFilter::default()
            };
            let mut rows = Vec::new();
            for page in 0..MAX_PAGES {
                let req = PageRequest::new(
                    u16::try_from(REGISTER_PAGE).unwrap_or(200),
                    page * u64::from(REGISTER_PAGE),
                )?;
                let got = s.transactions().list(&filter, req).await?;
                let n = got.data.len();
                rows.extend(got.data.iter().filter_map(|r| row_of(r, false)));
                if n < REGISTER_PAGE as usize {
                    break;
                }
            }
            if let Some(a) = account {
                let only = TransactionFilter {
                    account_id: Some(a),
                    ..filter
                };
                let req = PageRequest::new(u16::try_from(REGISTER_PAGE).unwrap_or(200), 0)?;
                let same: Vec<TransactionId> = s
                    .transactions()
                    .list(&only, req)
                    .await?
                    .data
                    .iter()
                    .map(|r| r.id)
                    .collect();
                mark_same(&mut rows, &same);
            }
            Ok(rows)
        })
        .ok()
        .map(Task::Pending);
    match started {
        Some(task) => Search::Started(task),
        None => Search::Failed,
    }
}

/// One row the dialog shows (owner data: drawn, never logged).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ShownRow {
    /// The record.
    pub id: TransactionId,
    /// Its date.
    pub date: Option<CalendarDate>,
    /// Its total.
    pub amount: Option<Money>,
    /// Its payee.
    pub payee: Option<String>,
    /// What it is, in words (i18n key).
    pub kind_key: &'static str,
    /// How close (`None` for the engine's own duplicate matches).
    pub label: Option<MatchLabel>,
    /// The document may be attached to it (an unpaid bill a receipt
    /// matched is shown for information only, Q-A04d / P18 hold).
    pub attachable: bool,
}

/// The document's own values, beside the rows.
#[derive(Clone, PartialEq, Eq, Default)]
pub(crate) struct DocSummary {
    pub date: Option<CalendarDate>,
    pub payee: Option<String>,
    pub amount: Option<Money>,
    pub account: Option<String>,
}

/// What the owner chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DialogAnswer {
    /// Attach the document to this record (picked and confirmed).
    Attach(TransactionId),
    /// Post the document as a new record, acknowledging these.
    EnterAsNew(Vec<TransactionId>),
    /// Back to the form; nothing sent.
    Cancel,
}

/// The attach dialog (see the module comment).
#[derive(Clone, Default)]
pub(crate) struct AttachDialog {
    pub rows: Vec<ShownRow>,
    pub doc: DocSummary,
    /// The picked row; nothing at first (Enter never attaches).
    pub selected: Option<usize>,
    /// "Attach this document to …? This can't be undone." is up.
    pub confirming: bool,
    /// The rows came from the engine's duplicate check (409).
    pub from_engine: bool,
    /// The engine can attach (U-95); without it only Enter as new.
    pub attach_ok: bool,
    /// Give the list the keyboard next frame.
    focus_list: bool,
    /// Give Cancel of the confirmation the keyboard next frame.
    focus_confirm_cancel: bool,
}

impl std::fmt::Debug for AttachDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachDialog")
            .field("rows", &self.rows.len())
            .field("selected", &self.selected)
            .field("confirming", &self.confirming)
            .finish_non_exhaustive()
    }
}

/// The non-empty parts joined with " · ".
fn joined(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

/// The text with its first letter upper case (a row starts a sentence).
fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The words for a row kind.
fn kind_key(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Expense => "doc_review.attach.kind.expense",
        RowKind::Check => "doc_review.attach.kind.check",
        RowKind::CardCharge => "doc_review.attach.kind.card_charge",
        RowKind::CardCredit => "doc_review.attach.kind.card_credit",
        RowKind::Bill => "doc_review.attach.kind.bill",
        RowKind::Deposit | RowKind::JournalEntry | RowKind::BillPayment | RowKind::Other => {
            "doc_review.attach.kind.other"
        }
    }
}

impl AttachDialog {
    fn summary(p: &MatchProbe) -> DocSummary {
        DocSummary {
            date: Some(p.date),
            payee: p.payee.clone(),
            amount: Some(p.amount),
            account: p.account_name.clone(),
        }
    }

    /// The register candidates for `p` (already ranked and capped by
    /// `register_match::candidates`).
    #[must_use]
    pub(crate) fn from_candidates(
        p: &MatchProbe,
        rows: &[RegisterRow],
        cands: &[Candidate],
    ) -> AttachDialog {
        let shown = cands
            .iter()
            .filter_map(|c| {
                let r = rows.get(c.row)?;
                Some(ShownRow {
                    id: r.id,
                    date: Some(r.date),
                    amount: Some(r.amount),
                    payee: r.payee.clone(),
                    kind_key: kind_key(r.kind),
                    label: Some(c.label),
                    attachable: true,
                })
            })
            .collect();
        AttachDialog {
            rows: shown,
            doc: Self::summary(p),
            attach_ok: true,
            focus_list: true,
            ..AttachDialog::default()
        }
    }

    /// The engine's duplicate check answered 409 `POSSIBLE_DUPLICATE`
    /// (`matches` block; other live drafts are not records and are not
    /// listed). A row may take the file only when it is the same purchase
    /// by the register rules (review S1): its total exactly the document's,
    /// dated inside the search window, and of an allowed kind (a bill for a
    /// bill document; an expense for a receipt). The engine also matches by
    /// reference alone, ignoring total and date: such rows are shown for
    /// information only.
    #[must_use]
    pub(crate) fn from_duplicates(
        p: &MatchProbe,
        matches: &[DuplicateMatch],
        bill_doc: bool,
        attach_ok: bool,
    ) -> AttachDialog {
        let probe = p.probe();
        let (start, end) = register_match::search_window(p.date);
        let shown = matches
            .iter()
            .filter_map(|m| {
                let MatchSource::Transaction(id) = m.source else {
                    return None;
                };
                let kind = RowKind::from_txn_type(m.txn_type.as_deref().unwrap_or(""));
                let kind_ok = if kind == RowKind::Bill {
                    bill_doc
                } else {
                    kind.matchable(probe.side)
                };
                let same_total = m.total == Some(p.amount);
                let in_window = m.date.is_some_and(|d| d >= start && d <= end);
                let attachable = kind_ok && same_total && in_window;
                Some(ShownRow {
                    id,
                    date: m.date,
                    amount: m.total,
                    payee: None,
                    kind_key: kind_key(kind),
                    label: None,
                    attachable,
                })
            })
            .collect();
        AttachDialog {
            rows: shown,
            doc: Self::summary(p),
            from_engine: true,
            attach_ok,
            focus_list: true,
            ..AttachDialog::default()
        }
    }

    /// The ids shown (what Enter as new acknowledges).
    #[must_use]
    pub(crate) fn ids(&self) -> Vec<TransactionId> {
        self.rows.iter().map(|r| r.id).collect()
    }

    fn picked(&self) -> Option<&ShownRow> {
        self.selected
            .and_then(|i| self.rows.get(i))
            .filter(|r| r.attachable && self.attach_ok)
    }

    /// Picks row `i` (a click, or the keyboard on the list).
    pub(crate) fn select(&mut self, i: usize) {
        if i < self.rows.len() {
            self.selected = Some(i);
            self.confirming = false;
        }
    }

    /// **Attach to selected**: asks for the confirmation; nothing without a
    /// pick.
    pub(crate) fn ask_attach(&mut self) -> bool {
        if self.picked().is_some() {
            self.confirming = true;
            self.focus_confirm_cancel = true;
            true
        } else {
            false
        }
    }

    /// The confirmation's Attach.
    pub(crate) fn confirm(&mut self) -> Option<DialogAnswer> {
        if !self.confirming {
            return None;
        }
        let id = self.picked()?.id;
        self.confirming = false;
        Some(DialogAnswer::Attach(id))
    }

    /// The form's keys while the dialog is up: Alt+T (Attach to selected,
    /// then the confirmation's own buttons) and Alt+E (Enter as new).
    /// `None` when the key did nothing here.
    #[allow(
        clippy::option_option,
        reason = "None: not this dialog's key; Some(None): taken, no answer yet"
    )]
    pub(crate) fn command(&mut self, id: CommandId) -> Option<Option<DialogAnswer>> {
        match id.as_str() {
            "form.attach_selected" if !self.confirming => {
                self.ask_attach();
                Some(None)
            }
            "form.enter_as_new" if !self.confirming => {
                Some(Some(DialogAnswer::EnterAsNew(self.ids())))
            }
            "form.revert" => {
                if self.confirming {
                    self.confirming = false;
                    Some(None)
                } else {
                    Some(Some(DialogAnswer::Cancel))
                }
            }
            _ => None,
        }
    }

    /// Draws the dialog; the owner's answer once.
    pub(crate) fn ui(&mut self, ctx: &egui::Context, style: MoneyStyle) -> Option<DialogAnswer> {
        let tokens = Tokens::of(ctx);
        let mut answer = None;
        let width = (ctx.content_rect().width() - 48.0).clamp(280.0, 620.0);
        egui::Window::new(t("doc_review.attach.title"))
            .id(egui::Id::unique("a04/attach_dialog"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(width);
                answer = self.body(ui, &tokens, style);
            });
        if self.confirming {
            egui::Window::new(t("doc_review.attach.confirm.title"))
                .id(egui::Id::unique("a04/attach_confirm"))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .collapsible(false)
                .resizable(false)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    ui.set_width((width - 80.0).max(240.0));
                    if let Some(a) = self.confirm_body(ui, style) {
                        answer = Some(a);
                    }
                });
        }
        answer
    }

    fn money(m: Option<Money>, style: MoneyStyle) -> String {
        m.map(|m| m.format(style)).unwrap_or_default()
    }

    fn date(d: Option<CalendarDate>) -> String {
        d.map(crate::shell::us_date).unwrap_or_default()
    }

    fn body(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        style: MoneyStyle,
    ) -> Option<DialogAnswer> {
        let mut answer = None;
        let intro = if self.from_engine {
            "doc_review.attach.intro.engine"
        } else {
            "doc_review.attach.intro"
        };
        ui.add(egui::Label::new(t(intro)).wrap());
        ui.add_space(6.0);
        // The document's own values.
        let d = &self.doc;
        // Only the values there are, no dangling separators (UI item 7).
        let doc_line = t_with(
            "doc_review.attach.document",
            &[(
                "details",
                &joined(&[
                    Self::date(d.date),
                    d.payee.clone().unwrap_or_default(),
                    Self::money(d.amount, style),
                    d.account.clone().unwrap_or_default(),
                ]),
            )],
        );
        ui.add(egui::Label::new(RichText::new(doc_line).strong()).wrap());
        ui.add_space(6.0);
        let focus = std::mem::take(&mut self.focus_list);
        let mut clicked = None;
        for (i, r) in self.rows.iter().enumerate() {
            let label = match r.label {
                Some(MatchLabel::Strong) => t("doc_review.attach.label.strong"),
                Some(MatchLabel::Possible) => t("doc_review.attach.label.possible"),
                None => String::new(),
            };
            let status = match (r.attachable, r.kind_key) {
                (true, _) => t("doc_review.attach.status.posted"),
                (false, "doc_review.attach.kind.bill") => t("doc_review.attach.status.info_only"),
                (false, _) => t("doc_review.attach.status.info_other"),
            };
            let head = t_with(
                "doc_review.attach.row.head",
                &[("kind", &t(r.kind_key)), ("date", &Self::date(r.date))],
            );
            let text = capitalized(&joined(&[
                head,
                r.payee.clone().unwrap_or_default(),
                Self::money(r.amount, style),
                format!("{status} {label}").trim().to_string(),
            ]));
            let resp = ui.selectable_label(self.selected == Some(i), text);
            if focus && i == 0 {
                resp.request_focus();
            }
            if resp.clicked() {
                clicked = Some(i);
            }
        }
        if let Some(i) = clicked {
            self.select(i);
        }
        ui.add_space(8.0);
        if !self.attach_ok {
            ui.add(
                egui::Label::new(
                    RichText::new(t("doc_review.attach.unavailable")).color(tokens.text_secondary),
                )
                .wrap(),
            );
        }
        ui.horizontal_wrapped(|ui| {
            let can_attach = self.picked().is_some() && !self.confirming;
            let r = ui
                .add_enabled(can_attach, egui::Button::new(t("doc_review.attach.attach")))
                .on_hover_text(t("doc_review.attach.attach.tip"));
            crate::widgets::kbd(
                ui,
                &crate::screens::card_charge::kit::chip("form.attach_selected"),
            );
            if r.clicked() {
                self.ask_attach();
            }
            let r = ui
                .button(t("doc_review.attach.enter_new"))
                .on_hover_text(t("doc_review.attach.enter_new.tip"));
            crate::widgets::kbd(
                ui,
                &crate::screens::card_charge::kit::chip("form.enter_as_new"),
            );
            if r.clicked() && !self.confirming {
                answer = Some(DialogAnswer::EnterAsNew(self.ids()));
            }
            // The default button (drawn as one, UI item 8): Enter on it
            // cancels, never attaches.
            let cancel = crate::widgets::primary_button(ui, &t("doc_review.attach.cancel"));
            if cancel.clicked() && !self.confirming {
                answer = Some(DialogAnswer::Cancel);
            }
        });
        // Enter with no button or row focused is Cancel (the default
        // button); Enter on a row only picks it.
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter
            && answer.is_none()
            && !self.confirming
            && ui.ctx().memory(egui::Memory::focused).is_none()
        {
            answer = Some(DialogAnswer::Cancel);
        }
        answer
    }

    fn confirm_body(&mut self, ui: &mut egui::Ui, style: MoneyStyle) -> Option<DialogAnswer> {
        let row = self.picked()?.clone();
        let text = t_with(
            "doc_review.attach.confirm",
            &[
                ("kind", &t(row.kind_key)),
                ("date", &Self::date(row.date)),
                ("amount", &Self::money(row.amount, style)),
            ],
        );
        ui.add(egui::Label::new(text).wrap());
        let mut answer = None;
        ui.horizontal(|ui| {
            if ui.button(t("doc_review.attach.confirm.attach")).clicked() {
                answer = self.confirm();
            }
            let cancel = crate::widgets::primary_button(ui, &t("doc_review.attach.cancel"));
            if std::mem::take(&mut self.focus_confirm_cancel) {
                cancel.request_focus();
            }
            if cancel.clicked() {
                self.confirming = false;
            }
        });
        answer
    }

    /// The control channel's view: counts, labels and flags only.
    #[must_use]
    pub(crate) fn state_json(&self) -> Value {
        json!({
            "rows": self.rows.len(),
            "attachable": self.rows.iter().filter(|r| r.attachable).count(),
            "labels": self.rows.iter().map(|r| match r.label {
                Some(MatchLabel::Strong) => "strong",
                Some(MatchLabel::Possible) => "possible",
                None => "engine",
            }).collect::<Vec<_>>(),
            "selected": self.selected,
            "confirming": self.confirming,
            "from_engine": self.from_engine,
            "attach_ok": self.attach_ok,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn day(d: u32) -> CalendarDate {
        CalendarDate::from_ymd(2026, 3, d).expect("date")
    }

    fn id(n: u8) -> TransactionId {
        TransactionId::parse(&format!("00000000-0000-4000-8000-0000000000{n:02}")).expect("id")
    }

    fn contact(n: u8) -> ContactId {
        ContactId::parse(&format!("00000000-0000-4000-8000-0000000001{n:02}")).expect("id")
    }

    fn row(n: u8, amount: &str, d: u32, c: Option<ContactId>) -> RegisterRow {
        RegisterRow {
            id: id(n),
            date: day(d),
            amount: money(amount),
            payee: None,
            contact: c,
            kind: RowKind::Expense,
            status: RowStatus::Posted,
            same_account: false,
        }
    }

    fn probe(contact: Option<ContactId>) -> MatchProbe {
        MatchProbe {
            amount: money("25.00"),
            date: day(14),
            payee: Some("Corner Hardware".into()),
            contact,
            account: None,
            account_name: None,
        }
    }

    /// T11 (M5): another known payee is never offered; a row with no payee
    /// is; amounts are exact.
    #[test]
    fn another_payee_is_never_offered_and_amounts_are_exact() {
        let p = probe(Some(contact(1)));
        let rows = vec![
            row(1, "25.00", 14, Some(contact(2))),
            row(2, "25.0", 15, None),
            row(3, "25.01", 14, None),
            row(4, "25", 13, Some(contact(1))),
        ];
        let cands = register_match::candidates(&p.probe(), &rows);
        let d = AttachDialog::from_candidates(&p, &rows, &cands);
        let ids = d.ids();
        assert!(!ids.contains(&id(1)), "another known payee");
        assert!(!ids.contains(&id(3)), "a cent off");
        assert!(ids.contains(&id(2)) && ids.contains(&id(4)));
        assert_eq!(d.rows[0].id, id(4), "same payee first");
        assert_eq!(d.selected, None, "nothing picked at first");
        assert!(format!("{d:?}").contains("rows: 2"));
    }

    /// T11: Enter never attaches; Attach needs a pick and a confirmation.
    #[test]
    fn attach_needs_a_pick_and_a_confirmation() {
        let p = probe(None);
        let rows = vec![row(1, "25.00", 14, None)];
        let cands = register_match::candidates(&p.probe(), &rows);
        let mut d = AttachDialog::from_candidates(&p, &rows, &cands);
        // Alt+T with nothing picked does nothing.
        assert_eq!(
            d.command(CommandId::new("form.attach_selected")),
            Some(None)
        );
        assert!(!d.confirming);
        assert_eq!(d.confirm(), None);
        d.select(0);
        assert_eq!(
            d.command(CommandId::new("form.attach_selected")),
            Some(None)
        );
        assert!(d.confirming, "asks first");
        // Esc backs out of the confirmation, then out of the dialog.
        assert_eq!(d.command(CommandId::new("form.revert")), Some(None));
        assert!(!d.confirming);
        assert_eq!(
            d.command(CommandId::new("form.revert")),
            Some(Some(DialogAnswer::Cancel))
        );
        assert!(d.ask_attach());
        assert_eq!(d.confirm(), Some(DialogAnswer::Attach(id(1))));
        assert_eq!(
            d.command(CommandId::new("form.enter_as_new")),
            Some(Some(DialogAnswer::EnterAsNew(vec![id(1)])))
        );
        assert_eq!(d.command(CommandId::new("form.save_close")), None);
        let s = d.state_json();
        assert_eq!(s["rows"], 1);
        assert!(!s.to_string().contains("25"), "no amounts");
    }

    /// A receipt matched to an unpaid bill sees it for information only.
    #[test]
    fn a_bill_is_information_only_for_a_receipt() {
        let m = DuplicateMatch {
            source: MatchSource::Transaction(id(7)),
            reason: cl_api::MatchReason::AmountDate,
            txn_type: Some("bill".into()),
            kind: None,
            date: Some(day(14)),
            total: Some(money("25.00")),
            reference: None,
        };
        let receipt =
            AttachDialog::from_duplicates(&probe(None), std::slice::from_ref(&m), false, true);
        assert!(!receipt.rows[0].attachable);
        let mut receipt = receipt;
        receipt.select(0);
        assert!(!receipt.ask_attach(), "nothing to attach to");
        let bill = AttachDialog::from_duplicates(&probe(None), &[m], true, true);
        assert!(bill.rows[0].attachable);
        assert!(bill.from_engine);
    }

    /// Review S1: an engine match at another total, outside the date
    /// window, without a total or date, or of a kind not allowed is shown
    /// for information only; Attach can't pick it.
    #[test]
    fn engine_matches_attach_only_at_the_exact_total_inside_the_window() {
        let m = |n: u8, kind: &str, d: Option<u32>, total: Option<&str>| DuplicateMatch {
            source: MatchSource::Transaction(id(n)),
            reason: cl_api::MatchReason::Reference,
            txn_type: Some(kind.into()),
            kind: None,
            date: d.map(day),
            total: total.map(money),
            reference: None,
        };
        let matches = [
            m(1, "expense", Some(14), Some("25.00")),
            m(2, "expense", Some(14), Some("25.01")),
            m(3, "expense", Some(22), Some("25.00")),
            m(4, "expense", Some(6), Some("25.00")),
            m(5, "expense", None, Some("25.00")),
            m(6, "expense", Some(14), None),
            m(7, "deposit", Some(14), Some("25.00")),
            m(8, "journal_entry", Some(14), Some("25.00")),
            m(9, "expense", Some(21), Some("25")),
        ];
        let mut d = AttachDialog::from_duplicates(&probe(None), &matches, false, true);
        let ok: Vec<TransactionId> = d
            .rows
            .iter()
            .filter(|r| r.attachable)
            .map(|r| r.id)
            .collect();
        assert_eq!(ok, vec![id(1), id(9)], "exact total, ±7 days, an expense");
        assert_eq!(d.rows.len(), 9, "the rest are still shown");
        d.select(1);
        assert!(!d.ask_attach(), "a cent off can't be attached");
        assert_eq!(d.confirm(), None);
        // A bill document: the bill row follows the same rules.
        let bills = [
            m(10, "bill", Some(14), Some("25.00")),
            m(11, "bill", Some(14), Some("2500.00")),
        ];
        let b = AttachDialog::from_duplicates(&probe(None), &bills, true, true);
        assert!(b.rows[0].attachable);
        assert!(!b.rows[1].attachable, "same reference, another total");
    }

    /// UI check item 7: no dangling separators; a row starts upper case.
    #[test]
    fn rows_and_the_document_line_read_cleanly() {
        assert_eq!(
            joined(&[
                "3/14/2026".into(),
                String::new(),
                "25.00".into(),
                " ".into()
            ]),
            "3/14/2026 \u{b7} 25.00"
        );
        assert_eq!(capitalized("expense of 3/14"), "Expense of 3/14");
        assert_eq!(capitalized(""), "");
    }

    #[test]
    fn a_summary_without_a_total_is_no_row() {
        let v = serde_json::json!({
            "id": "00000000-0000-4000-8000-000000000001",
            "txnType": "expense", "txnDate": "2026-03-14", "status": "posted",
            "total": "25.0000"
        });
        let s: TransactionSummary = serde_json::from_value(v).expect("summary");
        let r = row_of(&s, true).expect("a row");
        assert_eq!(r.amount, money("25"));
        assert_eq!(r.kind, RowKind::Expense);
        assert_eq!(r.status, RowStatus::Posted);
        assert!(r.same_account);
    }
}
