//! Check that it matches (step 6, mockup `QBDImport`): the tiles, the
//! Trial balance / A/R / A/P / Inventory switch, "Show only differences",
//! the table with the source program's figure, the books' figure and the
//! difference, the checks that must be 0.00 at the switch date (the
//! "before conversion" accounts and the valuation against the Inventory
//! Asset accounts), and Save PDF. `system_import` re-exports this view.
//!
//! Every figure is the engine's (`cl_model::imports::tieout`): nothing here
//! adds or subtracts an amount; the total row is the engine's `totals` and
//! stays blank without them. A difference under a cent shows its exact
//! digits. ✓ only when the engine says `allZero`, and Mark verified only
//! then (the engine checks again).

use std::path::Path;
use std::sync::mpsc::Receiver;

use cl_api::imports::{BalanceKey, TieOut, TieOutItem};
use cl_model::imports::tieout::{Row, RowIn, Section, View, diff_text};
use cl_print::reports::{Column, Report, Section as PdfSection};
use eframe::egui::{self, RichText, Ui};

use super::{ImportScreen, show_date};
use crate::i18n::{t, t_count, t_with};
use crate::theme::Tokens;

/// Which comparison the table shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    /// Trial balance accounts (and every check row).
    TrialBalance,
    /// Open invoices (and "Receivables before conversion").
    Receivables,
    /// Unpaid bills (and "Payables before conversion").
    Payables,
    /// Inventory items (and the inventory checks).
    Inventory,
}

impl Tab {
    /// Every tab, in the switch's order.
    pub const ALL: [Tab; 4] = [
        Tab::TrialBalance,
        Tab::Receivables,
        Tab::Payables,
        Tab::Inventory,
    ];

    /// A stable key (control channel, `imports.tieout.tab.<key>`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Tab::TrialBalance => "accounts",
            Tab::Receivables => "receivables",
            Tab::Payables => "payables",
            Tab::Inventory => "inventory",
        }
    }

    const fn section(self) -> Section {
        match self {
            Tab::TrialBalance => Section::Accounts,
            Tab::Receivables => Section::Receivables,
            Tab::Payables => Section::Payables,
            Tab::Inventory => Section::Inventory,
        }
    }

    /// The check rows shown with this tab.
    fn checks(self) -> &'static [BalanceKey] {
        match self {
            Tab::TrialBalance => &[
                BalanceKey::HistoryAr,
                BalanceKey::HistoryAp,
                BalanceKey::HistoryInventory,
                BalanceKey::ValuationVsAsset,
            ],
            Tab::Receivables => &[BalanceKey::HistoryAr],
            Tab::Payables => &[BalanceKey::HistoryAp],
            Tab::Inventory => &[BalanceKey::HistoryInventory, BalanceKey::ValuationVsAsset],
        }
    }

    /// The tile's one engine figure for the tab (A/R: "Receivables before
    /// conversion"; A/P likewise; Inventory: the valuation check).
    const fn tile_check(self) -> Option<BalanceKey> {
        match self {
            Tab::TrialBalance => None,
            Tab::Receivables => Some(BalanceKey::HistoryAr),
            Tab::Payables => Some(BalanceKey::HistoryAp),
            Tab::Inventory => Some(BalanceKey::ValuationVsAsset),
        }
    }
}

/// The tie-out as the screen shows it (built once per engine answer).
pub struct TieState {
    /// The rows (cl-model: the engine's figures as text).
    pub view: View,
    /// The table's comparison.
    pub tab: Tab,
}

fn item_label(i: &TieOutItem) -> String {
    match i.number.as_deref().filter(|n| !n.is_empty()) {
        Some(n) if !i.name.is_empty() => format!("{n} · {}", i.name),
        Some(n) => n.to_string(),
        None => i.name.clone(),
    }
}

/// The engine's rows for the cl-model view.
#[must_use]
pub fn rows_in(t: &TieOut) -> Vec<RowIn> {
    let mut rows: Vec<RowIn> = t
        .accounts
        .iter()
        .map(|a| RowIn {
            section: Section::Accounts,
            label: a.name.clone(),
            number: None,
            source_row: a.source_row,
            source: Some(a.source),
            books: Some(a.books),
            diff: a.diff,
        })
        .collect();
    for (section, items) in [
        (Section::Receivables, &t.ar_items),
        (Section::Payables, &t.ap_items),
        (Section::Inventory, &t.inventory_items),
    ] {
        rows.extend(items.iter().map(|i| RowIn {
            section,
            label: item_label(i),
            number: i.number.clone(),
            source_row: Some(i.source_row),
            source: Some(i.source),
            books: Some(i.books),
            diff: i.diff,
        }));
    }
    rows.extend(t.balance_rows.iter().map(|b| RowIn {
        section: Section::Checks,
        label: b.key.as_wire().to_string(),
        number: None,
        source_row: None,
        source: None,
        books: None,
        diff: b.diff,
    }));
    rows
}

