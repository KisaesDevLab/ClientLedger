//! Describe the report (A10, Q-A10b–i; reusing P28's Ask a question, Q-73):
//! the owner types what the report should show, the company's AI provider
//! drafts a builder definition, and the draft fills the builder as an
//! unsaved report to check.
//!
//! - Started from "Describe it" beside Guided's ready-made and blank
//!   choices, or "Describe the report…" in the Advanced toolbar (Q-A10c).
//! - The owner's words go out; the engine sends the model those words with
//!   amounts and other numbers hidden, and names from the books, never
//!   amounts, balances or transactions (Q-A10e, Q-A10l). A draft whose
//!   heading states a number in words is refused here too, unless the
//!   heading is exactly a books name. With AI
//!   off for reports (the Ask a question opt-in, Q-A10b) or an engine
//!   without `report-builder-ai-v1`, nothing is sent and the panel says how
//!   to turn it on.
//! - The draft opens in Advanced, unsaved, under a "Drafted by AI — check
//!   it" banner with Keep and Discard (Q-A10d). Names it couldn't match are
//!   rows marked in the outline (their own problem) and listed here to pick
//!   or remove; until none is left nothing is run, previewed or saved.
//!   Discard puts back what was there before. If the builder already had
//!   rows, the owner is asked before they are replaced.
//! - One-shot (Q-A10g): a new description makes a fresh draft.
//! - The app writes no number into a draft and reads none out of the
//!   description: every figure is the engine's, every amount filter the
//!   owner's.
//!
//! Nothing here logs the description, a name or the draft.

use std::collections::BTreeSet;

use cl_api::resources::reports::draft::{
    DraftKind, DraftRefusal, DraftReply, DraftRequest, NoteCode, draft_refusal,
};
use cl_api::{ApiError, Capabilities};
use cl_model::report_def::BuilderDefinition;
use cl_model::report_def::clreport::RefKind;
use cl_model::report_def::draft::{self as model, Answer, Describe, Draft, DraftName, Note, Phase};
use cl_model::report_def::editor::{BuilderEditor, Op, Origin};
use cl_model::report_def::guided::GuidedState;
use cl_model::reports::DateChoice;
use eframe::egui::{self, RichText, Ui};

use super::lists::{self, Which};
use super::{BuilderScreen, Pane, PropTab};
use crate::company::CompanyEvent;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;

/// What the builder held before a draft (Discard puts it back).
struct Before {
    ed: BuilderEditor,
    guided: Option<GuidedState>,
    custom: Option<DateChoice>,
}

/// A draft being checked: Keep or Discard.
pub(crate) struct Review {
    pub(crate) draft: Draft,
    before: Box<Before>,
}

/// The company's switches, read before the first draft: on, and the
/// provider's engine name when known.
type Gate = (bool, Option<String>);

/// The describe panel and a draft under review.
#[derive(Default)]
pub(crate) struct AiDraft {
    /// The panel is open.
    pub(crate) open: bool,
    /// The box takes the keyboard next frame.
    focus_box: bool,
    /// The owner's words and the request's phase.
    pub(crate) describe: Describe,
    gate: Option<Gate>,
    gate_job: Option<Job<Gate>>,
    job: Option<Job<DraftReply>>,
    /// A draft that came while the books' names are still being read: it
    /// is checked for number words once they are (Q-A10l).
    arrived: Option<Box<DraftReply>>,
    /// A draft that came while the builder had rows: Replace asks first.
    pub(crate) confirm: Option<Box<DraftReply>>,
    /// A refusal or a plain message (an i18n key).
    pub(crate) message: Option<&'static str>,
    /// Any other failure ("Copy details").
    pub(crate) error: Option<ApiError>,
    /// The draft on screen, until Keep or Discard.
    pub(crate) review: Option<Review>,
}

impl AiDraft {
    /// True while the switches or the draft are on their way.
    pub(crate) fn busy(&self) -> bool {
        self.gate_job.is_some() || self.job.is_some() || self.arrived.is_some()
    }

