//! Rules (P26 DI-2): the Bank Feed's rules beside the list, a rule from a
//! bank line (R), and the rules the engine suggests from repeated choices
//! (Q-141, Q-P26k: 5 accepts, overridden under 10%).
//!
//! A rule here has conditions on the description, the direction and
//! exact amount, and the bank account, and sets one account (plan §3,
//! Q-P26o: rules never post on the desktop). "Auto-accept" only offers
//! "Accept all from this rule" (Q-P26a, Q-P26n). Rules made elsewhere in
//! another shape are listed read-only. Hidden for a view-only user.

use cl_api::{FeedRule, FeedRules, RuleSign, RuleSuggestion};
use cl_core::Money;
use cl_core::ids::AccountId;
use cl_core::money::UserInput;
use eframe::egui::{self, RichText, Ui};

use super::BankFeedScreen;
use crate::company::demo::feed_review as demo;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::templates::engine::{Job, finished, run};
use crate::theme::Tokens;

/// The rule being edited.
#[derive(Clone, Default)]
pub(crate) struct Draft {
    pub rule: FeedRule,
    /// The exact amount as typed (empty: any amount).
    pub amount: String,
}

impl Draft {
    fn of(rule: FeedRule) -> Draft {
        let amount = rule
            .amount
            .map(|m| m.as_decimal().normalize().to_string())
            .unwrap_or_default();
        Draft { rule, amount }
    }

    /// The rule to send (`None` when the amount isn't a positive amount
    /// with at most 2 decimals).
    fn to_rule(&self) -> Option<FeedRule> {
        let mut r = self.rule.clone();
        let a = self.amount.trim();
        r.amount = if a.is_empty() {
            None
        } else {
            Some(
                Money::parse_user(a, UserInput::CENTS)
                    .ok()
                    .filter(|m| m.is_positive())?,
            )
        };
        cl_api::feed_rule_body(&r).ok().map(|_| r)
    }
}

/// The Rules panel.
#[derive(Default)]
pub(crate) struct RulesPanel {
    pub open: bool,
    pub rules: Vec<FeedRule>,
    pub ideas: Vec<RuleSuggestion>,
    pub draft: Option<Draft>,
    pub notice: Option<&'static str>,
    /// Waiting lines the last saved rule set up.
    pub applied: Option<u64>,
    load: Option<Job<(Vec<FeedRule>, Vec<RuleSuggestion>)>>,
    /// A save answers how many waiting lines the rule now sets up.
    act: Option<Job<Option<u64>>>,
}

/// Opens the panel and reads the rules.
pub(crate) fn open(s: &mut BankFeedScreen) {
    if s.view_only {
        return;
    }
    s.rules.open = true;
    read(s);
}

/// R: a rule from the row (its description's first words, its direction,
/// its bank account, its account).
pub(crate) fn from_row(s: &mut BankFeedScreen, id: cl_api::FeedItemId) {
    if s.view_only {
        return;
    }
    let Some(r) = s.row(id).cloned() else {
        return;
    };
    let words: Vec<&str> = r
        .description
        .split_whitespace()
        .take_while(|w| !w.chars().any(|c| c.is_ascii_digit()))
        .take(3)
        .collect();
    // The account just chosen in the panel first (P26 QA-11).
    let chosen = s.detail.account.filter(|_| s.detail.row == Some(id));
    let account = chosen.or_else(|| {
        r.assigned_account
            .as_ref()
            .or(r.suggested_account.as_ref())
            .map(|a| a.id)
    });
    let payee = r
        .assigned_contact
        .as_ref()
        .or(r.suggested_contact.as_ref())
        .map(|c| c.name.clone());
    open(s);
    s.rules.draft = Some(Draft::of(FeedRule {
        name: payee.unwrap_or_else(|| words.join(" ")),
        active: true,
        description: Some(words.join(" ")).filter(|w| !w.is_empty()),
        sign: if r.is_money_out() {
            RuleSign::MoneyOut
        } else {
            RuleSign::MoneyIn
        },
        bank_account: s.current_account().and_then(|a| a.account),
        account,
        ..FeedRule::default()
    }));
}

