//! The checkbox cell: Space or a click toggles it (the core commits a
//! `Bool` at once as one undo step); the Space key and its `Text(" ")` are
//! consumed together so nothing types a space elsewhere (S3, T12). The
//! cell's AccessKit node is a `CheckBox` with its toggled state.

use egui::accesskit::{Node, Role, Toggled};

/// Mark a node as a checkbox in this state.
pub fn node(node: &mut Node, checked: bool) {
    node.set_role(Role::CheckBox);
    node.set_toggled(if checked {
        Toggled::True
    } else {
        Toggled::False
    });
}
