//! The window manager: one tab per open window, Ctrl+Tab cycling, dirty
//! marks, close prompts, a saved layout per company, and pop-out to separate
//! operating-system windows (Q-57).
//!
//! The manager is plain data and logic; drawing (the tab strip, the pop-out
//! viewports, the prompts) lives in `shell`. A window shows a [`Screen`];
//! later phases add real screens, and until then a [`Placeholder`] stands in.

pub mod doc_viewer;

use std::collections::VecDeque;

use cl_model::{CommandId, Scope, commands};
use eframe::egui::Ui;
use serde::{Deserialize, Serialize};

use crate::i18n::{t, t_with};

/// What a window shows. Implemented by every screen.
pub trait Screen {
    /// Tab and title text.
    fn title(&self) -> String;

    /// Draw the window body.
    fn ui(&mut self, ui: &mut Ui);

    /// True when there are unsaved edits (shows the dot, triggers the close
    /// prompt).
    fn dirty(&self) -> bool {
        false
    }

    /// True while a save of this window ended with an unknown outcome: the
    /// close prompt then says it may already be in the books (review
    /// fix-PR-1 B2).
    fn save_unconfirmed(&self) -> bool {
        false
    }

    /// The keymap scope while this window has focus. Forms return
    /// [`Scope::Form`] so form shortcuts (Alt+N, Esc, Ctrl+Insert) apply.
    fn scope(&self) -> Scope {
        Scope::Window
    }

    /// Run a window or form command (`form.save_new`, `edit.undo`, ...).
    /// Returns true when the screen handled it.
    fn command(&mut self, _id: CommandId) -> bool {
        false
    }

    /// Save for the close prompt. Returns true when it saved (the window may
    /// close) and false when it could not (the window stays open).
    fn save(&mut self) -> bool {
        true
    }

    /// Load a saved transaction into a fresh form (`CompanyEvent::OpenRecord`,
    /// P14). Returns true when this screen shows saved records; the shell
    /// then opens it as a new window.
    fn open_record(&mut self, _txn: cl_core::ids::TransactionId) -> bool {
        false
    }

    /// Fill a fresh form from a template (`CompanyEvent::OpenTemplate`,
    /// P15; the form's `TemplateSource::fill_from_template`). Returns true when
    /// this screen takes templates; the shell then opens it as a new
    /// window.
    fn open_template(&mut self, _template: crate::forms::template::TemplateOpen) -> bool {
        false
    }

    /// Start a fresh window from what another screen hands over
    /// (`CompanyEvent::OpenWith`, P17): a document for the full-size tab, a
    /// prefilled bill, a receipt-inbox file. Returns true when this screen
    /// takes it; the shell then opens it as a new window.
    fn open_with(&mut self, _payload: OpenPayload) -> bool {
        false
    }

