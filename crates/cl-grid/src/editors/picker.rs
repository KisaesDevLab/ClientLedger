//! Picker cells (accounts, names, items, Client:Job; P10 plan Rev 2 §2.8).
//!
//! The editor is the single-line text editor. Typing ranks the list
//! (`cl_model::lookup` through `Lookups::suggest`) into a type-ahead popup.
//! Ctrl+L (`GridCommand::OpenList`) shows the full list, starting on the
//! cell's entry. "Add …" is offered when nothing matches exactly and the
//! column allows quick-add. The rules live in `core::state`: Tab accepts the
//! highlighted entry, an exact match, or an AI suggestion when the cell is
//! empty and unchanged. Enter, the arrows, a click and `commit_pending`
//! never accept a suggestion (NN4, T14). This module turns a popup into
//! what `view::overlay::popup` draws.

use cl_model::TextKey;

use crate::core::state::{POPUP_ROWS, Popup, PopupKind};
use crate::view::overlay::{PopupView, window_top};

/// The narrowest popup, in points.
pub const MIN_WIDTH: f32 = 220.0;

/// What the popup shows this frame. `header` is the column's resolved
/// header, `top` the first entry shown last frame.
#[must_use]
pub fn view<'p>(
    popup: &'p Popup,
    header: String,
    width: f32,
    top: usize,
    text: &dyn Fn(TextKey) -> String,
) -> PopupView<'p> {
    let status = if popup.loading {
        Some(text(TextKey("grid.popup.loading")))
    } else if popup.items.is_empty() && popup.add_new.is_none() {
        Some(text(TextKey("grid.popup.no_matches")))
    } else {
        None
    };
    let add_new = popup
        .add_new
        .as_ref()
        .map(|t| text(TextKey("grid.popup.add_new")).replace("{text}", t));
    let label = match popup.kind {
        PopupKind::Full | PopupKind::Browse => {
            text(TextKey("grid.popup.full_list")).replace("{column}", &header)
        }
        PopupKind::List | PopupKind::Choices => header,
    };
    PopupView {
        items: &popup.items,
        highlight: popup.highlight,
        top: window_top(top, popup.highlight, POPUP_ROWS, popup.items.len()),
        rows: POPUP_ROWS,
        add_new,
        status,
        label,
        width: width.max(MIN_WIDTH),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Suggestion;

    fn t(k: TextKey) -> String {
        match k.0 {
            "grid.popup.add_new" => "Add \"{text}\"".into(),
            "grid.popup.full_list" => "All {column}".into(),
            other => other.into(),
        }
    }

    fn popup(kind: PopupKind, n: u64, highlight: Option<usize>) -> Popup {
        Popup {
            kind,
            items: (0..n)
                .map(|id| Suggestion {
                    id,
                    label: format!("Entry {id}"),
                    group: None,
                })
                .collect(),
            highlight,
            loading: false,
            add_new: None,
        }
    }

    #[test]
    fn labels_status_and_add_new() {
        let p = popup(PopupKind::Full, 3, None);
        let v = view(&p, "Account".into(), 100.0, 0, &t);
        assert_eq!(v.label, "All Account");
        assert!((v.width - MIN_WIDTH).abs() < 0.01);
        assert!(v.status.is_none());
        let mut p = popup(PopupKind::List, 0, None);
        p.add_new = Some("Zebra".into());
        let v = view(&p, "Account".into(), 300.0, 0, &t);
        assert_eq!(v.add_new.as_deref(), Some("Add \"Zebra\""));
        assert!(v.status.is_none(), "Add is the answer, not 'no matches'");
        let p = Popup {
            loading: true,
            ..popup(PopupKind::List, 0, None)
        };
        let v = view(&p, "Account".into(), 300.0, 0, &t);
        assert_eq!(v.status.as_deref(), Some("grid.popup.loading"));
    }

    #[test]
    fn the_window_follows_the_highlight() {
        let p = popup(PopupKind::Full, 50, Some(20));
        let v = view(&p, "Account".into(), 300.0, 0, &t);
        assert_eq!(v.top, 20 + 1 - POPUP_ROWS);
        let p = popup(PopupKind::Full, 50, Some(3));
        let v = view(&p, "Account".into(), 300.0, 13, &t);
        assert_eq!(v.top, 3);
        let p = popup(PopupKind::Full, 5, None);
        let v = view(&p, "Account".into(), 300.0, 40, &t);
        assert_eq!(v.top, 0, "never past the end");
    }
}
