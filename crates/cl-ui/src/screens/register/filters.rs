//! The register toolbar (P12 WP5): Account, Show (date range), Sort by and
//! One line, with Go To, QuickReport and Edit Transaction; and the
//! quick-filter row (WP7, Q-189): Uncleared · Payee · Tag · Amount from/to,
//! "Clear filters" and the legend.
//!
//! Filters go to the engine as query fields; the screen never filters or
//! adds up rows itself. Typed amounts are exact `Money` from
//! `cl_model::amount_entry`. Uncleared and Tag need `register-exact-v1`
//! (R3); before it they are switched off with a plain tooltip (Q-P12d).

use cl_core::Money;
use cl_core::ids::{AccountId, TagId};
use cl_grid::{LookupResult, Lookups};
use cl_model::amount_entry::{self, AmountKind, AmountRules, Sign};
use cl_model::date_entry::{self, DateOrder};
use cl_model::register::{AccountClass, Filters, RangePreset, SortChoice};
use cl_model::{CommandId, commands};
use eframe::egui::{self, RichText, Ui};

use super::lists::NAMES;
use super::{RegisterScreen, account_text, class_of};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::theme::Tokens;

/// The typed text of the quick filters (applied on Enter or when the
/// field is left).
#[derive(Debug, Default)]
pub struct QuickInput {
    /// Payee text as typed.
    pub payee: String,
    /// Amount from, as typed.
    pub min: String,
    /// Amount to, as typed.
    pub max: String,
    /// Why the typed filters were not applied (i18n key).
    pub problem: Option<&'static str>,
}

/// Rules for a filter amount: cents, zero or more.
const FILTER_AMOUNT: AmountRules = AmountRules {
    kind: AmountKind::Money,
    input: cl_core::UserInput::CENTS,
    sign: Sign::NonNegative,
};

/// A typed filter amount: empty is "no bound".
///
/// # Errors
/// The i18n key of the owner message.
pub fn parse_bound(text: &str) -> Result<Option<Money>, &'static str> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    amount_entry::parse(text, FILTER_AMOUNT)
        .map(|p| p.value.as_money())
        .map_err(|e| e.key().as_str())
}

/// The shortcut text of a command ("Ctrl+F"), empty when it has none.
fn shortcut_hint(id: &str) -> String {
    commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
}

/// Register classes in the Account picker's order, with their group label.
const GROUPS: [(AccountClass, &str); 7] = [
    (AccountClass::Bank, "register.group.bank"),
    (AccountClass::CreditCard, "register.group.credit_card"),
    (AccountClass::Receivable, "register.group.receivable"),
    (AccountClass::OtherAsset, "register.group.other_asset"),
    (AccountClass::Payable, "register.group.payable"),
    (
        AccountClass::OtherLiability,
        "register.group.other_liability",
    ),
    (AccountClass::Equity, "register.group.equity"),
];

/// The line under the heading ("Bank account").
#[must_use]
pub fn class_key(c: AccountClass) -> &'static str {
    match c {
        AccountClass::Bank => "register.class.bank",
        AccountClass::CreditCard => "register.class.credit_card",
        AccountClass::Receivable => "register.class.receivable",
        AccountClass::OtherAsset => "register.class.other_asset",
        AccountClass::Payable => "register.class.payable",
        AccountClass::OtherLiability => "register.class.other_liability",
        AccountClass::Equity => "register.class.equity",
        AccountClass::IncomeOrExpense => "register.class.other",
    }
}

/// The class as the control channel reports it.
#[must_use]
pub fn class_name(c: AccountClass) -> &'static str {
    match c {
        AccountClass::Bank => "bank",
        AccountClass::CreditCard => "credit_card",
        AccountClass::Receivable => "receivable",
        AccountClass::OtherAsset => "other_asset",
        AccountClass::Payable => "payable",
        AccountClass::OtherLiability => "other_liability",
        AccountClass::Equity => "equity",
        AccountClass::IncomeOrExpense => "other",
    }
}

