//! The Splits panel under the entry row (P12 S3, Q-P12f): the lines on the
//! other side of the register's account, on the `line_entry` grid preset
//! (Account · Amount · Memo · Customer · Tag, one tag per line, Q-233).
//!
//! "Remaining to assign" is the header amount the owner typed minus the
//! amounts typed on the lines: exact `Money` form arithmetic on the owner's
//! own input, never a ledger figure (plan §4). Record is refused until it
//! is zero. P13 reuses this panel.

use std::borrow::Cow;
use std::sync::Arc;

use cl_core::Money;
use cl_core::ids::{AccountId, ContactId, TagId};
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    Grid, GridEvent, GridHeight, GridSpec, Line, Lookups, RecordOutcome, RowKey, RowKeyGen,
    RowModel, RowState, Source, TextKey, WidthSpec,
};
use cl_model::CommandId;
use cl_model::amount_entry::AmountRules;
use cl_model::forms::FormState;
use cl_model::forms::lines::{FormLine, FormLines, LinesProblem, ensure_entry, to_posting_lines};
use eframe::egui::{self, RichText, Ui};

use super::RegisterScreen;
use super::entry_row::{E_ACCOUNT, PLine};
use super::lists::{ACCOUNTS, NAMES, RegisterLists, TAGS};
use crate::grid_form::{GridForm, LinesAdapter, Routed};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// Account.
pub const S_ACCOUNT: ColumnId = ColumnId(1);
/// Amount.
pub const S_AMOUNT: ColumnId = ColumnId(2);
/// Memo.
pub const S_MEMO: ColumnId = ColumnId(3);
/// Customer.
pub const S_CUSTOMER: ColumnId = ColumnId(4);
/// Tag (one per line).
pub const S_TAG: ColumnId = ColumnId(5);

const PICK_FROM_LIST: TextKey = TextKey("register.entry.pick_from_list");
const SAVING: TextKey = TextKey("grid.form.saving");
const NEED_ACCOUNT: TextKey = TextKey("register.splits.need_account");

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

fn column(id: ColumnId, header: TextKey, kind: CellKind, width: (u16, u16, u8)) -> Column {
    Column {
        id,
        header,
        placeholder: None,
        help: None,
        kind,
        line: Line::First,
        align: Align::Left,
        width: WidthSpec {
            min: width.0,
            preferred: width.1,
            flex: width.2,
        },
        editable: true,
        mono: false,
    }
}

/// The Splits grid.
#[must_use]
pub fn spec() -> GridSpec {
    let picker = |list| CellKind::Picker {
        list,
        add_new: false,
    };
    GridSpec::line_entry(vec![
        Column {
            placeholder: Some(TextKey("register.entry.col.account")),
            ..column(
                S_ACCOUNT,
                TextKey("register.entry.col.account"),
                picker(ACCOUNTS),
                (120, 220, 2),
            )
        },
        Column {
            align: Align::Right,
            mono: true,
            ..column(
                S_AMOUNT,
                TextKey("register.splits.col.amount"),
                CellKind::Amount(AmountRules::MONEY),
                (90, 110, 0),
            )
        },
        column(
            S_MEMO,
            TextKey("register.entry.col.memo"),
            CellKind::Text { max_chars: 500 },
            (100, 200, 2),
        ),
        column(
            S_CUSTOMER,
            TextKey("register.splits.col.customer"),
            picker(NAMES),
            (110, 170, 1),
        ),
        column(
            S_TAG,
            TextKey("register.splits.col.tag"),
            picker(TAGS),
            (90, 130, 1),
        ),
    ])
}

/// Row keys, the lists and the Esc snapshot of the line being edited.
pub struct SplitsAdapter {
    /// Key generator.
    pub keys: RowKeyGen,
    /// Picker lists.
    pub lists: Arc<RegisterLists>,
    snapshot: Option<SplitRow>,
}

struct View<'a> {
    form: &'a mut FormState<SplitsValue>,
    a: &'a mut SplitsAdapter,
}

impl LinesAdapter<SplitsValue> for SplitsAdapter {
    fn model<'a>(
        &'a mut self,
        form: &'a mut FormState<SplitsValue>,
    ) -> Box<dyn EditableModel + 'a> {
        Box::new(View { form, a: self })
    }
}

impl View<'_> {
    fn lines(&self) -> &[SplitRow] {
        &self.form.current().lines
    }
}

fn picked(lists: &RegisterLists, list: cl_grid::ListId, id: Option<u64>) -> CellValue<'_> {
    id.map_or(CellValue::Empty, |id| CellValue::Ref {
        id,
        display: lists.display(list, id).unwrap_or(Cow::Borrowed("")),
    })
}

