//! Set Closing Date (P29 WP5, Q-155): the Admin sets, moves or clears the
//! closing date and changes the closing-date password. Transactions dated
//! on or before the date are locked; the engine checks the password, keeps
//! the audit trail and (U-229) refuses anyone but an Admin. The shell opens
//! this window for Admins only (`company.closing_date` is `AdminOnly`).
//!
//! Passwords live in [`SecretText`] fields, go to the engine once and are
//! emptied right after sending; nothing here logs them.

use std::sync::Arc;

use cl_api::{
    ApiError, Capabilities, ClosingDateChange, ClosingPasswordStatus, IdempotencyKey, OrphanLabel,
    RequestGroup,
};
use cl_core::{CalendarDate, Secret};
use cl_model::CommandId;
use eframe::egui::{RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_with};
use crate::screens::company::form::{self, DateInput, SecretText};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The window kind.
pub const KIND: &str = "company.closing_date";

/// The window for `kind`, if it is this one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(cl_core::ids::TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(ClosingDateScreen::new(Arc::clone(link), today)) as _)
}

/// What a finished write changed.
enum Wrote {
    Date(Option<CalendarDate>),
    Password,
}

/// The screen.
pub struct ClosingDateScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    status: Option<ClosingPasswordStatus>,
    read: Option<Job<ClosingPasswordStatus>>,
    write: Option<Job<Wrote>>,
    date: DateInput,
    password: SecretText,
    new_password: SecretText,
    confirm: SecretText,
    current: SecretText,
    changed: SecretText,
    changed_again: SecretText,
    /// i18n key of the last message, and whether it is good news.
    message: Option<(&'static str, bool)>,
    error: Option<ApiError>,
}

fn label() -> OrphanLabel {
    OrphanLabel {
        what: "orphan.closing_date",
        key: None,
    }
}

fn secret(s: &mut SecretText) -> Secret {
    let out = Secret::new(s.trimmed().to_owned());
    s.clear();
    out
}

/// The plain words for an engine refusal (`None`: the generic error box).
#[must_use]
pub fn refusal(e: &ApiError) -> Option<&'static str> {
    Some(match e {
        ApiError::ClosingPasswordRejected => "closing.admin.wrong_password",
        ApiError::ClosingPasswordRequired => "closing.admin.password_needed",
        ApiError::ClosingPasswordRateLimited { .. } => "closing.admin.too_many",
        ApiError::ClosingPasswordOwnerRequired | ApiError::Forbidden { .. } => {
            "closing.admin.only_admin"
        }
        ApiError::Local(cl_api::LocalValidation::InvalidValue) => "closing.admin.password_short",
        ApiError::Local(
            cl_api::LocalValidation::ClosingDateWriteUnavailable
            | cl_api::LocalValidation::ClosingPasswordUnavailable,
        ) => "closing.admin.needs_update",
        ApiError::Conflict {
            reason: cl_api::ConflictReason::Stale,
            ..
        } => "closing.admin.changed_elsewhere",
        _ => return None,
    })
}

