//! Check that it matches (step 6, mockup `QBDImport`): the tiles, the
//! Trial balance / A/R / A/P / Inventory switch, "Show only differences",
//! the table with the source program's figure, the books' figure and the
//! difference, the checks that must be 0.00 at the switch date (the
//! "before conversion" accounts and the valuation against the Inventory
//! Asset accounts), and Save PDF. `system_import` re-exports this view.
//!
//! Every figure is the engine's (E3, `cl_model::imports::tieout`): nothing
//! here adds or subtracts an amount. The trial balance total rows are the
//! engine's debit and credit columns (it sends no difference total, so
//! none is shown); the A/R, A/P and inventory tiles and total rows are the
//! engine's `totals`. A difference under a cent shows its exact digits; a
//! quantity difference shows too. ✓ (drawn: the bundled fonts have no check
//! glyph) only when the engine says `allZero`, and Mark verified only then
//! (the engine checks again). When verify is refused, the first difference
//! the engine named is shown and marked.

use std::path::Path;
use std::sync::mpsc::Receiver;

use cl_api::imports::{
    BalanceKey, DifferenceSection, TieOut, TieOutDifference, TieOutItem, TieOutTotal,
};
use cl_core::{Money, MoneyStyle};
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

    /// The tile's figure when the engine sent no total for the side (the
    /// side's own check).
    const fn tile_check(self) -> Option<BalanceKey> {
        match self {
            Tab::TrialBalance => None,
            Tab::Receivables => Some(BalanceKey::HistoryAr),
            Tab::Payables => Some(BalanceKey::HistoryAp),
            Tab::Inventory => Some(BalanceKey::ValuationVsAsset),
        }
    }

    /// The engine's totals for the side (A/R, A/P, inventory).
    fn total(self, tie: &TieOut) -> Option<TieOutTotal> {
        match self {
            Tab::TrialBalance => None,
            Tab::Receivables => tie.totals.ar,
            Tab::Payables => tie.totals.ap,
            Tab::Inventory => tie.totals.inventory,
        }
    }

    fn of_difference(section: DifferenceSection, key: Option<&str>) -> Tab {
        match (section, key) {
            (DifferenceSection::ArItems, _) | (DifferenceSection::Totals, Some("ar")) => {
                Tab::Receivables
            }
            (DifferenceSection::ApItems, _) | (DifferenceSection::Totals, Some("ap")) => {
                Tab::Payables
            }
            (DifferenceSection::InventoryItems, _)
            | (DifferenceSection::Totals, Some("inventory")) => Tab::Inventory,
            (DifferenceSection::BalanceRows, Some("history_ar")) => Tab::Receivables,
            (DifferenceSection::BalanceRows, Some("history_ap")) => Tab::Payables,
            (DifferenceSection::BalanceRows, Some("history_inventory" | "valuation_vs_asset")) => {
                Tab::Inventory
            }
            _ => Tab::TrialBalance,
        }
    }
}

/// The tie-out as the screen shows it (built once per engine answer).
pub struct TieState {
    /// The rows (cl-model: the engine's figures as text).
    pub view: View,
    /// The table's comparison.
    pub tab: Tab,
    /// The row the engine named first when verify was refused (an index
    /// into `view.rows`), marked in the table.
    pub focus: Option<usize>,
    /// Each side's totals differ (the engine's figure is not 0.00).
    totals_differ: Vec<Tab>,
}

/// A document number as the owner knows it: `None` for none, and for the
/// key the import makes up for a row with no number (`payment:<name>`),
/// which is never shown (UI review).
#[must_use]
pub fn shown_number(n: Option<&str>) -> Option<&str> {
    n.map(str::trim)
        .filter(|n| !n.is_empty() && !n.contains(':'))
}