/// Reads the rules and the suggestions (also on opening the window, for
/// the rule offer).
pub(crate) fn read(s: &mut BankFeedScreen) {
    if s.rules.load.is_some() {
        return;
    }
    s.rules.load = Some(run(
        &s.link,
        s.group.as_ref(),
        None,
        |d| Ok((demo::rules(d), demo::rule_suggestions(d))),
        |sess| async move {
            let r = sess.feed_rules();
            let rules = r.list().await?;
            let ideas = r.suggestions().await.unwrap_or_default();
            Ok((rules, ideas))
        },
    ));
}

enum Do {
    Save(FeedRule),
    Delete(String),
    Dismiss(RuleSuggestion),
}

fn act(s: &mut BankFeedScreen, what: Do) {
    if s.rules.act.is_some() || s.view_only {
        return;
    }
    let live = match &what {
        Do::Save(r) => Do::Save(r.clone()),
        Do::Delete(id) => Do::Delete(id.clone()),
        Do::Dismiss(x) => Do::Dismiss(x.clone()),
    };
    s.rules.act = Some(run(
        &s.link,
        s.group.as_ref(),
        Some("bank_feed.orphan.rule"),
        move |d| match what {
            // The saved rule sets up the lines already waiting (P26 QA-10).
            Do::Save(r) => {
                demo::save_rule(d, &r)?;
                Ok(demo::reprocess_rules(d, None).ok().map(|x| x.matched))
            }
            Do::Delete(id) => demo::delete_rule(d, &id).map(|()| None),
            Do::Dismiss(x) => {
                demo::dismiss_suggestion(d, &x);
                Ok(None)
            }
        },
        move |sess| async move {
            let r: FeedRules<'_> = sess.feed_rules();
            match live {
                Do::Save(x) => {
                    r.save(&x).await?;
                    // A failed rerun doesn't undo the save: the rule still
                    // applies to the next bank file.
                    Ok(r.reprocess(None).await.ok().map(|x| x.matched))
                }
                Do::Delete(id) => r.delete(&id).await.map(|()| None),
                Do::Dismiss(x) => r.dismiss(&x).await.map(|()| None),
            }
        },
    ));
}

/// Takes the answers that arrived.
pub(crate) fn poll(s: &mut BankFeedScreen) {
    if let Some(r) = finished(&mut s.rules.load) {
        match r {
            Ok((rules, ideas)) => {
                s.rules.rules = rules;
                s.rules.ideas = ideas;
            }
            Err(_) if s.rules.open => s.rules.notice = Some("bank_feed.rules.load_failed"),
            Err(_) => {}
        }
    }
    if let Some(r) = finished(&mut s.rules.act) {
        match r {
            Ok(applied) => {
                s.rules.draft = None;
                s.rules.applied = applied;
                s.rules.notice = Some(match applied {
                    Some(0) => "bank_feed.rules.applied.none",
                    Some(_) => "bank_feed.rules.applied",
                    None => "bank_feed.rules.saved",
                });
                // The panel reads its row again (its new suggestion).
                s.detail.row = None;
                s.read_meta();
                // The rows' rules (names, auto-accept, change tokens).
                s.reload();
            }
            Err(e) if cl_api::is_view_only(&e) => {
                s.view_only = true;
                s.rules.notice = Some("bank_feed.notice.view_only");
            }
            Err(_) => s.rules.notice = Some("bank_feed.rules.failed"),
        }
        read(s);
    }
}

fn name_of(chart: &[(AccountId, String)], a: Option<AccountId>) -> String {
    a.and_then(|a| chart.iter().find(|(id, _)| *id == a))
        .map_or_else(String::new, |(_, n)| n.clone())
}

fn combo(
    ui: &mut Ui,
    salt: &str,
    label: &str,
    chart: &[(AccountId, String)],
    value: &mut Option<AccountId>,
    none: Option<&str>,
) {
    let shown = value
        .and_then(|a| chart.iter().find(|(id, _)| *id == a))
        .map_or_else(
            || none.map_or_else(|| t("bank_feed.detail.choose_account"), str::to_string),
            |(_, n)| n.clone(),
        );
    let l = ui.label(label);
    egui::ComboBox::from_id_salt(salt)
        .selected_text(shown)
        .truncate()
        .width(ui.available_width().min(280.0))
        .show_ui(ui, |ui| {
            if let Some(n) = none {
                ui.selectable_value(value, None, n);
            }
            for (id, name) in chart {
                ui.selectable_value(value, Some(*id), name);
            }
        })
        .response
        .labelled_by(l.id);
}

