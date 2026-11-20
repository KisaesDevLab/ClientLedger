//! The Chart of Accounts' side of the save-conflict dialog (P33 D4 step
//! 11), as the Tags list's (header-only): an edit is guarded by the account
//! as the panel opened it, a 409 `STALE` reads their account, saves again
//! without asking when nothing editable changed on their side (once per
//! save), and opens [`crate::screens::conflict_dialog`] otherwise. The
//! sensitive mark is this computer's or the Admin's own setting, saved on
//! its own path, so it is never compared.

use cl_api::{Account, ApiError, EditableFields, Versioned};
use cl_core::ids::AccountId;
use cl_model::forms::SaveIntent;
use cl_model::lan::Conflict;
use cl_model::lists::accounts::AccountDraft;
use serde_json::{Value, json};

use super::model::{chart_entry, detail_text};
use super::{ChartScreen, Panel};
use crate::company::Task;
use crate::forms::conflict::{GONE, OpenConflict, Read, Words, read_of};
use crate::i18n::t;
use crate::screens::conflict_dialog::{ConflictChoice, ConflictDialog, fields_from_theirs};

/// The "changed elsewhere" notice (the plain refusal).
const CHANGED: &str = "lan.list.changed_elsewhere";

/// The editable fields of an account as the panel holds them.
#[must_use]
pub fn account_value(d: &AccountDraft) -> Value {
    json!({
        "accountType": d.kind.label().as_str(),
        "name": d.name.trim(),
        "accountNumber": d.number.trim(),
        "detailType": d.detail.trim(),
        "description": d.description.trim(),
        "parent": d.parent.map(|p| p.to_string()),
        "cardLast4": d.card_last4.0.trim(),
    })
}

/// Applies the fields Keep mine takes from `theirs` onto `mine`.
fn take_theirs(mine: &mut AccountDraft, theirs: &AccountDraft, fields: &[String]) {
    for field in fields {
        match field.as_str() {
            "accountType" => mine.kind = theirs.kind,
            "name" => mine.name.clone_from(&theirs.name),
            "accountNumber" => mine.number.clone_from(&theirs.number),
            "detailType" => mine.detail.clone_from(&theirs.detail),
            "description" => mine.description.clone_from(&theirs.description),
            "parent" => mine.parent = theirs.parent,
            "cardLast4" => mine.card_last4.clone_from(&theirs.card_last4),
            _ => {}
        }
    }
}

impl ChartScreen {
    /// An account as the panel would show it (the chart's own conversion,
    /// with this window's sensitive mark).
    pub(crate) fn draft_of(&self, a: &Account) -> AccountDraft {
        AccountDraft::from_entry(&chart_entry(a, self.sensitive.contains(&a.id)))
    }

    /// A save came back stale: read their account (the dialog follows).
    pub(crate) fn account_conflict_start(&mut self, id: AccountId, intent: SaveIntent) {
        self.conflict_for = Some(id);
        if self.link.is_demo() {
            let found = self.link.demo_state().and_then(|d| {
                d.chart
                    .as_ref()
                    .and_then(|c| c.iter().find(|x| x.id == id).cloned())
            });
            let got = found
                .map(|a| Versioned {
                    version: cl_api::RecordVersion::Fingerprint(a.fingerprint()),
                    record: a,
                    stock_warnings: Vec::new(),
                })
                .ok_or(ApiError::NotFound { code: None });
            self.account_conflict_read(got, intent);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.warn = Some(CHANGED);
            return;
        };
        match group.read(&session, move |s| async move { s.accounts().get(id).await }) {
            Ok(p) => self.acct_conflict.reading = Some((Task::Pending(p), intent)),
            Err(_) => self.warn = Some(CHANGED),
        }
    }

    /// Polls the read of their account.
    pub(crate) fn poll_account_conflict(&mut self) {
        if let Some((r, intent)) = self.acct_conflict.poll() {
            self.account_conflict_read(r, intent);
        }
    }

    /// The panel still edits the account the read was for.
    fn conflict_panel_matches(&self) -> bool {
        match &self.panel {
            Panel::Form(f) => f.editing.is_some() && f.editing == self.conflict_for,
            _ => false,
        }
    }