    /// Drops any request on its way and any draft not yet taken: the
    /// builder now shows another report (a late answer is never applied
    /// to it). The review goes too.
    pub(crate) fn forget(&mut self) {
        self.job = None;
        self.gate_job = None;
        self.arrived = None;
        self.confirm = None;
        self.review = None;
        self.describe.phase = Phase::Idle;
    }

    /// For the control channel: codes and counts only.
    pub(crate) fn state_json(&self, def: &BuilderDefinition) -> serde_json::Value {
        let review = self.review.as_ref();
        serde_json::json!({
            "panel": self.open,
            "phase": format!("{:?}", self.describe.phase).to_lowercase(),
            "described": !self.describe.text.trim().is_empty(),
            "message": self.message.map(|m| m.rsplit('.').next().unwrap_or_default()),
            "error": self.error.is_some(),
            "confirm_replace": self.confirm.is_some(),
            "drafted": review.is_some(),
            "unresolved": review.map_or(0, |r| r.draft.open(def).len()),
            "notes": review.map(|r| r.draft.notes.iter().map(|n| n.key().rsplit('.').next().unwrap_or_default().to_string()).collect::<Vec<_>>()),
            "provider": review.and_then(|r| r.draft.provider.clone()),
        })
    }
}

/// True when the engine drafts reports (the demo always does).
pub(crate) fn available(s: &BuilderScreen) -> bool {
    s.link.is_demo() || s.link.capabilities().has(Capabilities::REPORT_BUILDER_AI)
}

/// Opens the describe panel (both entry points).
pub(crate) fn open_panel(s: &mut BuilderScreen) {
    s.commit_pending();
    s.ai.open = true;
    s.ai.focus_box = true;
    s.ai.message = None;
    s.ai.error = None;
    // The books' names, for the number-word check of a draft (a heading
    // that is exactly a books name passes, Q-A10l) and the name picker.
    for w in [
        Which::Accounts,
        Which::Contacts,
        Which::Items,
        Which::Tags,
        Which::TagGroups,
    ] {
        lists::ensure(s, w);
    }
}

/// The names from the books the desktop knows: the lists read, and the
/// names and candidate labels the engine sent with the draft. An account
/// is known both as "6100 · Rent" and as "Rent".
fn known_names(s: &BuilderScreen, reply: &DraftReply) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let lists = [
        Which::Accounts,
        Which::Contacts,
        Which::Items,
        Which::Tags,
        Which::TagGroups,
    ];
    let listed = lists
        .into_iter()
        .filter_map(|w| s.lists.get(w))
        .flatten()
        .map(|(_, n)| n.as_str());
    let sent = reply.unresolved.iter().flat_map(|u| {
        u.name
            .as_deref()
            .into_iter()
            .chain(u.candidates.iter().map(|c| c.label.as_str()))
    });
    for n in listed.chain(sent) {
        let n = n.trim();
        out.insert(n.to_string());
        if let Some((_, plain)) = n.split_once(" \u{b7} ") {
            out.insert(plain.trim().to_string());
        }
    }
    out
}

/// A draft that came: refused when a heading states a number in words
/// (Q-A10l), else shown (asking first when the builder has rows).
pub(crate) fn arrived(s: &mut BuilderScreen, reply: DraftReply) {
    s.ai.describe.phase = Phase::Idle;
    let states_number = serde_json::from_value::<BuilderDefinition>(reply.definition.clone())
        .is_ok_and(|d| model::states_a_number(&d, &known_names(s, &reply)));
    if states_number {
        s.ai.message = Some("builder.ai.unmapped");
    } else if s.ed.definition().nodes.is_empty() {
        take(s, &reply);
    } else {
        s.ai.confirm = Some(Box::new(reply));
    }
}

/// The request for the owner's words: the builder's preset keys only.
pub(crate) fn request(text: &str) -> DraftRequest {
    DraftRequest {
        description: text.trim().to_string(),
        date_presets: super::PRESETS
            .iter()
            .map(|(_, n)| (*n).to_string())
            .collect(),
    }
}

