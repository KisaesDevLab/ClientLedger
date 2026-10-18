//! The AccessKit grid tree (P10 plan Rev 2 §2.11). Step 11 pins R7: cells
//! registered with `ui.interact(.., Sense::CLICK)` inside a row-scoped child
//! `Ui` (`UiBuilder::accessibility_parent(row)`) become children of the row
//! node, and rows children of the grid node. Step 14 adds T26: AccessKit
//! actions (scroll, focus, click, set value) and row and cell states.

mod support;

use egui::accesskit::{NodeId, Role};
use support::lines::{ACCOUNT, AMOUNT, Lines, line_spec};
use support::rig::Rig;

fn find(nodes: &[(NodeId, egui::accesskit::Node)], id: NodeId) -> &egui::accesskit::Node {
    &nodes.iter().find(|(n, _)| *n == id).expect("node").1
}

#[test]
fn cells_are_children_of_rows_and_rows_of_the_grid() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, ACCOUNT);
    r.frame(vec![]);
    let nodes = r.nodes();
    let grid = find(&nodes, r.grid.id().accesskit_id());
    assert_eq!(grid.role(), Role::Grid);
    assert_eq!(grid.label(), Some("Expense lines"));
    assert_eq!(grid.row_count(), Some(4), "3 rows + the header");
    let row_key = r.model.key(1);
    let row_id = r.grid.row_id(row_key).accesskit_id();
    let row = find(&nodes, row_id);
    assert_eq!(row.role(), Role::Row);
    assert_eq!(row.row_index(), Some(2));
    assert_eq!(row.label(), Some("row 2 of 3"));
    // R7: the row is the parent of its cells (walking up through any
    // intermediate nodes) and the grid an ancestor of the row.
    let cell_id = r
        .grid
        .cell_id(cl_grid::CellPos {
            row: row_key,
            col: AMOUNT,
        })
        .accesskit_id();
    let parent_of = |child: NodeId| {
        nodes
            .iter()
            .find(|(_, n)| n.children().contains(&child))
            .map(|(id, _)| *id)
    };
    assert_eq!(parent_of(cell_id), Some(row_id), "cell under its row");
    let mut up = parent_of(row_id);
    let mut seen_grid = false;
    while let Some(p) = up {
        if p == r.grid.id().accesskit_id() {
            seen_grid = true;
            break;
        }
        up = parent_of(p);
    }
    assert!(seen_grid, "row under the grid");
    let cell = find(&nodes, cell_id);
    assert_eq!(cell.role(), Role::GridCell);
    assert_eq!(cell.label(), Some("Amount, line 2"));
    // The focused editor carries the cursor cell's label.
    let editor = find(&nodes, r.grid.focus_id().accesskit_id());
    assert_eq!(editor.label(), Some("Account, line 1"));
}

// ---- T26: AccessKit actions and states (step 14) ------------------------

use cl_grid::{CellPos, ModelRef, RowKey, RowState};
use egui::accesskit::{Action, ActionData, ActionRequest, Invalid, Node, TreeId};
use egui::{Event, Id};
use support::browse::Browse;
use support::lines::{Account, MEMO, money};
use support::register::{self, Register};
use support::rig::typed;

fn request(id: Id, action: Action, data: Option<ActionData>) -> Vec<Event> {
    vec![Event::AccessKitActionRequest(ActionRequest {
        action,
        target_tree: TreeId::ROOT,
        target_node: id.accesskit_id(),
        data,
    })]
}

fn browse_nodes(b: &Browse<Register>) -> Vec<(NodeId, Node)> {
    b.last
        .as_ref()
        .and_then(|o| o.platform_output.accesskit_update.as_ref())
        .map(|u| u.nodes.clone())
        .unwrap_or_default()
}

fn top(b: &mut Browse<Register>) -> usize {
    b.grid.snapshot(&ModelRef::Browse(&mut b.model)).top_row
}

#[test]
fn scroll_actions_move_the_viewport() {
    let mut b = Browse::new(Register::new(500), register::spec());
    b.frame(vec![]);
    b.frame(vec![]);
    let grid = find(&browse_nodes(&b), b.grid.id().accesskit_id()).clone();
    assert!(grid.supports_action(Action::ScrollDown));
    b.frame(request(b.grid.id(), Action::ScrollDown, None));
    let paged = top(&mut b);
    assert!(paged > 0, "ScrollDown pages");
    b.frame(request(b.grid.id(), Action::ScrollUp, None));
    assert_eq!(top(&mut b), 0, "ScrollUp pages back");
    // The scrollbar names the top row as a whole number.
    b.frame(request(b.grid.id(), Action::ScrollDown, None));
    b.frame(vec![]);
    let bar = browse_nodes(&b)
        .into_iter()
        .find(|(_, n)| n.role() == Role::ScrollBar)
        .expect("scrollbar")
        .1;
    assert_eq!(
        bar.numeric_value(),
        Some(f64::from(u32::try_from(top(&mut b) + 1).expect("small")))
    );
    assert_eq!(bar.max_numeric_value(), Some(500.0));
}

