//! Ask a question (Q-73, Q-P28g, U-206): a question box above the report.
//!
//! The question goes to the engine with the catalog this company can run;
//! the company's AI provider (an online service with the owner's key, or a
//! local Ollama model) picks one report and its options; the app checks
//! the answer against what it sent, turns it into report options and runs
//! that report. The engine computes every figure.
//!
//! - With AI off, Ask switched off or consent out of date, the bar says so
//!   and links to Preferences › AI, and nothing is sent.
//! - "Ran as" is built from the report options alone (never the model's
//!   words); names the engine couldn't find show as chips, and they are
//!   always the owner's own words.

use cl_api::resources::reports::ask::{
    AskDates as ApiDates, AskMapping, AskParam as ApiParam, AskRefusal, AskReport, AskRequest,
    ask_refusal,
};
use cl_api::{ApiError, Capabilities};
use cl_model::reports::ask::{self as model, AskChoice, AskDates, AskParam};
use eframe::egui::{self, RichText, Ui};

use super::window::ReportWindow;
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;

/// The question bar's state.
#[derive(Default)]
pub(crate) struct AskBar {
    /// The question being typed.
    pub text: String,
    /// The company's switches allow Ask (read once, before the first ask).
    gate: Option<bool>,
    gate_job: Option<Job<bool>>,
    /// A question waits for the switches to be read.
    waiting: bool,
    job: Option<Job<AskMapping>>,
    /// The line built from the options that ran.
    pub ran_as: Option<String>,
    /// The owner's words the engine couldn't find.
    pub unresolved: Vec<String>,
    /// The report and options the line describes: it shows only while
    /// the window still runs exactly those.
    ran_for: Option<(&'static str, cl_model::reports::ReportOptions)>,
    /// A plain message (i18n key).
    pub message: Option<&'static str>,
    pub error: Option<ApiError>,
}

impl AskBar {
    pub(crate) fn busy(&self) -> bool {
        self.gate_job.is_some() || self.job.is_some()
    }

