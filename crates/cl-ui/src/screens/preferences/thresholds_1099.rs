//! Preferences › 1099 (P29 WP8, U-215, `1099-thresholds-v1`): the
//! company's own reporting threshold per 1099 box, Admin only.
//!
//! A blank box uses the year's standard amount, shown as the placeholder
//! only when the engine reports it (never invented here). Amounts are typed
//! and parsed with `rust_decimal` through [`cl_core::Money`] (two places at
//! most, zero or more), and only boxes whose amount changed are sent.
//! Logs carry box codes only, never amounts.

use std::collections::BTreeMap;

use cl_api::Thresholds1099;
use cl_core::{Money, UserInput};
use eframe::egui::{self, RichText, Ui};

use crate::i18n::{has, t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// What the owner typed per box code (blank: the year's standard amount).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThresholdEdits {
    /// Box code → typed text.
    pub typed: BTreeMap<String, String>,
}

impl ThresholdEdits {
    /// The engine's amounts as the fields show them.
    #[must_use]
    pub fn from_engine(t: Option<&Thresholds1099>) -> ThresholdEdits {
        ThresholdEdits {
            typed: t
                .map(|t| {
                    t.boxes
                        .iter()
                        .map(|b| (b.code.clone(), b.amount.map(show).unwrap_or_default()))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// The boxes whose typed amount differs from what the engine holds
    /// (`None`: back to the standard amount).
    ///
    /// # Errors
    /// The code of the first box whose text is not an amount of zero or
    /// more with at most two decimal places.
    pub fn changes(
        &self,
        loaded: &Thresholds1099,
    ) -> Result<BTreeMap<String, Option<Money>>, String> {
        let mut out = BTreeMap::new();
        for b in &loaded.boxes {
            let text = self.typed.get(&b.code).map_or("", |s| s.trim());
            let typed = parse(text).map_err(|NotAnAmount| b.code.clone())?;
            if typed != b.amount {
                out.insert(b.code.clone(), typed);
            }
        }
        Ok(out)
    }
}

/// The typed text is not a 1099 amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotAnAmount;

/// Parses one typed threshold: blank → `None`; otherwise an amount of zero
/// or more with at most two decimals (more are refused, not rounded).
///
/// # Errors
/// Anything else, a negative amount included.
pub fn parse(text: &str) -> Result<Option<Money>, NotAnAmount> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match Money::parse_user(text, UserInput::CENTS) {
        Ok(m) if !m.is_negative() => Ok(Some(m)),
        _ => Err(NotAnAmount),
    }
}

fn show(m: Money) -> String {
    m.format(cl_core::MoneyStyle::default())
}

/// The owner's name for a box: ours when we have one, else the engine's.
fn box_label(code: &str, engine_label: &str, form_name: &str, number: &str) -> String {
    let key = format!("prefs.1099.box.{code}");
    if has(&key) {
        t(&key)
    } else {
        t_with(
            "prefs.1099.box_other",
            &[
                ("label", engine_label),
                ("form", form_name),
                ("box", number),
            ],
        )
    }
}

/// Why the panel shows no amounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Amounts read; the owner may change them.
    Ready,
    /// The signed-in user is not an Admin.
    NotAdmin,
    /// The engine has no company thresholds (older engine).
    NeedsEngine,
    /// The read failed or has not finished.
    NotLoaded,
}

/// Draws the panel.
pub fn panel(
    ui: &mut Ui,
    tokens: &Tokens,
    edits: &mut ThresholdEdits,
    loaded: Option<&Thresholds1099>,
    available: Availability,
) {
    form::heading(ui, tokens, &t("prefs.1099.title"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        let (Availability::Ready, Some(loaded)) = (available, loaded) else {
            form::hint(
                ui,
                tokens,
                &t(match available {
                    Availability::NotAdmin => "prefs.1099.admin_only",
                    Availability::NeedsEngine => "prefs.1099.needs_engine",
                    _ => "prefs.1099.not_loaded",
                }),
            );
            return;
        };
        form::hint(
            ui,
            tokens,
            &t_with("prefs.1099.intro", &[("year", &loaded.year.to_string())]),
        );
        ui.add_space(6.0);
        egui::Grid::new("prefs-1099-grid")
            .num_columns(3)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label(RichText::new(t("prefs.1099.col_box")).strong());
                ui.label(RichText::new(t("prefs.1099.col_amount")).strong());
                ui.label(RichText::new(t("prefs.1099.col_standard")).strong());
                ui.end_row();
                for b in &loaded.boxes {
                    let label = box_label(&b.code, &b.label, &b.form, &b.box_number);
                    let l = ui.add(egui::Label::new(&label).selectable(false));
                    let standard = b.standard.map(show);
                    let text = edits.typed.entry(b.code.clone()).or_default();
                    let mut edit = egui::TextEdit::singleline(text).desired_width(140.0);
                    if let Some(s) = &standard {
                        edit = edit.hint_text(s.as_str());
                    }
                    ui.add(edit).labelled_by(l.id);
                    ui.label(
                        RichText::new(standard.unwrap_or_else(|| t("prefs.1099.standard_unknown")))
                            .color(tokens.text_secondary),
                    );
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        form::hint(ui, tokens, &t("prefs.1099.hint"));
    });
}

/// The demo engine's box table for 2026. It mirrors the engine's own table
/// (`engine/packages/api/src/services/portal-1099.boxes.ts`: the general
/// IRC 6041 amount is $2,000 from 2026, royalties $10, attorneys $600) so
/// the demo reports a standard amount the way the engine does; the app
/// itself never makes one up for a live company.
const DEMO_YEAR: i32 = 2026;
const DEMO_BOXES: [(&str, &str, &str, &str, &str); 6] = [
    ("NEC-1", "1099-NEC", "1", "Nonemployee compensation", "2000"),
    ("MISC-1", "1099-MISC", "1", "Rents", "2000"),
    ("MISC-2", "1099-MISC", "2", "Royalties", "10"),
    ("MISC-3", "1099-MISC", "3", "Other income", "2000"),
    (
        "MISC-6",
        "1099-MISC",
        "6",
        "Medical & health care payments",
        "2000",
    ),
    (
        "MISC-10",
        "1099-MISC",
        "10",
        "Gross proceeds paid to attorney",
        "600",
    ),
];

/// The demo engine's answer to `GET /company/1099-thresholds`.
#[must_use]
pub fn demo_read(set: &BTreeMap<String, Money>) -> Thresholds1099 {
    Thresholds1099 {
        year: DEMO_YEAR,
        boxes: DEMO_BOXES
            .iter()
            .map(|(code, form, number, label, standard)| {
                let standard = Money::parse_wire(standard).ok();
                let amount = set.get(*code).copied();
                cl_api::Threshold1099Box {
                    code: (*code).to_owned(),
                    form: (*form).to_owned(),
                    box_number: (*number).to_owned(),
                    label: (*label).to_owned(),
                    amount,
                    standard,
                    effective: amount.or(standard),
                }
            })
            .collect(),
    }
}

/// The demo engine's `PUT`: set or clear the boxes in `changes`.
pub fn demo_write(set: &mut BTreeMap<String, Money>, changes: &BTreeMap<String, Option<Money>>) {
    for (code, amount) in changes {
        match amount {
            Some(m) => {
                set.insert(code.clone(), *m);
            }
            None => {
                set.remove(code);
            }
        }
    }
}

/// The control channel's view: per box, the typed text and whether the
/// engine reported a standard amount (amounts only as typed, never logged).
#[must_use]
pub fn state_json(
    edits: &ThresholdEdits,
    loaded: Option<&Thresholds1099>,
    available: Availability,
) -> serde_json::Value {
    serde_json::json!({
        "available": format!("{available:?}").to_ascii_lowercase(),
        "year": loaded.map(|l| l.year),
        "boxes": loaded.map(|l| l.boxes.iter().map(|b| serde_json::json!({
            "code": b.code,
            "typed": edits.typed.get(&b.code).cloned().unwrap_or_default(),
            "saved": b.amount.map(show),
            "standard": b.standard.map(show),
        })).collect::<Vec<_>>()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::Threshold1099Box;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn loaded() -> Thresholds1099 {
        let b = |code: &str, amount: Option<Money>, standard: &str| Threshold1099Box {
            code: code.into(),
            form: "1099-MISC".into(),
            box_number: "1".into(),
            label: "x".into(),
            amount,
            standard: Some(m(standard)),
            effective: Some(amount.unwrap_or_else(|| m(standard))),
        };
        Thresholds1099 {
            year: 2026,
            boxes: vec![
                b("NEC-1", Some(m("1000")), "2000"),
                b("MISC-2", None, "10"),
                b("MISC-10", Some(m("600")), "600"),
            ],
        }
    }

    #[test]
    fn parse_takes_blank_cents_and_commas_and_refuses_the_rest() {
        assert_eq!(parse("  "), Ok(None));
        assert_eq!(parse("1,234.56"), Ok(Some(m("1234.56"))));
        assert_eq!(parse("$600"), Ok(Some(m("600"))));
        assert_eq!(parse("0"), Ok(Some(Money::ZERO)));
        assert_eq!(
            parse("1.005"),
            Err(NotAnAmount),
            "more than two places refused"
        );
        assert_eq!(parse("-1"), Err(NotAnAmount));
        assert_eq!(
            parse("(5.00)"),
            Err(NotAnAmount),
            "parentheses are negative"
        );
        assert_eq!(parse("1e3"), Err(NotAnAmount));
        assert_eq!(parse("abc"), Err(NotAnAmount));
    }

    #[test]
    fn only_changed_boxes_are_sent_and_the_round_trip_is_clean() {
        let l = loaded();
        let mut e = ThresholdEdits::from_engine(Some(&l));
        assert_eq!(e.typed["NEC-1"], "1,000.00");
        assert_eq!(e.typed["MISC-2"], "");
        assert!(e.changes(&l).expect("ok").is_empty(), "nothing changed");
        // Same amount typed differently is not a change.
        e.typed.insert("NEC-1".into(), "1000".into());
        assert!(e.changes(&l).expect("ok").is_empty());
        e.typed.insert("MISC-2".into(), "25.50".into());
        e.typed.insert("MISC-10".into(), String::new());
        let c = e.changes(&l).expect("ok");
        assert_eq!(c.len(), 2);
        assert_eq!(c["MISC-2"], Some(m("25.50")));
        assert_eq!(c["MISC-10"], None, "blank clears");
        e.typed.insert("NEC-1".into(), "-5".into());
        assert_eq!(e.changes(&l), Err("NEC-1".to_owned()));
    }

    #[test]
    fn known_boxes_have_owner_labels_and_others_use_the_engine_label() {
        assert!(!box_label("NEC-1", "x", "1099-NEC", "1").starts_with("prefs."));
        let other = box_label("MISC-99", "Future box", "1099-MISC", "99");
        assert!(
            other.contains("Future box") && other.contains("99"),
            "{other}"
        );
    }
}
