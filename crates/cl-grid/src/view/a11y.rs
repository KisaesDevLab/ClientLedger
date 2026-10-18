//! The grid's AccessKit semantics (P10 plan Rev 2 §2.11).
//!
//! Tree: grid (`Grid`) → header row → column headers; grid → row (`Row`)
//! → cells (`Cell` read-only, `GridCell` editable). Each `Ui` is a node in
//! egui 0.37, so the grid and row `Ui`s *are* the grid and row nodes (R7,
//! §14). The cursor cell's node is the grid's one focus widget: the
//! `TextEdit` while editing (`TextInput`), otherwise the focus target
//! speaking as a `CheckBox`, a `ComboBox` or a cell.
//!
//! Requests the grid answers for its own nodes (F5): Focus and Click on a
//! cell move the cursor there; `ScrollIntoView` on a cell or row brings it
//! into view; `SetValue` on an editable text-like cell types and commits the
//! value as one edit; `ScrollUp` / `ScrollDown` on the grid page it. Money is
//! never a `numeric_value`: amounts are text, exactly as shown.

use egui::accesskit::{Action, ActionData, Invalid, Node, Rect as AkRect, Role, Toggled};
use egui::{Id, InputState, Rect};

use crate::core::model::CellPos;
use crate::core::rowkey::RowKey;

/// AccessKit bounds in points, as egui sets them for its own widgets.
#[must_use]
pub fn ak_rect(r: Rect) -> AkRect {
    AkRect {
        x0: r.min.x.into(),
        y0: r.min.y.into(),
        x1: r.max.x.into(),
        y1: r.max.y.into(),
    }
}

/// A count or index as AccessKit's number type (exact below 2^32).
#[must_use]
pub fn number(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// One request for the grid, from last frame's nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Focus or Click on a cell: move the cursor there.
    Press(CellPos),
    /// `ScrollIntoView` on a cell or row.
    IntoView(RowKey),
    /// `SetValue` on a cell: put this text in and commit it.
    SetValue(CellPos, String),
    /// `ScrollDown` (true) or `ScrollUp` (false) on the grid.
    Page(bool),
}

/// Take this frame's requests aimed at the grid, its rows and its cells
/// (as drawn last frame). Other nodes' requests stay for egui.
pub fn take_requests(
    input: &mut InputState,
    grid: Id,
    rows: &[(Id, RowKey)],
    cells: &[(Id, CellPos)],
) -> Vec<Request> {
    let mut out = Vec::new();
    for (id, pos) in cells {
        input.consume_accesskit_action_requests(*id, |r| match r.action {
            Action::Focus | Action::Click => {
                out.push(Request::Press(*pos));
                true
            }
            Action::ScrollIntoView => {
                out.push(Request::IntoView(pos.row));
                true
            }
            Action::SetValue => match &r.data {
                Some(ActionData::Value(v)) => {
                    out.push(Request::SetValue(*pos, v.to_string()));
                    true
                }
                _ => false,
            },
            _ => false,
        });
    }
    for (id, key) in rows {
        input.consume_accesskit_action_requests(*id, |r| {
            let hit = r.action == Action::ScrollIntoView;
            if hit {
                out.push(Request::IntoView(*key));
            }
            hit
        });
    }
    input.consume_accesskit_action_requests(grid, |r| match r.action {
        Action::ScrollDown => {
            out.push(Request::Page(true));
            true
        }
        Action::ScrollUp => {
            out.push(Request::Page(false));
            true
        }
        _ => false,
    });
    out
}

/// The grid node: counts (rows + the header row) and paging actions.
pub fn grid_node(node: &mut Node, bounds: Rect, rows: usize, columns: usize) {
    node.set_bounds(ak_rect(bounds));
    node.set_row_count(rows + 1);
    node.set_column_count(columns);
    node.add_action(Action::ScrollUp);
    node.add_action(Action::ScrollDown);
}

/// What a row node says.
pub struct RowInfo {
    /// "{row label}, row n of N" (the control channel reads labels).
    pub label: String,
    /// "row n of N".
    pub row_of: String,
    /// Absolute index (the header is row 0, so this is index + 1).
    pub index: usize,
    /// In the selection.
    pub selected: bool,
    /// The row's problem in plain words (error rows).
    pub problem: Option<String>,
}

/// A row node.
pub fn row_node(node: &mut Node, bounds: Rect, row: &RowInfo) {
    node.set_bounds(ak_rect(bounds));
    node.set_label(row.label.clone());
    node.set_row_index(row.index);
    node.set_row_index_text(row.row_of.clone());
    node.add_action(Action::ScrollIntoView);
    if row.selected {
        node.set_selected(true);
    }
    if let Some(p) = &row.problem {
        node.set_invalid(Invalid::True);
        node.set_description(p.clone());
    }
}

