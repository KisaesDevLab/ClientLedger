//! The add/edit contact panel of a center: name, company, person, email,
//! phone, roles (Q-77), terms from the engine's fixed list, bill-to and the
//! one ship-to address (U-36), notes (one field, U-37) and, for vendors,
//! the default expense account (Q-192) and 1099 fields (Q-53). A tax ID is
//! never shown in full once saved; the 1099 profile (tax ID type, backup
//! withholding) is saved right after the contact.

use cl_api::{
    Account, AccountType, ApiError, Contact, ContactFields, ContactUpdate, EditableFields,
    IdempotencyKey, NewContact, OrphanLabel, RecordVersion, TinType, Vendor1099Profile,
    Vendor1099Update, Versioned,
};
use cl_core::Money;
use cl_core::ids::{AccountId, ContactId};
use cl_model::forms::{AfterSave, FormState, SaveIntent, SaveStart};
use cl_model::lists::contacts::{
    AddressDraft, CheckedContact, ContactDraft, ContactField, ContactProblem, Role, Roles,
    TaxIdKind, Terms, TermsChoice, check, masked_tax_id,
};
use eframe::egui::{self, RichText, Ui};

use super::model::{api_roles, roles_of};
use super::{CenterScreen, Panel};
use crate::company::Task;
use crate::i18n::{t, t_with};
use crate::screens::company::form as cform;
use crate::screens::lists::detail_pane as pane;
use crate::theme::Tokens;

// Field ids for undo merging.
const F_NAME: u64 = 1;
const F_COMPANY: u64 = 2;
const F_FIRST: u64 = 3;
const F_LAST: u64 = 4;
const F_EMAIL: u64 = 5;
const F_PHONE: u64 = 6;
const F_ROLES: u64 = 7;
const F_TERMS: u64 = 8;
const F_BILL: u64 = 10;
const F_SHIP: u64 = 20;
const F_NOTES: u64 = 30;
const F_EXPENSE: u64 = 31;
const F_VENDOR_ACCOUNT: u64 = 32;
const F_TRACK: u64 = 33;
const F_TAX_ID: u64 = 34;
const F_TAX_KIND: u64 = 35;
const F_BACKUP: u64 = 36;

fn field_id(f: ContactField) -> egui::Id {
    egui::Id::unique(("center-form", f as u8))
}

/// What a save answered: the contact, and how saving its 1099 profile went
/// (`None` when there was nothing to change).
pub(super) struct Saved {
    contact: Versioned<Contact>,
    profile: Option<Result<(), ApiError>>,
}

/// The add/edit panel.
pub(super) struct FormPanel {
    /// The contact being changed; `None` for a new one.
    pub(super) editing: Option<ContactId>,
    pub(super) form: FormState<ContactDraft>,
    pub(super) problems: Vec<ContactProblem>,
    pub(super) error: Option<ApiError>,
    focus: Option<ContactField>,
    pub(super) task: Option<Task<Saved>>,
    /// The tax ID on file (never shown in full).
    saved_tax_id: Option<String>,
    /// Tax ID type and backup withholding as the profile had them.
    profile_before: (Option<TaxIdKind>, bool),
    /// W-9 on file, from the profile (read only, U-44).
    w9_on_file: Option<bool>,
    /// A plain warning (i18n key) inside the panel.
    warn: Option<&'static str>,
}

pub(super) fn kind_of(t: Option<TinType>) -> Option<TaxIdKind> {
    t.map(|t| match t {
        TinType::Ssn => TaxIdKind::Ssn,
        TinType::Ein => TaxIdKind::Ein,
    })
}

fn tin_of(k: Option<TaxIdKind>) -> Option<TinType> {
    k.map(|k| match k {
        TaxIdKind::Ssn => TinType::Ssn,
        TaxIdKind::Ein => TinType::Ein,
    })
}

fn s(v: Option<&str>) -> String {
    v.unwrap_or_default().to_string()
}

/// The owner-facing text of stored terms.
pub(super) fn terms_text(terms: &TermsChoice) -> String {
    match terms {
        TermsChoice::None => String::new(),
        TermsChoice::Fixed(x) => t(x.label().as_str()),
        TermsChoice::Other(o) => o.clone(),
    }
}

