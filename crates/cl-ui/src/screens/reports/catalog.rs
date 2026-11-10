//! The catalog (mockup left column): "Find a report", the groups and their
//! reports, the owner's memorized reports. A report the engine can't run
//! yet shows greyed with "Needs an engine update" (Q-220).

use cl_model::reports::{CATALOG, CatalogGroup, ReportOptions, search};
use eframe::egui::{self, RichText, Ui};

use super::available;
use super::window::ReportWindow;
use crate::i18n::t;
use crate::theme::Tokens;

pub(super) fn show(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    let find = t("reports.find");
    let l = ui.label(RichText::new(&find).size(12.0).color(tokens.text_secondary));
    ui.add(
        egui::TextEdit::singleline(&mut w.search)
            .hint_text(&find)
            .desired_width(210.0),
    )
    .labelled_by(l.id);
    let hits = search(&w.search, |s| t(s.title));
    let mut pick = None;
    egui::ScrollArea::vertical()
        .id_salt("report_catalog")
        .max_height(ui.available_height().max(200.0))
        .show(ui, |ui| {
            for group in CatalogGroup::ALL {
                if group == CatalogGroup::Memorized {
                    super::memorized::catalog(w, ui, tokens);
                    continue;
                }
                let in_group: Vec<_> = hits.iter().filter(|s| s.group == group).collect();
                if in_group.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(t(group.title()).to_uppercase())
                        .size(11.0)
                        .color(tokens.text_secondary),
                );
                for spec in in_group {
                    let on = spec.key == w.spec.key;
                    let ok = available(&w.link, spec);
                    let title = t(spec.title);
                    let text = if ok {
                        RichText::new(&title)
                    } else {
                        RichText::new(&title).color(tokens.text_secondary)
                    };
                    let resp = ui.selectable_label(on, text);
                    let resp = if ok {
                        resp
                    } else {
                        resp.on_hover_text(t("reports.needs_update"))
                    };
                    if resp.clicked() {
                        pick = Some(*spec);
                    }
                }
            }
            if hits.is_empty() {
                ui.label(RichText::new(t("reports.find.none")).color(tokens.text_secondary));
            }
        });
    if let Some(spec) = pick
        && let Some(s) = CATALOG.iter().find(|s| s.key == spec.key)
    {
        w.show_report(s, ReportOptions::for_spec(s));
    }
}