/// One choice in a picker; `None` entries are group captions.
struct Choice {
    text: String,
    caption: bool,
}

/// A labelled choice that works from the keyboard: Up/Down change it while
/// it has focus, a click opens the list. Returns the picked index.
fn combo(
    ui: &mut Ui,
    salt: &str,
    label: &str,
    choices: &[Choice],
    current: usize,
) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        let caption = ui.add(egui::Label::new(label).selectable(false));
        let shown = choices
            .get(current)
            .map(|c| c.text.clone())
            .unwrap_or_default();
        let inner = egui::ComboBox::from_id_salt(salt)
            .selected_text(shown)
            .height(480.0)
            .show_ui(ui, |ui| {
                for (i, c) in choices.iter().enumerate() {
                    if c.caption {
                        ui.add(
                            egui::Label::new(RichText::new(&c.text).small().strong())
                                .selectable(false),
                        );
                    } else if ui.selectable_label(i == current, &c.text).clicked() {
                        picked = Some(i);
                    }
                }
            });
        let resp = inner.response.labelled_by(caption.id);
        if resp.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    resp.id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..egui::EventFilter::default()
                    },
                );
            });
            let (down, up) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                )
            });
            let step = |from: usize, fwd: bool| {
                let mut i = from;
                loop {
                    i = if fwd {
                        i.checked_add(1).filter(|n| *n < choices.len())?
                    } else {
                        i.checked_sub(1)?
                    };
                    if !choices[i].caption {
                        return Some(i);
                    }
                }
            };
            if down {
                picked = step(current, true).or(picked);
            }
            if up {
                picked = step(current, false).or(picked);
            }
        }
    });
    picked.filter(|p| *p != current)
}

impl RegisterScreen {
    /// The accounts with a register, grouped by class, as picker choices
    /// (captions included) and the account behind each choice.
    fn account_choices(&self) -> (Vec<Choice>, Vec<Option<AccountId>>) {
        let mut choices = Vec::new();
        let mut ids = Vec::new();
        for (class, caption) in GROUPS {
            let mut members: Vec<_> = self
                .accounts
                .iter()
                .filter(|a| {
                    class_of(a) == Some(class) && (a.is_active || Some(a.id) == self.account)
                })
                .collect();
            if members.is_empty() {
                continue;
            }
            members.sort_by(|a, b| {
                a.fields
                    .account_number
                    .cmp(&b.fields.account_number)
                    .then_with(|| a.fields.name.cmp(&b.fields.name))
            });
            choices.push(Choice {
                text: t(caption),
                caption: true,
            });
            ids.push(None);
            for a in members {
                choices.push(Choice {
                    text: account_text(a),
                    caption: false,
                });
                ids.push(Some(a.id));
            }
        }
        (choices, ids)
    }

    /// Picks another account: this window shows it when it shows none yet;
    /// otherwise its own tab opens (one tab per account).
    pub(crate) fn pick_account(&mut self, id: AccountId) {
        if self.account == Some(id) {
            return;
        }
        if self.account.is_none() {
            self.show_account(id);
        } else {
            self.link.push(CompanyEvent::OpenRegister(id));
        }
    }

    /// Changes the "Show" range.
    pub(crate) fn set_range(&mut self, range: RangePreset) {
        self.prefs.range = range;
        self.prefs_dirty = true;
        self.apply_query();
    }

    /// Changes the sort.
    pub(crate) fn set_sort(&mut self, sort: SortChoice) {
        self.prefs.sort = sort;
        self.prefs_dirty = true;
        self.apply_query();
    }

    /// One-line display on or off.
    pub(crate) fn set_one_line(&mut self, on: bool) {
        self.prefs.one_line = on;
        self.prefs_dirty = true;
        self.sync_grid();
    }