/// `number name` of an account (or a note that it is gone).
pub(super) fn account_label(accounts: &[Account], id: AccountId) -> String {
    accounts.iter().find(|a| a.id == id).map_or_else(
        || t("center.form.account_gone"),
        |a| match a.fields.account_number.as_deref().filter(|n| !n.is_empty()) {
            Some(n) => format!("{n} {}", crate::account_names::shown_name(a)),
            None => crate::account_names::shown_name(a),
        },
    )
}

/// An account a vendor's bills may start with.
fn expense_like(a: &Account) -> bool {
    matches!(
        a.fields.account_type,
        AccountType::Expense | AccountType::Cogs | AccountType::OtherExpense
    )
}

impl FormPanel {
    /// A new contact made in `role`'s center.
    pub(super) fn new_contact(role: Role) -> FormPanel {
        FormPanel {
            editing: None,
            form: FormState::new(ContactDraft::new(role)),
            problems: Vec::new(),
            error: None,
            focus: Some(ContactField::Name),
            task: None,
            saved_tax_id: None,
            profile_before: (None, false),
            w9_on_file: None,
            warn: None,
        }
    }

    /// The form for `c` (its 1099 profile if the pane loaded it).
    pub(super) fn edit(c: &Contact, profile: Option<&Vendor1099Profile>) -> FormPanel {
        let f = &c.fields;
        let address = |l1: &Option<String>,
                       l2: &Option<String>,
                       city: &Option<String>,
                       state: &Option<String>,
                       zip: &Option<String>| AddressDraft {
            line1: s(l1.as_deref()),
            line2: s(l2.as_deref()),
            city: s(city.as_deref()),
            state: s(state.as_deref()),
            zip: s(zip.as_deref()),
        };
        let kind = kind_of(profile.and_then(|p| p.tin_type));
        let backup = profile.is_some_and(|p| p.backup_withholding);
        let draft = ContactDraft {
            roles: roles_of(f.contact_type),
            name: f.display_name.clone(),
            company: s(f.company_name.as_deref()),
            first: s(f.first_name.as_deref()),
            last: s(f.last_name.as_deref()),
            email: s(f.email.as_deref()),
            phone: s(f.phone.as_deref()),
            billing: address(
                &f.billing_line1,
                &f.billing_line2,
                &f.billing_city,
                &f.billing_state,
                &f.billing_zip,
            ),
            shipping: address(
                &f.shipping_line1,
                &f.shipping_line2,
                &f.shipping_city,
                &f.shipping_state,
                &f.shipping_zip,
            ),
            terms: TermsChoice::from_stored(f.default_payment_terms.as_deref()),
            notes: s(f.notes.as_deref()),
            expense_account: f.default_expense_account_id,
            vendor_account: s(f.vendor_account_number.as_deref()),
            track_1099: f.is1099_eligible,
            // A saved tax ID is never put back in the field (masked hint).
            tax_id: String::new(),
            clear_tax_id: false,
            tax_id_kind: kind,
            backup_withholding: backup,
        };
        FormPanel {
            editing: Some(c.id),
            form: FormState::new(draft),
            problems: Vec::new(),
            error: None,
            focus: Some(ContactField::Name),
            task: None,
            saved_tax_id: f.tax_id.clone().filter(|x| !x.trim().is_empty()),
            profile_before: (kind, backup),
            w9_on_file: profile.map(|p| p.w9_on_file),
            warn: None,
        }
    }

    /// Esc on a dirty form: back to what was loaded, focus kept.
    pub(super) fn revert(&mut self) {
        self.form.revert_form();
        self.problems.clear();
        self.error = None;
        self.warn = None;
        self.focus = Some(ContactField::Name);
    }

    /// What the "Remove tax ID" button does.
    #[cfg(test)]
    pub(super) fn clear_tax_id_for_test(&mut self) {
        self.form.edit(F_TAX_ID, |x| x.clear_tax_id = true);
    }

    /// Puts the keyboard in the tax ID field on the next frame.
    #[cfg(test)]
    pub(super) fn focus_tax_id_for_test(&mut self) {
        self.focus = Some(ContactField::TaxId);
    }

    fn save_failed(&mut self, e: ApiError) {
        self.form.save_failed(cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        });
        self.error = Some(e);
    }
}