/// Draft: the switches first (nothing is sent with AI off), then the
/// description.
pub(crate) fn send(s: &mut BuilderScreen) {
    s.ai.message = None;
    s.ai.error = None;
    if !available(s) {
        s.ai.message = Some("builder.ai.unsupported");
        return;
    }
    if !s.ai.describe.can_send() {
        return;
    }
    #[allow(clippy::single_match_else, reason = "the read below is the usual case")]
    match s.ai.gate {
        Some((true, _)) => start_draft(s),
        // Not read yet, or off last time: read again (the owner may have
        // turned it on since).
        _ => {
            s.ai.describe.phase = Phase::Checking;
            s.ai.gate_job = Some(engine::read(
                &s.link,
                s.group.as_ref(),
                // The demo company's AI switch (its report task is on).
                |d| Ok((d.ai.enabled, Some("demo".to_string()))),
                |s| async move {
                    let c = s.company_ai_disclosure().await?;
                    let on =
                        c.ai_enabled && !c.is_stale && c.enabled_tasks.report_summary == Some(true);
                    // The provider is read where the user may read it (an
                    // admin); otherwise the panel names none.
                    let provider = if on {
                        s.ai_config().await.ok().and_then(|c| drafting_provider(&c))
                    } else {
                        None
                    };
                    Ok((on, provider))
                },
            ));
        }
    }
}

fn start_draft(s: &mut BuilderScreen) {
    let req = request(&s.ai.describe.text);
    s.ai.describe.phase = Phase::Drafting;
    let demo_req = req.clone();
    s.ai.job = Some(engine::read(
        &s.link,
        s.group.as_ref(),
        move |d| crate::company::demo::report_draft::draft(d, &demo_req),
        move |s| async move { s.draft_report(&req).await },
    ));
}

/// Takes the switches and the draft when they come.
pub(crate) fn poll(s: &mut BuilderScreen) {
    if let Some(r) = engine::poll(&mut s.ai.gate_job) {
        // A failed read is "not on": nothing goes out.
        let gate = r.unwrap_or((false, None));
        let on = gate.0;
        s.ai.gate = Some(gate);
        s.ai.describe.phase = Phase::Idle;
        if on {
            start_draft(s);
        } else {
            s.ai.message = Some("builder.ai.ai_off");
        }
    }
    if let Some(r) = engine::poll(&mut s.ai.job) {
        match r {
            Ok(reply) => s.ai.arrived = Some(Box::new(reply)),
            Err(e) => {
                s.ai.describe.phase = Phase::Idle;
                refused(s, e);
            }
        }
    }
    // Held until the books' names are read (they are asked for when the
    // panel opens).
    if !s.lists.busy()
        && let Some(reply) = s.ai.arrived.take()
    {
        arrived(s, *reply);
    }
}

/// A refusal in the owner's words with a fix; anything else with "Copy
/// details".
fn refused(s: &mut BuilderScreen, e: ApiError) {
    s.ai.message = match draft_refusal(&e) {
        Some(DraftRefusal::ConsentRequired) => {
            s.ai.gate = Some((false, None));
            Some("builder.ai.ai_off")
        }
        Some(DraftRefusal::Unmapped { reason })
            if reason.as_deref() == Some(crate::company::demo::report_draft::DEMO_ONLY) =>
        {
            Some("builder.ai.demo_only")
        }
        Some(DraftRefusal::Unmapped { .. }) => Some("builder.ai.unmapped"),
        Some(DraftRefusal::Unavailable) => Some(unavailable_key(
            s.ai.gate.as_ref().and_then(|g| g.1.as_deref()),
        )),
        Some(DraftRefusal::Invalid) => Some("builder.ai.invalid"),
        None if matches!(e, ApiError::ReportShape { .. }) => Some("builder.ai.unmapped"),
        None if matches!(e, ApiError::Local(cl_api::LocalValidation::Unsupported)) => {
            Some("builder.ai.unsupported")
        }
        None => {
            s.ai.error = Some(e);
            None
        }
    };
}