/// An open item's words: "1005 · Brightwater HOA"; with no number of its
/// own, its kind ("Credit memo · Riverside Café").
fn item_label(i: &TieOutItem) -> String {
    // A made-up key `<kind>:<name>` gives its kind and name in words.
    let made_up = i
        .number
        .as_deref()
        .and_then(|n| n.split_once(':'))
        .map(|(k, n)| (k.trim().to_ascii_lowercase(), n.trim()));
    let kind_word = |kind: &str| {
        [
            format!("imports.spot.type.{kind}"),
            format!("imports.spot.type.customer_{kind}"),
        ]
        .into_iter()
        .find(|k| crate::i18n::has(k))
        .map(|k| t(&k))
    };
    let lead = shown_number(i.number.as_deref())
        .map(str::to_string)
        .or_else(|| i.doc_type.as_deref().and_then(kind_word))
        .or_else(|| made_up.as_ref().and_then(|(k, _)| kind_word(k)));
    let name = if i.name.is_empty() {
        made_up.as_ref().map_or("", |(_, n)| *n)
    } else {
        i.name.as_str()
    };
    match lead {
        Some(n) if !name.is_empty() => format!("{n} · {name}"),
        Some(n) => n,
        None => name.to_string(),
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
            label: a.label(),
            number: a.number.clone(),
            source_row: a.source_row,
            source: Some(a.source),
            books: Some(a.books),
            diff: a.diff,
            quantity_diff: None,
            id: a.account.map(|x| x.to_string()),
        })
        .collect();
    for (section, items) in [
        (Section::Receivables, &t.ar_items),
        (Section::Payables, &t.ap_items),
    ] {
        rows.extend(items.iter().map(|i| RowIn {
            section,
            label: item_label(i),
            number: i.number.clone(),
            source_row: i.source_row,
            source: Some(i.source),
            books: Some(i.books),
            diff: i.diff,
            quantity_diff: None,
            id: i.transaction.map(|x| x.to_string()),
        }));
    }
    rows.extend(t.inventory_items.iter().map(|i| RowIn {
        section: Section::Inventory,
        label: i.name.clone(),
        number: None,
        source_row: i.source_row,
        source: Some(i.source),
        books: Some(i.books),
        diff: i.diff,
        quantity_diff: Some(i.quantity_diff),
        id: i.item.map(|x| x.to_string()),
    }));
    rows.extend(t.balance_rows.iter().map(|b| RowIn {
        section: Section::Checks,
        label: b.key.as_wire().to_string(),
        number: None,
        source_row: None,
        source: None,
        books: None,
        diff: b.diff,
        quantity_diff: None,
        id: b.account.map(|x| x.to_string()),
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
            focus: None,
            totals_differ: Tab::ALL
                .into_iter()
                .filter(|tab| tab.total(t).is_some_and(|x| !x.diff.is_zero()))
                .collect(),
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
                    || self.totals_differ.contains(tab)
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

    /// The table's rows for `tab` (the filter applied), with their index
    /// in `view.rows`.
    #[must_use]
    pub fn rows(&self, tab: Tab) -> Vec<&Row> {
        self.indexed(tab).into_iter().map(|(_, r)| r).collect()
    }

    fn indexed(&self, tab: Tab) -> Vec<(usize, &Row)> {
        let section = tab.section();
        self.view
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.input.section == section)
            .filter(|(_, r)| !self.view.differences_only || !r.zero)
            .collect()
    }

    /// The check rows under the table for `tab` (the filter applied).
    #[must_use]
    pub fn checks(&self, tab: Tab) -> Vec<&Row> {
        self.checks_of(tab)
            .filter(|r| !self.view.differences_only || !r.zero)
            .collect()
    }

    /// Rows of `tab`, its checks and its totals that differ.
    #[must_use]
    pub fn differences(&self, tab: Tab) -> usize {
        self.of(tab.section())
            .chain(self.checks_of(tab))
            .filter(|r| !r.zero)
            .count()
            + usize::from(self.totals_differ.contains(&tab))
    }

    /// Everything that differs, totals included.
    #[must_use]
    pub fn all_differences(&self) -> usize {
        self.view.differences().len() + self.totals_differ.len()
    }

    /// Points the owner at the first difference the engine named (verify
    /// refused): its tab, only differences, and the row marked.
    pub fn point_at(&mut self, first: &[TieOutDifference]) {
        let Some(d) = first.iter().find(|d| d.section.is_some()) else {
            return;
        };
        let Some(section) = d.section else {
            return;
        };
        self.tab = Tab::of_difference(section, d.key.as_deref());
        self.view.differences_only = true;
        let id = d
            .account
            .map(|x| x.to_string())
            .or_else(|| d.transaction.map(|x| x.to_string()))
            .or_else(|| d.item.map(|x| x.to_string()));
        self.focus = match section {
            DifferenceSection::BalanceRows => self.view.rows.iter().position(|r| {
                r.input.section == Section::Checks
                    && Some(r.input.label.as_str()) == d.key.as_deref()
            }),
            DifferenceSection::Totals => None,
            _ => self
                .view
                .find(self.tab.section(), d.source_row, id.as_deref()),
        };
    }

    /// A tab's tile: its heading, its figure (the engine's), the line
    /// under it, and whether it reads as matching.
    #[must_use]
    pub fn tile(&self, tab: Tab, tie: &TieOut) -> Tile {
        let n = self.of(tab.section()).count();
        let differing = self.differences(tab);
        let total = tab.total(tie);
        let figure = if tab == Tab::TrialBalance {
            // No difference total from the engine: 0.00 when nothing on
            // the tab differs, else how many things do. The same count as
            // the line under it (accounts, the tab's check rows and its
            // totals), so the two never disagree.
            if differing == 0 {
                Money::ZERO.format(MoneyStyle::default())
            } else {
                t_count("imports.tieout.accounts_differ", differing, &[])
            }
        } else {
            // The aging total's difference, unless it is 0.00 and the
            // side's own check is not: the tile never reads 0.00 while the
            // side differs.
            let check = tab.tile_check().and_then(|k| {
                self.checks_of(tab)
                    .find(|r| r.input.label == k.as_wire())
                    .map(|r| (r.zero, r.diff_text.clone()))
            });
            match (total, check) {
                (Some(x), _) if !x.diff.is_zero() => diff_text(x.diff),
                (Some(x), None | Some((true, _))) => diff_text(x.diff),
                (_, Some((_, d))) => d,
                (None, None) => String::new(),
            }
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
        // "9 open invoices · 6,880.00": the file's own total.
        let count = match total {
            Some(x) => format!("{count} · {}", x.source.format(MoneyStyle::default())),
            None => count,
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
            figure,
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

/// The source as a column heading (short: "Other program" for the
/// spreadsheet layout).
fn source_name(s: &ImportScreen) -> String {
    let short = format!("imports.source_short.{}", s.wizard.source.key());
    if crate::i18n::has(&short) {
        t(&short)
    } else {
        t(&format!("imports.source.{}", s.wizard.source.key()))
    }
}

/// The source inside a sentence ("…exported from another program").
fn source_phrase(s: &ImportScreen) -> String {
    let phrase = format!("imports.source_phrase.{}", s.wizard.source.key());
    if crate::i18n::has(&phrase) {
        t(&phrase)
    } else {
        t(&format!("imports.source.{}", s.wizard.source.key()))
    }
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

/// A row's first cell: its label, and "only in ClientLedger" for a row of
/// the books no source row made.
fn row_label(r: &Row) -> String {
    let label = if r.input.label.is_empty() {
        t("imports.tieout.unnamed")
    } else {
        r.input.label.clone()
    };
    if r.input.books_only() {
        t_with("imports.tieout.books_only", &[("label", &label)])
    } else {
        label
    }
}

/// A row's difference cell: the amount, and the quantity when it differs.
fn row_diff(r: &Row) -> String {
    match &r.quantity_text {
        Some(q) => t_with(
            "imports.tieout.quantity",
            &[("diff", &r.diff_text), ("qty", q)],
        ),
        None => r.diff_text.clone(),
    }
}

fn amount(m: Money) -> String {
    m.format(MoneyStyle::default())
}

/// The total rows of a tab: the trial balance's debit and credit columns,
/// or the side's totals (label, source, books, difference).
fn total_rows(st: &TieState, tab: Tab, tie: &TieOut) -> Vec<[String; 4]> {
    match tab {
        Tab::TrialBalance => tie.totals.accounts.map_or_else(Vec::new, |x| {
            // The mockup's "Total (all N accounts)" row, with the tab's
            // difference figure (the tile's: 0.00 when no account differs,
            // else how many do; never a sum the engine didn't send).
            vec![
                [
                    t_count(
                        "imports.tieout.total.debits_of",
                        st.of(Section::Accounts).count(),
                        &[],
                    ),
                    amount(x.source_debit),
                    amount(x.books_debit),
                    st.tile(Tab::TrialBalance, tie).figure,
                ],
                [
                    t("imports.tieout.total.credits"),
                    amount(x.source_credit),
                    amount(x.books_credit),
                    String::new(),
                ],
            ]
        }),
        _ => tab.total(tie).map_or_else(Vec::new, |x| {
            vec![[
                t_count(
                    "imports.tieout.total.rows",
                    st.of(tab.section()).count(),
                    &[],
                ),
                amount(x.source),
                amount(x.books),
                diff_text(x.diff),
            ]]
        }),
    }
}

/// The step's width that is on screen: the body scrolls both ways, so its
/// `max_rect` can run past what shows (D6: the 4th tile, Save PDF and the
/// Difference column were cut off at 1440 wide).
#[allow(clippy::float_arithmetic, reason = "layout pixels")]
fn visible_width(ui: &Ui) -> f32 {
    let left = ui.cursor().left();
    (ui.clip_rect().right() - left)
        .min(ui.available_width())
        .max(MIN_WIDTH)
}

const MIN_WIDTH: f32 = 340.0;
const TILE_GAP: f32 = 8.0;
const TILE_MIN: f32 = 180.0;

/// Draws the step.
pub fn tieout_ui(s: &mut ImportScreen, ui: &mut Ui, tokens: &Tokens) {
    let width = visible_width(ui);
    ui.set_max_width(width);
    let source = source_name(s);
    ui.add(
        egui::Label::new(
            RichText::new(t_with(
                "imports.tieout.intro",
                &[("source", &source_phrase(s))],
            ))
            .color(tokens.text_secondary),
        )
        .wrap(),
    );
    ui.add_space(10.0);
    // An undone import has nothing to check (UI review: no ticks, no
    // verify).
    if s.undone {
        ui.label(RichText::new(t("imports.tieout.undone")).color(tokens.warn_text));
        return;
    }
    let (Some(tie), Some(st)) = (s.tieout.as_ref(), s.tie.as_ref()) else {
        ui.label(t("imports.tieout.loading"));
        return;
    };
    let tabs = st.tabs();
    tiles_ui(
        ui,
        tokens,
        width,
        &tabs
            .iter()
            .map(|tab| st.tile(*tab, tie))
            .collect::<Vec<_>>(),
    );
    ui.add_space(10.0);
    let mut save = false;
    ui.horizontal_wrapped(|ui| {
        // Each tab is named by its own words (UI review: all four read
        // "Compare" when labelled by the group).
        ui.label(RichText::new(t("imports.tieout.compare")).color(tokens.text_secondary));
        for tab in &tabs {
            let on = s.tie.as_ref().is_some_and(|x| x.tab == *tab);
            let r = ui.selectable_label(on, t(&format!("imports.tieout.tab.{}", tab.key())));
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
        // F4: Save PDF at the right end of the tab row (when it fits).
        let pdf = |ui: &mut Ui| {
            ui.add_enabled(
                s.pdf.is_none(),
                egui::Button::new(t("imports.tieout.save_pdf")),
            )
            .clicked()
        };
        if ui.available_width() > 140.0 {
            save = ui
                .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    pdf(ui)
                })
                .inner;
        } else {
            save = pdf(ui);
        }
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
    table_ui(ui, tokens, width, st, tie, &source);
    ui.add_space(8.0);
    if st.view.checkmark() {
        ui.horizontal(|ui| {
            crate::widgets::check_mark(ui, 16.0, tokens.accent, &t("imports.tieout.check_mark"));
            ui.label(t("imports.tieout.all_zero"));
        });
    } else {
        let n = st.all_differences();
        ui.add(
            egui::Label::new(
                RichText::new(t_count("imports.tieout.differences", n, &[]))
                    .color(tokens.warn_text),
            )
            .wrap(),
        );
        if st.focus.is_some() {
            ui.label(RichText::new(t("imports.tieout.first_marked")).color(tokens.warn_text));
        }
    }
    if !s.allowed() {
        ui.label(t("imports.tieout.admin_only"));
    }
}

/// The tiles in one row when they fit, else two to a row; each the same
/// width, so none runs past the window.
#[allow(
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    reason = "layout pixels"
)]
fn tiles_ui(ui: &mut Ui, tokens: &Tokens, width: f32, tiles: &[Tile]) {
    if tiles.is_empty() {
        return;
    }
    let fits = |n: usize| (width - TILE_GAP * (n as f32 - 1.0)) / n as f32 >= TILE_MIN;
    let per_row = if fits(tiles.len()) {
        tiles.len()
    } else if fits(2) {
        2.min(tiles.len())
    } else {
        1
    };
    let w = (width - TILE_GAP * (per_row as f32 - 1.0)) / per_row as f32;
    for chunk in tiles.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TILE_GAP;
            for tile in chunk {
                tile_ui(ui, tokens, w, tile);
            }
        });
        ui.add_space(TILE_GAP);
    }
}

#[allow(clippy::float_arithmetic, reason = "layout pixels")]
fn tile_ui(ui: &mut Ui, tokens: &Tokens, width: f32, tile: &Tile) {
    // Frame margins (14 + 14) and the stroke come off the inside width.
    let inner = (width - 30.0).max(60.0);
    egui::Frame::new()
        .fill(tokens.accent_tint)
        .stroke(egui::Stroke::new(1.0, tokens.accent_border))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.vertical(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(&tile.head)
                            .size(12.0)
                            .color(tokens.text_secondary),
                    )
                    .truncate(),
                );
                let color = if tile.matches {
                    tokens.accent
                } else {
                    tokens.warn_text
                };
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&tile.figure)
                                .monospace()
                                .size(20.0)
                                .color(color),
                        )
                        .truncate(),
                    );
                    if tile.tick {
                        crate::widgets::check_mark(
                            ui,
                            18.0,
                            color,
                            &t("imports.tieout.check_mark"),
                        );
                    }
                });
                ui.add(
                    egui::Label::new(
                        RichText::new(&tile.under)
                            .size(12.0)
                            .color(tokens.text_secondary),
                    )
                    .wrap(),
                );
            });
        });
}

