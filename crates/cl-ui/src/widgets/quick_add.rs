//! Quick Add (Q-PR4d, QA-13): the prompt under a name field when the typed
//! name is not on the list. One component for every form with a client,
//! vendor or payee field; the choices and keys live in
//! [`cl_model::quick_add`].
//!
//! - **Quick Add** creates the name alone (with an `Idempotency-Key`, kept
//!   for a retry) and hands the new contact back to the form.
//! - **Set Up** opens the Client or Vendor Center's new-name form with the
//!   name filled in (`OpenPayload::SetUpName`); the shell brings the form
//!   back to the front when it is saved, and this component finds the new
//!   name and hands it back the same way.
//! - **Cancel** (Esc) leaves the field as it is.
//!
//! The form shows the prompt with [`QuickAdd::ui`] under its field, calls
//! [`QuickAdd::offer`] when the owner leaves the field with an unknown
//! name, [`QuickAdd::cancel`] on `form.revert` (Esc), and takes
//! [`QuickAddEvent::Added`] like a pick.

use std::sync::Arc;

use cl_api::{
    ApiError, Contact, ContactFields, ContactFilter, ContactType, IdempotencyKey, NewContact,
    RequestGroup,
};
use cl_model::quick_add::{Accepts, Choice, NameKind, Prompt, PromptKey, Stage};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_with};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;

/// What the prompt hands the form.
#[derive(Debug)]
pub(crate) enum QuickAddEvent {
    /// A new name (Quick Add, or Set Up saved): the form picks it.
    Added(Box<Contact>),
}

/// The contact type a kind is created as.
fn contact_type(kind: NameKind) -> ContactType {
    match kind {
        NameKind::Client => ContactType::Customer,
        NameKind::Vendor => ContactType::Vendor,
    }
}

/// True when `c` can fill a field of `kind`.
fn is_kind(c: &Contact, kind: NameKind) -> bool {
    matches!(
        (c.fields.contact_type, kind),
        (ContactType::Both, _)
            | (ContactType::Customer, NameKind::Client)
            | (ContactType::Vendor, NameKind::Vendor)
    )
}

/// The Quick Add prompt of one name field.
pub(crate) struct QuickAdd {
    link: Arc<CompanyLink>,
    prompt: Option<Prompt>,
    /// The create's key, kept for a retry of the same name.
    key: Option<IdempotencyKey>,
    job: Option<Job<Contact>>,
    error: Option<ApiError>,
    /// Focus Quick Add on the frame the prompt opens (Enter adds).
    focus: bool,
    /// Set Up is open for this name and kind.
    waiting: Option<(String, NameKind)>,
    /// Finding the Set Up name after a contacts change.
    find: Option<Job<Option<Contact>>>,
    bus: crate::data_bus::Subscription,
}

impl std::fmt::Debug for QuickAdd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuickAdd")
            .field("open", &self.prompt.is_some())
            .field("waiting", &self.waiting.is_some())
            .finish_non_exhaustive()
    }
}

impl QuickAdd {
    /// The prompt of a form over `link`.
    pub(crate) fn new(link: &Arc<CompanyLink>) -> QuickAdd {
        QuickAdd {
            bus: link.bus().subscribe(),
            link: Arc::clone(link),
            prompt: None,
            key: None,
            job: None,
            error: None,
            focus: false,
            waiting: None,
            find: None,
        }
    }

    /// The owner left the field with `typed`, not a name on the list:
    /// offer Quick Add (a blank name offers nothing).
    pub(crate) fn offer(&mut self, typed: &str, accepts: Accepts) {
        if self.job.is_some() {
            return;
        }
        let same = self
            .prompt
            .as_ref()
            .is_some_and(|p| p.name() == typed.trim() && p.accepts() == accepts);
        if same {
            return;
        }
        self.prompt = Prompt::new(typed, accepts);
        self.key = None;
        self.error = None;
        self.focus = self.prompt.is_some();
    }

    /// Esc (`form.revert`): closes the prompt. True when it was open.
    pub(crate) fn cancel(&mut self) -> bool {
        if self.prompt.is_none() || self.job.is_some() {
            return false;
        }
        self.close();
        true
    }

    /// The field changed or a name was picked: the prompt goes away.
    pub(crate) fn close(&mut self) {
        if self.job.is_some() {
            return;
        }
        self.prompt = None;
        self.key = None;
        self.error = None;
    }

    /// Set Up is open for a name (the form reads its lists again even when
    /// it has unsaved edits).
    pub(crate) fn waiting(&self) -> bool {
        self.waiting.is_some()
    }