    pub(super) fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal_wrapped(|ui| {
            let (choices, ids) = self.account_choices();
            let current = ids
                .iter()
                .position(|id| id.is_some() && *id == self.account)
                .unwrap_or(0);
            if let Some(i) = combo(
                ui,
                "register/account",
                &t("register.account"),
                &choices,
                current,
            ) && let Some(Some(id)) = ids.get(i)
            {
                self.pick_account(*id);
            }

            let mut ranges: Vec<RangePreset> = RangePreset::PRESETS.to_vec();
            if let RangePreset::Custom { .. } = self.prefs.range {
                ranges.push(self.prefs.range);
            }
            let mut texts: Vec<Choice> = ranges
                .iter()
                .map(|r| Choice {
                    text: self.range_text(*r),
                    caption: false,
                })
                .collect();
            texts.push(Choice {
                text: t("register.range.custom"),
                caption: false,
            });
            let current = ranges
                .iter()
                .position(|r| *r == self.prefs.range)
                .unwrap_or(0);
            if let Some(i) = combo(ui, "register/range", &t("register.show"), &texts, current) {
                match ranges.get(i) {
                    Some(r) => {
                        self.custom.open = false;
                        self.set_range(*r);
                    }
                    None => self.open_custom(),
                }
            }

            let sorts: Vec<Choice> = SortChoice::ALL
                .iter()
                .map(|s| Choice {
                    text: t(s.label().as_str()),
                    caption: false,
                })
                .collect();
            let current = SortChoice::ALL
                .iter()
                .position(|s| *s == self.prefs.sort)
                .unwrap_or(0);
            if let Some(i) = combo(ui, "register/sort", &t("register.sort_by"), &sorts, current)
                && let Some(s) = SortChoice::ALL.get(i)
            {
                self.set_sort(*s);
            }

            let mut one = self.prefs.one_line;
            if ui.checkbox(&mut one, t("register.one_line")).changed() {
                self.set_one_line(one);
            }

            ui.separator();
            let b = |ui: &mut Ui, key: &str, cmd: &str| {
                let hint = shortcut_hint(cmd);
                let tip = if hint.is_empty() {
                    t(key)
                } else {
                    format!("{} ({hint})", t(key))
                };
                ui.button(t(key)).on_hover_text(tip)
            };
            if b(ui, "register.goto.open", "edit.find").clicked() {
                self.command_from_toolbar("edit.find");
            }
            if b(ui, "register.quick_report", "list.quick_report").clicked() {
                self.command_from_toolbar("list.quick_report");
            }
            if b(ui, "register.edit_transaction", "edit.edit_transaction").clicked() {
                self.command_from_toolbar("edit.edit_transaction");
            }
        });
        if self.custom.open {
            self.custom_ui(ui, tokens);
        }
        self.quick_filters_ui(ui, tokens);
    }

    fn command_from_toolbar(&mut self, id: &'static str) {
        crate::windows::Screen::command(self, CommandId::new(id));
    }

    /// Replaces the filters (after the amount range is checked) and asks
    /// the engine again.
    pub(crate) fn set_filters(&mut self, f: Filters) {
        if let Err(p) = f.check() {
            self.quick.problem = Some(p.key().as_str());
            return;
        }
        self.quick.problem = None;
        if f != self.filters {
            self.filters = f;
            self.apply_query();
        }
    }

    /// "Clear filters".
    pub(crate) fn clear_filters(&mut self) {
        self.quick = QuickInput::default();
        let mut f = self.filters.clone();
        f.clear();
        self.set_filters(f);
    }

    /// The typed payee, completed to a known name when the typing starts
    /// one (autofill, as in the entry row).
    fn complete_payee(&self, typed: &str) -> String {
        let typed = typed.trim();
        if typed.is_empty() {
            return String::new();
        }
        if let Some(s) = self.lists.exact(NAMES, typed) {
            return s.label;
        }
        if let LookupResult::Ready(s) = self.lists.suggest(NAMES, typed, 1)
            && let Some(first) = s.first()
            && first
                .label
                .to_lowercase()
                .starts_with(&typed.to_lowercase())
        {
            return first.label.clone();
        }
        typed.to_string()
    }

    /// Applies the typed payee and amounts.
    pub(crate) fn apply_typed_filters(&mut self) {
        let payee = self.complete_payee(&self.quick.payee);
        self.quick.payee.clone_from(&payee);
        let (min, max) = match (parse_bound(&self.quick.min), parse_bound(&self.quick.max)) {
            (Ok(lo), Ok(hi)) => (lo, hi),
            (Err(p), _) | (_, Err(p)) => {
                self.quick.problem = Some(p);
                return;
            }
        };
        let mut f = self.filters.clone();
        f.payee = (!payee.is_empty()).then_some(payee);
        f.min = min;
        f.max = max;
        self.set_filters(f);
    }

    /// Sets the tag filter (`None` is any tag).
    pub(crate) fn set_tag_filter(&mut self, tag: Option<TagId>) {
        let mut f = self.filters.clone();
        f.tag = tag;
        self.set_filters(f);
    }

    /// Uncleared on or off.
    pub(crate) fn set_uncleared(&mut self, on: bool) {
        let mut f = self.filters.clone();
        f.uncleared = on;
        self.set_filters(f);
    }

    /// The quick-filter row and the legend.
    fn quick_filters_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let exact = self.register_exact();
        let mut typed_done = false;
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(t("register.filter.title")).color(tokens.text_secondary),
                )
                .selectable(false),
            );
            let mut uncleared = self.filters.uncleared;
            let r = ui.add_enabled(
                exact,
                egui::Checkbox::new(&mut uncleared, t("register.filter.uncleared")),
            );
            let r = if exact {
                r.on_hover_text(t("register.filter.uncleared_help"))
            } else {
                r.on_disabled_hover_text(t("register.filter.needs_engine"))
            };
            if r.changed() {
                self.set_uncleared(uncleared);
            }

            let l = ui.add(egui::Label::new(t("register.filter.payee")).selectable(false));
            let r = ui
                .add_sized(
                    [150.0, 22.0],
                    egui::TextEdit::singleline(&mut self.quick.payee)
                        .hint_text(t("register.filter.payee_hint")),
                )
                .labelled_by(l.id);
            if r.has_focus() {
                let typed = self.quick.payee.trim();
                let done = self.complete_payee(typed);
                if !typed.is_empty() && done != typed {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("\u{2192} {done}"))
                                .size(12.0)
                                .color(tokens.text_secondary),
                        )
                        .selectable(false),
                    );
                }
            }
            typed_done |= r.lost_focus();

            let (choices, ids) = self.tag_choices();
            let current = ids
                .iter()
                .position(|id| *id == self.filters.tag)
                .unwrap_or(0);
            ui.add_enabled_ui(exact, |ui| {
                if let Some(i) = combo(
                    ui,
                    "register/filter-tag",
                    &t("register.filter.tag"),
                    &choices,
                    current,
                ) && let Some(id) = ids.get(i)
                {
                    self.set_tag_filter(*id);
                }
            })
            .response
            .on_disabled_hover_text(t("register.filter.needs_engine"));

            let l = ui.add(egui::Label::new(t("register.filter.amount_from")).selectable(false));
            let r = ui
                .add_sized(
                    [90.0, 22.0],
                    egui::TextEdit::singleline(&mut self.quick.min),
                )
                .labelled_by(l.id);
            typed_done |= r.lost_focus();
            let l = ui.add(egui::Label::new(t("register.filter.amount_to")).selectable(false));
            let r = ui
                .add_sized(
                    [90.0, 22.0],
                    egui::TextEdit::singleline(&mut self.quick.max),
                )
                .labelled_by(l.id);
            typed_done |= r.lost_focus();

            if self.filters.is_active() && ui.link(t("register.filter.clear")).clicked() {
                self.clear_filters();
            }

            ui.separator();
            legend(ui, tokens);
        });
        if typed_done {
            self.apply_typed_filters();
        }
        if let Some(p) = self.quick.problem {
            ui.label(RichText::new(t(p)).color(tokens.warn_text));
        }
    }

    /// "Any tag" then the active tags (and the chosen one), by name.
    fn tag_choices(&self) -> (Vec<Choice>, Vec<Option<TagId>>) {
        let mut tags: Vec<_> = self
            .entry
            .tags
            .iter()
            .filter(|g| g.is_active || Some(g.id) == self.filters.tag)
            .collect();
        tags.sort_by(|a, b| a.fields.name.cmp(&b.fields.name));
        let mut choices = vec![Choice {
            text: t("register.filter.any_tag"),
            caption: false,
        }];
        let mut ids = vec![None];
        for g in tags {
            choices.push(Choice {
                text: g.fields.name.clone(),
                caption: false,
            });
            ids.push(Some(g.id));
        }
        (choices, ids)
    }
}