impl ClosingDateScreen {
    /// Reads the closing date and password state.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> ClosingDateScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = ClosingDateScreen {
            group,
            today,
            status: None,
            read: None,
            write: None,
            date: DateInput::new(today),
            password: SecretText::default(),
            new_password: SecretText::default(),
            confirm: SecretText::default(),
            current: SecretText::default(),
            changed: SecretText::default(),
            changed_again: SecretText::default(),
            message: None,
            error: None,
            link,
        };
        s.reload();
        s
    }

    fn supported(&self) -> bool {
        self.link.is_demo() || self.link.capabilities().has(Capabilities::CLOSING_PASSWORD)
    }

    fn reload(&mut self) {
        if !self.supported() {
            return;
        }
        self.read = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::closing::status(d)),
            |s| async move { s.closing_password_status().await },
        ));
        self.poll();
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.read) {
            match r {
                Ok(st) => {
                    if let Some(d) = st.closing_date {
                        self.date.set(d);
                    }
                    self.status = Some(st);
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.write) {
            match r {
                Ok(Wrote::Date(d)) => {
                    self.message = Some((
                        if d.is_some() {
                            "closing.admin.saved"
                        } else {
                            "closing.admin.cleared"
                        },
                        true,
                    ));
                    self.link.push(CompanyEvent::ClosingDate(d));
                    // Locked rows change in every register and form.
                    self.link
                        .bus()
                        .publish(crate::data_bus::Change::Ledger(None));
                    tracing::info!("closing date changed");
                    self.reload();
                }
                Ok(Wrote::Password) => {
                    self.message = Some(("closing.admin.password_changed", true));
                    tracing::info!("closing-date password changed");
                    self.reload();
                }
                Err(e) => {
                    if matches!(
                        e,
                        ApiError::Conflict {
                            reason: cl_api::ConflictReason::Stale,
                            ..
                        }
                    ) {
                        // Show the date as it is now.
                        self.reload();
                    }
                    match refusal(&e) {
                        Some(k) => self.message = Some((k, false)),
                        None => self.error = Some(e),
                    }
                }
            }
        }
    }

    fn password_set(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.password_set)
    }

    /// Save the date (`clearing`: open the books).
    fn save_date(&mut self, clearing: bool) {
        self.error = None;
        let set = self.password_set();
        if let Some(k) = cl_model::closing::date_change_problem(
            self.date.value,
            clearing,
            set,
            self.password.trimmed(),
            self.new_password.trimmed(),
            self.confirm.trimmed(),
        ) {
            self.message = Some((k, false));
            return;
        }
        let password = if set {
            Some(secret(&mut self.password))
        } else if clearing {
            None
        } else {
            Some(secret(&mut self.new_password))
        };
        self.confirm.clear();
        let change = ClosingDateChange {
            closing_date: if clearing { None } else { self.date.value },
            closing_password: password,
        };
        // PR-8 S3: refused if another computer changed the date since this
        // screen read it (the owner sees the new one first).
        let shown = self.status.as_ref().and_then(|s| s.closing_date);
        let demo_change = change.clone();
        self.message = None;
        self.write = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label(),
            move |d| crate::company::demo::closing::set_date(d, &demo_change).map(Wrote::Date),
            move |s| async move {
                let key = IdempotencyKey::new();
                let out = s.set_closing_date_from(shown, &change, key).await?;
                Ok(Wrote::Date(out.lock_date))
            },
        ));
        self.poll();
    }

    fn change_password(&mut self) {
        self.error = None;
        let set = self.password_set();
        if let Some(k) = cl_model::closing::password_change_problem(
            set,
            self.current.trimmed(),
            self.changed.trimmed(),
            self.changed_again.trimmed(),
        ) {
            self.message = Some((k, false));
            return;
        }
        let current = set.then(|| secret(&mut self.current));
        let new = secret(&mut self.changed);
        self.changed_again.clear();
        let (c2, n2) = (current.clone(), new.clone());
        self.message = None;
        self.write = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label(),
            move |d| {
                crate::company::demo::closing::set_password(d, c2.as_ref(), Some(&n2))
                    .map(|_| Wrote::Password)
            },
            move |s| async move {
                s.set_closing_password(current.as_ref(), Some(&new), IdempotencyKey::new())
                    .await
                    .map(|_| Wrote::Password)
            },
        ));
        self.poll();
    }

    fn exceptions(&self) {
        self.link.push(CompanyEvent::OpenWith(
            CommandId::new(crate::screens::reports::KIND),
            Box::new(crate::windows::OpenPayload::Report(Box::new(
                crate::screens::reports::ReportOpen {
                    key: "closing_date_exceptions".into(),
                    options: None,
                },
            ))),
        ));
    }

    fn status_text(&self) -> String {
        match self.status.as_ref().and_then(|s| s.closing_date) {
            Some(d) => t_with(
                "closing.admin.closed_through",
                &[("date", &crate::shell::us_date(d))],
            ),
            None => t("closing.admin.open"),
        }
    }
}

impl Screen for ClosingDateScreen {
    fn title(&self) -> String {
        t("cmd.company.closing_date")
    }

    fn dirty(&self) -> bool {
        !(self.password.is_empty()
            && self.new_password.is_empty()
            && self.current.is_empty()
            && self.changed.is_empty())
    }

    fn can_save(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        let tokens = Tokens::of(ui.ctx());
        if self.read.is_some() || self.write.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        form::heading(ui, &tokens, &t("cmd.company.closing_date"));
        form::hint(ui, &tokens, &t("closing.admin.intro"));
        if !self.supported() {
            form::inline_warning(ui, &tokens, &t("closing.admin.needs_update"));
            return;
        }
        if self.status.is_none() {
            if let Some(e) = &self.error {
                form::inline_error_details(ui, &tokens, e, "closing.admin");
            } else {
                form::progress_line(ui, &tokens, &t("closing.admin.loading"));
            }
            return;
        }
        ui.add_space(6.0);
        ui.label(RichText::new(self.status_text()).strong());
        ui.add_space(8.0);
        let set = self.password_set();
        let busy = self.write.is_some();
        let mut act: Option<u8> = None;
        form::card(ui, &tokens, |ui| {
            ui.label(RichText::new(t("closing.admin.date_card")).strong());
            form::date_field(ui, &t("closing.admin.date"), &mut self.date, self.today);
            if set {
                form::password_secret(ui, &t("closing.admin.password"), &mut self.password);
            } else {
                form::hint(ui, &tokens, &t("closing.admin.first_password"));
                form::password_secret(ui, &t("closing.admin.new_password"), &mut self.new_password);
                form::password_secret(ui, &t("closing.admin.confirm"), &mut self.confirm);
            }
            ui.add_enabled_ui(!busy, |ui| {
                match form::actions(ui, &t("closing.admin.save"), &[&t("closing.admin.clear")]) {
                    Some(0) => act = Some(0),
                    Some(1) => act = Some(1),
                    _ => {}
                }
            });
        });
        if set {
            ui.add_space(8.0);
            form::card(ui, &tokens, |ui| {
                ui.label(RichText::new(t("closing.admin.password_card")).strong());
                form::password_secret(ui, &t("closing.admin.current"), &mut self.current);
                form::password_secret(ui, &t("closing.admin.new_password"), &mut self.changed);
                form::password_secret(ui, &t("closing.admin.confirm"), &mut self.changed_again);
                ui.add_enabled_ui(!busy, |ui| {
                    if form::actions(ui, &t("closing.admin.change_password"), &[]).is_some() {
                        act = Some(2);
                    }
                });
            });
        }
        ui.add_space(8.0);
        if ui
            .link(t("closing.admin.exceptions"))
            .on_hover_text(t("closing.admin.exceptions.tip"))
            .clicked()
        {
            act = Some(3);
        }
        if let Some((k, good)) = self.message {
            if good {
                form::inline_note(ui, &tokens, &t(k));
            } else {
                form::inline_warning(ui, &tokens, &t(k));
            }
        }
        if let Some(e) = &self.error {
            form::inline_error_details(ui, &tokens, e, "closing.admin");
        }
        match act {
            Some(0) => self.save_date(false),
            Some(1) => self.save_date(true),
            Some(2) => self.change_password(),
            Some(3) => self.exceptions(),
            _ => {}
        }
    }

