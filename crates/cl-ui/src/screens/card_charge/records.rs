//! Saved records in the P14 lane C helper forms (Record Card Charge, Loan
//! Payment, Pay Yourself or Put Money In, Paid with Personal Funds), on the
//! Write Check pattern: Prev/Next through the saved records of the form's
//! kind (newest first), opening one from the register
//! (`CompanyEvent::OpenRecord` → `Screen::open_record`), and the standard
//! Save / Discard changes / Keep editing question when the form has
//! unsaved edits.
//!
//! A record is always read with `GET /transactions/{id}` (exact amounts,
//! with its version for the stale check); the list only gives ids and
//! dates. The save itself, the version guard and the closing-date password
//! are `kit::SaveFlow`'s.

use std::sync::Arc;

use cl_api::{
    ApiError, PageRequest, RequestGroup, SortDir, Transaction, TransactionFilter, TxnSort, TxnType,
    Versioned,
};
use cl_core::ids::{AccountId, TransactionId};
use eframe::egui::{self, Ui};

use crate::company::{CompanyLink, Task};
use crate::i18n::t;
use crate::windows::PromptAnswer;

/// How many of an account's transactions Prev/Next looks through.
const LIST_PAGE: u16 = 200;

/// Which saved records a form steps through: the transactions of these
/// kinds touching `account` (the card, the loan, the equity or offset
/// account).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecordScope {
    pub account: AccountId,
    pub types: &'static [TxnType],
}

/// What a Prev/Next step or an arrival asks the form to do.
#[derive(Debug)]
pub(crate) enum Step {
    /// Nothing (busy, or waiting for the engine).
    None,
    /// Unsaved edits: the question is open.
    Prompt,
    /// Next past the newest: a new, blank form.
    New,
    /// Prev past the oldest.
    Oldest,
    /// A saved record to show (or why it couldn't be read).
    Record(Box<Result<Versioned<Transaction>, ApiError>>),
}

/// Prev/Next and the record being read.
#[derive(Default)]
pub(crate) struct Records {
    list: Vec<TransactionId>,
    /// The account the list is for.
    list_for: Option<AccountId>,
    /// The list has been read.
    listed: bool,
    list_task: Option<Task<Vec<TransactionId>>>,
    load_task: Option<Task<Versioned<Transaction>>>,
    /// Where the form is in the list (`None`: a new record, or one opened
    /// from elsewhere).
    pub at: Option<usize>,
    want_older: Option<bool>,
    /// Prev (true) or Next asked on unsaved edits: the question is open.
    pub prompt: Option<bool>,
    /// Prev/Next to take once the question's Save is done.
    pub after_save: Option<bool>,
}

/// The transactions of `scope` in the demo engine, newest first.
fn demo_list(link: &CompanyLink, scope: RecordScope) -> Vec<TransactionId> {
    let Some(mut demo) = link.demo_state() else {
        return Vec::new();
    };
    let chart = demo
        .chart
        .get_or_insert_with(crate::company::demo::chart)
        .clone();
    let Some(number) = chart
        .iter()
        .find(|a| a.id == scope.account)
        .and_then(|a| a.fields.account_number.clone())
    else {
        return Vec::new();
    };
    let mut rows: Vec<_> = demo
        .ledger
        .transactions()
        .into_iter()
        .filter(|t| {
            scope
                .types
                .contains(&crate::company::demo::register::txn_type(&t.kind))
                && t.lines.iter().any(|l| l.account == number)
        })
        .map(|t| {
            (
                t.date,
                crate::company::demo::register::DemoLedger::id_of(&t),
            )
        })
        .collect();
    rows.sort_by(|a, b| b.cmp(a));
    rows.into_iter().map(|(_, id)| id).collect()
}