    /// Screen state for the control channel (`state.screen`); no amounts,
    /// names or secrets.
    fn state(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    /// True when the screen lays itself out with [`crate::layout::FixedFrame`]
    /// (its bars stay put and only its body scrolls, Q-265): the shell then
    /// gives it the window's rectangle instead of scrolling it whole.
    fn fixed_layout(&self) -> bool {
        false
    }

    /// True when a text field of this window keeps `s` (one of the keys
    /// [`crate::input::reserved_by_text_fields`] names: Ctrl+A, C, V, X,
    /// Delete) for itself this frame, so the shell does not run its
    /// shortcut. The default: whenever any widget has the keyboard. A grid
    /// screen says false for Ctrl+A when its cell editor has nothing left to
    /// select, so Select All Rows (`Scope::Grid`) reaches it.
    fn keeps_text_key(&self, ctx: &eframe::egui::Context, s: cl_model::Shortcut) -> bool {
        let _ = s;
        ctx.egui_wants_keyboard_input()
    }

    /// The document this window shows, if any (A02): the pop-out viewer
    /// follows the window in front through it, each frame. A new
    /// [`doc_viewer::DocKey`] opens the new document there.
    fn document(&self) -> Option<doc_viewer::DocRef> {
        None
    }

    /// A04 WP-C: the compare tools the pop-out viewer draws over this
    /// window's document, each frame (a form in draft mode answers its
    /// `CompareTools::view`). `None`: a plain viewer.
    fn compare(&self) -> Option<crate::widgets::compare::CompareView> {
        None
    }

    /// A04 WP-C: an event of the compare tools in the pop-out viewer (a
    /// word clicked, a box dragged, a target clicked, "Looks right",
    /// Highlights), delivered through the shell's outbox the frame after.
    /// The form runs it through `CompareTools::on_event`, as it does its
    /// docked viewer's events.
    fn on_viewer_event(&mut self, _event: crate::widgets::compare::CompareEvent) {}
}

/// What one screen hands another to open with (P17, [`Screen::open_with`]).
/// Each payload's `Debug` is redacted: file names, refs and amounts are
/// owner data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenPayload {
    /// A document in its own tab ("Open full size", `nav.document_view`).
    Document(crate::widgets::pdf_viewer::DocSource),
    /// A fresh Enter Bill or vendor credit filled from a vendor statement
    /// line ("Create bill" / "Create credit", `vendors.enter_bill`).
    BillPrefill(crate::screens::bill::BillPrefill),
    /// A receipt-inbox file already uploaded as the new form's draft
    /// ("New bill" on `vendors.enter_bill`, "New check" on
    /// `vendors.write_check`).
    InboxDraft(crate::screens::receipt_inbox::InboxDraft),
    /// A04 WP-F2: a document draft opened in its form in draft mode
    /// (`screens::doc_review::review::open`; Enter Bill, Record Card
    /// Charge, Write Check).
    DocumentDraft(crate::screens::doc_review::review::DraftOpen),
    /// A06: a statement draft (U-94) opened in Match Vendor Statement
    /// (`nav.statement_match`), its file read there.
    StatementDraft(cl_api::DocumentDraftId),
    /// P19: checks to print now (Write Check / Pay Bills `Print`, Reprint
    /// check): File > Print Forms opens on their preview.
    PrintChecks(Vec<cl_core::ids::TransactionId>),
    /// P19: Reprint check (Q-P19ab): the printed check goes back to the
    /// queue (closing-date password when closed) and previews with the
    /// next number.
    ReprintCheck(cl_core::ids::TransactionId, cl_core::CalendarDate),
    /// P19 (Q-P19z): a saved deposit's summary, previewed in File > Print
    /// Forms. The summary is the saved record's, with the names Make
    /// Deposit already resolved (Print Forms has no name lists).
    PrintDeposit(
        cl_core::ids::TransactionId,
        Box<cl_model::forms::deposit::DepositSummary>,
    ),
    /// P21: Receive Payment from this client (Client Center's action).
    ReceiveFrom(cl_core::ids::ContactId),
    /// P22: Refund a Credit for this client with this credit picked
    /// (Receive Payment's "Refund it", a credit memo's Refund).
    RefundFrom(
        cl_core::ids::ContactId,
        cl_model::forms::refund::RefundSourceChoice,
    ),
    /// P22: a new document of the window's kind for this client (Client
    /// Center's "New credit memo", "Refund").
    ForClient(cl_core::ids::ContactId),
    /// P22 (P21 follow-up 3): Receive Payment opens this saved payment and
    /// its bounced-check flow (Client Center's "Record bounced check").
    BounceCheck(cl_core::ids::TransactionId),
    /// P22: Assess Finance Charges on this date (Statements' "Assess
    /// finance charges first?").
    OnDate(cl_core::CalendarDate),
}

/// Stand-in for a screen a later phase builds.
#[derive(Debug, Clone)]
pub struct Placeholder {
    kind: CommandId,
    title: String,
    arrives: Option<&'static str>,
    dirty: bool,
}