    fn choose(&mut self, choice: Choice, group: Option<&RequestGroup>) {
        let Some(p) = self.prompt.as_mut() else {
            return;
        };
        match choice {
            Choice::Cancel => self.close(),
            Choice::SetUp(kind) => {
                let name = p.name().to_string();
                let center = match kind {
                    NameKind::Client => crate::screens::lists::center::CLIENTS_KIND,
                    NameKind::Vendor => crate::screens::lists::center::VENDORS_KIND,
                };
                self.link.push(CompanyEvent::OpenWith(
                    cl_model::CommandId::new(center),
                    Box::new(crate::windows::OpenPayload::SetUpName(
                        crate::windows::SetUpName {
                            name: name.clone(),
                            kind,
                        },
                    )),
                ));
                self.waiting = Some((name, kind));
                self.close();
            }
            Choice::QuickAdd(kind) => {
                p.adding();
                self.error = None;
                let name = p.name().to_string();
                let key = self.key.get_or_insert_with(IdempotencyKey::new).clone();
                let label = engine::label("orphan.quick_add", Some(key.clone()));
                let body = NewContact {
                    fields: ContactFields::named(contact_type(kind), name.clone()),
                    opening_balance: cl_core::Money::ZERO,
                    opening_balance_date: None,
                };
                self.job = Some(engine::write(
                    &self.link,
                    group,
                    label,
                    move |d| demo_create(d, &name, kind),
                    move |s| async move { s.contacts().create(&body, key).await.map(|v| v.record) },
                ));
            }
        }
    }

    /// Takes the finished create or Set Up find.
    fn poll(&mut self, group: Option<&RequestGroup>) -> Option<QuickAddEvent> {
        if let Some(r) = engine::poll(&mut self.job) {
            match r {
                Ok(c) => {
                    self.prompt = None;
                    self.key = None;
                    self.link
                        .bus()
                        .publish_from(&self.bus, crate::data_bus::Change::Contacts);
                    return Some(QuickAddEvent::Added(Box::new(c)));
                }
                Err(e) => {
                    if let Some(p) = self.prompt.as_mut() {
                        p.failed();
                    }
                    self.error = Some(e);
                }
            }
        }
        let changes = self.link.bus().take(&mut self.bus);
        if changes.contacts
            && self.find.is_none()
            && let Some((name, kind)) = self.waiting.clone()
        {
            let n = name.clone();
            self.find = Some(engine::read(
                &self.link,
                group,
                move |d| {
                    Ok(d.contacts_mut()
                        .iter()
                        .find(|c| found(c, &n, kind))
                        .cloned())
                },
                move |s| async move {
                    let all = s
                        .contacts()
                        .list_all(&ContactFilter {
                            search: Some(name.clone()),
                            ..ContactFilter::default()
                        })
                        .await?;
                    Ok(all.into_iter().find(|c| found(c, &name, kind)))
                },
            ));
        }
        if let Some(r) = engine::poll(&mut self.find)
            && let Ok(Some(c)) = r
        {
            self.waiting = None;
            return Some(QuickAddEvent::Added(Box::new(c)));
        }
        None
    }