impl RowModel for View<'_> {
    fn revision(&self) -> u64 {
        self.form.revision()
    }
    fn row_count(&self) -> usize {
        self.lines().len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        self.lines().get(index).map(|l| l.key)
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        self.lines().iter().position(|l| l.key == key)
    }
    fn row_state(&self, index: usize) -> RowState {
        match self.lines().get(index) {
            Some(l) if index + 1 == self.lines().len() && l.is_blank() => RowState::ENTRY,
            _ => RowState::empty(),
        }
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(l) = self.lines().get(index) else {
            return CellValue::Empty;
        };
        let lists = &*self.a.lists;
        match col {
            S_ACCOUNT => picked(
                lists,
                ACCOUNTS,
                l.account.and_then(|a| lists.account_key(a)),
            ),
            S_AMOUNT => l.amount.map_or(CellValue::Empty, CellValue::Money),
            S_MEMO => CellValue::Text(Cow::Borrowed(&l.memo)),
            S_CUSTOMER => picked(lists, NAMES, l.customer.and_then(|c| lists.name_key(c))),
            S_TAG => picked(lists, TAGS, l.tag.and_then(|g| lists.tag_key(g))),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}

/// Puts one committed cell into a line.
fn apply(
    l: &mut SplitRow,
    col: ColumnId,
    value: CellInput,
    lists: &RegisterLists,
) -> Option<TextKey> {
    match (col, value) {
        (S_ACCOUNT, CellInput::Ref(id)) => match lists.account(id) {
            Some(a) => l.account = Some(a),
            None => return Some(PICK_FROM_LIST),
        },
        (S_CUSTOMER, CellInput::Ref(id)) => match lists.name(id) {
            Some(c) => l.customer = Some(c),
            None => return Some(PICK_FROM_LIST),
        },
        (S_TAG, CellInput::Ref(id)) => match lists.tag(id) {
            Some(g) => l.tag = Some(g),
            None => return Some(PICK_FROM_LIST),
        },
        (S_ACCOUNT | S_CUSTOMER | S_TAG, CellInput::Unresolved(_)) => return Some(PICK_FROM_LIST),
        (S_ACCOUNT, CellInput::Clear) => l.account = None,
        (S_CUSTOMER, CellInput::Clear) => l.customer = None,
        (S_TAG, CellInput::Clear) => l.tag = None,
        (S_AMOUNT, CellInput::Money(m)) => l.amount = Some(m),
        (S_AMOUNT, CellInput::Clear) => l.amount = None,
        (S_MEMO, CellInput::Text(v)) => l.memo = v,
        (S_MEMO, CellInput::Clear) => l.memo.clear(),
        _ => return Some(TextKey("register.entry.bad_value")),
    }
    None
}

impl EditableModel for View<'_> {
    fn cell_editable(&self, index: usize, _col: ColumnId) -> bool {
        index < self.lines().len()
    }
    fn row_entered(&mut self, key: RowKey) {
        self.a.snapshot = self.lines().iter().find(|l| l.key == key).cloned();
    }
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        _source: Source,
    ) -> Result<(), CellProblem> {
        let lists = Arc::clone(&self.a.lists);
        let keys = &mut self.a.keys;
        let mut problem = None;
        let applied = self.form.edit_step(|v| {
            if let Some(l) = v.lines.iter_mut().find(|l| l.key == pos.row) {
                problem = apply(l, pos.col, value, &lists);
            }
            ensure_entry(&mut v.lines, keys);
        });
        if !applied {
            return Err(CellProblem::new(SAVING).at(pos.col));
        }
        problem.map_or(Ok(()), |k| Err(CellProblem::new(k).at(pos.col)))
    }
    fn revert_row(&mut self, key: RowKey) -> bool {
        let Some(snap) = self.a.snapshot.clone().filter(|s| s.key == key) else {
            return false;
        };
        if self.lines().contains(&snap) {
            return false;
        }
        let keys = &mut self.a.keys;
        self.form.edit_step(|v| {
            if let Some(l) = v.lines.iter_mut().find(|l| l.key == key) {
                *l = snap;
            }
            ensure_entry(&mut v.lines, keys);
        })
    }
    fn insert_line(&mut self, before: Option<RowKey>) -> Option<RowKey> {
        let key = self.a.keys.next_key();
        self.form
            .edit_step(|v| {
                let at = before
                    .and_then(|b| v.lines.iter().position(|l| l.key == b))
                    .unwrap_or(v.lines.len());
                v.lines.insert(at, SplitRow::blank(key));
            })
            .then_some(key)
    }
    fn delete_line(&mut self, key: RowKey) -> bool {
        if self.index_of(key).is_none() {
            return false;
        }
        let keys = &mut self.a.keys;
        self.form.edit_step(|v| {
            v.lines.retain(|l| l.key != key);
            ensure_entry(&mut v.lines, keys);
        })
    }
    fn record_row(&mut self, key: RowKey) -> RecordOutcome {
        let Some(i) = self.index_of(key) else {
            return RecordOutcome::Invalid(Vec::new());
        };
        if self.lines()[i].account.is_none() {
            return RecordOutcome::Invalid(vec![CellProblem::new(NEED_ACCOUNT).at(S_ACCOUNT)]);
        }
        RecordOutcome::Recorded {
            next: self.row_key(i + 1),
        }
    }
}

/// The open Splits panel.
pub struct SplitsPanel {
    form: GridForm<SplitsValue, SplitsAdapter>,
}