/// The legend: locked rows, R and ✓ (mockup `Register.html`).
fn legend(ui: &mut Ui, tokens: &Tokens) {
    let small = |s: String| RichText::new(s).size(12.0).color(tokens.text_secondary);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().rect(
        rect,
        0.0,
        tokens.locked_row,
        egui::Stroke::new(1.0, tokens.border_default),
        egui::StrokeKind::Inside,
    );
    ui.add(egui::Label::new(small(t("register.legend.locked"))).selectable(false));
    ui.add(egui::Label::new(small(t("register.legend.reconciled"))).selectable(false));
    ui.add(egui::Label::new(small(t("register.legend.cleared"))).selectable(false));
}

impl RegisterScreen {
    /// "Last 90 days (08/06/2026 – 11/04/2026)".
    fn range_text(&self, r: RangePreset) -> String {
        let d = r.resolve(self.today, &self.fiscal);
        let dates = format!(
            "{} \u{2013} {}",
            crate::shell::us_date(d.start),
            crate::shell::us_date(d.end)
        );
        match r {
            RangePreset::Custom { .. } => dates,
            RangePreset::AllDates => t(r.label().as_str()),
            _ => format!("{} ({dates})", t(r.label().as_str())),
        }
    }

    fn open_custom(&mut self) {
        let d = self.prefs.range.resolve(self.today, &self.fiscal);
        self.custom.open = true;
        self.custom.from = crate::shell::us_date(d.start);
        self.custom.to = crate::shell::us_date(d.end);
        self.custom.problem = None;
    }