impl Placeholder {
    /// A placeholder for the command `kind`.
    #[must_use]
    pub fn for_command(kind: CommandId) -> Placeholder {
        let cmd = commands::find(kind.as_str());
        Placeholder {
            kind,
            title: cmd.map_or_else(|| kind.to_string(), |c| t(&format!("cmd.{}", c.id))),
            arrives: cmd.and_then(|c| c.arrives),
            dirty: false,
        }
    }

    /// Mark the placeholder as having unsaved edits (demo data and tests).
    #[must_use]
    pub fn with_dirty(mut self, dirty: bool) -> Placeholder {
        self.dirty = dirty;
        self
    }

    /// Replace the tab title (demo data and tests).
    #[must_use]
    pub fn with_title(mut self, title: &str) -> Placeholder {
        self.title = title.to_string();
        self
    }

    /// The command id this stands in for.
    #[must_use]
    pub fn kind(&self) -> CommandId {
        self.kind
    }

    /// The phase that builds the real screen (`None` for a built command
    /// shown without a company).
    #[must_use]
    pub fn arrives(&self) -> Option<&'static str> {
        self.arrives
    }
}

impl Screen for Placeholder {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = crate::theme::Tokens::of(ui.ctx());
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.heading(&self.title);
        });
        ui.add_space(6.0);
        ui.label(
            eframe::egui::RichText::new(t("window.placeholder.coming"))
                .color(tokens.text_secondary),
        );
    }

    fn dirty(&self) -> bool {
        self.dirty
    }
}

/// Identifies one window for its whole life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(u64);

impl WindowId {
    /// The number (egui viewport ids and tests use it).
    #[must_use]
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// One open window.
pub struct Window {
    /// Identity.
    pub id: WindowId,
    /// The command that opened it.
    pub kind: CommandId,
    /// What it shows.
    pub screen: Box<dyn Screen>,
    /// Shown in its own operating-system window instead of a tab body.
    pub popped_out: bool,
}

impl Window {
    /// Title, from the screen.
    #[must_use]
    pub fn title(&self) -> String {
        self.screen.title()
    }

    /// Unsaved edits.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.screen.dirty()
    }

    /// A save of this window may already be in the books.
    #[must_use]
    pub fn save_unconfirmed(&self) -> bool {
        self.screen.save_unconfirmed()
    }
}

/// Why a window is being closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prompt {
    /// The dirty window.
    pub window: WindowId,
    /// Quitting the app (a Keep answer cancels the quit).
    pub quitting: bool,
}

/// The owner's answer to a close prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAnswer {
    /// Save, then close.
    Save,
    /// Throw the edits away and close.
    Discard,
    /// Do not close.
    Keep,
}

/// Result of asking to close a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    /// It closed.
    Closed,
    /// It has unsaved edits: show the prompt.
    Prompt,
    /// No such window.
    Unknown,
}

/// Where a quit request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitProgress {
    /// Nothing left to ask; close the application.
    Ready,
    /// Show this prompt first.
    Prompt(Prompt),
}

/// The tabs, in order, and which one is in front.
#[derive(Default)]
pub struct Windows {
    items: Vec<Window>,
    active: Option<WindowId>,
    next_id: u64,
    prompt: Option<Prompt>,
    quitting: bool,
}

impl Windows {
    /// No windows open.
    #[must_use]
    pub fn new() -> Windows {
        Windows::default()
    }

    /// All windows in tab order.
    #[must_use]
    pub fn all(&self) -> &[Window] {
        &self.items
    }

    /// Mutable access (drawing bodies).
    pub fn all_mut(&mut self) -> &mut [Window] {
        &mut self.items
    }

    /// Number of open windows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when nothing is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The window in front.
    #[must_use]
    pub fn active_id(&self) -> Option<WindowId> {
        self.active
    }

    /// The window in front.
    #[must_use]
    pub fn active(&self) -> Option<&Window> {
        self.active.and_then(|id| self.get(id))
    }

    /// The window in front, mutable.
    pub fn active_mut(&mut self) -> Option<&mut Window> {
        let id = self.active?;
        self.get_mut(id)
    }

