//! About box and privacy notice.
//!
//! About shows the name and branding (Q-30: "ClientLedger, powered by Vibe
//! MyBooks"), the version, the license notices, the engine's
//! internal-business-use scope (Q-137: here and in the docs only), the bundled
//! components, and the QuickBooks non-affiliation notice. The privacy notice
//! (Q-135) is a short plain-language page of its own.

use eframe::egui::{self, Align2, Context, RichText};

use crate::i18n::t;
use crate::theme::Tokens;

/// The SIL Open Font License text for the bundled IBM Plex fonts.
pub const FONT_LICENSE: &str = include_str!("../../../assets/fonts/OFL.txt");

/// Which page is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    /// Nothing.
    #[default]
    Closed,
    /// About ClientLedger.
    About,
    /// Privacy notice.
    Privacy,
}

/// The About and privacy dialog.
#[derive(Debug, Default, Clone)]
pub struct AboutDialog {
    page: Page,
    show_font_license: bool,
}

impl AboutDialog {
    /// Open the About page.
    pub fn open_about(&mut self) {
        self.page = Page::About;
    }

    /// Open the privacy notice.
    pub fn open_privacy(&mut self) {
        self.page = Page::Privacy;
    }

    /// Showing either page.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.page != Page::Closed
    }

    /// The page showing.
    #[must_use]
    pub fn page(&self) -> Page {
        self.page
    }

    /// Draw the dialog when open.
    pub fn show(&mut self, ctx: &Context) {
        if !self.is_open() {
            return;
        }
        let tokens = Tokens::of(ctx);
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let title = match self.page {
            Page::Privacy => t("privacy.title"),
            _ => t("about.title"),
        };
        let page = self.page;
        let mut show_font_license = self.show_font_license;
        egui::Window::new(title)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .default_height(560.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(ui.available_height() - 40.0)
                    .show(ui, |ui| match page {
                        Page::Privacy => privacy(ui, &tokens),
                        _ => about(ui, &tokens, &mut show_font_license),
                    });
                ui.add_space(8.0);
                if ui.button(t("about.close")).clicked() {
                    close = true;
                }
            });
        self.show_font_license = show_font_license;
        if close {
            self.page = Page::Closed;
        }
    }
}

fn font_license() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| FONT_LICENSE.replace("\r\n", "\n"))
}

fn paragraph(ui: &mut egui::Ui, tokens: &Tokens, key: &str) {
    ui.label(RichText::new(t(key)).color(tokens.text_secondary));
    ui.add_space(6.0);
}

fn heading(ui: &mut egui::Ui, key: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(t(key)).text_style(crate::theme::styles::section()));
    ui.add_space(2.0);
}

fn about(ui: &mut egui::Ui, tokens: &Tokens, show_font_license: &mut bool) {
    ui.label(
        RichText::new(t("app.name"))
            .size(22.0)
            .family(crate::fonts::semibold()),
    );
    ui.label(RichText::new(t("app.tagline")).color(tokens.accent));
    ui.label(
        RichText::new(format!(
            "{} {}",
            t("about.version"),
            env!("CARGO_PKG_VERSION")
        ))
        .color(tokens.text_secondary),
    );
    heading(ui, "about.h.license");
    paragraph(ui, tokens, "about.license.body");
    heading(ui, "about.h.engine");
    paragraph(ui, tokens, "about.engine.body");
    heading(ui, "about.h.components");
    paragraph(ui, tokens, "about.components.body");
    ui.checkbox(show_font_license, t("about.font_license.show"));
    if *show_font_license {
        ui.add(
            egui::TextEdit::multiline(&mut font_license())
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY)
                .desired_rows(8),
        );
    }
    heading(ui, "about.h.privacy");
    paragraph(ui, tokens, "privacy.body");
    heading(ui, "about.h.affiliation");
    paragraph(ui, tokens, "about.affiliation.body");
}

fn privacy(ui: &mut egui::Ui, tokens: &Tokens) {
    paragraph(ui, tokens, "privacy.body");
    heading(ui, "privacy.h.online");
    paragraph(ui, tokens, "privacy.online.body");
    heading(ui, "privacy.h.secrets");
    paragraph(ui, tokens, "privacy.secrets.body");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_and_close_pages() {
        let mut d = AboutDialog::default();
        assert!(!d.is_open());
        d.open_about();
        assert_eq!(d.page(), Page::About);
        d.open_privacy();
        assert_eq!(d.page(), Page::Privacy);
    }

    #[test]
    fn about_carries_the_required_notices() {
        let body = |k: &str| t(k);
        assert!(body("app.tagline").contains("powered by Vibe MyBooks"));
        assert!(body("about.affiliation.body").contains("Intuit"));
        assert!(body("about.engine.body").contains("internal"));
        assert!(body("about.license.body").contains("MIT"));
        assert!(body("privacy.body").contains("no analytics"));
        assert!(FONT_LICENSE.contains("SIL Open Font License"));
    }
}