/// Engine fields for a checked draft, keeping what the form does not edit
/// (countries, default tag) and, for a contact without the vendor role,
/// its vendor fields as they were.
fn fields_for(checked: &CheckedContact, existing: Option<&ContactFields>) -> ContactFields {
    let mut f = existing
        .cloned()
        .unwrap_or_else(|| ContactFields::named(api_roles(checked.roles), checked.name.clone()));
    f.contact_type = api_roles(checked.roles);
    f.display_name.clone_from(&checked.name);
    f.company_name.clone_from(&checked.company);
    f.first_name.clone_from(&checked.first);
    f.last_name.clone_from(&checked.last);
    f.email.clone_from(&checked.email);
    f.phone.clone_from(&checked.phone);
    let [l1, l2, city, state, zip] = checked.billing.clone();
    (
        f.billing_line1,
        f.billing_line2,
        f.billing_city,
        f.billing_state,
        f.billing_zip,
    ) = (l1, l2, city, state, zip);
    let [l1, l2, city, state, zip] = checked.shipping.clone();
    (
        f.shipping_line1,
        f.shipping_line2,
        f.shipping_city,
        f.shipping_state,
        f.shipping_zip,
    ) = (l1, l2, city, state, zip);
    f.default_payment_terms.clone_from(&checked.terms);
    f.notes.clone_from(&checked.notes);
    if checked.roles.has(Role::Vendor) {
        f.default_expense_account_id = checked.expense_account;
        f.vendor_account_number.clone_from(&checked.vendor_account);
        f.is1099_eligible = checked.track_1099;
        // An empty field keeps the tax ID on file unless it was removed.
        if checked.tax_id.is_some() || checked.clear_tax_id || existing.is_none() {
            f.tax_id.clone_from(&checked.tax_id);
        }
    }
    f
}

impl CenterScreen {
    /// Alt+N / Alt+S / Enter in a field. Returns true when the save
    /// finished at once (demo) or nothing needed saving.
    pub(super) fn save_form(&mut self, intent: SaveIntent) -> bool {
        let accounts = &self.accounts;
        let list = self.model.list();
        let Panel::Form(panel) = &mut self.panel else {
            return false;
        };
        if panel.task.is_some() {
            return false;
        }
        let ok_account = |a: AccountId| {
            accounts.is_empty() || accounts.iter().any(|x| x.id == a && expense_like(x))
        };
        let checked = match check(panel.form.current(), list, panel.editing, ok_account) {
            Ok(c) => c,
            Err(problems) => {
                panel.focus = problems.first().map(|p| p.field());
                panel.problems = problems;
                return false;
            }
        };
        // The client's Sales section is checked before anything is sent
        // (S9): a bad markup keeps the form open.
        if !self.sales.check() {
            return false;
        }
        let Panel::Form(panel) = &mut self.panel else {
            return false;
        };
        panel.problems.clear();
        panel.error = None;
        match panel.form.begin_save(intent) {
            SaveStart::Send(_) => {}
            SaveStart::Skip(after) => {
                // Only the Sales section changed (P20).
                if let Some(id) = panel.editing {
                    self.sales.save(&self.link, self.group.as_ref(), id);
                }
                self.after_save(after, None);
                return true;
            }
            SaveStart::Busy | SaveStart::Unconfirmed => return false,
        }
        let editing = panel.editing;
        let existing = editing.and_then(|id| self.contacts.iter().find(|c| c.id == id).cloned());
        let fields = fields_for(&checked, existing.as_ref().map(|c| &c.fields));
        let vendor = checked.roles.has(Role::Vendor);
        let (kind0, backup0) = panel.profile_before;
        let update = Vendor1099Update {
            tin_type: (vendor && checked.tax_id_kind != kind0).then(|| tin_of(checked.tax_id_kind)),
            backup_withholding: (vendor && checked.backup_withholding != backup0)
                .then_some(checked.backup_withholding),
        };
        let update = (!update.is_empty()).then_some(update);
        if self.link.is_demo() {
            let saved = demo_save(&self.link, existing, fields, update.as_ref());
            let Panel::Form(panel) = &mut self.panel else {
                return false;
            };
            panel.task = Some(Task::ready(saved));
            return self.poll_save();
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return false;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.contact_save",
            key: Some(key.clone()),
        };
        let pending = group.write(&session, label, move |s| async move {
            let contact = if let Some(c) = existing {
                let version = RecordVersion::Fingerprint(c.fingerprint());
                let body = ContactUpdate {
                    fields,
                    is_active: c.is_active,
                };
                s.contacts().update(c.id, &version, &body, key).await?
            } else {
                let body = NewContact {
                    fields,
                    opening_balance: Money::ZERO,
                    opening_balance_date: None,
                };
                s.contacts().create(&body, key).await?
            };
            let profile = match update {
                Some(u) => Some(
                    s.form1099()
                        .update_profile(contact.record.id, &u, IdempotencyKey::new())
                        .await,
                ),
                None => None,
            };
            Ok(Saved { contact, profile })
        });
        if let (Ok(p), Panel::Form(panel)) = (pending, &mut self.panel) {
            panel.task = Some(Task::Pending(p));
        }
        false
    }

