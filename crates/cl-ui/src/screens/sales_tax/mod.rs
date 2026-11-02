//! Sales tax screens (P23): Sales Tax Setup (WP-2); Sales Tax Owed, Pay
//! Sales Tax, exemption certificates and the bad-debt recovery dialog
//! (WP-3).

pub mod exemptions;
pub mod liability;
pub mod pay;
pub mod recovery;
pub mod setup;

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::windows::Screen;

/// The sales tax window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    match kind.as_str() {
        setup::KIND => Some(Box::new(setup::SetupScreen::new(Arc::clone(link)))),
        liability::KIND => Some(Box::new(liability::OwedScreen::new(
            Arc::clone(link),
            today,
        ))),
        pay::KIND => Some(Box::new(pay::PayScreen::new(Arc::clone(link), today))),
        exemptions::KIND => Some(Box::new(exemptions::ExemptionsScreen::new(
            Arc::clone(link),
            today,
        ))),
        recovery::KIND => Some(Box::new(recovery::RecoveryScreen::new(
            Arc::clone(link),
            today,
        ))),
        _ => None,
    }
}

/// Tests: the unnamed interactive widgets of `s` drawn three frames in
/// `theme` (AccessKit; screen readers and the control channel need a
/// name on each).
#[cfg(test)]
pub(crate) fn unnamed_in(s: &mut dyn Screen, theme: crate::theme::ThemeChoice) -> Vec<String> {
    use eframe::egui;
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, theme);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut nodes = Vec::new();
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440.0, 940.0),
            )),
            ..egui::RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
        nodes.extend(
            out.platform_output
                .accesskit_update
                .map(|u| u.nodes)
                .unwrap_or_default(),
        );
    }
    crate::screens::unnamed_widgets(&nodes)
}

/// Both themes.
#[cfg(test)]
pub(crate) const THEMES: [crate::theme::ThemeChoice; 2] = [
    crate::theme::ThemeChoice::Light,
    crate::theme::ThemeChoice::Dark,
];