/// "Can't be reached", worded for where the provider is: the internet is
/// named only for a cloud service; Ollama is on this computer (A10 review
/// Minor 8); an unknown one names neither.
pub(crate) fn unavailable_key(provider: Option<&str>) -> &'static str {
    match provider {
        Some("anthropic" | "openai") => "builder.ai.unavailable_cloud",
        Some("ollama") => "builder.ai.unavailable_local",
        _ => "builder.ai.unavailable",
    }
}

/// The provider the engine drafts with, when the admin settings say:
/// its chat provider, else its categorization provider (the engine's
/// `chatProvider || categorizationProvider`). None otherwise: the panel
/// then names no service rather than guess.
pub(crate) fn drafting_provider(c: &cl_api::resources::ai::AiConfig) -> Option<String> {
    c.chat_provider
        .as_deref()
        .or(c.categorization_provider.as_deref())
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
}

const fn ref_kind(k: DraftKind) -> RefKind {
    match k {
        DraftKind::Account => RefKind::Account,
        DraftKind::Contact => RefKind::Contact,
        DraftKind::Item => RefKind::Item,
        DraftKind::Tag => RefKind::Tag,
        DraftKind::TagGroup => RefKind::TagGroup,
    }
}

const fn which(k: RefKind) -> Option<Which> {
    match k {
        RefKind::Account => Some(Which::Accounts),
        RefKind::Contact => Some(Which::Contacts),
        RefKind::Item => Some(Which::Items),
        RefKind::Tag => Some(Which::Tags),
        RefKind::TagGroup => Some(Which::TagGroups),
        RefKind::Budget => None,
    }
}

/// The draft as the model keeps it (the engine's notes as codes only).
pub(crate) fn draft_of(reply: &DraftReply) -> Draft {
    Draft {
        names: reply
            .unresolved
            .iter()
            .map(|u| DraftName {
                token: u.token.clone(),
                kind: ref_kind(u.kind),
                name: u.name.clone(),
                ambiguous: !u.candidates.is_empty(),
                candidates: u
                    .candidates
                    .iter()
                    .map(|c| (c.id.clone(), c.label.clone()))
                    .collect(),
            })
            .collect(),
        notes: reply
            .notes
            .iter()
            .map(|n| match n.code {
                NoteCode::PickUnresolved => Note::PickUnresolved,
                NoteCode::AmountFilterLeftOut => Note::AmountFilterLeftOut(n.node_id.clone()),
                NoteCode::PresetLeftOut => Note::PresetLeftOut,
                NoteCode::NumbersNotUsed => Note::NumbersNotUsed,
            })
            .collect(),
        provider: reply.provider.clone(),
    }
}

/// The draft fills the builder: unsaved, in Advanced, what was there kept
/// for Discard.
pub(crate) fn take(s: &mut BuilderScreen, reply: &DraftReply) {
    // The definition is read as schema v1, placeholders and all; the rules
    // (and the placeholders' own problem) are the editor's.
    let Ok(def) = serde_json::from_value::<BuilderDefinition>(reply.definition.clone()) else {
        s.ai.message = Some("builder.ai.unmapped");
        return;
    };
    s.commit_pending();
    let draft = draft_of(reply);
    // A draft over a draft still under review keeps what was there before
    // the first one: Discard always puts that back (A10 review S2,
    // Q-A10d), never the earlier draft.
    let before = match s.ai.review.take() {
        Some(r) => r.before,
        None => Box::new(Before {
            ed: s.ed.clone(),
            guided: s.guided.take(),
            custom: s.band.custom.take(),
        }),
    };
    let name = def.title.clone().unwrap_or_default();
    let mut ed = BuilderEditor::new(def, &name, Origin::Drafted, None);
    // The first row with a name to pick is selected; else the first row.
    let first = draft
        .open(ed.definition())
        .first()
        .and_then(|n| {
            model::rows_with(ed.definition(), &n.token)
                .into_iter()
                .next()
        })
        .or_else(|| ed.definition().nodes.first().map(|n| n.id().to_string()));
    ed.select(first.as_deref());
    s.ed = ed;
    s.status = None;
    s.tab = PropTab::Row;
    s.pane = Pane::Outline;
    s.focus_to = Some(Pane::Outline);
    s.preview.edited(&s.ed);
    s.ai.open = false;
    s.ai.confirm = None;
    s.ai.message = None;
    s.ai.review = Some(Review { draft, before });
}

