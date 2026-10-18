//! The date editor (P10 plan §2.6, Q-P10f): the text editor in mono. The
//! date keys (`+ - T M H Y R W K`) arrive as single-character `Text` events
//! and are taken by `view::input` before the editor draws, then applied by
//! `cl_model::date_entry` in the core (never during an IME composition). On
//! commit the cell shows the full date, so `1/5/99` becomes `01/05/2099`
//! (N2).

use egui::{Align2, Id, TextEdit};

use crate::view::style::GridStyle;

/// The `TextEdit` for a date cell.
pub fn editor<'t>(buffer: &'t mut String, id: Id, style: &GridStyle, width: f32) -> TextEdit<'t> {
    super::text::editor(
        buffer,
        id,
        style.mono.clone(),
        Align2::LEFT_CENTER,
        width,
        style.pad,
        Some(32),
    )
}