    /// Draws the prompt (when open) and answers its keys; polls the create
    /// and the Set Up find every frame.
    pub(crate) fn ui(
        &mut self,
        ui: &mut Ui,
        tokens: &Tokens,
        group: Option<&RequestGroup>,
    ) -> Option<QuickAddEvent> {
        let event = self.poll(group);
        if event.is_some() {
            ui.ctx().request_repaint();
            return event;
        }
        let p = self.prompt.clone()?;
        let mut choice = None;
        egui::Frame::new()
            .fill(tokens.surface_subtle)
            .stroke(egui::Stroke::new(1.0, tokens.border_default))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.label(t_with("quick_add.not_on_list", &[("name", p.name())]));
                if p.accepts().asks_kind() {
                    ui.horizontal(|ui| {
                        let l = ui.label(
                            RichText::new(t("quick_add.add_as")).color(tokens.text_secondary),
                        );
                        for k in [NameKind::Vendor, NameKind::Client] {
                            let r = ui
                                .add_enabled(
                                    p.stage() != Stage::Adding,
                                    egui::RadioButton::new(p.kind() == k, t(k.key())),
                                )
                                .labelled_by(l.id);
                            if r.clicked()
                                && let Some(p) = self.prompt.as_mut()
                            {
                                p.set_kind(k);
                            }
                        }
                    });
                }
                let kind = self.prompt.as_ref().map_or(p.kind(), Prompt::kind);
                ui.horizontal(|ui| {
                    let adding = p.stage() == Stage::Adding;
                    let add = ui.add_enabled(
                        !adding,
                        egui::Button::new(
                            RichText::new(t("quick_add.quick_add"))
                                .color(tokens.on_accent)
                                .strong(),
                        )
                        .fill(tokens.accent),
                    );
                    if std::mem::take(&mut self.focus) {
                        add.request_focus();
                    }
                    if add.clicked() {
                        choice = Some(Choice::QuickAdd(kind));
                    }
                    if ui
                        .add_enabled(!adding, egui::Button::new(t("quick_add.set_up")))
                        .on_hover_text(t("quick_add.set_up.tip"))
                        .clicked()
                    {
                        choice = Some(Choice::SetUp(kind));
                    }
                    if ui
                        .add_enabled(!adding, egui::Button::new(t("quick_add.cancel")))
                        .clicked()
                    {
                        choice = Some(Choice::Cancel);
                    }
                    if adding {
                        ui.spinner();
                    }
                });
                if let Some(e) = &self.error {
                    // A name another window added first is a conflict.
                    let text = if matches!(e, ApiError::Conflict { .. }) {
                        t("quick_add.name_taken")
                    } else {
                        t("quick_add.failed")
                    };
                    ui.label(RichText::new(text).color(tokens.warn_text));
                }
                ui.label(
                    RichText::new(t("quick_add.keys"))
                        .size(12.0)
                        .color(tokens.text_tertiary),
                );
            });
        // Esc never reaches here through a field; Enter on the focused
        // button is its click. A key pressed with nothing focused still
        // answers (keyboard-only).
        if choice.is_none() && ui.ctx().memory(|m| m.focused().is_none()) {
            let key = ui.input(|i| {
                if i.key_pressed(egui::Key::Enter) {
                    Some(PromptKey::Enter)
                } else if i.key_pressed(egui::Key::Escape) {
                    Some(PromptKey::Escape)
                } else {
                    None
                }
            });
            choice = key.and_then(|k| self.prompt.as_ref().and_then(|p| p.on_key(k)));
        }
        if let Some(c) = choice {
            self.choose(c, group);
            ui.ctx().request_repaint();
        }
        None
    }

    /// The prompt for the control channel: no names.
    pub(crate) fn state(&self) -> Value {
        json!({
            "open": self.prompt.is_some(),
            "kind": self.prompt.as_ref().map(|p| match p.kind() {
                NameKind::Client => "client",
                NameKind::Vendor => "vendor",
            }),
            "asks_kind": self.prompt.as_ref().is_some_and(|p| p.accepts().asks_kind()),
            "adding": self.job.is_some(),
            "failed": self.error.is_some(),
            "waiting_set_up": self.waiting.is_some(),
        })
    }
}

fn found(c: &Contact, name: &str, kind: NameKind) -> bool {
    c.is_active && c.fields.display_name.trim().eq_ignore_ascii_case(name) && is_kind(c, kind)
}

/// The demo engine's create: a name already on the list is refused as the
/// engine refuses it.
fn demo_create(
    d: &mut crate::company::demo::DemoState,
    name: &str,
    kind: NameKind,
) -> Result<Contact, ApiError> {
    let list = d.contacts_mut();
    if list.iter().any(|c| {
        c.fields
            .display_name
            .trim()
            .eq_ignore_ascii_case(name.trim())
    }) {
        return Err(cl_api::engine_refusal(
            409,
            &json!({"error": {"message": "refused", "code": "DUPLICATE_NAME", "details": {}}}),
        ));
    }
    let c = Contact {
        id: crate::company::demo::contacts::contact_id(
            910_000 + u32::try_from(list.len()).unwrap_or(0),
        ),
        fields: ContactFields::named(contact_type(kind), name.trim()),
        is_active: true,
        opening_balance: Some(cl_core::Money::ZERO),
        opening_balance_date: None,
        created_at: Some(crate::company::demo::DemoState::now()),
        updated_at: Some(crate::company::demo::DemoState::now()),
    };
    list.push(c.clone());
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_create_adds_once_and_matches_kinds() {
        let mut d = crate::company::demo::DemoState::new();
        let c = demo_create(&mut d, " Bluff Hardware ", NameKind::Vendor).expect("made");
        assert_eq!(c.fields.display_name, "Bluff Hardware");
        assert_eq!(c.fields.contact_type, ContactType::Vendor);
        assert!(found(&c, "bluff hardware", NameKind::Vendor));
        assert!(!found(&c, "bluff hardware", NameKind::Client));
        assert!(matches!(
            demo_create(&mut d, "BLUFF HARDWARE", NameKind::Client),
            Err(ApiError::Conflict { .. })
        ));
    }
}