/// One transaction from the demo engine.
fn demo_get(link: &CompanyLink, id: TransactionId) -> Result<Versioned<Transaction>, ApiError> {
    let mut demo = link
        .demo_state()
        .ok_or(ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
    let chart = demo
        .chart
        .get_or_insert_with(crate::company::demo::chart)
        .clone();
    let contacts = demo.contacts_mut().clone();
    demo.ledger
        .get(&chart, &contacts, id)
        .ok_or(ApiError::NotFound { code: None })
}

impl Records {
    /// The list or a record is being read.
    pub(crate) fn busy(&self) -> bool {
        self.list_task.is_some() || self.load_task.is_some()
    }

    /// A record is being read.
    pub(crate) fn loading(&self) -> bool {
        self.load_task.is_some()
    }

    /// Saves and voids change the list: it is read again on the next step.
    pub(crate) fn forget(&mut self) {
        self.list.clear();
        self.listed = false;
    }

    /// A new, blank form: no place in the list.
    pub(crate) fn new_record(&mut self) {
        self.at = None;
    }

    /// Prev (`older`) or Next.
    pub(crate) fn navigate(
        &mut self,
        link: &CompanyLink,
        group: Option<&RequestGroup>,
        scope: Option<RecordScope>,
        older: bool,
        dirty: bool,
        busy: bool,
    ) -> Step {
        if busy || self.busy() {
            return Step::None;
        }
        if dirty {
            self.prompt = Some(older);
            return Step::Prompt;
        }
        let Some(scope) = scope else {
            return Step::None;
        };
        if self.list_for != Some(scope.account) {
            self.list.clear();
            self.listed = false;
            self.at = None;
            self.list_for = Some(scope.account);
        }
        if !self.listed {
            self.want_older = Some(older);
            if link.is_demo() {
                self.list = demo_list(link, scope);
                self.listed = true;
                self.want_older = None;
                return self.step(link, group, older);
            }
            let (Some(session), Some(group)) = (link.session(), group) else {
                return Step::None;
            };
            let types = scope.types;
            if let Ok(p) = group.read(&session, move |s| async move {
                let filter = TransactionFilter {
                    account_id: Some(scope.account),
                    sort: Some((TxnSort::Date, SortDir::Desc)),
                    ..TransactionFilter::default()
                };
                let page = s
                    .transactions()
                    .list(&filter, PageRequest::new(LIST_PAGE, 0)?)
                    .await?;
                Ok(page
                    .data
                    .into_iter()
                    .filter(|r| types.contains(&r.txn_type))
                    .map(|r| r.id)
                    .collect())
            }) {
                self.list_task = Some(Task::Pending(p));
            }
            return Step::None;
        }
        self.step(link, group, older)
    }

    fn step(&mut self, link: &CompanyLink, group: Option<&RequestGroup>, older: bool) -> Step {
        let target = match (self.at, older) {
            (None, true) => Some(0),
            (None, false) => return Step::None,
            (Some(i), true) => Some(i + 1),
            (Some(0), false) => None,
            (Some(i), false) => Some(i - 1),
        };
        match target {
            None => {
                self.at = None;
                Step::New
            }
            Some(i) if i < self.list.len() => {
                self.at = Some(i);
                let id = self.list[i];
                self.load(link, group, id)
            }
            Some(_) => Step::Oldest,
        }
    }

    /// Opens a saved record (the register, or Reload).
    pub(crate) fn open(
        &mut self,
        link: &CompanyLink,
        group: Option<&RequestGroup>,
        id: TransactionId,
    ) -> Step {
        self.at = self.list.iter().position(|x| *x == id);
        self.load(link, group, id)
    }

    fn load(
        &mut self,
        link: &CompanyLink,
        group: Option<&RequestGroup>,
        id: TransactionId,
    ) -> Step {
        if link.is_demo() {
            return Step::Record(Box::new(demo_get(link, id)));
        }
        let (Some(session), Some(group)) = (link.session(), group) else {
            return Step::Record(Box::new(Err(ApiError::NoCompanyOpen)));
        };
        match group.read(
            &session,
            move |s| async move { s.transactions().get(id).await },
        ) {
            Ok(p) => {
                self.load_task = Some(Task::Pending(p));
                Step::None
            }
            Err(_) => Step::Record(Box::new(Err(ApiError::NoCompanyOpen))),
        }
    }

    /// What arrived: a record, or the list (then the step waiting for it).
    pub(crate) fn poll(&mut self, link: &CompanyLink, group: Option<&RequestGroup>) -> Step {
        if let Some(r) = self.load_task.as_mut().and_then(Task::try_take) {
            self.load_task = None;
            return Step::Record(Box::new(r));
        }
        if let Some(r) = self.list_task.as_mut().and_then(Task::try_take) {
            self.list_task = None;
            let older = self.want_older.take();
            match r {
                Ok(list) => {
                    self.list = list;
                    self.listed = true;
                    if let Some(older) = older {
                        return self.step(link, group, older);
                    }
                }
                Err(e) => return Step::Record(Box::new(Err(e))),
            }
        }
        Step::None
    }
}

/// The standard question on unsaved edits before Prev/Next: Save /
/// Discard changes / Keep editing (the window title names the form).
pub(crate) fn prompt_ui(ui: &Ui, id_salt: &str, title: &str) -> Option<PromptAnswer> {
    let mut answer = None;
    egui::Window::new(t("window.prompt.title"))
        .id(egui::Id::unique(id_salt).with("nav_prompt"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.label(crate::windows::prompt_message(title));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if crate::widgets::primary_button(ui, &t("window.prompt.save")).clicked() {
                    answer = Some(PromptAnswer::Save);
                }
                if ui.button(t("window.prompt.discard")).clicked() {
                    answer = Some(PromptAnswer::Discard);
                }
                if ui.button(t("window.prompt.keep")).clicked() {
                    answer = Some(PromptAnswer::Keep);
                }
            });
        });
    answer
}

