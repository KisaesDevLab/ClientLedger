//! Preferences › AI › **Reading documents online** (A03 step 21, plan D5,
//! Q-A03k, Q-284): the company's own consent to send a document to an
//! online AI service when the owner asks AI to help read it.
//!
//! - Two switches, both **off** for every company: the page **text**
//!   (`document_text_cloud`) and the page **pictures**
//!   (`document_images_cloud`). Pictures need the text switch too. Neither
//!   is ever turned on by "Read bills and receipts" (`receipt_ocr`) or by
//!   agreeing to the AI notice.
//! - The disclosure says plainly that a picture can't have anything hidden.
//! - Only the company's owner may change them; the engine records every
//!   change in the audit trail (`PATCH /ai/consent/{company}/tasks`). Saved
//!   with the window's Save, after the AI panel's own changes.
//! - A local Ollama model needs neither switch: nothing leaves the
//!   computer or network. An Ollama cloud model (`…:cloud`, `…-cloud`)
//!   runs online and needs them like any online service (review S5).
//! - The engine's cloud-vision setting is shown, never changed (the
//!   desktop keeps the strict privacy level, Q-139, Q-A03v).

use cl_api::{AiTaskToggles, CompanyConsent};
use eframe::egui::{RichText, Ui};
use serde_json::{Value, json};

use super::ai::switch_row;
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;

/// The two switches as the panel edits them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloudReadingEdits {
    /// Send the page text online.
    pub text: bool,
    /// Send page pictures online as well.
    pub images: bool,
}

impl CloudReadingEdits {
    /// What the engine stores (an absent switch is off).
    #[must_use]
    pub fn from_consent(consent: Option<&CompanyConsent>) -> CloudReadingEdits {
        consent.map_or_else(CloudReadingEdits::default, |c| CloudReadingEdits {
            text: c.enabled_tasks.document_text_cloud_on(),
            images: c.enabled_tasks.document_images_cloud_on(),
        })
    }

    /// Pictures only with text.
    pub fn normalize(&mut self) {
        if !self.text {
            self.images = false;
        }
    }

    /// The switches as sent: only these two keys.
    #[must_use]
    pub fn toggles(self) -> AiTaskToggles {
        AiTaskToggles {
            document_text_cloud: Some(self.text),
            document_images_cloud: Some(self.text && self.images),
            ..AiTaskToggles::default()
        }
    }
}

/// Whether the switches can be changed here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// They can.
    Ready,
    /// The engine has no document reading assist (`document-assist-v1`).
    NeedsEngine,
    /// Only the company's owner may change them.
    NotOwner,
    /// AI is off for this company (the engine keeps no switch then).
    AiOff,
}

impl Availability {
    /// Decides from the engine, the user's role and the AI switch.
    #[must_use]
    pub fn of(capable: bool, owner: bool, ai_on: bool) -> Availability {
        if !capable {
            Availability::NeedsEngine
        } else if !owner {
            Availability::NotOwner
        } else if !ai_on {
            Availability::AiOff
        } else {
            Availability::Ready
        }
    }

    fn note_key(self) -> Option<&'static str> {
        match self {
            Availability::Ready => None,
            Availability::NeedsEngine => Some("prefs.cloud_reading.needs_engine"),
            Availability::NotOwner => Some("prefs.cloud_reading.not_owner"),
            Availability::AiOff => Some("prefs.cloud_reading.ai_off"),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Availability::Ready => "ready",
            Availability::NeedsEngine => "needs_engine",
            Availability::NotOwner => "not_owner",
            Availability::AiOff => "ai_off",
        }
    }
}

/// Draws the panel. `cloud_vision` is the engine's own picture setting.
pub fn panel(
    ui: &mut Ui,
    tokens: &Tokens,
    edits: &mut CloudReadingEdits,
    available: Availability,
    cloud_vision: bool,
) {
    form::heading(ui, tokens, &t("prefs.cloud_reading.title"));
    form::hint(ui, tokens, &t("prefs.cloud_reading.intro"));
    form::hint(ui, tokens, &t("prefs.cloud_reading.local"));
    if let Some(key) = available.note_key() {
        ui.add_space(2.0);
        ui.label(RichText::new(t(key)).strong().color(tokens.warn_text));
    }
    ui.add_space(4.0);
    let enabled = available == Availability::Ready;
    form::card_tight(ui, tokens, |ui| {
        form::heading(ui, tokens, &t("prefs.cloud_reading.what"));
        ui.add_enabled_ui(enabled, |ui| {
            switch_row(
                ui,
                tokens,
                &mut edits.text,
                &t("prefs.cloud_reading.text"),
                &t("prefs.cloud_reading.text_desc"),
            );
        });
        ui.separator();
        ui.add_enabled_ui(enabled && edits.text, |ui| {
            switch_row(
                ui,
                tokens,
                &mut edits.images,
                &t("prefs.cloud_reading.images"),
                &t("prefs.cloud_reading.images_desc"),
            );
        });
        // A picture can't be redacted: said plainly, always.
        ui.label(
            RichText::new(t("prefs.cloud_reading.images_warning"))
                .strong()
                .color(tokens.warn_text),
        );
        if edits.text && edits.images && !cloud_vision {
            form::hint(ui, tokens, &t("prefs.cloud_reading.images_blocked"));
        }
    });
    edits.normalize();
    ui.add_space(4.0);
    form::hint(ui, tokens, &t("prefs.cloud_reading.amounts"));
    form::hint(ui, tokens, &t("prefs.cloud_reading.audit"));
}

/// The control channel's view.
#[must_use]
pub fn state_json(edits: &CloudReadingEdits, available: Availability, cloud_vision: bool) -> Value {
    json!({
        "text": edits.text,
        "images": edits.images,
        "available": available.key(),
        "cloud_vision": cloud_vision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::needless_pass_by_value)]
    fn consent(tasks: Value) -> CompanyConsent {
        serde_json::from_value(json!({
            "companyId": "d0000000-0000-4000-8000-0000000000c1",
            "aiEnabled": true, "enabledTasks": tasks,
        }))
        .unwrap()
    }

    #[test]
    fn receipt_ocr_never_turns_them_on() {
        let c = consent(json!({"receipt_ocr": true, "document_images_cloud": true}));
        assert_eq!(
            CloudReadingEdits::from_consent(Some(&c)),
            CloudReadingEdits::default(),
            "pictures without text count as off"
        );
        assert_eq!(
            CloudReadingEdits::from_consent(None),
            CloudReadingEdits::default()
        );
        let c = consent(json!({"document_text_cloud": true, "document_images_cloud": true}));
        assert_eq!(
            CloudReadingEdits::from_consent(Some(&c)),
            CloudReadingEdits {
                text: true,
                images: true
            }
        );
    }

    #[test]
    fn only_the_two_switches_are_sent_and_pictures_need_text() {
        let t = CloudReadingEdits {
            text: false,
            images: true,
        }
        .toggles();
        assert_eq!(
            serde_json::to_value(t).unwrap(),
            json!({"document_text_cloud": false, "document_images_cloud": false})
        );
        let mut e = CloudReadingEdits {
            text: false,
            images: true,
        };
        e.normalize();
        assert!(!e.images);
    }

    #[test]
    fn availability_follows_engine_role_and_ai() {
        assert_eq!(Availability::of(true, true, true), Availability::Ready);
        assert_eq!(
            Availability::of(false, true, true),
            Availability::NeedsEngine
        );
        assert_eq!(Availability::of(true, false, true), Availability::NotOwner);
        assert_eq!(Availability::of(true, true, false), Availability::AiOff);
    }
}