    fn custom_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal_wrapped(|ui| {
            let l = ui.add(egui::Label::new(t("register.range.from")).selectable(false));
            ui.add_sized(
                [110.0, 24.0],
                egui::TextEdit::singleline(&mut self.custom.from),
            )
            .labelled_by(l.id);
            let l = ui.add(egui::Label::new(t("register.range.to")).selectable(false));
            ui.add_sized(
                [110.0, 24.0],
                egui::TextEdit::singleline(&mut self.custom.to),
            )
            .labelled_by(l.id);
            if ui.button(t("register.range.apply")).clicked() {
                self.apply_custom();
            }
            if ui.button(t("register.range.cancel")).clicked() {
                self.custom.open = false;
            }
        });
        if let Some(p) = self.custom.problem {
            ui.label(RichText::new(t(p)).color(tokens.warn_text));
        }
    }

    /// Applies the typed custom range; a bad date or From after To is
    /// refused with a plain message.
    pub(crate) fn apply_custom(&mut self) {
        let parse = |s: &str| date_entry::parse(s, DateOrder::Mdy, self.today).ok();
        let (Some(from), Some(to)) = (parse(&self.custom.from), parse(&self.custom.to)) else {
            self.custom.problem = Some("register.range.problem.bad_date");
            return;
        };
        match RangePreset::custom(from, to) {
            Ok(r) => {
                self.custom = super::CustomRange::default();
                self.set_range(r);
            }
            Err(p) => self.custom.problem = Some(p.key().as_str()),
        }
    }
}
