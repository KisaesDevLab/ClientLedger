//! The Splits panel under the entry row (P12 S3, Q-P12f): the lines on the
//! other side of the register's account, on the `line_entry` grid preset
//! (Account · Amount · Memo · Customer · Tag, one tag per line, Q-233).
//!
//! "Remaining to assign" is the header amount the owner typed minus the
//! amounts typed on the lines: exact `Money` form arithmetic on the owner's
//! own input, never a ledger figure (plan §4). Record is refused until it
//! is zero. The grid itself is the shared `crate::forms::lines_grid`
//! (P13 WP5), which Write Check also uses.

use std::sync::Arc;

use cl_core::Money;
use cl_core::ids::{AccountId, ContactId, TagId};
use cl_grid::{GridEvent, ListId, RowKeyGen, TextKey};
use cl_model::CommandId;
use cl_model::forms::lines::{FormLine, FormLines, LinesProblem, to_posting_lines};
use eframe::egui::{self, RichText, Ui};

use super::RegisterScreen;
use super::entry_row::{E_ACCOUNT, PLine};
use super::lists::{ACCOUNTS, NAMES, RegisterLists, TAGS};
use crate::forms::lines_grid::{LineLists, LinesGrid, LinesSpec};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// A line to open the panel with: account, amount, memo, customer, tag.
pub type LineSeed = (
    Option<AccountId>,
    Option<Money>,
    String,
    Option<ContactId>,
    Option<TagId>,
);

/// One split line: the shared form line (P13 WP3.1). The register never sets
/// `billable`.
pub type SplitRow = FormLine;

/// The panel's value: its lines, a blank entry line last.
pub type SplitsValue = FormLines;

pub use cl_model::forms::lines::remaining;

/// The lines as posting lines, or the first problem.
///
/// # Errors
/// A used line without an account or a positive amount, or lines that
/// don't add up to the header amount.
pub fn to_lines(header: Money, v: &SplitsValue) -> Result<Vec<PLine>, TextKey> {
    let lines = to_posting_lines(header, v).map_err(LinesProblem::key)?;
    Ok(lines
        .into_iter()
        .map(|l| PLine {
            account: l.account,
            amount: l.amount,
            memo: l.memo,
            contact: l.contact,
            tag: l.tag,
        })
        .collect())
}

impl LineLists for RegisterLists {
    const ACCOUNTS: ListId = ACCOUNTS;
    const CUSTOMERS: ListId = NAMES;
    const TAGS: ListId = TAGS;

    fn account(&self, key: u64) -> Option<AccountId> {
        RegisterLists::account(self, key)
    }
    fn account_key(&self, id: AccountId) -> Option<u64> {
        RegisterLists::account_key(self, id)
    }
    fn customer(&self, key: u64) -> Option<ContactId> {
        self.name(key)
    }
    fn customer_key(&self, id: ContactId) -> Option<u64> {
        self.name_key(id)
    }
    fn tag(&self, key: u64) -> Option<TagId> {
        RegisterLists::tag(self, key)
    }
    fn tag_key(&self, id: TagId) -> Option<u64> {
        RegisterLists::tag_key(self, id)
    }
}

/// The open Splits panel: the shared lines grid with the register's
/// columns.
pub struct SplitsPanel {
    grid: LinesGrid<RegisterLists>,
}

impl SplitsPanel {
    /// A panel with `lines` (a blank entry line is added).
    #[must_use]
    pub fn new(lists: Arc<RegisterLists>, lines: Vec<LineSeed>) -> SplitsPanel {
        let mut tmp = RowKeyGen::new();
        let seeds = lines
            .into_iter()
            .map(|(account, amount, memo, customer, tag)| SplitRow {
                account,
                amount,
                memo,
                customer,
                tag,
                ..SplitRow::blank(tmp.next_key())
            })
            .collect();
        SplitsPanel {
            grid: LinesGrid::new(&LinesSpec::REGISTER_SPLITS, lists, seeds),
        }
    }