impl TieState {
    /// The view of `t`, on the Trial balance.
    #[must_use]
    pub fn new(t: &TieOut) -> TieState {
        TieState {
            view: View::new(rows_in(t), t.all_zero),
            tab: Tab::TrialBalance,
        }
    }

    /// The tabs this tie-out has (A/R, A/P and Inventory only when the
    /// engine compared them: other programs have neither).
    #[must_use]
    pub fn tabs(&self) -> Vec<Tab> {
        Tab::ALL
            .into_iter()
            .filter(|tab| {
                *tab == Tab::TrialBalance
                    || self.of(tab.section()).next().is_some()
                    || self.checks_of(*tab).next().is_some()
            })
            .collect()
    }

    fn of(&self, section: Section) -> impl Iterator<Item = &Row> {
        self.view
            .rows
            .iter()
            .filter(move |r| r.input.section == section)
    }

    fn checks_of(&self, tab: Tab) -> impl Iterator<Item = &Row> {
        self.of(Section::Checks)
            .filter(move |r| tab.checks().iter().any(|k| k.as_wire() == r.input.label))
    }

    /// The table's rows for `tab` (the filter applied).
    #[must_use]
    pub fn rows(&self, tab: Tab) -> Vec<&Row> {
        self.of(tab.section())
            .filter(|r| !self.view.differences_only || !r.zero)
            .collect()
    }

    /// The check rows under the table for `tab` (the filter applied).
    #[must_use]
    pub fn checks(&self, tab: Tab) -> Vec<&Row> {
        self.checks_of(tab)
            .filter(|r| !self.view.differences_only || !r.zero)
            .collect()
    }

    /// Rows of `tab` (and its checks) that differ.
    #[must_use]
    pub fn differences(&self, tab: Tab) -> usize {
        self.of(tab.section())
            .chain(self.checks_of(tab))
            .filter(|r| !r.zero)
            .count()
    }

    /// A tab's tile: its heading, its figure (the engine's), the line
    /// under it, and whether it reads as matching.
    #[must_use]
    pub fn tile(&self, tab: Tab, tie: &TieOut) -> Tile {
        let n = self.of(tab.section()).count();
        let differing = self.differences(tab);
        let figure = match tab.tile_check() {
            None => tie.totals.diff.map(diff_text),
            Some(k) => self
                .checks_of(tab)
                .find(|r| r.input.label == k.as_wire())
                .map(|r| r.diff_text.clone()),
        };
        let date = show_date(tie.as_of);
        let (head, count) = match tab {
            Tab::TrialBalance => (
                t_with("imports.tieout.tile", &[("date", &date)]),
                t_count("imports.tieout.compared", n, &[]),
            ),
            Tab::Receivables => (
                t("imports.tieout.tile.receivables"),
                t_count("imports.tieout.open_invoices", n, &[]),
            ),
            Tab::Payables => (
                t("imports.tieout.tile.payables"),
                t_count("imports.tieout.unpaid_bills", n, &[]),
            ),
            Tab::Inventory => (
                t("imports.tieout.tile.inventory"),
                t_count("imports.tieout.items", n, &[]),
            ),
        };
        let under = if differing == 0 {
            count
        } else {
            format!(
                "{count} · {}",
                t_count("imports.tieout.rows_differ", differing, &[])
            )
        };
        Tile {
            head,
            figure: figure.unwrap_or_default(),
            under,
            matches: differing == 0,
            tick: self.view.checkmark(),
        }
    }
}

/// One tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    /// "Trial balance at 10/27/2026".
    pub head: String,
    /// The engine's figure (exact under a cent); blank without one.
    pub figure: String,
    /// "46 accounts compared" (and how many rows differ).
    pub under: String,
    /// No row of the tile differs.
    pub matches: bool,
    /// ✓: the engine said every difference is zero.
    pub tick: bool,
}

/// The name of a check row.
fn check_name(key: &str) -> String {
    t(&format!("imports.tieout.check.{key}"))
}