    /// Takes a finished save. Returns true when it saved.
    pub(super) fn poll_save(&mut self) -> bool {
        let Panel::Form(panel) = &mut self.panel else {
            return false;
        };
        let Some(result) = panel.task.as_mut().and_then(Task::try_take) else {
            return false;
        };
        panel.task = None;
        match result {
            Ok(saved) => {
                let after = panel.form.save_ok(panel.form.current().clone());
                let contact = saved.contact.record;
                let id = contact.id;
                // The client's price level and markup go after the contact.
                self.sales.save(&self.link, self.group.as_ref(), id);
                let here = roles_of(contact.fields.contact_type).has(self.role);
                match self.contacts.iter_mut().find(|c| c.id == id) {
                    Some(c) => *c = contact,
                    None => self.contacts.push(contact),
                }
                self.want_cursor = Some(id);
                let contacts = std::mem::take(&mut self.contacts);
                self.apply_contacts(contacts);
                // The pane reloads (1099 profile, roles).
                self.detail = None;
                self.warn = None;
                self.note = Some(if here {
                    "center.saved"
                } else {
                    "center.saved_elsewhere"
                });
                if matches!(saved.profile, Some(Err(_))) {
                    self.warn = Some("center.profile_not_saved");
                }
                // A new contact's opening balance is a transaction.
                self.publish(true);
                self.after_save(after, Some(id));
                true
            }
            Err(e) => {
                panel.save_failed(e);
                false
            }
        }
    }

