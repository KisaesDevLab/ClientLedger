//! The amount editor: the text editor in mono, right-aligned (P10 plan
//! §2.5). Typing goes to the buffer unchanged; the exact parse and the
//! calculator run on commit in `core::codec` through `cl_model::amount_entry`.
//! Pastes are reduced to one value or refused before the editor sees them
//! (`view::input`, Q-P10j).

use egui::{Align2, Id, TextEdit};

use crate::view::style::GridStyle;

/// The `TextEdit` for an amount cell.
pub fn editor<'t>(buffer: &'t mut String, id: Id, style: &GridStyle, width: f32) -> TextEdit<'t> {
    super::text::editor(
        buffer,
        id,
        style.mono.clone(),
        Align2::RIGHT_CENTER,
        width,
        style.pad,
        Some(64),
    )
}
