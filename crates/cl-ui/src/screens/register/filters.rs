//! The register toolbar (P12 WP5, part 1 of `filters.rs`): Account, Show
//! (date range), Sort by and One line. WP7 adds the quick-filter row.

use cl_core::ids::AccountId;
use cl_model::date_entry::{self, DateOrder};
use cl_model::register::{AccountClass, RangePreset, SortChoice};
use eframe::egui::{self, RichText, Ui};

use super::{RegisterScreen, account_text, class_of};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::theme::Tokens;

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
        });
        if self.custom.open {
            self.custom_ui(ui, tokens);
        }
    }

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
