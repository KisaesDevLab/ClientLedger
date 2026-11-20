//! The attachment files a backup or a transfer could not read (P32 QA-1),
//! each with its reason and Open record. Back Up Company and Send to My
//! Accountant show the same list: the engine stops every desktop backup on
//! a missing or changed file, and trying again never helps.

use cl_company::MissingAttachment;
use cl_model::CommandId;
use eframe::egui::{self, RichText, Ui};

use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;

/// The record an Open record button asked for: its kind and id.
pub type OpenAsk = (Option<String>, Option<String>);

/// Draws the list in a card; `body_key` says what did not happen and what
/// to do. Returns the card (to scroll to) and the record asked for.
pub fn panel(
    ui: &mut Ui,
    tokens: &Tokens,
    files: &[MissingAttachment],
    body_key: &str,
    id_salt: &str,
) -> (egui::Response, Option<OpenAsk>) {
    let mut ask = None;
    let r = form::card_tight(ui, tokens, |ui| {
        ui.label(RichText::new(t("send.missing.title")).strong());
        form::hint(ui, tokens, &t(body_key));
        egui::Grid::new(("missing_files", id_salt))
            .num_columns(3)
            .show(ui, |ui| {
                for f in files {
                    ui.label(
                        f.file_name
                            .clone()
                            .unwrap_or_else(|| t("send.missing.unknown")),
                    );
                    ui.label(t(&format!("send.missing.reason.{}", f.reason)));
                    if ui
                        .add_enabled(
                            openable(f.record_type.as_deref()).is_some() && f.record_id.is_some(),
                            egui::Button::new(t("send.open_record")),
                        )
                        .clicked()
                    {
                        ask = Some((f.record_type.clone(), f.record_id.clone()));
                    }
                    ui.end_row();
                }
            });
        ui.label("")
    });
    (r, ask)
}

/// The form that opens a record of `kind` (an attachment's
/// `attachableType`), if there is one.
#[must_use]
pub fn openable(kind: Option<&str>) -> Option<&'static str> {
    let kind = kind?;
    crate::screens::reports::drill::form_of(kind).or(match kind {
        "expense" => Some(crate::screens::write_check::KIND),
        _ => None,
    })
}

/// Opens the record a file belongs to (transactions only; other kinds
/// have no form to open).
pub fn open_record(link: &CompanyLink, kind: Option<&str>, id: Option<&str>) {
    let (Some(form), Some(id)) = (openable(kind), id) else {
        return;
    };
    if let Ok(id) = cl_core::ids::TransactionId::parse(id) {
        link.push(CompanyEvent::OpenRecord(CommandId::new(form), id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transactions_open_and_other_kinds_do_not() {
        assert_eq!(
            openable(Some("expense")),
            Some(crate::screens::write_check::KIND)
        );
        assert!(openable(Some("bill")).is_some());
        assert!(openable(Some("contact")).is_none());
        assert!(openable(None).is_none());
    }
}