fn source_name(s: &ImportScreen) -> String {
    t(&format!("imports.source.{}", s.wizard.source.key()))
}

/// The first column's heading for a tab.
fn label_heading(tab: Tab) -> String {
    t(match tab {
        Tab::TrialBalance => "imports.tieout.col.account",
        Tab::Receivables => "imports.tieout.col.invoice",
        Tab::Payables => "imports.tieout.col.bill",
        Tab::Inventory => "imports.tieout.col.item",
    })
}

/// Draws the step.
pub fn tieout_ui(s: &mut ImportScreen, ui: &mut Ui, tokens: &Tokens) {
    let source = source_name(s);
    ui.add(
        egui::Label::new(
            RichText::new(t_with("imports.tieout.intro", &[("source", &source)]))
                .color(tokens.text_secondary),
        )
        .wrap(),
    );
    ui.add_space(10.0);
    let (Some(tie), Some(st)) = (s.tieout.as_ref(), s.tie.as_ref()) else {
        ui.label(t("imports.tieout.loading"));
        return;
    };
    let tabs = st.tabs();
    ui.horizontal_wrapped(|ui| {
        for tab in &tabs {
            tile_ui(ui, tokens, &st.tile(*tab, tie));
        }
    });
    ui.add_space(10.0);
    let mut save = false;
    ui.horizontal_wrapped(|ui| {
        let group =
            ui.label(RichText::new(t("imports.tieout.compare")).color(tokens.text_secondary));
        for tab in &tabs {
            let on = s.tie.as_ref().is_some_and(|x| x.tab == *tab);
            let r = ui
                .selectable_label(on, t(&format!("imports.tieout.tab.{}", tab.key())))
                .labelled_by(group.id);
            if r.clicked()
                && let Some(x) = s.tie.as_mut()
            {
                x.tab = *tab;
            }
        }
        ui.add_space(12.0);
        if let Some(x) = s.tie.as_mut() {
            ui.checkbox(
                &mut x.view.differences_only,
                t("imports.tieout.only_differences"),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            save = ui
                .add_enabled(
                    s.pdf.is_none(),
                    egui::Button::new(t("imports.tieout.save_pdf")),
                )
                .clicked();
        });
    });
    if save {
        s.start_save_pdf(ui.ctx());
    }
    if let Some((ok, text)) = &s.pdf_message {
        let color = if *ok {
            tokens.text_secondary
        } else {
            tokens.warn_text
        };
        ui.label(RichText::new(text).color(color));
    }
    ui.add_space(8.0);
    let (Some(tie), Some(st)) = (s.tieout.as_ref(), s.tie.as_ref()) else {
        return;
    };
    table_ui(ui, tokens, st, tie, &source);
    ui.add_space(8.0);
    if st.view.checkmark() {
        ui.label(t("imports.tieout.all_zero"));
    } else {
        let n = st.view.differences().len();
        ui.add(
            egui::Label::new(
                RichText::new(t_count("imports.tieout.differences", n, &[]))
                    .color(tokens.warn_text),
            )
            .wrap(),
        );
    }
    if !s.allowed() {
        ui.label(t("imports.tieout.admin_only"));
    }
}

fn tile_ui(ui: &mut Ui, tokens: &Tokens, tile: &Tile) {
    egui::Frame::new()
        .fill(tokens.accent_tint)
        .stroke(egui::Stroke::new(1.0, tokens.accent_border))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_min_width(200.0);
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(&tile.head)
                        .size(12.0)
                        .color(tokens.text_secondary),
                );
                let mark = if tile.tick { " ✓" } else { "" };
                ui.label(
                    RichText::new(format!("{}{mark}", tile.figure))
                        .monospace()
                        .size(20.0)
                        .color(if tile.matches {
                            tokens.accent
                        } else {
                            tokens.warn_text
                        }),
                );
                ui.label(
                    RichText::new(&tile.under)
                        .size(12.0)
                        .color(tokens.text_secondary),
                );
            });
        });
}

/// Column widths (pixels only; never an amount).
#[allow(clippy::float_arithmetic, reason = "layout pixels")]
fn widths(total: f32) -> [f32; 4] {
    let amount = (total * 0.2).clamp(110.0, 200.0);
    [(total - amount * 3.0).max(160.0), amount, amount, amount]
}

const ROW_H: f32 = 24.0;
/// The table's height before it scrolls (14 rows).
const TABLE_H: f32 = 336.0;