/// Keep: the draft is the report (still unsaved until Memorize).
pub(crate) fn keep(s: &mut BuilderScreen) {
    if s.ed.has_placeholders() {
        return;
    }
    end_review(s);
    s.status = Some(t("builder.ai.kept"));
}

/// The review ends (Keep, or a Memorize or Save changes during it): the
/// banner and Discard go, and so do the undo steps taken while names were
/// picked, so an undo never brings back a name to pick with no list to
/// pick it from (A10 review S3). True when a review was on.
pub(crate) fn end_review(s: &mut BuilderScreen) -> bool {
    if s.ai.review.take().is_none() {
        return false;
    }
    s.ed.fence_history();
    true
}

/// Discard: what was there before comes back.
pub(crate) fn discard(s: &mut BuilderScreen) {
    let Some(r) = s.ai.review.take() else {
        return;
    };
    let b = *r.before;
    s.ed = b.ed;
    s.guided = b.guided;
    s.band.custom = b.custom;
    s.status = Some(t("builder.ai.discarded"));
    s.preview.edited(&s.ed);
    s.focus_to = Some(Pane::Outline);
}

/// Picks a row for a name, or removes it: one undo step.
pub(crate) fn answer(s: &mut BuilderScreen, token: &str, a: &Answer) {
    let Some(def) = model::resolve(s.ed.definition(), token, a) else {
        return;
    };
    let name = s.ed.name().to_string();
    let group_id = s.ed.group_id().map(str::to_string);
    let selected = s.ed.selection().map(str::to_string);
    s.apply(Op::Load {
        def: Box::new(def),
        name,
        group_id,
    });
    s.ed.select(selected.as_deref());
}

/// Goes to row `id` (its Row tab).
fn go_to(s: &mut BuilderScreen, id: &str) {
    s.ed.select(Some(id));
    s.tab = PropTab::Row;
    s.focus_to = Some(Pane::Outline);
}

/// The provider's name for the panel and the banner.
fn provider_text(p: Option<&str>) -> String {
    match p {
        Some("anthropic") => t("builder.ai.provider.anthropic"),
        Some("openai") => t("builder.ai.provider.openai"),
        Some("ollama") => t("builder.ai.provider.ollama"),
        Some("demo") => t("builder.ai.provider.demo"),
        _ => t("builder.ai.provider.unknown"),
    }
}

/// The caption of row `id` (owner text, shown as typed).
fn row_caption(def: &BuilderDefinition, id: &str) -> String {
    match cl_model::report_def::outline::find(&def.nodes, id) {
        Some(cl_model::report_def::Node::Section(x)) => x.caption.clone(),
        Some(cl_model::report_def::Node::Group(x)) => x.caption.clone(),
        _ => t("builder.problem.the_report"),
    }
}

/// The panel, the replace question and the review: drawn under the
/// toolbar in Guided and Advanced.
pub(crate) fn draw(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    if s.ai.busy() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
    if s.ai.open {
        panel(s, ui, tokens);
    }
    if s.ai.confirm.is_some() {
        confirm(s, ui, tokens);
    }
    if s.ai.review.is_some() {
        review(s, ui, tokens);
    }
}