    fn after_save(&mut self, after: AfterSave, saved: Option<ContactId>) {
        match after {
            AfterSave::Stay => {
                if let Some(c) = saved.and_then(|id| self.contacts.iter().find(|c| c.id == id)) {
                    let mut p = FormPanel::edit(c, None);
                    if let Panel::Form(old) = &self.panel {
                        p.profile_before = (
                            old.form.current().tax_id_kind,
                            old.form.current().backup_withholding,
                        );
                        p.w9_on_file = old.w9_on_file;
                        let mut d = p.form.current().clone();
                        d.tax_id_kind = p.profile_before.0;
                        d.backup_withholding = p.profile_before.1;
                        p.form = FormState::new(d);
                    }
                    p.focus = None;
                    self.panel = Panel::Form(Box::new(p));
                }
            }
            AfterSave::NewForm => {
                self.panel = Panel::Form(Box::new(FormPanel::new_contact(self.role)));
            }
            AfterSave::CloseWindow => {
                // Save & Close closes the panel; the window stays.
                self.panel = Panel::None;
                self.place_cursor(saved.or_else(|| self.selected()));
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one labelled field after another, in tab order"
    )]
    pub(super) fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let accounts = self.accounts.clone();
        let role = self.role;
        let sales = &mut self.sales;
        let Panel::Form(panel) = &mut self.panel else {
            return;
        };
        let mut save: Option<SaveIntent> = None;
        let mut cancel = false;
        let heading = match (panel.editing.is_some(), role) {
            (true, Role::Client) => t("center.form.edit_client"),
            (true, Role::Vendor) => t("center.form.edit_vendor"),
            (false, Role::Client) => t("center.form.new_client"),
            (false, Role::Vendor) => t("center.form.new_vendor"),
        };
        cform::heading(ui, tokens, &heading);
        ui.add_space(6.0);
        let saving = panel.task.is_some();
        ui.add_enabled_ui(!saving, |ui| {
            let d = panel.form.current().clone();
            let focus = panel.focus.take();
            let want = |f: ContactField| focus == Some(f);
            let mut text = |ui: &mut Ui,
                            panel: &mut FormPanel,
                            f: ContactField,
                            undo: u64,
                            key: &str,
                            value: &str,
                            set: fn(&mut ContactDraft, String)| {
                let mut v = value.to_string();
                let r = pane::text_row(ui, field_id(f), &t(key), &mut v, want(f));
                if r.changed() {
                    panel.form.edit(undo, |x| set(x, v));
                }
                if crate::forms::enter::entered(&r).saves() {
                    save = Some(SaveIntent::SaveAndNew);
                }
            };
            text(
                ui,
                panel,
                ContactField::Name,
                F_NAME,
                "center.form.name",
                &d.name,
                |x, v| x.name = v,
            );
            text(
                ui,
                panel,
                ContactField::Company,
                F_COMPANY,
                "center.form.company",
                &d.company,
                |x, v| x.company = v,
            );
            text(
                ui,
                panel,
                ContactField::First,
                F_FIRST,
                "center.form.first",
                &d.first,
                |x, v| x.first = v,
            );
            text(
                ui,
                panel,
                ContactField::Last,
                F_LAST,
                "center.form.last",
                &d.last,
                |x, v| x.last = v,
            );
            text(
                ui,
                panel,
                ContactField::Email,
                F_EMAIL,
                "center.form.email",
                &d.email,
                |x, v| x.email = v,
            );
            text(
                ui,
                panel,
                ContactField::Phone,
                F_PHONE,
                "center.form.phone",
                &d.phone,
                |x, v| x.phone = v,
            );

            // Roles (Q-77): a contact keeps at least one.
            ui.horizontal(|ui| {
                pane::caption(ui, RichText::new(t("center.form.roles")));
                for (r, key) in [
                    (Role::Client, "center.form.is_client"),
                    (Role::Vendor, "center.form.is_vendor"),
                ] {
                    let mut on = d.roles.has(r);
                    let resp = ui.checkbox(&mut on, t(key));
                    if want(ContactField::Roles) && r == Role::Client {
                        resp.request_focus();
                    }
                    if resp.changed() {
                        match d.roles.with(r, on) {
                            Some(next) => {
                                panel.form.edit(F_ROLES, |x| x.roles = next);
                                panel.warn = None;
                            }
                            None => panel.warn = Some("center.form.needs_role"),
                        }
                    }
                }
            });
            if d.roles == Roles::Both {
                cform::hint(ui, tokens, &t("center.form.both_hint"));
            }

            // Terms: the engine's fixed list (U-43), other stored text kept.
            let mut choices: Vec<TermsChoice> = vec![TermsChoice::None];
            choices.extend(Terms::ALL.into_iter().map(TermsChoice::Fixed));
            if let TermsChoice::Other(_) = &d.terms {
                choices.push(d.terms.clone());
            }
            let labels: Vec<String> = choices
                .iter()
                .map(|c| match c {
                    TermsChoice::None => t("center.form.no_terms"),
                    other => terms_text(other),
                })
                .collect();
            let cur = choices.iter().position(|c| *c == d.terms).unwrap_or(0);
            if let Some(i) = pane::picker(
                ui,
                field_id(ContactField::Terms),
                &t("center.form.terms"),
                &labels,
                cur,
                want(ContactField::Terms),
            ) {
                let v = choices[i].clone();
                panel.form.edit(F_TERMS, |x| x.terms = v);
            }

            // Bill-to and the one ship-to address.
            address_ui(
                ui,
                tokens,
                panel,
                true,
                &d.billing,
                want(ContactField::Billing),
            );
            if ui.button(t("center.form.ship_same")).clicked() {
                panel.form.edit_step(ContactDraft::ship_to_billing);
            }
            address_ui(
                ui,
                tokens,
                panel,
                false,
                &d.shipping,
                want(ContactField::Shipping),
            );
            pane::pending(ui, tokens, &t("center.ship_to.later"));

            let mut notes = d.notes.clone();
            if pane::text_area(
                ui,
                field_id(ContactField::Notes),
                &t("center.form.notes"),
                &mut notes,
                want(ContactField::Notes),
            )
            .changed()
            {
                panel.form.edit(F_NOTES, |x| x.notes = notes);
            }
            if role == Role::Client {
                sales.ui(ui, tokens);
            }

            if d.roles.has(Role::Vendor) {
                pane::section(ui, tokens, &t("center.details.vendor"));
                let mut ids: Vec<Option<AccountId>> = vec![None];
                let mut labels = vec![t("center.form.no_account")];
                let mut list: Vec<&Account> = accounts
                    .iter()
                    .filter(|a| a.is_active && expense_like(a))
                    .collect();
                list.sort_by(|a, b| {
                    a.fields
                        .account_number
                        .cmp(&b.fields.account_number)
                        .then_with(|| a.fields.name.cmp(&b.fields.name))
                });
                for a in list {
                    ids.push(Some(a.id));
                    labels.push(account_label(&accounts, a.id));
                }
                if let Some(cur) = d.expense_account
                    && !ids.contains(&Some(cur))
                {
                    ids.push(Some(cur));
                    labels.push(account_label(&accounts, cur));
                }
                let cur = ids
                    .iter()
                    .position(|i| *i == d.expense_account)
                    .unwrap_or(0);
                if let Some(i) = pane::picker(
                    ui,
                    field_id(ContactField::ExpenseAccount),
                    &t("center.field.expense_account"),
                    &labels,
                    cur,
                    want(ContactField::ExpenseAccount),
                ) {
                    let v = ids[i];
                    panel.form.edit(F_EXPENSE, |x| x.expense_account = v);
                }
                cform::hint(ui, tokens, &t("center.form.expense_hint"));
                text(
                    ui,
                    panel,
                    ContactField::VendorAccount,
                    F_VENDOR_ACCOUNT,
                    "center.field.vendor_account",
                    &d.vendor_account,
                    |x, v| x.vendor_account = v,
                );

                pane::section(ui, tokens, &t("center.details.1099"));
                let mut track = d.track_1099;
                if ui
                    .checkbox(&mut track, t("center.field.track_1099"))
                    .changed()
                {
                    panel.form.edit(F_TRACK, |x| x.track_1099 = track);
                }
                let mut tax = d.tax_id.clone();
                let r = ui
                    .horizontal(|ui| {
                        let l = pane::caption(ui, RichText::new(t("center.field.tax_id")));
                        let hint = match (&panel.saved_tax_id, d.clear_tax_id) {
                            (Some(saved), false) => {
                                masked_tax_id(saved, d.tax_id_kind).unwrap_or_default()
                            }
                            _ => String::new(),
                        };
                        ui.add_sized(
                            [ui.available_width().min(260.0), 24.0],
                            // A tax ID is masked on screen and in the
                            // accessibility tree, like a password.
                            egui::TextEdit::singleline(&mut tax)
                                .password(true)
                                .id(field_id(ContactField::TaxId))
                                .hint_text(hint),
                        )
                        .labelled_by(l.id)
                    })
                    .inner;
                if want(ContactField::TaxId) {
                    r.request_focus();
                }
                if r.changed() {
                    panel.form.edit(F_TAX_ID, |x| x.tax_id = tax);
                }
                if panel.saved_tax_id.is_some() {
                    let key = if d.clear_tax_id {
                        "center.form.tax_id_cleared"
                    } else {
                        "center.form.tax_id_on_file"
                    };
                    cform::hint(ui, tokens, &t(key));
                    if !d.clear_tax_id
                        && d.tax_id.trim().is_empty()
                        && ui.button(t("center.form.tax_id_remove")).clicked()
                    {
                        panel.form.edit(F_TAX_ID, |x| x.clear_tax_id = true);
                    }
                }
                let kinds = [None, Some(TaxIdKind::Ein), Some(TaxIdKind::Ssn)];
                let labels = [
                    t("center.tin.unknown"),
                    t("center.tin.ein"),
                    t("center.tin.ssn"),
                ];
                let cur = kinds.iter().position(|k| *k == d.tax_id_kind).unwrap_or(0);
                if let Some(i) = pane::picker(
                    ui,
                    egui::Id::unique("center-form-tin-kind"),
                    &t("center.field.tin_type"),
                    &labels,
                    cur,
                    false,
                ) {
                    let v = kinds[i];
                    panel.form.edit(F_TAX_KIND, |x| x.tax_id_kind = v);
                }
                let mut backup = d.backup_withholding;
                if ui.checkbox(&mut backup, t("center.field.backup")).changed() {
                    panel.form.edit(F_BACKUP, |x| x.backup_withholding = backup);
                }
                let w9 = match panel.w9_on_file {
                    Some(true) => t("center.yes"),
                    Some(false) => t("center.no"),
                    None => String::new(),
                };
                pane::row(ui, tokens, &t("center.field.w9"), &w9);
                pane::pending(ui, tokens, &t("center.w9.later"));
            }
        });
        ui.add_space(6.0);
        if let Some(w) = panel.warn {
            ui.label(RichText::new(t(w)).color(tokens.warn_text));
        }
        for p in &panel.problems {
            ui.label(RichText::new(t(p.key().as_str())).color(tokens.warn_text));
        }
        if let Some(e) = &panel.error {
            cform::inline_error(ui, tokens, e);
        }
        if saving {
            cform::progress_line(ui, tokens, &t("center.form.saving"));
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let hint = |id: &str| super::shortcut_hint(id);
            if crate::widgets::primary_button(ui, &t("center.form.save_close"))
                .on_hover_text(t_with(
                    "center.form.key_hint",
                    &[("key", &hint("form.save_close"))],
                ))
                .clicked()
            {
                save = Some(SaveIntent::SaveAndClose);
            }
            if ui
                .button(t("center.form.save_new"))
                .on_hover_text(t_with(
                    "center.form.key_hint",
                    &[("key", &hint("form.save_new"))],
                ))
                .clicked()
            {
                save = Some(SaveIntent::SaveAndNew);
            }
            if ui.button(t("center.form.cancel")).clicked() {
                cancel = true;
            }
        });
        if let Some(intent) = save {
            self.save_form(intent);
        }
        if cancel {
            self.close_panel();
        }
    }
}