/// A helper form with saved records: the screen gives its parts, the
/// default methods do Prev/Next, the question and opening a record.
pub(crate) trait RecordForm {
    /// The records state, the company link and the window's request group.
    fn parts(&mut self) -> (&mut Records, &Arc<CompanyLink>, Option<&RequestGroup>);
    /// Which records Prev/Next steps through (`None`: no account yet).
    fn record_scope(&self) -> Option<RecordScope>;
    /// Unsaved edits.
    fn form_dirty(&self) -> bool;
    /// A save or void is running.
    fn form_busy(&self) -> bool;
    /// Shows a saved record read from the engine.
    fn show_record(&mut self, v: &Versioned<Transaction>);
    /// A new, blank form (Next past the newest).
    fn blank_after_nav(&mut self);
    /// Throws the unsaved edits away.
    fn discard_edits(&mut self);
    /// Saves for the question's Save; true when it finished at once.
    fn save_for_nav(&mut self) -> bool;
    /// A save is waiting for the engine.
    fn save_queued(&self) -> bool;
    /// A plain note (i18n key).
    fn record_notice(&mut self, key: &'static str);
    /// Reading failed.
    fn record_failed(&mut self, e: ApiError);

    /// Does what a step asks.
    fn apply_step(&mut self, step: Step) {
        match step {
            Step::None | Step::Prompt => {}
            Step::New => self.blank_after_nav(),
            Step::Oldest => self.record_notice("helper.nav.oldest"),
            Step::Record(r) => match *r {
                Ok(v) => self.show_record(&v),
                Err(e) => self.record_failed(e),
            },
        }
    }

    /// Prev (`older`) or Next.
    fn navigate(&mut self, older: bool) {
        let scope = self.record_scope();
        let (dirty, busy) = (self.form_dirty(), self.form_busy());
        let (r, link, group) = self.parts();
        let step = r.navigate(link, group, scope, older, dirty, busy);
        self.apply_step(step);
    }

    /// Polls the list and the record being read.
    fn poll_records(&mut self) {
        let (r, link, group) = self.parts();
        let step = r.poll(link, group);
        self.apply_step(step);
    }

    /// Opens a saved record; true when it is shown or being read.
    fn open_saved(&mut self, id: TransactionId) -> bool {
        let (r, link, group) = self.parts();
        let step = r.open(link, group, id);
        let ok = matches!(&step, Step::Record(x) if x.is_ok()) || r.loading();
        self.apply_step(step);
        ok
    }

    /// The question's answer.
    fn answer_nav(&mut self, answer: PromptAnswer) {
        let Some(older) = self.parts().0.prompt.take() else {
            return;
        };
        match answer {
            PromptAnswer::Keep => {}
            PromptAnswer::Discard => {
                self.discard_edits();
                self.navigate(older);
            }
            PromptAnswer::Save => {
                self.parts().0.after_save = Some(older);
                let done = self.save_for_nav();
                // Refused or waiting on a question: no move. An engine save
                // moves on when it finishes.
                if !done && !self.save_queued() {
                    self.parts().0.after_save = None;
                }
            }
        }
    }

    /// After a save: the Prev/Next the question's Save was for.
    fn after_saved(&mut self) {
        if let Some(older) = self.parts().0.after_save.take() {
            self.navigate(older);
        }
    }
}