    pub(crate) fn state_json(&self) -> serde_json::Value {
        serde_json::json!({
            "asking": self.busy(),
            "ran_as": self.ran_as,
            "unresolved": self.unresolved.len(),
            "message": self.message.map(|m| m.rsplit('.').next().unwrap_or_default()),
            "error": self.error.is_some(),
        })
    }
}

/// True when the engine maps questions (the bar shows).
fn shown(w: &ReportWindow) -> bool {
    w.link.is_demo() || w.link.capabilities().has(Capabilities::REPORTS_ASK)
}

fn api_param(p: AskParam) -> ApiParam {
    match p {
        AskParam::Range => ApiParam::Range,
        AskParam::AsOf => ApiParam::AsOf,
        AskParam::Basis => ApiParam::Basis,
        AskParam::Accounts => ApiParam::Accounts,
        AskParam::Contacts => ApiParam::Contacts,
        AskParam::Tags => ApiParam::Tags,
    }
}

/// The request for `question`: the catalog this company can run, in the
/// owner's words for the titles.
pub(crate) fn request(
    question: &str,
    runs: impl Fn(&cl_model::reports::ReportSpec) -> bool,
    today: cl_core::CalendarDate,
) -> AskRequest {
    AskRequest {
        question: question.trim().to_string(),
        reports: model::catalog(runs)
            .into_iter()
            .map(|e| AskReport {
                key: e.key.to_string(),
                title: t(e.title),
                params: e.params.into_iter().map(api_param).collect(),
                columns: e.columns,
                extras: e.extras,
            })
            .collect(),
        date_presets: model::preset_tokens(),
        today: Some(today),
    }
}

/// The model's terms for a checked mapping.
pub(crate) fn choice(m: &AskMapping) -> AskChoice {
    AskChoice {
        report_key: m.report_key.clone(),
        dates: match &m.dates {
            ApiDates::Preset(p) => AskDates::Preset(p.clone()),
            ApiDates::Range(a, b) => AskDates::Range(*a, *b),
            ApiDates::AsOf(d) => AskDates::AsOf(*d),
            ApiDates::Default => AskDates::Default,
        },
        basis: m.basis,
        columns: m.columns.clone(),
        extras: m.extras.clone(),
        account_ids: m.account_ids.clone(),
        contact_ids: m.contact_ids.clone(),
        tag_ids: m.tag_ids.clone(),
    }
}

fn start(w: &mut ReportWindow) {
    let a = &mut w.ask;
    a.message = None;
    a.error = None;
    if a.text.trim().is_empty() {
        return;
    }
    match a.gate {
        // Not read yet, or off last time: read the switches again (the
        // owner may have turned Ask on since).
        None | Some(false) => {
            a.waiting = true;
            if a.gate_job.is_none() {
                a.gate_job = Some(engine::read(
                    &w.link,
                    w.group.as_ref(),
                    // The demo company's AI switch (its report task is on).
                    |d| Ok(d.ai.enabled),
                    |s| async move {
                        let c = s.company_ai_disclosure().await?;
                        Ok(c.ai_enabled
                            && !c.is_stale
                            && c.enabled_tasks.report_summary == Some(true))
                    },
                ));
            }
        }
        Some(true) => {
            let link = std::sync::Arc::clone(&w.link);
            let req = request(&a.text, |s| super::available(&link, s), w.today);
            a.job = Some(engine::read(
                &w.link,
                w.group.as_ref(),
                {
                    let req = req.clone();
                    move |_| crate::company::demo::report_store::ask(&req)
                },
                move |s| async move { s.ask_report(&req).await },
            ));
        }
    }
}

/// Shows the mapping's report: its options, the "Ran as" line and the
/// chips.
pub(crate) fn apply(w: &mut ReportWindow, m: &AskMapping) {
    let Some((spec, options)) = model::options_for(&choice(m)) else {
        w.ask.message = Some("reports.ask.unmapped");
        return;
    };
    w.ask.ran_as = Some(model::ran_as(spec, &options, &w.fiscal, w.today, &|k| t(k)));
    w.ask.unresolved = m.unresolved.iter().map(|(_, s)| s.clone()).collect();
    w.ask.ran_for = Some((spec.key, options.clone()));
    w.show_report(spec, options);
}

pub(super) fn poll(w: &mut ReportWindow) {
    if let Some(r) = engine::poll(&mut w.ask.gate_job) {
        // A failed read is "not on": nothing goes out.
        let on = r.unwrap_or(false);
        w.ask.gate = Some(on);
        if std::mem::take(&mut w.ask.waiting) {
            if on {
                start(w);
            } else {
                w.ask.message = Some("reports.ask.ai_off");
            }
        }
    }
    if let Some(r) = engine::poll(&mut w.ask.job) {
        match r {
            Ok(m) => apply(w, &m),
            Err(e) => {
                w.ask.message = match ask_refusal(&e) {
                    Some(AskRefusal::ConsentRequired) => {
                        w.ask.gate = Some(false);
                        Some("reports.ask.ai_off")
                    }
                    Some(AskRefusal::Unmapped) => Some("reports.ask.unmapped"),
                    Some(AskRefusal::Unavailable) => Some("reports.ask.unavailable"),
                    None if matches!(e, ApiError::ReportShape { .. }) => {
                        Some("reports.ask.unmapped")
                    }
                    None => {
                        w.ask.error = Some(e);
                        None
                    }
                };
            }
        }
    }
}

pub(super) fn ui(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    if !shown(w) {
        return;
    }
    let mut ask = false;
    let mut settings = false;
    let mut change = false;
    ui.horizontal(|ui| {
        let l = ui.label(RichText::new(t("reports.ask.label")).size(12.0));
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut w.ask.text)
                    .hint_text(t("reports.ask.hint"))
                    .char_limit(cl_api::resources::reports::ask::MAX_QUESTION_CHARS)
                    .desired_width(420.0),
            )
            .labelled_by(l.id);
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            ask = true;
        }
        if ui
            .add_enabled(!w.ask.busy(), egui::Button::new(t("reports.ask.button")))
            .clicked()
        {
            ask = true;
        }
        if w.ask.busy() {
            ui.spinner();
        }
    });
    let current = w
        .ask
        .ran_for
        .as_ref()
        .is_some_and(|(k, o)| *k == w.spec.key && *o == w.options);
    if let Some(line) = w.ask.ran_as.as_ref().filter(|_| current) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{} {line}", t("reports.ask.ran_as")))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
            if ui.link(t("reports.ask.change")).clicked() {
                change = true;
            }
            for u in &w.ask.unresolved {
                ui.label(
                    RichText::new(format!("{} {u}", t("reports.ask.not_found")))
                        .size(12.0)
                        .background_color(tokens.neutral_chip)
                        .color(tokens.neutral_chip_text),
                );
            }
        });
    }
    if let Some(m) = w.ask.message {
        ui.horizontal_wrapped(|ui| {
            form::inline_warning(ui, tokens, &t(m));
            if m == "reports.ask.ai_off" && ui.link(t("reports.ask.open_settings")).clicked() {
                settings = true;
            }
        });
    }
    if let Some(e) = &w.ask.error {
        form::inline_error(ui, tokens, e);
    }
    if ask {
        start(w);
    }
    if change {
        w.customize = true;
    }
    if settings {
        // The answer may change there: read the switches again next time.
        w.ask.gate = None;
        w.link
            .push(CompanyEvent::OpenWindow(cl_model::CommandId::new(
                "edit.preferences",
            )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::resources::reports::ask::{NameKind, validate};
    use serde_json::json;

    fn today() -> cl_core::CalendarDate {
        cl_core::CalendarDate::from_ymd(2026, 11, 10).expect("d")
    }

    /// A stubbed model answer, checked as the live path checks it.
    fn stub(answer: &serde_json::Value, question: &str) -> AskMapping {
        let req = request(question, |_| true, today());
        validate(&req, answer).expect("mapping")
    }

    #[test]
    fn the_request_carries_titles_and_presets_never_figures() {
        let r = request("  What did we earn last month?  ", |_| true, today());
        assert_eq!(r.question, "What did we earn last month?");
        assert!(r.reports.iter().any(|x| x.key == "profit_loss"));
        assert!(!r.reports.iter().any(|x| x.key == "budget_vs_actual"));
        let v = serde_json::to_value(&r).expect("json");
        assert!(!v.to_string().to_lowercase().contains("amount"));
    }

    #[test]
    fn a_stubbed_model_s_numbers_never_reach_the_screen() {
        // The model tries to slip figures in: a made-up total in free text
        // and a number in an unresolved name. Neither is shown.
        let answer = json!({
            "reportKey": "profit_loss",
            "dates": {"from": "2026-10-01", "to": "2026-10-31"},
            "basis": "cash",
            "summary": "You spent $4,213.77 on fuel",
            "filters": {"accountIds": [], "contactIds": [], "tagIds": []},
            "unresolved": [{"kind": "account", "text": "Fuel 4213.77"}, {"kind": "account", "text": "Fuel"}],
        });
        let m = stub(&answer, "How much did Fuel cost in October, cash basis?");
        assert_eq!(m.unresolved, vec![(NameKind::Account, "Fuel".to_string())]);
        let (spec, o) = model::options_for(&choice(&m)).expect("options");
        let line = model::ran_as(
            spec,
            &o,
            &cl_core::fiscal::FiscalYearSpec::CALENDAR_YEAR,
            today(),
            &|k| t(k),
        );
        assert!(!line.contains("4213") && !line.contains("4,213"), "{line}");
        assert!(
            line.contains("10/01/2026") && line.contains("10/31/2026"),
            "{line}"
        );
        assert!(o.filters.amount.is_none());
    }

    #[test]
    fn works_with_a_self_hosted_reader() {
        let answer = json!({"reportKey": "balance_sheet", "dates": {"preset": "last_month"},
                            "reader": "self_hosted", "provider": "ollama", "model": "llama3"});
        let m = stub(&answer, "What did the balance sheet look like last month?");
        assert_eq!(m.reader.as_deref(), Some("self_hosted"));
        assert!(model::options_for(&choice(&m)).is_some());
    }
}