/// Column widths (pixels only; never an amount).
#[allow(clippy::float_arithmetic, reason = "layout pixels")]
fn widths(total: f32) -> [f32; 4] {
    let amount = (total * 0.2).clamp(80.0, 200.0);
    [(total - amount * 3.0).max(90.0), amount, amount, amount]
}

const ROW_H: f32 = 24.0;
/// The table's height before it scrolls (14 rows).
const TABLE_H: f32 = 336.0;
/// Rows the table always shows, however small the window.
const MIN_ROWS: f32 = 3.0;
/// The lines under the table (all zero or the differences, the Admin note).
const FOOTER_H: f32 = 64.0;

fn row_ui(ui: &mut Ui, w: &[f32; 4], cells: [RichText; 4]) -> egui::Response {
    row_ui_tick(ui, w, cells, None)
}

/// A row; `tick` draws a check mark after the last cell (the total row's
/// "0.00 ✓" when every difference is zero).
fn row_ui_tick(
    ui: &mut Ui,
    w: &[f32; 4],
    cells: [RichText; 4],
    tick: Option<egui::Color32>,
) -> egui::Response {
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
                if i == 3
                    && let Some(color) = tick
                {
                    crate::widgets::check_mark(ui, 14.0, color, &t("imports.tieout.check_mark"));
                    ui.add_space(4.0);
                }
                ui.add(egui::Label::new(cell).truncate());
            });
        }
    })
    .response
}