impl SplitsPanel {
    /// A panel with `lines` (a blank entry line is added).
    ///
    /// # Panics
    /// Never: the Splits spec is fixed and covered by tests.
    #[must_use]
    pub fn new(lists: Arc<RegisterLists>, lines: Vec<LineSeed>) -> SplitsPanel {
        let mut keys = RowKeyGen::new();
        let mut rows: Vec<SplitRow> = lines
            .into_iter()
            .map(|(account, amount, memo, customer, tag)| SplitRow {
                key: keys.next_key(),
                account,
                amount,
                memo,
                customer,
                billable: false,
                tag,
            })
            .collect();
        ensure_entry(&mut rows, &mut keys);
        // A fixed spec, covered by `splits_spec_is_valid`.
        let grid = Grid::new("register/splits", spec()).expect("the Splits spec is valid");
        SplitsPanel {
            form: GridForm::new(
                FormState::new(SplitsValue { lines: rows }),
                SplitsAdapter {
                    keys,
                    lists,
                    snapshot: None,
                },
                grid,
                t("register.splits.label"),
                GridHeight::Rows { min: 3, max: 6 },
            ),
        }
    }

    /// The panel's form, for tests.
    #[cfg(test)]
    pub(super) fn form_mut(&mut self) -> &mut GridForm<SplitsValue, SplitsAdapter> {
        &mut self.form
    }

    /// The panel's grid has the keyboard.
    #[must_use]
    pub fn has_focus(&self) -> bool {
        self.form.grid().has_focus()
    }

    /// The lines.
    #[must_use]
    pub fn value(&self) -> &SplitsValue {
        self.form.form().current()
    }
}

impl RegisterScreen {
    /// The first split line's account (it decides the kind with the side).
    pub(super) fn splits_first_account(&self) -> Option<AccountId> {
        self.splits.as_ref()?.value().used().find_map(|l| l.account)
    }

    /// Commits the open Splits cell. False when it is not valid.
    pub(super) fn splits_commit(&mut self) -> bool {
        let lists = Arc::clone(&self.lists);
        self.splits
            .as_mut()
            .is_none_or(|s| s.form.commit_pending(&*lists).is_ok())
    }

    /// The Splits lines as posting lines (`None` when Splits is closed).
    pub(super) fn splits_lines(&self, amount: Money) -> Option<Result<Vec<PLine>, TextKey>> {
        self.splits.as_ref().map(|s| to_lines(amount, s.value()))
    }

    /// Splits has unsaved edits.
    pub(super) fn splits_dirty(&self) -> bool {
        self.splits.as_ref().is_some_and(|s| s.form.dirty())
    }

    /// Closes the panel (its lines go).
    pub(super) fn close_splits(&mut self) {
        self.splits = None;
        self.entry.form.adapter_mut().splits_open = false;
    }

    /// Opens the panel with the given lines.
    pub(super) fn open_splits(&mut self, lines: Vec<LineSeed>) {
        let panel = SplitsPanel::new(Arc::clone(&self.lists), lines);
        let first = panel.value().lines.first().map(|l| l.key);
        self.splits = Some(panel);
        self.entry.form.adapter_mut().splits_open = true;
        if let (Some(key), Some(s)) = (first, self.splits.as_mut()) {
            let lists = Arc::clone(&self.lists);
            s.form.focus(
                CellPos {
                    row: key,
                    col: S_ACCOUNT,
                },
                &*lists,
            );
        }
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
        let lists = Arc::clone(&self.lists);
        let s = self.splits.as_mut()?;
        if !s.form.grid().has_focus()
            || !matches!(
                id.as_str(),
                "form.revert"
                    | "edit.undo"
                    | "edit.redo"
                    | "edit.open_list"
                    | "form.insert_line"
                    | "form.delete_line"
            )
        {
            return None;
        }
        match s.form.command(id, &*lists) {
            Routed::Handled => Some(true),
            _ => None,
        }
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
            "focused": s.form.grid().has_focus(),
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
            s.form.adapter_mut().lists = Arc::clone(&self.lists);
        }
    }

    /// Draws the panel (inside the entry row's frame).
    pub(super) fn splits_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let lists = Arc::clone(&self.lists);
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
        let out = s.form.ui(ui, &*lists);
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
        assert_eq!(spec().validate(), Ok(()));
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

    #[test]
    fn a_line_takes_one_tag_and_picks_must_match() {
        let lists = RegisterLists::default();
        let mut l = SplitRow::blank(RowKeyGen::new().next_key());
        assert_eq!(
            apply(&mut l, S_TAG, CellInput::Ref(1), &lists),
            Some(PICK_FROM_LIST)
        );
        assert_eq!(
            apply(&mut l, S_AMOUNT, CellInput::Money(money("2")), &lists),
            None
        );
        assert_eq!(
            apply(&mut l, S_MEMO, CellInput::Text("x".into()), &lists),
            None
        );
        assert!(!l.is_blank());
        assert_eq!(apply(&mut l, S_TAG, CellInput::Clear, &lists), None);
        assert_eq!(l.tag, None);
    }
}