    /// Look a window up.
    #[must_use]
    pub fn get(&self, id: WindowId) -> Option<&Window> {
        self.items.iter().find(|w| w.id == id)
    }

    /// Look a window up, mutable.
    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.items.iter_mut().find(|w| w.id == id)
    }

    /// Position of a window among the tabs.
    #[must_use]
    pub fn index_of(&self, id: WindowId) -> Option<usize> {
        self.items.iter().position(|w| w.id == id)
    }

    /// The window already open for `kind`, if any.
    #[must_use]
    pub fn find_kind(&self, kind: CommandId) -> Option<WindowId> {
        self.items.iter().find(|w| w.kind == kind).map(|w| w.id)
    }

    /// Open a new window and bring it to the front.
    pub fn open_new(&mut self, kind: CommandId, screen: Box<dyn Screen>) -> WindowId {
        self.next_id += 1;
        let id = WindowId(self.next_id);
        self.items.push(Window {
            id,
            kind,
            screen,
            popped_out: false,
        });
        self.active = Some(id);
        id
    }

    /// Bring the existing window for `kind` to the front, or open one with
    /// `make`. A popped-out window is only raised (it keeps its own window).
    pub fn open_or_focus(
        &mut self,
        kind: CommandId,
        make: impl FnOnce() -> Box<dyn Screen>,
    ) -> WindowId {
        if let Some(id) = self.find_kind(kind) {
            self.activate(id);
            id
        } else {
            self.open_new(kind, make())
        }
    }

    /// Bring a window to the front.
    pub fn activate(&mut self, id: WindowId) {
        if self.get(id).is_some() {
            self.active = Some(id);
        }
    }

    /// Ctrl+Tab: the next tab, wrapping.
    pub fn next(&mut self) {
        self.step(1);
    }

    /// Ctrl+Shift+Tab: the previous tab, wrapping.
    pub fn prev(&mut self) {
        self.step(-1);
    }

    fn step(&mut self, delta: isize) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let cur = self.active.and_then(|id| self.index_of(id)).unwrap_or(0);
        let n_signed = isize::try_from(n).unwrap_or(isize::MAX);
        let cur_signed = isize::try_from(cur).unwrap_or(0);
        let next = (cur_signed + delta).rem_euclid(n_signed);
        if let Ok(i) = usize::try_from(next) {
            self.active = self.items.get(i).map(|w| w.id);
        }
    }

    /// Move a window out into its own operating-system window.
    pub fn pop_out(&mut self, id: WindowId) {
        if let Some(w) = self.get_mut(id) {
            w.popped_out = true;
        }
    }

    /// Put a popped-out window back as a tab.
    pub fn dock(&mut self, id: WindowId) {
        if let Some(w) = self.get_mut(id) {
            w.popped_out = false;
        }
        self.activate(id);
    }

    /// Close without asking (after a prompt answer, or for a clean window).
    pub fn close_now(&mut self, id: WindowId) {
        let Some(i) = self.index_of(id) else { return };
        self.items.remove(i);
        if self.prompt.is_some_and(|p| p.window == id) {
            self.prompt = None;
        }
        if self.active == Some(id) {
            // The neighbour to the left, else the new first tab.
            let pick = i.saturating_sub(1).min(self.items.len().saturating_sub(1));
            self.active = self.items.get(pick).map(|w| w.id);
        }
    }

    /// Ask to close a window: clean windows close, dirty ones raise a prompt.
    pub fn request_close(&mut self, id: WindowId) -> CloseOutcome {
        match self.get(id) {
            None => CloseOutcome::Unknown,
            Some(w) if w.dirty() => {
                self.activate(id);
                self.prompt = Some(Prompt {
                    window: id,
                    quitting: false,
                });
                CloseOutcome::Prompt
            }
            Some(_) => {
                self.close_now(id);
                CloseOutcome::Closed
            }
        }
    }

    /// The prompt on screen, if any.
    #[must_use]
    pub fn prompt(&self) -> Option<Prompt> {
        self.prompt
    }

    /// Windows with unsaved edits.
    #[must_use]
    pub fn dirty_ids(&self) -> Vec<WindowId> {
        self.items
            .iter()
            .filter(|w| w.dirty())
            .map(|w| w.id)
            .collect()
    }

    /// Apply the owner's answer to the current prompt. Returns the quit
    /// progress when the prompt was part of a quit.
    pub fn answer_prompt(&mut self, answer: PromptAnswer) -> Option<QuitProgress> {
        let prompt = self.prompt.take()?;
        match answer {
            PromptAnswer::Keep => {
                self.quitting = false;
                return None;
            }
            PromptAnswer::Discard => self.close_now(prompt.window),
            PromptAnswer::Save => {
                let saved = self.get_mut(prompt.window).is_none_or(|w| w.screen.save());
                if saved {
                    self.close_now(prompt.window);
                } else {
                    // Could not save: stay open, and a quit is called off.
                    self.quitting = false;
                    return None;
                }
            }
        }
        prompt.quitting.then(|| self.continue_quit())
    }

    /// Start quitting: prompt for each dirty window in turn.
    pub fn begin_quit(&mut self) -> QuitProgress {
        self.quitting = true;
        self.continue_quit()
    }

    fn continue_quit(&mut self) -> QuitProgress {
        match self.dirty_ids().first().copied() {
            Some(id) => {
                self.activate(id);
                let p = Prompt {
                    window: id,
                    quitting: true,
                };
                self.prompt = Some(p);
                QuitProgress::Prompt(p)
            }
            None => QuitProgress::Ready,
        }
    }

    /// True between `begin_quit` and its end.
    #[must_use]
    pub fn is_quitting(&self) -> bool {
        self.quitting
    }

    /// The layout to remember for a company.
    #[must_use]
    pub fn snapshot(&self) -> SessionLayout {
        SessionLayout {
            version: SessionLayout::VERSION,
            // Developer-only windows are never remembered (S8).
            tabs: self
                .items
                .iter()
                .filter(|w| !w.kind.as_str().starts_with("dev."))
                .map(|w| SavedTab {
                    kind: w.kind.as_str().to_string(),
                    popped_out: w.popped_out,
                })
                .collect(),
            active: self
                .active
                .and_then(|id| {
                    self.items
                        .iter()
                        .filter(|w| !w.kind.as_str().starts_with("dev."))
                        .position(|w| w.id == id)
                })
                .unwrap_or_default(),
        }
    }

    /// Reopen a remembered layout with `make` building each screen. Unknown
    /// kinds (a command renamed or removed) are skipped. Replaces what is open
    /// only when nothing is dirty; returns false otherwise.
    pub fn restore(
        &mut self,
        layout: &SessionLayout,
        make: &dyn Fn(CommandId) -> Box<dyn Screen>,
    ) -> bool {
        if !self.dirty_ids().is_empty() {
            return false;
        }
        self.items.clear();
        self.active = None;
        self.prompt = None;
        let mut order: VecDeque<WindowId> = VecDeque::new();
        for tab in &layout.tabs {
            let Some(cmd) = commands::find(&tab.kind) else {
                continue;
            };
            if !cmd.opens || self.find_kind(cmd.id).is_some() {
                continue;
            }
            let id = self.open_new(cmd.id, make(cmd.id));
            if tab.popped_out {
                self.pop_out(id);
            }
            order.push_back(id);
        }
        let want = layout.active.min(order.len().saturating_sub(1));
        self.active = order.get(want).copied();
        true
    }
}