fn diff_cell(r: &Row, tokens: &Tokens) -> RichText {
    RichText::new(row_diff(r)).monospace().color(if r.zero {
        tokens.accent
    } else {
        tokens.warn_text
    })
}

#[allow(
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    reason = "layout pixels"
)]
fn table_ui(ui: &mut Ui, tokens: &Tokens, width: f32, st: &TieState, tie: &TieOut, source: &str) {
    let tab = st.tab;
    let rows = st.indexed(tab);
    // The frame's margins (12 + 12) and stroke come off the inside width.
    let inner = (width - 26.0).max(MIN_WIDTH - 26.0);
    let w = widths(inner);
    // The rows scroll inside the table and the total rows stay below them
    // (Q-265, P31 UI review): the rows get what is left of the visible body
    // once the heading, the totals, the checks and the closing lines have
    // their room, so the totals never fall off the bottom.
    let totals_n = total_rows(st, tab, tie).len();
    let checks_n = st.checks(tab).len();
    let reserve = ROW_H * 2.0
        + ROW_H * totals_n as f32
        + if checks_n > 0 {
            ROW_H * (checks_n as f32 + 1.0) + 8.0
        } else {
            0.0
        }
        + FOOTER_H;
    let visible = ui.clip_rect().bottom() - ui.cursor().top();
    let rows_h = (visible - reserve).clamp(ROW_H * MIN_ROWS, TABLE_H);
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, tokens.border_default))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(inner);
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
                    .max_height(rows_h)
                    .auto_shrink([false, true])
                    .show_rows(ui, ROW_H, rows.len(), |ui, range| {
                        for (i, r) in &rows[range] {
                            let marked = st.focus == Some(*i);
                            let label = RichText::new(row_label(r));
                            let r_ui = row_ui(
                                ui,
                                &w,
                                [
                                    if marked { label.strong() } else { label },
                                    RichText::new(&r.source_text).monospace(),
                                    RichText::new(&r.books_text).monospace(),
                                    diff_cell(r, tokens),
                                ],
                            );
                            if marked {
                                ui.painter().rect_stroke(
                                    r_ui.rect.expand(1.0),
                                    3,
                                    egui::Stroke::new(1.5, tokens.warn_text),
                                    egui::StrokeKind::Inside,
                                );
                            }
                        }
                    });
            }
            let totals = total_rows(st, tab, tie);
            if !totals.is_empty() {
                ui.separator();
            }
            for [label, src, books, diff] in totals {
                let zero = diff.is_empty() || diff == amount(Money::ZERO);
                let tick =
                    (zero && !diff.is_empty() && st.view.checkmark()).then_some(tokens.accent);
                row_ui_tick(
                    ui,
                    &w,
                    [
                        RichText::new(label).strong(),
                        RichText::new(src).monospace(),
                        RichText::new(books).monospace(),
                        RichText::new(diff).monospace().strong().color(if zero {
                            tokens.accent
                        } else {
                            tokens.warn_text
                        }),
                    ],
                    tick,
                );
            }
        });
    let checks = st.checks(tab);
    if !checks.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new(t("imports.tieout.checks")).strong());
        let w = widths(inner);
        for r in checks {
            let name = check_name(&r.input.label);
            row_ui(
                ui,
                &w,
                [
                    RichText::new(name),
                    RichText::new(""),
                    RichText::new(""),
                    diff_cell(r, tokens),
                ],
            )
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
            row_label(r),
            r.source_text.clone(),
            r.books_text.clone(),
            row_diff(r),
        ]
    };
    let mut sections = Vec::new();
    for tab in st.tabs() {
        let mut rows: Vec<Vec<String>> = st.of(tab.section()).map(line).collect();
        rows.extend(total_rows(st, tab, tie).into_iter().map(Vec::from));
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
    use cl_api::imports::{BalanceRow, TieOutAccount, TieOutStock, TieOutTotals};
    use cl_core::CalendarDate;
    use rust_decimal::Decimal;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn account(key: &str, d: &str) -> TieOutAccount {
        TieOutAccount {
            account: None,
            number: Some(key.into()),
            name: String::new(),
            source_row: Some(3),
            source_rows: vec![3],
            source_keys: vec![key.into()],
            source: money("10.00"),
            books: money("10.00"),
            diff: money(d),
        }
    }

    fn balance(key: BalanceKey, d: &str) -> BalanceRow {
        BalanceRow {
            key,
            account: None,
            source: money("0.00"),
            books: money(d),
            diff: money(d),
        }
    }

    fn total(source: &str, d: &str) -> TieOutTotal {
        TieOutTotal {
            source: money(source),
            books: money(source),
            diff: money(d),
        }
    }

    fn tie(accounts: Vec<TieOutAccount>, all_zero: bool) -> TieOut {
        TieOut {
            batch: None,
            as_of: CalendarDate::from_ymd(2026, 10, 27).expect("date"),
            computed_at: None,
            books: None,
            reference: None,
            accounts,
            ar_items: vec![TieOutItem {
                source_row: Some(2),
                session: None,
                transaction: None,
                name: "Brightwater HOA".into(),
                number: Some("1005".into()),
                doc_type: Some("invoice".into()),
                date: None,
                source: money("3500.00"),
                books: money("3500.00"),
                diff: money("0"),
            }],
            ap_items: Vec::new(),
            inventory_items: Vec::new(),
            balance_rows: vec![
                balance(BalanceKey::HistoryAr, "-10.00"),
                balance(BalanceKey::HistoryAp, "0"),
            ],
            totals: TieOutTotals {
                accounts: None,
                ar: Some(total("6880.00", "0.00")),
                ap: None,
                inventory: None,
            },
            difference_count: 0,
            all_zero,
            verified_at: None,
        }
    }

    /// UI review: a made-up key is never shown; its kind and name are.
    #[test]
    fn a_made_up_key_reads_as_its_kind_and_name() {
        let i = TieOutItem {
            source_row: Some(4),
            session: None,
            transaction: None,
            name: String::new(),
            number: Some("payment:Riverside Cafe".into()),
            doc_type: None,
            date: None,
            source: money("-300.00"),
            books: money("-300.00"),
            diff: money("0.00"),
        };
        assert_eq!(item_label(&i), "Payment · Riverside Cafe");
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
        // No total is derived: the count of what differs (both accounts and
        // the history A/R check), as the line under it says.
        assert_eq!(tile.figure, "3 differ");
        assert!(tile.under.ends_with("· 3 differ"), "{}", tile.under);
        assert!(!tile.tick && !tile.matches);
        assert_eq!(tile.head, "Trial balance at 10/27/2026");
        // ✓ only on the engine's word, whatever the rows say.
        let t = tie(vec![account("1000", "0")], false);
        let tile = TieState::new(&t).tile(Tab::TrialBalance, &t);
        assert!(!tile.tick);
        // The accounts tie but the history A/R check does not: the tile
        // says so, as its line does.
        assert_eq!(tile.figure, "1 differs");
    }

    /// The trial balance tile's "N differ" and the line under it count the
    /// same things: an account difference and a check-row difference are 2
    /// in both (it read "1 differs" over "· 2 differ" before).
    #[test]
    fn the_trial_balance_tile_figure_and_line_count_the_same() {
        let mut t = tie(vec![account("1000", "5.00"), account("1200", "0")], false);
        t.balance_rows = vec![
            balance(BalanceKey::HistoryAr, "3.00"),
            balance(BalanceKey::HistoryAp, "0.00"),
        ];
        let st = TieState::new(&t);
        let tile = st.tile(Tab::TrialBalance, &t);
        // The account and the check row.
        assert_eq!(st.differences(Tab::TrialBalance), 2);
        assert_eq!(tile.figure, "2 differ");
        assert!(tile.under.ends_with("· 2 differ"), "{}", tile.under);
        let t = tie(vec![account("1000", "5.00")], false);
        let st = TieState::new(&t);
        let tile = st.tile(Tab::TrialBalance, &t);
        assert_eq!(st.differences(Tab::TrialBalance), 2);
        assert_eq!(tile.figure, "2 differ");
        assert!(tile.under.ends_with("· 2 differ"), "{}", tile.under);
    }

    #[test]
    fn the_aging_tiles_are_the_engines_totals() {
        let mut t = tie(vec![account("1000", "0")], false);
        let st = TieState::new(&t);
        let tile = st.tile(Tab::Receivables, &t);
        assert_eq!(tile.figure, "-10.00", "the aging ties; its check does not");
        assert!(
            tile.under.starts_with("1 open invoice · 6,880.00"),
            "{}",
            tile.under
        );
        assert!(!tile.matches, "Receivables before conversion differs");
        // A side's total that differs counts, even with every row at 0.00.
        t.balance_rows.clear();
        t.totals.ar = Some(total("6880.00", "45.00"));
        let st = TieState::new(&t);
        assert_eq!(st.tile(Tab::Receivables, &t).figure, "45.00");
        assert_eq!(st.differences(Tab::Receivables), 1);
        assert_eq!(st.all_differences(), 1);
        let rows = total_rows(&st, Tab::Receivables, &t);
        assert_eq!(rows[0][1], "6,880.00");
        assert_eq!(rows[0][3], "45.00");
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
        assert_eq!(st.differences(Tab::Receivables), 1);
        assert!(st.tile(Tab::Payables, &t).matches);
        st.view.differences_only = true;
        assert_eq!(st.rows(Tab::TrialBalance).len(), 1);
        assert_eq!(st.rows(Tab::Receivables).len(), 0);
        assert_eq!(st.checks(Tab::TrialBalance).len(), 1);
    }

    #[test]
    fn books_only_rows_and_quantities_show() {
        let mut t = tie(vec![account("1000", "0")], false);
        let mut extra = account("3900", "-25.00");
        extra.source_row = None;
        extra.source_keys.clear();
        extra.name = "Retained Earnings".into();
        t.accounts.push(extra);
        t.inventory_items.push(TieOutStock {
            source_row: Some(5),
            name: "Live Oak 15gal".into(),
            item: None,
            source_quantity: Decimal::new(125, 1),
            books_quantity: Decimal::new(12, 0),
            quantity_diff: Decimal::new(-5, 1),
            source: money("1125.00"),
            books: money("1125.00"),
            diff: money("0.00"),
        });
        let st = TieState::new(&t);
        let rows = st.rows(Tab::TrialBalance);
        assert_eq!(
            row_label(rows[1]),
            "3900 · Retained Earnings · only in ClientLedger"
        );
        let stock = st.rows(Tab::Inventory);
        assert!(!stock[0].zero, "a quantity difference is a difference");
        assert_eq!(row_diff(stock[0]), "0.00 · quantity -0.5");
        assert!(st.tabs().contains(&Tab::Inventory));
    }

    #[test]
    fn a_refused_verify_points_at_the_first_difference() {
        let mut t = tie(vec![account("1000", "0"), account("1200", "10.00")], false);
        t.accounts[1].source_row = Some(7);
        let mut st = TieState::new(&t);
        st.point_at(&[TieOutDifference {
            section: Some(DifferenceSection::Accounts),
            key: None,
            source_row: Some(7),
            account: None,
            transaction: None,
            item: None,
        }]);
        assert_eq!(st.tab, Tab::TrialBalance);
        assert!(st.view.differences_only);
        assert_eq!(st.focus, Some(1));
        st.point_at(&[TieOutDifference {
            section: Some(DifferenceSection::BalanceRows),
            key: Some("history_ar".into()),
            source_row: None,
            account: None,
            transaction: None,
            item: None,
        }]);
        assert_eq!(st.tab, Tab::Receivables);
        let focused = st.focus.map(|i| st.view.rows[i].input.label.clone());
        assert_eq!(focused.as_deref(), Some("history_ar"));
    }

    #[test]
    fn other_programs_have_only_the_trial_balance() {
        let mut t = tie(vec![account("1000", "0")], true);
        t.ar_items.clear();
        t.balance_rows.clear();
        t.totals.ar = None;
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
        for k in [
            "imports.tieout.accounts_differ",
            "imports.tieout.total.debits",
            "imports.tieout.total.credits",
            "imports.tieout.total.rows",
            "imports.tieout.books_only",
            "imports.tieout.unnamed",
            "imports.tieout.quantity",
            "imports.tieout.first_marked",
            "imports.tieout.check_mark",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
    }
}