    fn state(&self) -> Value {
        json!({
            "closed_through": self.status.as_ref().and_then(|s| s.closing_date).map(CalendarDate::to_wire),
            "password_set": self.password_set(),
            "message": self.message.map(|(k, _)| k),
            "busy": self.read.is_some() || self.write.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> (tempfile::TempDir, ClosingDateScreen) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("d");
        let s = ClosingDateScreen::new(link, today);
        (dir, s)
    }

    #[test]
    fn the_admin_moves_and_clears_the_date_with_the_password() {
        let (_d, mut s) = screen();
        assert_eq!(s.state()["password_set"], true);
        let start = s.state()["closed_through"].clone();
        s.date.set(CalendarDate::from_ymd(2026, 10, 31).expect("d"));
        // No password: refused before sending.
        s.save_date(false);
        assert_eq!(s.state()["message"], "closing.admin.password_needed");
        // Wrong password: the engine's refusal in plain words.
        s.password = SecretText::from("nope");
        s.save_date(false);
        assert_eq!(s.state()["message"], "closing.admin.wrong_password");
        assert_eq!(s.state()["closed_through"], start);
        assert!(s.password.is_empty(), "the password field is emptied");
        s.password = SecretText::from(crate::company::demo::register::DEMO_CLOSING_PASSWORD);
        s.save_date(false);
        assert_eq!(s.state()["message"], "closing.admin.saved");
        assert_eq!(s.state()["closed_through"], "2026-10-31");
        assert!(s.link.drain().contains(&CompanyEvent::ClosingDate(
            CalendarDate::from_ymd(2026, 10, 31).ok()
        )));
        s.password = SecretText::from(crate::company::demo::register::DEMO_CLOSING_PASSWORD);
        s.save_date(true);
        assert_eq!(s.state()["closed_through"], Value::Null);
        assert!(s.status_text().contains("open"));
    }

    #[test]
    fn the_password_changes_with_the_current_one() {
        let (_d, mut s) = screen();
        s.changed = SecretText::from("new-pass");
        s.changed_again = SecretText::from("new-pass");
        s.change_password();
        assert_eq!(s.state()["message"], "closing.admin.password_needed");
        s.current = SecretText::from(crate::company::demo::register::DEMO_CLOSING_PASSWORD);
        s.changed = SecretText::from("new-pass");
        s.changed_again = SecretText::from("new-pass");
        s.change_password();
        assert_eq!(s.state()["message"], "closing.admin.password_changed");
        assert!(
            s.link
                .demo_state()
                .is_some_and(|d| d.closing_password.as_deref() == Some("new-pass"))
        );
    }

    #[test]
    fn refusals_read_in_plain_words() {
        assert_eq!(
            refusal(&ApiError::Forbidden { code: None }),
            Some("closing.admin.only_admin")
        );
        assert_eq!(
            refusal(&ApiError::ClosingPasswordRejected),
            Some("closing.admin.wrong_password")
        );
        assert_eq!(refusal(&ApiError::NoCompanyOpen), None);
        // PR-8 S3: changed on another computer since the screen read it.
        assert_eq!(
            refusal(&ApiError::Conflict {
                reason: cl_api::ConflictReason::Stale,
                current_version: None,
                code: None,
                message: cl_api::UserText::default(),
                details: cl_api::ConflictDetails::default(),
            }),
            Some("closing.admin.changed_elsewhere")
        );
        for k in [
            "closing.admin.intro",
            "closing.admin.changed_elsewhere",
            "closing.admin.saved",
            "closing.admin.only_admin",
            "closing.admin.date_needed",
            "closing.admin.password_mismatch",
            "orphan.closing_date",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
    }
}
