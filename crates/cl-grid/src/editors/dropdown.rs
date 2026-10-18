//! The dropdown cell: Space or Alt+Down opens the column's choices, a
//! letter jumps to the first choice starting with it, Up/Down move, Enter or
//! Tab picks (`CellInput::Choice`). Labels are i18n keys resolved here.

use cl_model::TextKey;

use crate::core::model::{OptionSpec, Suggestion};

/// The choices as popup items, with labels resolved.
pub fn items(options: &[OptionSpec], text: &dyn Fn(TextKey) -> String) -> Vec<Suggestion> {
    options
        .iter()
        .map(|o| Suggestion {
            id: o.id,
            label: text(o.label),
            group: None,
        })
        .collect()
}

/// Mark the cursor cell's node as a combo box showing `value`.
pub fn node(node: &mut egui::accesskit::Node, value: &str, expanded: bool) {
    node.set_role(egui::accesskit::Role::ComboBox);
    node.set_value(value);
    node.set_expanded(expanded);
}