fn editor(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    let chart = s.chart.clone();
    let busy = s.rules.act.is_some();
    let Some(d) = s.rules.draft.as_mut() else {
        return;
    };
    let mut save = false;
    let mut delete = false;
    let mut cancel = false;
    form::card(ui, tokens, |ui| {
        form::heading(
            ui,
            tokens,
            &t(if d.rule.id.is_some() {
                "bank_feed.rules.edit"
            } else {
                "bank_feed.rules.new"
            }),
        );
        field(ui, &t("bank_feed.rules.name"), &mut d.rule.name);
        let mut text = d.rule.description.clone().unwrap_or_default();
        if field(ui, &t("bank_feed.rules.contains"), &mut text).changed() {
            d.rule.description = Some(text).filter(|x| !x.trim().is_empty());
        }
        let l = ui.label(t("bank_feed.rules.direction"));
        egui::ComboBox::from_id_salt("bank_feed.rules.sign")
            .selected_text(t(sign_key(d.rule.sign)))
            .show_ui(ui, |ui| {
                for v in [RuleSign::Either, RuleSign::MoneyOut, RuleSign::MoneyIn] {
                    ui.selectable_value(&mut d.rule.sign, v, t(sign_key(v)));
                }
            })
            .response
            .labelled_by(l.id);
        field(ui, &t("bank_feed.rules.amount"), &mut d.amount);
        combo(
            ui,
            "bank_feed.rules.bank",
            &t("bank_feed.rules.bank"),
            &chart,
            &mut d.rule.bank_account,
            Some(&t("bank_feed.rules.any_bank")),
        );
        combo(
            ui,
            "bank_feed.rules.account",
            &t("bank_feed.rules.account"),
            &chart,
            &mut d.rule.account,
            None,
        );
        ui.checkbox(&mut d.rule.auto_accept, t("bank_feed.rules.auto_accept"))
            .on_hover_text(t("bank_feed.rules.auto_accept.tip"));
        ui.checkbox(&mut d.rule.active, t("bank_feed.rules.active"));
        let ready = d.to_rule();
        if ready.is_none() {
            form::hint(ui, tokens, &t("bank_feed.rules.incomplete"));
        }
        ui.horizontal(|ui| {
            save = ui
                .add_enabled(
                    !busy && ready.is_some(),
                    egui::Button::new(t("bank_feed.rules.save")),
                )
                .clicked();
            if d.rule.id.is_some() {
                delete =
                    crate::widgets::destructive_button(ui, &t("bank_feed.rules.delete")).clicked();
            }
            cancel = ui.button(t("bank_feed.rules.cancel")).clicked();
        });
    });
    if save {
        save_draft(s);
    } else if delete && let Some(id) = s.rules.draft.as_ref().and_then(|d| d.rule.id.clone()) {
        act(s, Do::Delete(id));
    } else if cancel {
        s.rules.draft = None;
    }
}

/// Saves the rule being edited (Save rule).
pub(crate) fn save_draft(s: &mut BankFeedScreen) {
    if let Some(r) = s.rules.draft.as_ref().and_then(Draft::to_rule) {
        act(s, Do::Save(r));
    }
}

/// A label over a text box as wide as the panel.
fn field(ui: &mut Ui, label: &str, value: &mut String) -> egui::Response {
    let l = ui.label(label);
    ui.add_sized(
        [ui.available_width().min(290.0), 24.0],
        egui::TextEdit::singleline(value),
    )
    .labelled_by(l.id)
}

fn sign_key(v: RuleSign) -> &'static str {
    match v {
        RuleSign::Either => "bank_feed.rules.sign.either",
        RuleSign::MoneyOut => "bank_feed.rules.sign.out",
        RuleSign::MoneyIn => "bank_feed.rules.sign.in",
    }
}

