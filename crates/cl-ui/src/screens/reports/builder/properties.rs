//! The properties pane: the Row, Columns and Report tabs (plan §3.8). The
//! Row tab shows the selected row's form: a section (`sources.rs`), a
//! group, a formula (`formula.rs`), a text row, or a plain note for blank
//! rows and page breaks.

use cl_model::report_def::Node;
use cl_model::report_def::editor::{Op, RowKind};
use eframe::egui::{self, RichText, Ui};

use super::picker::{self, Choice};
use super::{BuilderScreen, FieldKey, Pane, PropTab};
use crate::i18n::t;
use crate::theme::Tokens;

/// The pane.
pub(crate) fn draw(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let tabs: [(PropTab, &str); 3] = [
        (PropTab::Row, "builder.tab.row"),
        (PropTab::Columns, "builder.tab.columns"),
        (PropTab::Report, "builder.tab.report"),
    ];
    ui.horizontal(|ui| {
        for (i, (tab, key)) in tabs.iter().enumerate() {
            let r = ui.selectable_label(s.tab == *tab, t(key));
            let name = t(key);
            let on = s.tab == *tab;
            r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, &name));
            if i == 0 && s.focus_to == Some(Pane::Properties) {
                r.request_focus();
            }
            if r.has_focus() {
                s.pane = Pane::Properties;
            }
            if r.clicked() {
                s.tab = *tab;
            }
        }
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt(("builder-props", s.id.0))
        .auto_shrink([false, false])
        .show(ui, |ui| match s.tab {
            PropTab::Row => row_tab(s, ui, tokens),
            PropTab::Columns => super::columns::draw(s, ui, tokens),
            PropTab::Report => report_tab(s, ui, tokens),
        });
}

/// The kind line at the top of a row form ("S Section adds up
/// transactions").
fn kind_line(ui: &mut Ui, tokens: &Tokens, kind: RowKind) {
    ui.horizontal(|ui| {
        // The kind's badge, as in the outline (UI review M2).
        super::outline::badge(ui, kind, tokens);
        ui.label(RichText::new(t(super::outline::kind_key(kind))).strong());
        ui.label(
            RichText::new(t(&format!(
                "builder.kind_hint.{}",
                super::outline::kind_key(kind).trim_start_matches("builder.kind.")
            )))
            .size(12.0)
            .color(tokens.text_secondary),
        );
    });
}

fn row_tab(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let Some(node) = s.ed.selected_node().cloned() else {
        ui.label(RichText::new(t("builder.row.none")).color(tokens.text_secondary));
        return;
    };
    kind_line(ui, tokens, RowKind::of(&node));
    match node {
        Node::Section(sec) => super::sources::section(s, ui, tokens, &sec),
        Node::Group(g) => {
            picker::text(
                s,
                ui,
                FieldKey::Caption(g.id.clone()),
                &t("builder.row.caption"),
                &g.caption,
                280.0,
            );
            ui.add_space(6.0);
            if let Some(v) = picker::check(ui, g.show_total, &t("builder.row.show_total")) {
                let mut n = g.clone();
                n.show_total = v;
                s.apply(Op::Edit(Node::Group(n)));
            }
            if g.show_total {
                picker::text(
                    s,
                    ui,
                    FieldKey::TotalCaption(g.id.clone()),
                    &t("builder.row.total_caption"),
                    g.total_caption.as_deref().unwrap_or_default(),
                    280.0,
                );
            }
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(crate::i18n::t_count(
                        "builder.row.group_inside",
                        g.children.len(),
                        &[],
                    ))
                    .size(12.0)
                    .color(tokens.text_tertiary),
                )
                .wrap(),
            );
        }
        Node::Formula(f) => super::formula::draw(s, ui, tokens, &f),
        Node::Text(x) => {
            picker::text(
                s,
                ui,
                FieldKey::Caption(x.id.clone()),
                &t("builder.row.text"),
                &x.text,
                280.0,
            );
        }
        Node::Blank(_) | Node::PageBreak(_) => {
            ui.label(RichText::new(t("builder.row.nothing_to_set")).color(tokens.text_secondary));
        }
    }
    // Owner language for this row's problems, under its form.
    let mine: Vec<_> =
        s.ed.problems()
            .into_iter()
            .filter(|p| p.node_id.as_deref() == s.ed.selection())
            .collect();
    for p in &mine {
        crate::screens::company::form::inline_note(ui, tokens, &super::preview::problem_text(s, p));
    }
}

fn report_tab(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let name = s.ed.name().to_string();
    picker::text(
        s,
        ui,
        FieldKey::Name,
        &t("builder.report.name"),
        &name,
        280.0,
    );
    ui.add_space(6.0);
    // The memorized group, by name (UI review M7).
    super::save::load_groups(s);
    let groups = s.save.groups().to_vec();
    let mut choices: Vec<Choice> = vec![picker::on(t("builder.report.no_group"))];
    choices.extend(groups.iter().map(|(_, n)| picker::on(n.clone())));
    let current =
        s.ed.group_id()
            .and_then(|g| groups.iter().position(|(id, _)| id == g))
            .map_or(0, |i| i + 1);
    if let Some(i) = picker::pick(
        ui,
        ("report-group", s.id.0),
        &t("builder.report.memorized_group"),
        &choices,
        Some(current),
        260.0,
    ) {
        let g = i
            .checked_sub(1)
            .and_then(|i| groups.get(i))
            .map(|g| g.0.clone());
        s.apply(Op::MemorizedGroup(g));
    }
    ui.add_space(6.0);
    let line = match s.ed.saved() {
        None => t("builder.report.who_new"),
        Some(x) if x.can_edit => t("builder.report.who_can"),
        Some(_) => t("builder.save.not_author"),
    };
    ui.add(egui::Label::new(RichText::new(line).size(12.0).color(tokens.text_secondary)).wrap());
    ui.add_space(8.0);
    if ui.button(t("builder.toolbar.memorize")).clicked() {
        s.commit_pending();
        super::save::open_memorize(s, false);
    }
    report_file_buttons(s, ui, tokens);
}

/// Export report file / Import report file (A09 WP-A; the mockup draws
/// Export, Import sits beside it, Q-A09e). Only a memorized report exports
/// (Q-A09l); its saved version is what the file holds.
fn report_file_buttons(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    if !crate::screens::reports::clreport::available(&s.link) {
        return;
    }
    ui.add_space(8.0);
    let saved = s.ed.saved().map(|x| (x.id.clone(), x.name.clone()));
    let mut note = None;
    ui.horizontal(|ui| {
        let export = ui
            .add_enabled(
                saved.is_some(),
                egui::Button::new(t("cmd.reports.export_report_file")),
            )
            .on_hover_text(t("report_file.export.tip"))
            .on_disabled_hover_text(t("report_file.export.memorize_first"));
        if export.clicked()
            && let Some((id, name)) = saved.clone()
        {
            note = crate::screens::reports::clreport::start_export(
                &s.link,
                crate::screens::reports::clreport::Target::Saved { id, name },
            );
        }
        if ui
            .button(t("cmd.reports.import_report_file"))
            .on_hover_text(t("report_file.import.tip"))
            .clicked()
        {
            note = crate::screens::reports::clreport::start_import(&s.link);
        }
    });
    if saved.is_none() {
        ui.label(
            RichText::new(t("report_file.export.memorize_first"))
                .size(12.0)
                .color(tokens.text_tertiary),
        );
    }
    if let Some(k) = note {
        s.status = Some(t(k));
    }
}