fn row_ui(ui: &mut Ui, w: &[f32; 4], cells: [RichText; 4]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, cell) in cells.into_iter().enumerate() {
            let layout = if i == 0 {
                egui::Layout::left_to_right(egui::Align::Center)
            } else {
                egui::Layout::right_to_left(egui::Align::Center)
            };
            ui.allocate_ui_with_layout(egui::vec2(w[i], ROW_H), layout, |ui| {
                ui.set_min_size(egui::vec2(w[i], ROW_H));
                ui.add(egui::Label::new(cell).truncate());
            });
        }
    });
}

fn diff_cell(r: &Row, tokens: &Tokens) -> RichText {
    RichText::new(&r.diff_text).monospace().color(if r.zero {
        tokens.accent
    } else {
        tokens.warn_text
    })
}

fn table_ui(ui: &mut Ui, tokens: &Tokens, st: &TieState, tie: &TieOut, source: &str) {
    let tab = st.tab;
    let rows = st.rows(tab);
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, tokens.border_default))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let w = widths(ui.available_width());
            let head = |s: String| RichText::new(s).color(tokens.text_secondary);
            row_ui(
                ui,
                &w,
                [
                    head(label_heading(tab)),
                    head(source.to_string()),
                    head(t("imports.tieout.col.books")),
                    head(t("imports.tieout.col.difference")),
                ],
            );
            ui.separator();
            if rows.is_empty() {
                ui.label(
                    RichText::new(t(if st.view.differences_only {
                        "imports.tieout.no_differences"
                    } else {
                        "imports.tieout.none"
                    }))
                    .color(tokens.text_tertiary),
                );
            } else {
                // Virtualized: only the rows on screen are laid out.
                egui::ScrollArea::vertical()
                    .id_salt("imports/tieout/rows")
                    .max_height(TABLE_H)
                    .auto_shrink([false, true])
                    .show_rows(ui, ROW_H, rows.len(), |ui, range| {
                        for r in &rows[range] {
                            row_ui(
                                ui,
                                &w,
                                [
                                    RichText::new(&r.input.label),
                                    RichText::new(&r.source_text).monospace(),
                                    RichText::new(&r.books_text).monospace(),
                                    diff_cell(r, tokens),
                                ],
                            );
                        }
                    });
            }
            if tab == Tab::TrialBalance {
                ui.separator();
                let total = |m: Option<cl_core::Money>| {
                    m.map_or_else(String::new, |m| m.format(cl_core::MoneyStyle::default()))
                };
                let diff = tie.totals.diff.map_or_else(String::new, diff_text);
                let mark = if st.view.checkmark() { " ✓" } else { "" };
                row_ui(
                    ui,
                    &w,
                    [
                        RichText::new(t_count(
                            "imports.tieout.total",
                            st.of(Section::Accounts).count(),
                            &[],
                        ))
                        .strong(),
                        RichText::new(total(tie.totals.source)).monospace(),
                        RichText::new(total(tie.totals.books)).monospace(),
                        RichText::new(format!("{diff}{mark}")).monospace().strong(),
                    ],
                );
            }
        });
    let checks = st.checks(tab);
    if !checks.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new(t("imports.tieout.checks")).strong());
        let w = widths(ui.available_width());
        for r in checks {
            let name = check_name(&r.input.label);
            ui.horizontal(|ui| {
                row_ui(
                    ui,
                    &w,
                    [
                        RichText::new(name),
                        RichText::new(""),
                        RichText::new(""),
                        diff_cell(r, tokens),
                    ],
                );
            })
            .response
            .on_hover_text(t(&format!("imports.tieout.check.{}.tip", r.input.label)));
        }
    }
}

// ---- Save PDF ------------------------------------------------------------

