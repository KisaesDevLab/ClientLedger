//! The AccessKit grid tree (P10 plan Rev 2 §2.11). Step 11 pins R7: cells
//! registered with `ui.interact(.., Sense::CLICK)` inside a row-scoped child
//! `Ui` (`UiBuilder::accessibility_parent(row)`) become children of the row
//! node, and rows children of the grid node.

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