/// One address part: label key, current text, and where it lives.
type AddressPart<'a> = (&'static str, &'a str, fn(&mut AddressDraft) -> &mut String);

/// One address block (bill-to or ship-to).
fn address_ui(
    ui: &mut Ui,
    tokens: &Tokens,
    panel: &mut FormPanel,
    billing: bool,
    a: &AddressDraft,
    focus: bool,
) {
    let (title, base) = if billing {
        ("center.field.bill_to", F_BILL)
    } else {
        ("center.field.ship_to", F_SHIP)
    };
    pane::section(ui, tokens, &t(title));
    let parts: [AddressPart<'_>; 5] = [
        ("center.form.line1", &a.line1, |x| &mut x.line1),
        ("center.form.line2", &a.line2, |x| &mut x.line2),
        ("center.form.city", &a.city, |x| &mut x.city),
        ("center.form.state", &a.state, |x| &mut x.state),
        ("center.form.zip", &a.zip, |x| &mut x.zip),
    ];
    for (i, (key, value, get)) in parts.into_iter().enumerate() {
        let mut v = value.to_string();
        let id = egui::Id::unique(("center-form-address", billing, i));
        let label = t_with(key, &[("which", &t(title))]);
        let r = pane::text_row(ui, id, &label, &mut v, focus && i == 0);
        if r.changed() {
            let undo = base + u64::try_from(i).unwrap_or(0);
            panel.form.edit(undo, |x| {
                let addr = if billing {
                    &mut x.billing
                } else {
                    &mut x.shipping
                };
                *get(addr) = v;
            });
        }
    }
}

