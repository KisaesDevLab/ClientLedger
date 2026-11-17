//! Security review minor 5 (P29): a temporary password or recovery key the
//! owner copies leaves the clipboard again — [`CLEAR_AFTER`] after the
//! copy, or at the next frame once its dialog closes. The clipboard is
//! overwritten with an empty text (egui's clipboard can't read it back, so
//! something copied since in another app is overwritten too; the window
//! says so next to the Copy button). Clipboard history tools outside the
//! app keep their own copy; that is the platform's limit.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use eframe::egui::Context;

/// How long a copied secret stays on the clipboard.
pub const CLEAR_AFTER: Duration = Duration::from_secs(30);

/// When the copied secret is taken off the clipboard (`None`: nothing to do).
static CLEAR_AT: Mutex<Option<Instant>> = Mutex::new(None);

fn set(at: Option<Instant>) {
    *CLEAR_AT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = at;
}

fn get() -> Option<Instant> {
    *CLEAR_AT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Copies `text` and schedules the clipboard to be cleared.
pub fn copy(ctx: &Context, text: String) {
    tracing::info!("secret clipboard: copied");
    ctx.copy_text(text);
    set(Some(Instant::now() + CLEAR_AFTER));
    ctx.request_repaint_after(CLEAR_AFTER);
}

/// The dialog holding the secret closed: clear at the next frame.
pub fn clear_soon() {
    if get().is_some() {
        tracing::info!("secret clipboard: clear asked by a closing dialog");
        set(Some(Instant::now()));
    }
}

/// A copied secret is waiting to be cleared.
#[must_use]
pub fn pending() -> bool {
    get().is_some()
}

/// Every frame (the shell): clears the clipboard when it is due.
pub fn tick(ctx: &Context) {
    let Some(at) = get() else {
        return;
    };
    let now = Instant::now();
    if now >= at {
        tracing::info!("secret clipboard: cleared");
        ctx.copy_text(String::new());
        set(None);
    } else {
        ctx.request_repaint_after(at - now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_cleared_when_due_or_when_its_dialog_closes() {
        let ctx = Context::default();
        copy(&ctx, "Tmp-1234".into());
        assert!(pending());
        tick(&ctx);
        assert!(pending(), "not due yet");
        clear_soon();
        tick(&ctx);
        assert!(!pending(), "cleared once the dialog closed");
        // Nothing copied: closing a dialog schedules nothing.
        clear_soon();
        assert!(!pending());
    }
}