    pub(crate) fn account_conflict_read(
        &mut self,
        r: Result<Versioned<Account>, ApiError>,
        intent: SaveIntent,
    ) {
        if !self.conflict_panel_matches() {
            return;
        }
        let theirs = match read_of(r) {
            Read::Theirs(v) => v,
            Read::Gone => {
                self.warn = Some(GONE);
                return;
            }
            Read::Reload => {
                self.warn = Some(CHANGED);
                return;
            }
        };
        let theirs_draft = self.draft_of(&theirs.record);
        let Panel::Form(f) = &self.panel else {
            return;
        };
        let conflict = Conflict::new(
            &account_value(f.form.original()),
            &account_value(f.form.current()),
            &account_value(&theirs_draft),
            "lines",
        );
        if conflict.silent_resave() {
            if !std::mem::take(&mut self.resave_left) {
                // A second stale save in a row: say so, never loop.
                self.warn = Some(CHANGED);
                return;
            }
            let mine = f.form.current().clone();
            self.adopt_account(theirs.record, mine);
            self.save_inner(intent);
            return;
        }
        let names: Vec<(String, String)> = self
            .accounts
            .iter()
            .map(|a| {
                (
                    a.id.to_string(),
                    self.model.chart().display_name(a.id).unwrap_or_default(),
                )
            })
            .collect();
        let words = Words::new(
            std::sync::Arc::default(),
            None,
            cl_core::MoneyStyle::default(),
            &[],
        );
        let dialog = ConflictDialog::new(conflict, words.line_text()).with_field_text(Box::new(
            move |field, v| {
                let s = v.as_str()?;
                match field {
                    "accountType" => Some(t(s)),
                    "detailType" if !s.is_empty() => Some(detail_text(Some(s))),
                    "parent" => names.iter().find(|(k, _)| k == s).map(|(_, n)| n.clone()),
                    _ => None,
                }
            },
        ));
        self.acct_conflict.open = Some(OpenConflict {
            dialog,
            theirs,
            intent,
        });
    }

    /// Their account becomes the panel's "as opened" and `mine` its fields;
    /// the chart shows their account.
    fn adopt_account(&mut self, theirs: Account, mine: AccountDraft) {
        self.remember_account(&theirs);
        let draft = self.draft_of(&theirs);
        if let Panel::Form(f) = &mut self.panel {
            f.form.load(draft);
            if *f.form.current() != mine {
                f.form.edit_step(|d| *d = mine);
            }
            f.error = None;
            f.opened = Some(theirs);
        }
        self.warn = None;
    }

    /// The chart's copy of `account` becomes `account`.
    fn remember_account(&mut self, account: &Account) {
        match self.accounts.iter_mut().find(|a| a.id == account.id) {
            Some(a) => *a = account.clone(),
            None => self.accounts.push(account.clone()),
        }
        self.want_cursor = Some(account.id);
        let accounts = std::mem::take(&mut self.accounts);
        self.apply_accounts(accounts);
    }

    /// Draws the open dialog and runs the owner's choice.
    pub(crate) fn account_conflict_ui(&mut self, ctx: &eframe::egui::Context) {
        if let Some((open, choice)) = self.acct_conflict.ui(ctx) {
            self.run_account_choice(open, choice);
        }
    }

    #[allow(clippy::needless_pass_by_value)] // the dialog hands over its choice
    pub(crate) fn run_account_choice(
        &mut self,
        open: OpenConflict<Versioned<Account>>,
        choice: ConflictChoice,
    ) {
        if !self.conflict_panel_matches() {
            return;
        }
        match choice {
            ConflictChoice::Back => self.warn = Some(CHANGED),
            ConflictChoice::UseTheirs => {
                let theirs = open.theirs.record;
                self.remember_account(&theirs);
                let draft = self.draft_of(&theirs);
                if let Panel::Form(f) = &mut self.panel {
                    f.form.load(draft);
                    f.error = None;
                    f.opened = Some(theirs);
                }
                self.warn = None;
            }
            ConflictChoice::KeepMine(_) => {
                let Panel::Form(f) = &self.panel else {
                    return;
                };
                let mut mine = f.form.current().clone();
                take_theirs(
                    &mut mine,
                    &self.draft_of(&open.theirs.record),
                    &fields_from_theirs(open.dialog.conflict()),
                );
                self.adopt_account(open.theirs.record, mine);
                self.resave_left = true;
                self.save_inner(open.intent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_model::lists::accounts::AccountKind;

    #[test]
    fn every_field_has_plain_words() {
        for k in account_value(&AccountDraft::new(AccountKind::Expense))
            .as_object()
            .expect("object")
            .keys()
        {
            assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
        }
    }

    #[test]
    fn keep_mine_takes_every_field_it_is_given() {
        let mut theirs = AccountDraft::new(AccountKind::Income);
        theirs.name = "T".into();
        theirs.number = "4100".into();
        theirs.description = "d".into();
        let fields: Vec<String> = account_value(&theirs)
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect();
        let mut mine = AccountDraft::new(AccountKind::Expense);
        take_theirs(&mut mine, &theirs, &fields);
        assert_eq!(account_value(&mine), account_value(&theirs));
    }
}