/// The tie-out as a report: every comparison and every check, whatever the
/// tab or the filter (cl-print draws the engine's figures as text).
#[must_use]
pub fn report(st: &TieState, tie: &TieOut, source: &str, verified: bool) -> Report {
    let col = |h: String, right: bool, share: f32| Column {
        heading: h,
        share,
        right,
        whole: right,
    };
    let columns = |tab: Tab| {
        vec![
            col(label_heading(tab), false, 0.46),
            col(source.to_string(), true, 0.18),
            col(t("imports.tieout.col.books"), true, 0.18),
            col(t("imports.tieout.col.difference"), true, 0.18),
        ]
    };
    let line = |r: &Row| {
        vec![
            r.input.label.clone(),
            r.source_text.clone(),
            r.books_text.clone(),
            r.diff_text.clone(),
        ]
    };
    let mut sections = Vec::new();
    for tab in st.tabs() {
        let mut rows: Vec<Vec<String>> = st.of(tab.section()).map(line).collect();
        if tab == Tab::TrialBalance {
            let total = |m: Option<cl_core::Money>| {
                m.map_or_else(String::new, |m| m.format(cl_core::MoneyStyle::default()))
            };
            rows.push(vec![
                t_count(
                    "imports.tieout.total",
                    st.of(Section::Accounts).count(),
                    &[],
                ),
                total(tie.totals.source),
                total(tie.totals.books),
                tie.totals.diff.map_or_else(String::new, diff_text),
            ]);
        }
        sections.push(PdfSection::Table {
            heading: t(&format!("imports.tieout.tab.{}", tab.key())),
            columns: columns(tab),
            rows,
            empty: t("imports.tieout.none"),
            breaks: Vec::new(),
        });
    }
    let checks: Vec<(String, String, bool)> = st
        .of(Section::Checks)
        .map(|r| (check_name(&r.input.label), r.diff_text.clone(), false))
        .collect();
    if !checks.is_empty() {
        sections.push(PdfSection::Summary {
            heading: t("imports.tieout.checks"),
            rows: checks,
        });
    }
    let date = show_date(tie.as_of);
    Report {
        title: t("imports.tieout.pdf.title"),
        subtitle: vec![
            t_with(
                "imports.tieout.pdf.subtitle",
                &[("source", source), ("date", &date)],
            ),
            t(if verified {
                "imports.tieout.pdf.verified"
            } else if st.view.checkmark() {
                "imports.tieout.pdf.all_zero"
            } else {
                "imports.tieout.pdf.differs"
            }),
        ],
        sections,
        footer: t("imports.tieout.pdf.footer"),
        page_label: t("imports.tieout.pdf.page"),
    }
}

/// A Save PDF on its way: the bytes and the owner's Save dialog.
pub(crate) struct PdfSave {
    bytes: Option<Vec<u8>>,
    dialog: Receiver<Option<std::path::PathBuf>>,
}

impl ImportScreen {
    /// The tie-out's PDF, or `None` before the tie-out came.
    pub(crate) fn tieout_pdf(&self) -> Option<Vec<u8>> {
        let (tie, st) = (self.tieout.as_ref()?, self.tie.as_ref()?);
        let doc = report(st, tie, &source_name(self), self.wizard.verified);
        cl_print::reports::render_report(&doc).ok()
    }

    /// Save PDF: renders now, then asks where.
    pub(crate) fn start_save_pdf(&mut self, ctx: &egui::Context) {
        let Some(bytes) = self.tieout_pdf() else {
            self.pdf_message = Some((false, t("imports.tieout.pdf.failed")));
            return;
        };
        let name = format!(
            "{}.pdf",
            cl_print::files::safe_component(&t("imports.tieout.pdf.file"))
        );
        let dialog = crate::widgets::file_picker::save_file_as(
            ctx,
            crate::widgets::file_picker::PickUse::SaveReport,
            None,
            &name,
            ("picker.filter.pdf", "pdf"),
        );
        self.pdf_message = None;
        self.pdf = Some(PdfSave {
            bytes: Some(bytes),
            dialog,
        });
    }

    /// The Save dialog's answer: the file is written where the owner said.
    pub(crate) fn poll_pdf(&mut self) {
        let Some(p) = self.pdf.as_mut() else {
            return;
        };
        let Ok(answer) = p.dialog.try_recv() else {
            return;
        };
        let bytes = p.bytes.take();
        self.pdf = None;
        if let (Some(path), Some(bytes)) = (answer, bytes) {
            self.write_pdf(&path, &bytes);
        }
    }