fn panel(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let mut go = false;
    let mut close = false;
    let mut settings = false;
    let can = available(s);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let l = ui.label(RichText::new(t("builder.ai.panel_title")).strong());
        let r = ui
            .add_enabled(
                can && !s.ai.describe.busy(),
                egui::TextEdit::multiline(&mut s.ai.describe.text)
                    .hint_text(t("builder.ai.hint"))
                    .char_limit(model::MAX_DESCRIPTION_CHARS)
                    .desired_rows(3)
                    .desired_width(ui.available_width().min(640.0)),
            )
            .labelled_by(l.id);
        if std::mem::take(&mut s.ai.focus_box) {
            r.request_focus();
        }
        // Ctrl+Enter drafts from the box (Enter is a new line).
        if r.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            go = true;
        }
        if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        let provider = s.ai.gate.as_ref().and_then(|g| g.1.as_deref());
        ui.label(
            RichText::new(t("builder.ai.what_is_sent"))
                .size(12.0)
                .color(tokens.text_secondary),
        );
        ui.label(
            RichText::new(t_with(
                "builder.ai.sent_to",
                &[("provider", &provider_text(provider))],
            ))
            .size(12.0)
            .color(tokens.text_secondary),
        );
        ui.horizontal_wrapped(|ui| {
            let why = if can {
                t("builder.ai.draft_off")
            } else {
                t("builder.ai.unsupported")
            };
            let b = ui
                .add_enabled(
                    can && s.ai.describe.can_send(),
                    egui::Button::new(t("builder.ai.draft")).shortcut_text("Ctrl Enter"),
                )
                .on_disabled_hover_text(why);
            if b.clicked() {
                go = true;
            }
            if ui.button(t("builder.ai.cancel")).clicked() {
                close = true;
            }
            match s.ai.describe.phase {
                Phase::Checking | Phase::Drafting => {
                    form::progress_line(ui, tokens, &t("builder.ai.drafting"));
                }
                Phase::Idle => {}
            }
        });
        if !can {
            form::inline_note(ui, tokens, &t("builder.ai.unsupported"));
        }
        if let Some(m) = s.ai.message {
            form::inline_note(ui, tokens, &t(m));
            if m == "builder.ai.ai_off" && ui.link(t("reports.ask.open_settings")).clicked() {
                settings = true;
            }
        }
        if let Some(e) = &s.ai.error {
            form::inline_error_details(ui, tokens, e, "report_draft");
        }
    });
    if go {
        send(s);
    }
    if close {
        s.ai.open = false;
        s.ai.message = None;
        s.ai.error = None;
        // A request on its way is dropped: its answer is never shown.
        s.ai.job = None;
        s.ai.gate_job = None;
        s.ai.describe.phase = Phase::Idle;
        s.focus_to = Some(Pane::Outline);
    }
    if settings {
        s.ai.gate = None;
        s.link
            .push(CompanyEvent::OpenWindow(cl_model::CommandId::new(
                "edit.preferences",
            )));
    }
}

fn confirm(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let mut replace = None;
    form::inline_note(ui, tokens, &t("builder.ai.replace_question"));
    ui.horizontal(|ui| {
        let r = ui.button(t("builder.ai.replace"));
        if r.clicked() {
            replace = Some(true);
        }
        if ui.button(t("builder.ai.keep_mine")).clicked() {
            replace = Some(false);
        }
    });
    match replace {
        Some(true) => {
            if let Some(reply) = s.ai.confirm.take() {
                take(s, &reply);
            }
        }
        Some(false) => {
            s.ai.confirm = None;
        }
        None => {}
    }
}