/// What a cell node says.
pub struct CellInfo<'a> {
    /// "{column}, line n" or "{column}, {row label}".
    pub label: &'a str,
    /// The display text (empty for an amount suggestion, NN4).
    pub value: &'a str,
    /// The placeholder of an empty cell: spoken as a description, never
    /// as the value.
    pub hint: Option<&'a str>,
    /// The column's explanation ("R = reconciled, C = cleared").
    pub help: Option<&'a str>,
    /// Column index.
    pub column: usize,
    /// The owner can edit it.
    pub editable: bool,
    /// A checkbox's state.
    pub toggled: Option<bool>,
    /// The row is locked (closed period, reconciled): this text.
    pub locked: Option<&'a str>,
    /// The row's problem in plain words.
    pub problem: Option<&'a str>,
    /// The value is an AI suggestion, not yet accepted: this text.
    pub suggested: Option<&'a str>,
}

/// A cell node (`Cell` read-only, `GridCell` editable).
pub fn cell_node(node: &mut Node, bounds: Rect, cell: &CellInfo<'_>) {
    node.set_bounds(ak_rect(bounds));
    node.set_role(if cell.editable {
        Role::GridCell
    } else {
        Role::Cell
    });
    node.set_label(cell.label);
    node.set_value(cell.value);
    node.set_column_index(cell.column);
    node.add_action(Action::Focus);
    node.add_action(Action::Click);
    node.add_action(Action::ScrollIntoView);
    if cell.editable && cell.locked.is_none() && cell.toggled.is_none() {
        node.add_action(Action::SetValue);
    }
    if let Some(b) = cell.toggled {
        node.set_toggled(if b { Toggled::True } else { Toggled::False });
    }
    let mut description = Vec::new();
    if let Some(h) = cell.hint {
        description.push(h);
    }
    if let Some(l) = cell.locked {
        node.set_read_only();
        description.push(l);
    }
    if let Some(p) = cell.problem {
        node.set_invalid(Invalid::True);
        description.push(p);
    }
    if let Some(s) = cell.suggested {
        description.push(s);
    }
    if let Some(h) = cell.help {
        description.push(h);
    }
    if !description.is_empty() {
        node.set_description(description.join(". "));
    }
}

/// The cursor cell's node while a popup is open: it controls the list,
/// and the highlighted entry is its active descendant.
pub fn popup_owner(node: &mut Node, popup: Id, highlighted: Option<Id>) {
    node.set_controls(vec![popup.accesskit_id()]);
    if let Some(h) = highlighted {
        node.set_active_descendant(h.accesskit_id());
    }
}

/// The grid's scrollbar: the top row as a whole number (1-based).
pub fn scrollbar_node(node: &mut Node, bounds: Rect, label: String, top: usize, rows: usize) {
    node.set_bounds(ak_rect(bounds));
    node.set_role(Role::ScrollBar);
    node.set_label(label);
    node.set_numeric_value(number(top + 1));
    node.set_min_numeric_value(1.0);
    node.set_max_numeric_value(number(rows.max(1)));
    node.set_numeric_value_step(1.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::accesskit::{ActionRequest, TreeId};
    use egui::{Event, pos2};

    fn request(id: Id, action: Action, data: Option<ActionData>) -> Event {
        Event::AccessKitActionRequest(ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: id.accesskit_id(),
            data,
        })
    }

    #[test]
    fn requests_for_cells_rows_and_the_grid() {
        let grid = Id::unique("g");
        let cell = (
            grid.with(1),
            CellPos {
                row: RowKey(4),
                col: crate::core::model::ColumnId(2),
            },
        );
        let row = (grid.with(2), RowKey(9));
        let other = Id::unique("elsewhere");
        let mut input = InputState::default();
        input.events = vec![
            request(cell.0, Action::Click, None),
            request(
                cell.0,
                Action::SetValue,
                Some(ActionData::Value("12.50".into())),
            ),
            request(row.0, Action::ScrollIntoView, None),
            request(grid, Action::ScrollDown, None),
            request(other, Action::Click, None),
        ];
        let got = take_requests(&mut input, grid, &[row], &[cell]);
        assert_eq!(
            got,
            vec![
                Request::Press(cell.1),
                Request::SetValue(cell.1, "12.50".into()),
                Request::IntoView(RowKey(9)),
                Request::Page(true),
            ]
        );
        assert_eq!(input.events.len(), 1, "other nodes' requests stay");
    }

    #[test]
    fn cell_semantics() {
        let mut node = Node::new(Role::Unknown);
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0));
        cell_node(
            &mut node,
            r,
            &CellInfo {
                label: "Amount, line 2",
                value: "444.25",
                hint: None,
                help: None,
                column: 1,
                editable: true,
                toggled: None,
                locked: Some("Locked"),
                problem: Some("Pick an account"),
                suggested: None,
            },
        );
        assert_eq!(node.role(), Role::GridCell);
        assert!(node.is_read_only());
        assert_eq!(node.invalid(), Some(Invalid::True));
        assert_eq!(node.description(), Some("Locked. Pick an account"));
        assert_eq!(node.numeric_value(), None, "money is never a number");
        assert!(!node.supports_action(Action::SetValue), "locked");
    }
}