/// Draws the panel in place of the detail panel.
pub(crate) fn draw(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(t("bank_feed.rules")).strong().size(16.0));
        ui.add_space(12.0);
        if ui.button(t("bank_feed.rules.close")).clicked() {
            s.rules.open = false;
            s.rules.draft = None;
        }
    });
    if let Some(k) = s.rules.notice {
        let n = s.rules.applied.unwrap_or(0).to_string();
        form::hint(ui, tokens, &t_with(k, &[("n", &n)]));
    }
    egui::ScrollArea::vertical()
        .id_salt("bank_feed.rules")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            editor(s, ui, tokens);
            if s.rules.draft.is_none() && ui.button(t("bank_feed.rules.add")).clicked() {
                s.rules.draft = Some(Draft::of(FeedRule {
                    active: true,
                    ..FeedRule::default()
                }));
            }
            let chart = s.chart.clone();
            if !s.rules.ideas.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new(t("bank_feed.rules.ideas")).strong());
                let mut make = None;
                let mut dismiss = None;
                for (i, x) in s.rules.ideas.iter().enumerate() {
                    ui.add(
                        egui::Label::new(t_with(
                            "bank_feed.rules.idea",
                            &[
                                ("text", &x.payee_pattern),
                                ("account", &x.account_name),
                                ("n", &x.times_confirmed.to_string()),
                            ],
                        ))
                        .wrap(),
                    );
                    ui.horizontal(|ui| {
                        if ui.small_button(t("bank_feed.rules.idea.make")).clicked() {
                            make = Some(i);
                        }
                        if ui.small_button(t("bank_feed.rules.idea.dismiss")).clicked() {
                            dismiss = Some(i);
                        }
                    });
                }
                if let Some(i) = make {
                    let x = s.rules.ideas[i].clone();
                    s.rules.draft = Some(Draft::of(FeedRule {
                        name: x.payee_pattern.clone(),
                        active: true,
                        description: Some(x.payee_pattern),
                        account: Some(x.account),
                        ..FeedRule::default()
                    }));
                } else if let Some(i) = dismiss {
                    let x = s.rules.ideas[i].clone();
                    act(s, Do::Dismiss(x));
                }
            }
            ui.add_space(8.0);
            if s.rules.load.is_some() {
                form::progress_line(ui, tokens, &t("bank_feed.rules.reading"));
            } else if s.rules.rules.is_empty() {
                form::hint(ui, tokens, &t("bank_feed.rules.none"));
            }
            let mut edit = None;
            for (i, r) in s.rules.rules.iter().enumerate() {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&r.name).strong());
                    if r.auto_accept {
                        ui.label(
                            RichText::new(t("bank_feed.rules.auto_badge"))
                                .size(11.0)
                                .color(tokens.accent),
                        );
                    }
                    if !r.active {
                        ui.label(
                            RichText::new(t("bank_feed.rules.off"))
                                .size(11.0)
                                .color(tokens.text_tertiary),
                        );
                    }
                });
                ui.label(
                    RichText::new(name_of(&chart, r.account))
                        .size(12.0)
                        .color(tokens.text_secondary),
                );
                let label = t_with("bank_feed.rules.edit_one", &[("name", &r.name)]);
                if r.simple {
                    let b = ui.small_button(t("bank_feed.rules.edit_button"));
                    b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
                    if b.clicked() {
                        edit = Some(i);
                    }
                } else {
                    form::hint(ui, tokens, &t("bank_feed.rules.read_only"));
                }
            }
            if let Some(i) = edit {
                s.rules.draft = Some(Draft::of(s.rules.rules[i].clone()));
            }
        });
}

/// The panel's control-channel state.
pub(crate) fn state(s: &BankFeedScreen) -> serde_json::Value {
    serde_json::json!({
        "open": s.rules.open,
        "rules": s.rules.rules.len(),
        "ideas": s.rules.ideas.len(),
        "editing": s.rules.draft.is_some(),
        "draft_ready": s.rules.draft.as_ref().and_then(Draft::to_rule).is_some(),
        "auto_accept": s.rules.rules.iter().filter(|r| r.auto_accept).count(),
        "notice": s.rules.notice,
        "applied": s.rules.applied,
    })
}