/// Saves into the demo contacts at once (no engine).
fn demo_save(
    link: &crate::company::CompanyLink,
    existing: Option<Contact>,
    fields: ContactFields,
    update: Option<&Vendor1099Update>,
) -> Saved {
    let mut d = link.demo_state();
    let contact = if let Some(mut c) = existing {
        c.fields = fields;
        c
    } else {
        let n = d.as_mut().map_or(0, |d| d.contacts_mut().len());
        Contact {
            id: crate::company::demo::contacts::contact_id(900_000 + u32::try_from(n).unwrap_or(0)),
            fields,
            is_active: true,
            opening_balance: Some(Money::ZERO),
            opening_balance_date: None,
            created_at: Some(crate::company::demo::DemoState::now()),
            updated_at: Some(crate::company::demo::DemoState::now()),
        }
    };
    let mut profile = None;
    if let Some(d) = d.as_mut() {
        let list = d.contacts_mut();
        match list.iter_mut().find(|c| c.id == contact.id) {
            Some(c) => *c = contact.clone(),
            None => list.push(contact.clone()),
        }
        if let Some(u) = update {
            let mut p = crate::company::demo::contacts::profile(d, contact.id);
            if let Some(k) = u.tin_type {
                p.tin_type = k;
            }
            if let Some(b) = u.backup_withholding {
                p.backup_withholding = b;
            }
            d.profiles.insert(contact.id, p);
            profile = Some(Ok(()));
        }
    }
    Saved {
        contact: Versioned {
            version: RecordVersion::Fingerprint(contact.fingerprint()),
            record: contact,
        },
        profile,
    }
}
