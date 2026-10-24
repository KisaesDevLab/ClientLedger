//! Opening a web page in the owner's browser (P16 D2, Q-278).
//!
//! [`open_external`] is the only place the app asks egui to open a URL
//! (`ctx.open_url`; eframe's `links` feature hands it to the OS browser via
//! `webbrowser`, so no process code lives in this crate). It is called only
//! from an explicit click or keyboard activation; the app itself never
//! fetches the page (Non-negotiable 3).
//!
//! Under the control channel and in tests a [`LinkSink`] sits in the egui
//! context: the URL is recorded there instead and nothing opens, so a
//! scenario or a unit test can never start a browser.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Context};

/// Where URLs go instead of the browser (control channel, tests).
#[derive(Clone, Default)]
pub struct LinkSink(Arc<Mutex<Vec<&'static str>>>);

fn sink_id() -> egui::Id {
    egui::Id::unique("cl.link_sink")
}

impl LinkSink {
    /// Puts a sink into `ctx`: from now on links are recorded, not opened.
    #[must_use]
    pub fn install(ctx: &Context) -> LinkSink {
        if let Some(s) = LinkSink::of(ctx) {
            return s;
        }
        let s = LinkSink::default();
        ctx.data_mut(|d| d.insert_temp(sink_id(), s.clone()));
        s
    }

    /// The sink in `ctx`, if one was installed.
    #[must_use]
    pub fn of(ctx: &Context) -> Option<LinkSink> {
        ctx.data(|d| d.get_temp::<LinkSink>(sink_id()))
    }

    /// The URLs "opened" so far, oldest first.
    #[must_use]
    pub fn opened(&self) -> Vec<&'static str> {
        self.0.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn push(&self, url: &'static str) {
        if let Ok(mut v) = self.0.lock() {
            v.push(url);
        }
    }
}

/// Opens `url` in a new browser tab, or records it in the [`LinkSink`]
/// when there is one (always in unit tests).
pub fn open_external(ctx: &Context, url: &'static str) {
    let sink = if cfg!(test) {
        Some(LinkSink::install(ctx))
    } else {
        LinkSink::of(ctx)
    };
    if let Some(s) = sink {
        s.push(url);
        return;
    }
    tracing::info!("opening an external page in the browser");
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sink_records_instead_of_opening() {
        let ctx = Context::default();
        assert!(LinkSink::of(&ctx).is_none());
        let sink = LinkSink::install(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            open_external(ui.ctx(), "https://example.invalid/");
        });
        out.textures_delta.clear();
        assert_eq!(sink.opened(), ["https://example.invalid/"]);
        assert!(
            !out.platform_output
                .commands
                .iter()
                .any(|c| matches!(c, egui::OutputCommand::OpenUrl(_))),
            "nothing reaches the browser"
        );
        assert_eq!(
            LinkSink::install(&ctx).opened().len(),
            1,
            "one sink per context"
        );
    }
}