/// Saved tab layout (no screen content, no unsaved edits).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLayout {
    /// Format version.
    pub version: u32,
    /// Tabs in order.
    pub tabs: Vec<SavedTab>,
    /// Index of the tab in front.
    pub active: usize,
}

impl SessionLayout {
    /// Current format version.
    pub const VERSION: u32 = 1;

    /// Parse saved text; anything unreadable or from a newer format is
    /// `None` (the app then starts with Home only).
    #[must_use]
    pub fn parse(text: &str) -> Option<SessionLayout> {
        let l: SessionLayout = serde_json::from_str(text).ok()?;
        (l.version == SessionLayout::VERSION && l.tabs.len() <= 64).then_some(l)
    }

    /// Text to store.
    #[must_use]
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// One saved tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedTab {
    /// Command id that opens it.
    pub kind: String,
    /// Was in its own window.
    pub popped_out: bool,
}

/// Text for the close prompt.
#[must_use]
pub fn prompt_message(title: &str) -> String {
    t_with("window.prompt.message", &[("title", title)])
}

/// The close prompt's words for a window whose last save could not be
/// confirmed: it may already be in the books.
#[must_use]
pub fn prompt_message_unconfirmed(title: &str) -> String {
    t_with("window.prompt.message_unconfirmed", &[("title", title)])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy {
        title: &'static str,
        dirty: bool,
        saves: bool,
    }

    impl Screen for Dummy {
        fn title(&self) -> String {
            self.title.to_string()
        }
        fn ui(&mut self, _ui: &mut Ui) {}
        fn dirty(&self) -> bool {
            self.dirty
        }
        fn save(&mut self) -> bool {
            if self.saves {
                self.dirty = false;
            }
            self.saves
        }
    }

    fn dummy(title: &'static str, dirty: bool) -> Box<dyn Screen> {
        Box::new(Dummy {
            title,
            dirty,
            saves: true,
        })
    }

    fn id(s: &'static str) -> CommandId {
        commands::find(s).expect("known command").id
    }

    fn three() -> (Windows, WindowId, WindowId, WindowId) {
        let mut w = Windows::new();
        let a = w.open_new(id("nav.home"), dummy("Home", false));
        let b = w.open_new(id("vendors.write_check"), dummy("Write Check", true));
        let c = w.open_new(id("banking.register"), dummy("Register", false));
        (w, a, b, c)
    }

    #[test]
    fn opening_brings_to_front_and_refocuses_existing_kinds() {
        let (mut w, a, _b, c) = three();
        assert_eq!(w.active_id(), Some(c));
        let again = w.open_or_focus(id("nav.home"), || dummy("Home again", false));
        assert_eq!(again, a);
        assert_eq!(w.len(), 3);
        assert_eq!(w.active_id(), Some(a));
    }

    #[test]
    fn ctrl_tab_cycles_forward_and_back_with_wrap() {
        let (mut w, a, b, c) = three();
        w.activate(a);
        w.next();
        assert_eq!(w.active_id(), Some(b));
        w.next();
        w.next();
        assert_eq!(w.active_id(), Some(a), "wraps");
        w.prev();
        assert_eq!(w.active_id(), Some(c), "wraps backwards");
        let mut empty = Windows::new();
        empty.next();
        assert_eq!(empty.active_id(), None);
    }

    #[test]
    fn clean_windows_close_and_focus_moves_left() {
        let (mut w, a, b, c) = three();
        assert_eq!(w.request_close(c), CloseOutcome::Closed);
        assert_eq!(w.active_id(), Some(b));
        w.close_now(a);
        assert_eq!(w.active_id(), Some(b));
        assert_eq!(w.request_close(WindowId(999)), CloseOutcome::Unknown);
    }

    #[test]
    fn dirty_windows_prompt_and_each_answer_works() {
        for (answer, closed) in [
            (PromptAnswer::Save, true),
            (PromptAnswer::Discard, true),
            (PromptAnswer::Keep, false),
        ] {
            let (mut w, _a, b, _c) = three();
            assert_eq!(w.request_close(b), CloseOutcome::Prompt);
            assert_eq!(w.active_id(), Some(b), "the dirty window comes forward");
            assert_eq!(w.prompt().map(|p| p.window), Some(b));
            assert_eq!(w.answer_prompt(answer), None);
            assert_eq!(w.get(b).is_none(), closed, "{answer:?}");
            assert_eq!(w.prompt(), None);
        }
    }

    #[test]
    fn a_failed_save_keeps_the_window_open() {
        let mut w = Windows::new();
        let x = w.open_new(
            id("vendors.write_check"),
            Box::new(Dummy {
                title: "x",
                dirty: true,
                saves: false,
            }),
        );
        assert_eq!(w.request_close(x), CloseOutcome::Prompt);
        w.answer_prompt(PromptAnswer::Save);
        assert!(w.get(x).is_some());
    }

    #[test]
    fn quit_walks_through_dirty_windows_and_keep_cancels() {
        let mut w = Windows::new();
        let a = w.open_new(id("vendors.write_check"), dummy("A", true));
        let b = w.open_new(id("banking.register"), dummy("B", true));
        let QuitProgress::Prompt(p) = w.begin_quit() else {
            panic!("dirty windows must prompt");
        };
        assert!(p.quitting);
        assert_eq!(p.window, a);
        let next = w.answer_prompt(PromptAnswer::Discard);
        assert_eq!(
            next,
            Some(QuitProgress::Prompt(Prompt {
                window: b,
                quitting: true
            }))
        );
        assert_eq!(
            w.answer_prompt(PromptAnswer::Save),
            Some(QuitProgress::Ready)
        );
        assert!(w.is_empty());

        let mut w = Windows::new();
        w.open_new(id("vendors.write_check"), dummy("A", true));
        w.begin_quit();
        assert!(w.is_quitting());
        w.answer_prompt(PromptAnswer::Keep);
        assert!(!w.is_quitting());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn quit_with_nothing_dirty_is_ready() {
        let mut w = Windows::new();
        w.open_new(id("nav.home"), dummy("Home", false));
        assert_eq!(w.begin_quit(), QuitProgress::Ready);
    }

    #[test]
    fn pop_out_and_dock() {
        let (mut w, a, _b, _c) = three();
        w.pop_out(a);
        assert!(w.get(a).unwrap().popped_out);
        w.dock(a);
        assert!(!w.get(a).unwrap().popped_out);
        assert_eq!(w.active_id(), Some(a));
    }

    #[test]
    fn layout_round_trips_and_restores_without_unknown_kinds() {
        let (mut w, a, _b, _c) = three();
        w.pop_out(a);
        w.activate(w.all()[1].id);
        let layout = w.snapshot();
        let text = layout.to_text();
        let parsed = SessionLayout::parse(&text).expect("round trip");
        assert_eq!(parsed, layout);

        let mut fresh = Windows::new();
        let mut with_unknown = parsed.clone();
        with_unknown.tabs.insert(
            1,
            SavedTab {
                kind: "gone.command".into(),
                popped_out: false,
            },
        );
        let restored = fresh.restore(&with_unknown, &|k| {
            Box::new(Placeholder::for_command(k)) as Box<dyn Screen>
        });
        assert!(restored);
        assert_eq!(fresh.len(), 3, "unknown kind skipped, others restored");
        assert!(fresh.all()[0].popped_out);
        assert_eq!(
            fresh.active().map(|w| w.kind.as_str()),
            Some("vendors.write_check")
        );
    }

    #[test]
    fn restore_refuses_to_replace_unsaved_windows() {
        let (mut w, ..) = three();
        let layout = SessionLayout {
            version: SessionLayout::VERSION,
            tabs: vec![],
            active: 0,
        };
        assert!(!w.restore(&layout, &|k| Box::new(Placeholder::for_command(k))));
        assert_eq!(w.len(), 3);
    }

    #[test]
    fn unreadable_layouts_are_ignored() {
        assert!(SessionLayout::parse("not json").is_none());
        assert!(SessionLayout::parse(r#"{"version":99,"tabs":[],"active":0}"#).is_none());
        assert!(SessionLayout::parse("").is_none());
    }

    #[test]
    fn placeholder_titles_come_from_the_command_table() {
        let p = Placeholder::for_command(id("clients.statements"));
        assert_eq!(p.title(), "Statements");
        assert!(!p.dirty());
        assert!(
            Placeholder::for_command(id("clients.statements"))
                .with_dirty(true)
                .dirty()
        );
    }
}