    /// The panel's form, for tests.
    #[cfg(test)]
    pub(super) fn form_mut(
        &mut self,
    ) -> &mut crate::grid_form::GridForm<
        SplitsValue,
        crate::forms::lines_grid::LinesState<RegisterLists>,
    > {
        self.grid.form_mut()
    }

    /// The panel's grid has the keyboard.
    #[must_use]
    pub fn has_focus(&self) -> bool {
        self.grid.has_focus()
    }

    /// The lines.
    #[must_use]
    pub fn value(&self) -> &SplitsValue {
        self.grid.value()
    }
}

impl RegisterScreen {
    /// The first split line's account (it decides the kind with the side).
    pub(super) fn splits_first_account(&self) -> Option<AccountId> {
        self.splits.as_ref()?.value().used().find_map(|l| l.account)
    }

    /// Commits the open Splits cell. False when it is not valid.
    pub(super) fn splits_commit(&mut self) -> bool {
        self.splits.as_mut().is_none_or(|s| s.grid.commit())
    }

    /// The Splits lines as posting lines (`None` when Splits is closed).
    pub(super) fn splits_lines(&self, amount: Money) -> Option<Result<Vec<PLine>, TextKey>> {
        self.splits.as_ref().map(|s| to_lines(amount, s.value()))
    }

    /// Splits has unsaved edits.
    pub(super) fn splits_dirty(&self) -> bool {
        self.splits.as_ref().is_some_and(|s| s.grid.dirty())
    }

    /// Closes the panel (its lines go).
    pub(super) fn close_splits(&mut self) {
        self.splits = None;
        self.entry.form.adapter_mut().splits_open = false;
    }

    /// Opens the panel with the given lines.
    pub(super) fn open_splits(&mut self, lines: Vec<LineSeed>) {
        let mut panel = SplitsPanel::new(Arc::clone(&self.lists), lines);
        panel.grid.focus_first();
        self.splits = Some(panel);
        self.entry.form.adapter_mut().splits_open = true;
    }

    /// The Splits button (and Ctrl+E on a new entry): opens the panel with
    /// the row's account and amount as the first line, or closes it when at
    /// most one line is used (that line's account goes back to the row).
    pub(super) fn toggle_splits(&mut self) {
        let lists = Arc::clone(&self.lists);
        if self.entry.form.commit_pending(&*lists).is_err() {
            return;
        }
        if let Some(s) = &self.splits {
            let used: Vec<&SplitRow> = s.value().used().collect();
            if used.len() > 1 {
                self.entry.problems = vec![TextKey("register.splits.close_first")];
                return;
            }
            let account = used.first().and_then(|l| l.account);
            self.close_splits();
            if account.is_some() {
                self.entry.form.edit_step(|d| d.account = account);
            }
            self.entry.focus = Some(E_ACCOUNT);
            return;
        }
        let d = self.entry.draft().clone();
        let first = d
            .account
            .map(|a| (Some(a), d.payment.or(d.deposit), String::new(), None, None));
        self.open_splits(first.into_iter().collect());
    }

    /// Keymap commands for the panel while it has the keyboard.
    pub(super) fn splits_command(&mut self, id: CommandId) -> Option<bool> {
        self.splits.as_mut()?.grid.command(id)
    }

    /// The panel's control-channel state (no amounts).
    pub(super) fn splits_state(&self) -> serde_json::Value {
        let Some(s) = &self.splits else {
            return serde_json::json!({"open": false});
        };
        let header = {
            let d = self.entry.draft();
            d.payment.or(d.deposit)
        };
        serde_json::json!({
            "open": true,
            "lines": s.value().used().count(),
            "remaining_zero": remaining(header, s.value()) == Some(Money::ZERO),
            "focused": s.has_focus(),
        })
    }

    /// An edited single line's memo, kept as it was.
    pub(super) fn edit_line_memo(&self) -> Option<String> {
        self.edit_lines
            .first()
            .filter(|_| self.edit_lines.len() == 1)
            .map(|l| l.memo.clone())
            .filter(|m| !m.is_empty())
    }