/// The banner, the names to pick and the notes.
fn review(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let Some(r) = s.ai.review.as_ref() else {
        return;
    };
    let def = s.ed.definition().clone();
    let open: Vec<DraftName> = r.draft.open(&def).into_iter().cloned().collect();
    let notes = r.draft.notes.clone();
    let provider = provider_text(r.draft.provider.as_deref());
    let mut answers: Vec<(String, Answer)> = Vec::new();
    let mut want: Vec<Which> = Vec::new();
    let mut go: Option<String> = None;
    let mut do_keep = false;
    let mut do_discard = false;
    egui::Frame::new()
        .fill(tokens.info_tint)
        .stroke(egui::Stroke::new(1.0, tokens.info))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("builder.ai.banner")).strong());
                ui.label(
                    RichText::new(t_with("builder.ai.banner_by", &[("provider", &provider)]))
                        .size(12.0)
                        .color(tokens.text_secondary),
                );
            });
            ui.label(
                RichText::new(t("builder.ai.banner_hint"))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
            for n in &open {
                name_row(s, ui, &def, n, &mut answers, &mut want, &mut go);
            }
            for note in &notes {
                ui.horizontal_wrapped(|ui| {
                    let text = match note {
                        Note::AmountFilterLeftOut(Some(id)) => {
                            t_with(note.key(), &[("row", &row_caption(&def, id))])
                        }
                        _ => t(note.key()),
                    };
                    ui.label(RichText::new(text).size(12.0).color(tokens.warn_text));
                    if let Note::AmountFilterLeftOut(Some(id)) = note
                        && cl_model::report_def::outline::find(&def.nodes, id).is_some()
                        && ui.link(t("builder.problems.go")).clicked()
                    {
                        go = Some(id.clone());
                    }
                });
            }
            ui.horizontal(|ui| {
                let ready = open.is_empty();
                let k = ui
                    .add_enabled(ready, egui::Button::new(t("builder.ai.keep")))
                    .on_disabled_hover_text(t("builder.ai.keep_off"));
                if k.clicked() {
                    do_keep = true;
                }
                if ui
                    .button(t("builder.ai.discard"))
                    .on_hover_text(t("builder.ai.discard_tip"))
                    .clicked()
                {
                    do_discard = true;
                }
            });
        });
    for w in want {
        lists::ensure(s, w);
    }
    for (token, a) in answers {
        answer(s, &token, &a);
    }
    if let Some(id) = go {
        go_to(s, &id);
    }
    if do_keep {
        keep(s);
    }
    if do_discard {
        discard(s);
    }
}

/// One name to pick: what it was meant to be, where, the rows to pick
/// from (the candidates, then the full list) and Remove.
fn name_row(
    s: &BuilderScreen,
    ui: &mut Ui,
    def: &BuilderDefinition,
    n: &DraftName,
    answers: &mut Vec<(String, Answer)>,
    want: &mut Vec<Which>,
    go: &mut Option<String>,
) {
    let rows = model::rows_with(def, &n.token);
    let row = rows
        .first()
        .map(|id| row_caption(def, id))
        .unwrap_or_default();
    let what = n.name.as_deref().map_or_else(
        || t(&format!("builder.ai.kind.{}", kind_key(n.kind))),
        |name| {
            t_with(
                super::super::clreport::ref_kind_key(n.kind),
                &[("name", name)],
            )
        },
    );
    let key = if n.ambiguous {
        "builder.ai.name_ambiguous"
    } else {
        "builder.ai.name_not_found"
    };
    ui.add_space(4.0);
    ui.label(t_with(key, &[("what", &what), ("row", &row)]));
    ui.horizontal_wrapped(|ui| {
        let l = ui.label(t("builder.ai.use"));
        let list = which(n.kind);
        egui::ComboBox::from_id_salt(("ai-draft-pick", s.id.0, n.token.as_str()))
            .width(260.0)
            .selected_text(t("report_file.card.pick_one"))
            .show_ui(ui, |ui| {
                for (id, label) in &n.candidates {
                    if ui.selectable_label(false, label.as_str()).clicked() {
                        answers.push((n.token.clone(), Answer::Pick(id.clone())));
                    }
                }
                // The full list, read when the list first opens.
                if let Some(w) = list {
                    want.push(w);
                    for (id, label) in s.lists.get(w).into_iter().flatten() {
                        if n.candidates.iter().any(|(c, _)| c == id) {
                            continue;
                        }
                        if ui.selectable_label(false, label.as_str()).clicked() {
                            answers.push((n.token.clone(), Answer::Pick(id.clone())));
                        }
                    }
                }
            })
            .response
            .labelled_by(l.id);
        if ui
            .button(t("builder.ai.remove"))
            .on_hover_text(t("builder.ai.remove_tip"))
            .clicked()
        {
            answers.push((n.token.clone(), Answer::Remove));
        }
        if let Some(id) = rows.first()
            && ui.link(t("builder.problems.go")).clicked()
        {
            *go = Some(id.clone());
        }
    });
}

const fn kind_key(k: RefKind) -> &'static str {
    match k {
        RefKind::Account => "account",
        RefKind::Contact => "contact",
        RefKind::Item => "item",
        RefKind::Tag => "tag",
        RefKind::TagGroup => "tag_group",
        RefKind::Budget => "budget",
    }
}