    /// Writes the PDF to `path` (Save PDF; the control channel).
    pub(crate) fn write_pdf(&mut self, path: &Path, bytes: &[u8]) {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if cl_print::report_export::write_atomic(path, bytes).is_ok() {
            tracing::info!(target: "imports", "tie-out PDF saved");
            self.pdfs_saved += 1;
            self.pdf_message = Some((true, t_with("imports.tieout.pdf.saved", &[("file", &file)])));
        } else {
            tracing::warn!(target: "imports", "a tie-out PDF could not be written");
            self.pdf_message = Some((false, t("imports.tieout.pdf.failed")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::imports::{BalanceRow, TieOutAccount, TieOutTotals};
    use cl_core::{CalendarDate, Money};

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn account(name: &str, d: &str) -> TieOutAccount {
        TieOutAccount {
            account: None,
            name: name.into(),
            source_row: None,
            source_keys: Vec::new(),
            source: money("10.00"),
            books: money("10.00"),
            diff: money(d),
        }
    }

    fn tie(accounts: Vec<TieOutAccount>, all_zero: bool) -> TieOut {
        TieOut {
            as_of: CalendarDate::from_ymd(2026, 10, 27).expect("date"),
            accounts,
            ar_items: vec![TieOutItem {
                source_row: 2,
                name: "Brightwater HOA".into(),
                number: Some("1005".into()),
                source: money("3500.00"),
                books: money("3500.00"),
                diff: money("0"),
            }],
            ap_items: Vec::new(),
            inventory_items: Vec::new(),
            balance_rows: vec![
                BalanceRow {
                    key: BalanceKey::HistoryAr,
                    diff: money("-10.00"),
                },
                BalanceRow {
                    key: BalanceKey::HistoryAp,
                    diff: money("0"),
                },
            ],
            totals: TieOutTotals {
                source: None,
                books: None,
                diff: Some(money("10.0040")),
            },
            all_zero,
        }
    }

    #[test]
    fn a_difference_under_a_cent_is_shown_exactly_and_never_ticked() {
        let t = tie(
            vec![account("1000", "0.0040"), account("1200", "10.00")],
            false,
        );
        let st = TieState::new(&t);
        let rows = st.rows(Tab::TrialBalance);
        assert_eq!(rows[0].diff_text, "0.0040");
        assert!(!rows[0].zero);
        let tile = st.tile(Tab::TrialBalance, &t);
        assert_eq!(tile.figure, "10.0040", "the engine's total, exact");
        assert!(!tile.tick && !tile.matches);
        assert_eq!(tile.head, "Trial balance at 10/27/2026");
        // ✓ only on the engine's word, whatever the rows say.
        let t = tie(vec![account("1000", "0")], false);
        assert!(!TieState::new(&t).tile(Tab::TrialBalance, &t).tick);
    }

    #[test]
    fn each_tab_has_its_rows_and_its_checks() {
        let t = tie(vec![account("1200", "10.00"), account("1000", "0")], false);
        let mut st = TieState::new(&t);
        assert_eq!(
            st.tabs(),
            [Tab::TrialBalance, Tab::Receivables, Tab::Payables],
            "no inventory compared: no Inventory tab"
        );
        assert_eq!(st.rows(Tab::Receivables).len(), 1);
        assert_eq!(
            st.rows(Tab::Receivables)[0].input.label,
            "1005 · Brightwater HOA"
        );
        assert_eq!(st.checks(Tab::Receivables).len(), 1);
        assert_eq!(st.checks(Tab::TrialBalance).len(), 2);
        // A/R: the item ties, "Receivables before conversion" does not.
        let tile = st.tile(Tab::Receivables, &t);
        assert_eq!(tile.figure, "-10.00");
        assert!(!tile.matches);
        assert_eq!(st.differences(Tab::Receivables), 1);
        assert!(st.tile(Tab::Payables, &t).matches);
        st.view.differences_only = true;
        assert_eq!(st.rows(Tab::TrialBalance).len(), 1);
        assert_eq!(st.rows(Tab::Receivables).len(), 0);
        assert_eq!(st.checks(Tab::TrialBalance).len(), 1);
    }

    #[test]
    fn other_programs_have_only_the_trial_balance() {
        let mut t = tie(vec![account("1000", "0")], true);
        t.ar_items.clear();
        t.balance_rows.clear();
        assert_eq!(TieState::new(&t).tabs(), [Tab::TrialBalance]);
    }

    #[test]
    fn every_tieout_text_key_resolves() {
        for tab in Tab::ALL {
            assert!(crate::i18n::has(&format!(
                "imports.tieout.tab.{}",
                tab.key()
            )));
        }
        for k in [
            BalanceKey::HistoryAr,
            BalanceKey::HistoryAp,
            BalanceKey::HistoryInventory,
            BalanceKey::ValuationVsAsset,
        ] {
            assert!(crate::i18n::has(&format!(
                "imports.tieout.check.{}",
                k.as_wire()
            )));
            assert!(crate::i18n::has(&format!(
                "imports.tieout.check.{}.tip",
                k.as_wire()
            )));
        }
    }
}