    /// An edited single line's tag, kept as it was.
    pub(super) fn edit_line_tag(&self) -> Option<TagId> {
        self.edit_lines
            .first()
            .filter(|_| self.edit_lines.len() == 1)
            .and_then(|l| l.tag)
    }

    /// The pickers changed: the panel uses the new lists.
    pub(super) fn entry_lists_changed(&mut self) {
        if let Some(s) = self.splits.as_mut() {
            s.grid.set_lists(Arc::clone(&self.lists));
        }
    }

    /// A split line that just got its account with no amount takes what is
    /// left of the typed payment or deposit (Q-304).
    pub(super) fn splits_fill_remaining(&mut self) {
        let header = {
            let d = self.entry.draft();
            d.payment.or(d.deposit)
        };
        let allowed = self.entry.task.is_none()
            && !self.view_only()
            && !self.entry.form.form().is_unconfirmed();
        if let Some(s) = self.splits.as_mut() {
            s.grid.fill_remaining(header, Money::ZERO, allowed);
        }
    }

    /// Draws the panel (inside the entry row's frame).
    pub(super) fn splits_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let header = {
            let d = self.entry.draft();
            d.payment.or(d.deposit)
        };
        let style = self.parts.frame.money_style;
        let Some(s) = self.splits.as_mut() else {
            return;
        };
        ui.add_space(4.0);
        ui.label(
            RichText::new(t("register.splits.title"))
                .strong()
                .color(tokens.text_primary),
        );
        let out = s.grid.ui(ui);
        self.splits_fill_remaining();
        let Some(s) = self.splits.as_ref() else {
            return;
        };
        let left = remaining(header, s.value());
        let mut record = false;
        for e in out.events {
            if e == GridEvent::EnterOnEmptyEntry {
                record = true;
            }
        }
        ui.horizontal(|ui| {
            let text = left.map_or_else(
                || t("register.splits.remaining_unknown"),
                |m| t_with("register.splits.remaining", &[("amount", &m.format(style))]),
            );
            let color = if left == Some(Money::ZERO) {
                tokens.text_secondary
            } else {
                tokens.warn_text
            };
            ui.add(egui::Label::new(RichText::new(text).color(color)).selectable(false));
        });
        if record {
            self.start_record();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_model::forms::lines::ensure_entry;

    const NEED_ACCOUNT: TextKey = LinesSpec::REGISTER_SPLITS.need_account;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn row(keys: &mut RowKeyGen, account: u32, amount: &str) -> SplitRow {
        SplitRow {
            account: Some(crate::company::demo::account_id(account)),
            amount: Some(money(amount)),
            ..SplitRow::blank(keys.next_key())
        }
    }

    #[test]
    fn splits_spec_is_valid() {
        assert_eq!(
            LinesSpec::REGISTER_SPLITS
                .grid_spec::<RegisterLists>()
                .validate(),
            Ok(())
        );
    }

    #[test]
    fn remaining_is_the_typed_amount_less_the_typed_lines() {
        let mut keys = RowKeyGen::new();
        let mut v = SplitsValue {
            lines: vec![
                row(&mut keys, 6100, "100.10"),
                row(&mut keys, 6350, "25.40"),
            ],
        };
        ensure_entry(&mut v.lines, &mut keys);
        assert_eq!(v.lines.len(), 3, "one blank entry line");
        assert_eq!(remaining(Some(money("125.50")), &v), Some(Money::ZERO));
        assert_eq!(remaining(Some(money("130.00")), &v), Some(money("4.50")));
        assert_eq!(remaining(None, &v), None);
        let lines = to_lines(money("125.50"), &v).expect("lines");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].amount, money("25.40"));
        assert_eq!(
            to_lines(money("130.00"), &v).err(),
            Some(TextKey("register.splits.not_assigned"))
        );
        v.lines[0].account = None;
        assert_eq!(to_lines(money("125.50"), &v).err(), Some(NEED_ACCOUNT));
        assert_eq!(
            to_lines(money("1"), &SplitsValue::default()).err(),
            Some(NEED_ACCOUNT)
        );
    }
}
