//! A center's (customers or vendors) side of the save-conflict dialog (P33
//! D4 step 11), as the Tags list's (header-only): an edit is guarded by the
//! contact as the panel opened it, a 409 `STALE` reads their contact, saves
//! again without asking when nothing editable changed on their side (once
//! per save), and opens [`crate::screens::conflict_dialog`] otherwise.
//!
//! Never compared: the tax ID (never shown, an empty field keeps the one on
//! file) and the 1099 profile (its own record, saved after the contact).

use cl_api::{ApiError, Contact, Versioned};
use cl_core::ids::ContactId;
use cl_model::forms::SaveIntent;
use cl_model::lan::Conflict;
use cl_model::lists::contacts::{AddressDraft, ContactDraft, Roles, TermsChoice};
use serde_json::{Value, json};

use super::form::{FormPanel, account_label, terms_text};
use super::{CenterScreen, Panel};
use crate::company::Task;
use crate::forms::conflict::{GONE, OpenConflict, Read, Words, read_of};
use crate::i18n::t;
use crate::screens::conflict_dialog::{ConflictChoice, ConflictDialog, fields_from_theirs};

/// The "changed elsewhere" notice (the plain refusal).
const CHANGED: &str = "lan.list.changed_elsewhere";

/// An address in one line, as the dialog compares it (whole).
fn address_text(a: &AddressDraft) -> String {
    let zip = [a.state.trim(), a.zip.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    [a.line1.trim(), a.line2.trim(), a.city.trim(), zip.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The editable fields of a contact as the panel holds them.
#[must_use]
pub fn contact_value(d: &ContactDraft) -> Value {
    json!({
        "contactType": d.roles.wire(),
        "name": d.name.trim(),
        "companyName": d.company.trim(),
        "firstName": d.first.trim(),
        "lastName": d.last.trim(),
        "email": d.email.trim(),
        "phone": d.phone.trim(),
        "billTo": address_text(&d.billing),
        "shipTo": address_text(&d.shipping),
        "terms": d.terms.stored(),
        "notes": d.notes.trim(),
        "expenseAccount": d.expense_account.map(|a| a.to_string()),
        "vendorAccountNumber": d.vendor_account.trim(),
        "track1099": d.track_1099,
    })
}

/// Applies the fields Keep mine takes from `theirs` onto `mine`.
fn take_theirs(mine: &mut ContactDraft, theirs: &ContactDraft, fields: &[String]) {
    for field in fields {
        match field.as_str() {
            "contactType" => mine.roles = theirs.roles,
            "name" => mine.name.clone_from(&theirs.name),
            "companyName" => mine.company.clone_from(&theirs.company),
            "firstName" => mine.first.clone_from(&theirs.first),
            "lastName" => mine.last.clone_from(&theirs.last),
            "email" => mine.email.clone_from(&theirs.email),
            "phone" => mine.phone.clone_from(&theirs.phone),
            "billTo" => mine.billing.clone_from(&theirs.billing),
            "shipTo" => mine.shipping.clone_from(&theirs.shipping),
            "terms" => mine.terms.clone_from(&theirs.terms),
            "notes" => mine.notes.clone_from(&theirs.notes),
            "expenseAccount" => mine.expense_account = theirs.expense_account,
            "vendorAccountNumber" => mine.vendor_account.clone_from(&theirs.vendor_account),
            "track1099" => mine.track_1099 = theirs.track_1099,
            _ => {}
        }
    }
}

/// Roles in words.
fn roles_text(wire: &str) -> Option<String> {
    Some(match Roles::from_wire(wire)? {
        Roles::Client => t("center.form.is_client"),
        Roles::Vendor => t("center.form.is_vendor"),
        Roles::Both => format!(
            "{}, {}",
            t("center.form.is_client"),
            t("center.form.is_vendor")
        ),
    })
}

/// A contact as the panel would show it (the panel's own conversion).
pub(crate) fn draft_of(c: &Contact) -> ContactDraft {
    FormPanel::edit(c, None).form.current().clone()
}

impl CenterScreen {
    /// A save came back stale: read their contact (the dialog follows).
    pub(super) fn contact_conflict_start(&mut self, id: ContactId, intent: SaveIntent) {
        self.conflict_for = Some(id);
        if self.link.is_demo() {
            let found = self
                .link
                .demo_state()
                .and_then(|mut d| d.contacts_mut().iter().find(|x| x.id == id).cloned());
            let got = found
                .map(|c| Versioned {
                    version: c.version(),
                    record: c,
                    stock_warnings: Vec::new(),
                })
                .ok_or(ApiError::NotFound { code: None });
            self.contact_conflict_read(got, intent);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.warn = Some(CHANGED);
            return;
        };
        match group.read(&session, move |s| async move { s.contacts().get(id).await }) {
            Ok(p) => self.contact_conflict.reading = Some((Task::Pending(p), intent)),
            Err(_) => self.warn = Some(CHANGED),
        }
    }

    /// Polls the read of their contact.
    pub(super) fn poll_contact_conflict(&mut self) {
        if let Some((r, intent)) = self.contact_conflict.poll() {
            self.contact_conflict_read(r, intent);
        }
    }

    /// The panel still edits the contact the read was for.
    fn conflict_panel_matches(&self) -> bool {
        match &self.panel {
            Panel::Form(f) => f.editing.is_some() && f.editing == self.conflict_for,
            _ => false,
        }
    }

    pub(super) fn contact_conflict_read(
        &mut self,
        r: Result<Versioned<Contact>, ApiError>,
        intent: SaveIntent,
    ) {
        if !self.conflict_panel_matches() {
            return;
        }
        let theirs = match read_of(r) {
            Read::Theirs(v) => v,
            Read::Gone => {
                self.warn = Some(GONE);
                return;
            }
            Read::Reload => {
                self.warn = Some(CHANGED);
                return;
            }
        };
        let Panel::Form(f) = &self.panel else {
            return;
        };
        let conflict = Conflict::new(
            &contact_value(f.form.original()),
            &contact_value(f.form.current()),
            &contact_value(&draft_of(&theirs.record)),
            "lines",
        );
        if conflict.silent_resave() {
            if !std::mem::take(&mut self.resave_left) {
                // A second stale save in a row: say so, never loop.
                self.warn = Some(CHANGED);
                return;
            }
            let mine = f.form.current().clone();
            self.adopt_contact(&theirs.record, mine);
            self.save_inner(intent);
            return;
        }
        let accounts = self.accounts.clone();
        let words = Words::new(
            std::sync::Arc::default(),
            None,
            cl_core::MoneyStyle::default(),
            &[],
        );
        let dialog = ConflictDialog::new(conflict, words.line_text()).with_field_text(Box::new(
            move |field, v| {
                let s = v.as_str()?;
                match field {
                    "contactType" => roles_text(s),
                    "terms" => Some(terms_text(&TermsChoice::from_stored(Some(s)))),
                    "expenseAccount" => cl_core::ids::AccountId::parse(s)
                        .ok()
                        .map(|a| account_label(&accounts, a)),
                    _ => None,
                }
            },
        ));
        self.contact_conflict.open = Some(OpenConflict {
            dialog,
            theirs,
            intent,
        });
    }

    /// Their contact becomes the panel's "as opened" and `mine` its fields;
    /// the list shows their contact.
    fn adopt_contact(&mut self, theirs: &Contact, mine: ContactDraft) {
        self.remember_contact(theirs);
        if let Panel::Form(f) = &mut self.panel {
            f.take_theirs_record(theirs);
            if *f.form.current() != mine {
                f.form.edit_step(|d| *d = mine);
            }
        }
        self.warn = None;
    }

    /// The list's copy of `contact` becomes `contact`.
    fn remember_contact(&mut self, contact: &Contact) {
        match self.contacts.iter_mut().find(|c| c.id == contact.id) {
            Some(c) => *c = contact.clone(),
            None => self.contacts.push(contact.clone()),
        }
        self.want_cursor = Some(contact.id);
        let contacts = std::mem::take(&mut self.contacts);
        self.apply_contacts(contacts);
    }

    /// Draws the open dialog and runs the owner's choice.
    pub(super) fn contact_conflict_ui(&mut self, ctx: &eframe::egui::Context) {
        if let Some((open, choice)) = self.contact_conflict.ui(ctx) {
            self.run_contact_choice(open, choice);
        }
    }

    #[allow(clippy::needless_pass_by_value)] // the dialog hands over its choice
    pub(super) fn run_contact_choice(
        &mut self,
        open: OpenConflict<Versioned<Contact>>,
        choice: ConflictChoice,
    ) {
        if !self.conflict_panel_matches() {
            return;
        }
        match choice {
            ConflictChoice::Back => self.warn = Some(CHANGED),
            ConflictChoice::UseTheirs => {
                let theirs = open.theirs.record;
                self.remember_contact(&theirs);
                if let Panel::Form(f) = &mut self.panel {
                    f.take_theirs_record(&theirs);
                }
                self.warn = None;
            }
            ConflictChoice::KeepMine(_) => {
                let Panel::Form(f) = &self.panel else {
                    return;
                };
                let mut mine = f.form.current().clone();
                take_theirs(
                    &mut mine,
                    &draft_of(&open.theirs.record),
                    &fields_from_theirs(open.dialog.conflict()),
                );
                self.adopt_contact(&open.theirs.record, mine);
                self.resave_left = true;
                self.save_inner(open.intent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_model::lists::contacts::Role;

    #[test]
    fn every_field_has_plain_words() {
        for k in contact_value(&ContactDraft::new(Role::Vendor))
            .as_object()
            .expect("object")
            .keys()
        {
            assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
        }
    }

    #[test]
    fn keep_mine_takes_every_field_it_is_given() {
        let mut theirs = ContactDraft::new(Role::Vendor);
        theirs.name = "T".into();
        theirs.billing.city = "Springfield".into();
        theirs.track_1099 = true;
        theirs.terms = TermsChoice::from_stored(Some("net_30"));
        let fields: Vec<String> = contact_value(&theirs)
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect();
        let mut mine = ContactDraft::new(Role::Client);
        take_theirs(&mut mine, &theirs, &fields);
        assert_eq!(contact_value(&mine), contact_value(&theirs));
    }

    #[test]
    fn roles_read_as_words() {
        assert_eq!(roles_text("vendor"), Some(t("center.form.is_vendor")));
        assert!(roles_text("both").is_some_and(|s| s.contains(", ")));
        assert_eq!(roles_text("nope"), None);
    }
}