#[test]
fn scroll_into_view_brings_a_cut_row_in() {
    let mut b = Browse::new(Register::new(500), register::spec());
    b.size = egui::Vec2::new(1440.0, 333.0);
    b.frame(vec![]);
    b.frame(vec![]);
    let nodes = browse_nodes(&b);
    let grid = find(&nodes, b.grid.id().accesskit_id())
        .bounds()
        .expect("grid");
    // The last row node drawn is cut by the bottom edge.
    let (last_key, before) = (0..500u64)
        .filter_map(|k| {
            let id = b.grid.row_id(RowKey(k + 1)).accesskit_id();
            nodes
                .iter()
                .find(|(n, _)| *n == id)
                .map(|(_, n)| (k + 1, n.bounds()))
        })
        .last()
        .expect("rows");
    b.frame(request(
        b.grid.row_id(RowKey(last_key)),
        Action::ScrollIntoView,
        None,
    ));
    b.frame(vec![]);
    let nodes = browse_nodes(&b);
    let row = find(&nodes, b.grid.row_id(RowKey(last_key)).accesskit_id())
        .bounds()
        .expect("row");
    let before = before.expect("bounds");
    assert!(row.y0 < before.y0, "the view moved: {before:?} -> {row:?}");
    assert!(
        row.y1 - row.y0 > before.y1 - before.y0,
        "no longer cut: {before:?} -> {row:?}"
    );
    assert!(row.y1 <= grid.y1 + 0.5, "row {row:?} inside {grid:?}");
}

#[test]
fn focus_click_and_set_value_on_cells() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, ACCOUNT);
    r.frame(vec![]);
    let memo = r.grid.cell_id(CellPos {
        row: r.model.key(1),
        col: MEMO,
    });
    r.frame(request(memo, Action::Click, None));
    r.frame(vec![]);
    assert_eq!(r.at(), (1, MEMO));
    assert_eq!(r.focused(), Some(r.grid.focus_id()));
    let amount = r.grid.cell_id(CellPos {
        row: r.model.key(0),
        col: AMOUNT,
    });
    r.frame(request(amount, Action::Focus, None));
    assert_eq!(r.at(), (0, AMOUNT));
    // SetValue types and commits as one edit (the calculator applies).
    let account = r.grid.cell_id(CellPos {
        row: r.model.key(1),
        col: ACCOUNT,
    });
    r.frame(request(
        account,
        Action::SetValue,
        Some(ActionData::Value("6200".into())),
    ));
    assert_eq!(r.model.lines()[1].account, Account::Id(6200));
    r.frame(vec![]);
    let amount1 = r.grid.cell_id(CellPos {
        row: r.model.key(1),
        col: AMOUNT,
    });
    r.frame(request(
        amount1,
        Action::SetValue,
        Some(ActionData::Value("400+44.25".into())),
    ));
    assert_eq!(r.model.lines()[1].amount, Some(money("444.25")));
    assert_eq!(r.at(), (1, AMOUNT));
}

#[test]
fn selected_locked_and_error_rows_say_so() {
    let mut model = Register::new(50);
    model.states = vec![(1, RowState::LOCKED), (2, RowState::ERROR)];
    let mut b = Browse::new(model, register::spec());
    b.focus();
    b.frame(support::rig::key(egui::Key::Space, egui::Modifiers::NONE));
    b.frame(vec![]);
    let nodes = browse_nodes(&b);
    let row0 = find(&nodes, b.grid.row_id(RowKey(1)).accesskit_id());
    assert_eq!(row0.is_selected(), Some(true));
    assert!(
        row0.label().is_some_and(|l| l.ends_with("row 1 of 50")),
        "{:?}",
        row0.label()
    );
    let locked = find(
        &nodes,
        b.grid
            .cell_id(CellPos {
                row: RowKey(2),
                col: register::PAYEE,
            })
            .accesskit_id(),
    );
    assert!(locked.is_read_only());
    assert_eq!(locked.description(), Some("grid.a11y.locked"));
    assert!(!locked.supports_action(Action::SetValue));
    let row2 = find(&nodes, b.grid.row_id(RowKey(3)).accesskit_id());
    assert_eq!(row2.invalid(), Some(Invalid::True));
    assert!(row2.description().is_some());
    let bad = find(
        &nodes,
        b.grid
            .cell_id(CellPos {
                row: RowKey(3),
                col: register::PAYMENT,
            })
            .accesskit_id(),
    );
    assert_eq!(bad.invalid(), Some(Invalid::True));
    // Money is text, never a number.
    for (_, n) in &nodes {
        if matches!(n.role(), Role::Cell | Role::GridCell) {
            assert_eq!(n.numeric_value(), None);
        }
    }
}

#[test]
fn the_editor_owns_the_open_list() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, ACCOUNT);
    r.frame(typed("51"));
    r.frame(vec![]);
    let editor = find(&r.nodes(), r.grid.focus_id().accesskit_id()).clone();
    assert_eq!(editor.controls(), [r.grid.popup_id().accesskit_id()]);
    assert_eq!(
        editor.active_descendant(),
        Some(cl_grid::view::overlay::item_id(r.grid.popup_id(), 0).accesskit_id())
    );
}

#[test]
fn a_suggestion_is_described_as_one() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.model.form.edit_step(|l| l[1].suggested = Some(5100));
    r.focus_cell(0, AMOUNT);
    r.frame(vec![]);
    let cell = find(
        &r.nodes(),
        r.grid
            .cell_id(CellPos {
                row: r.model.key(1),
                col: ACCOUNT,
            })
            .accesskit_id(),
    )
    .clone();
    assert_eq!(cell.description(), Some("grid.a11y.suggested"));
}
