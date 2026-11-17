//! Company › My Sign-in (P29 WP2): the signed-in user changes their own
//! password, turns automatic sign-in on or off (Q-P29o) and picks the idle
//! lock time for this computer (Q-P29p).
//!
//! - Automatic sign-in is offered only when the OS keychain works
//!   (`AutoSignIn::available`) and the workspace has exactly one active
//!   user; turning it on asks for the password and proves it by signing in
//!   once more (the idle unlock's check) before it is kept in the keychain.
//! - A password change turns automatic sign-in off (the saved password no
//!   longer works).
//! - The idle time goes to the shell as `CompanyEvent::IdleLockMinutes`,
//!   which changes only that field of the shell's own preferences.
//!
//! Passwords live in [`SecretText`] fields, go to the engine as `Secret`s
//! and are emptied right after sending; nothing here logs them.

use std::sync::Arc;

use cl_api::{ApiError, OrphanLabel, RequestGroup};
use cl_core::Secret;
use cl_model::CommandId;
use eframe::egui::{RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::i18n::t;
use crate::screens::company::form::{self, SecretText};
use crate::screens::invoice::engine::{self, Job};
use crate::screens::login::{new_password_problem, sign_in_message};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The window kind.
pub const KIND: &str = "company.my_sign_in";

/// The window for `kind`, if it is this one. `idle_minutes` is the shell's
/// current idle-lock time.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    idle_minutes: u16,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(MySignIn::new(Arc::clone(link), idle_minutes)) as _)
}

/// Whether automatic sign-in can be offered (Q-P29o).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoOffer {
    /// The switch shows.
    Offered,
    /// No working OS keychain on this computer.
    NoKeychain,
    /// More than one person can sign in.
    Team,
    /// The team could not be read (no company open, still reading).
    Unknown,
}

/// The rule: a working keychain and exactly one active user.
#[must_use]
pub fn auto_offer(keychain: bool, active_users: Option<usize>) -> AutoOffer {
    match (keychain, active_users) {
        (false, _) => AutoOffer::NoKeychain,
        (true, Some(1)) => AutoOffer::Offered,
        (true, Some(_)) => AutoOffer::Team,
        (true, None) => AutoOffer::Unknown,
    }
}

impl AutoOffer {
    fn key(self) -> &'static str {
        match self {
            AutoOffer::Offered => "mysignin.auto.offered",
            AutoOffer::NoKeychain => "mysignin.auto.no_keychain",
            AutoOffer::Team => "mysignin.auto.team",
            AutoOffer::Unknown => "mysignin.auto.unknown",
        }
    }
}

/// The screen.
pub struct MySignIn {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    current: SecretText,
    new_password: SecretText,
    confirm: SecretText,
    auto_password: SecretText,
    idle: u16,
    keychain: bool,
    auto_on: bool,
    active_users: Option<usize>,
    users: Option<Job<usize>>,
    change: Option<Job<()>>,
    proof: Option<Task<Result<(), ApiError>>>,
    /// The password being proven for automatic sign-in (wiped after).
    proving: Option<Secret>,
    /// i18n key of the last message, and whether it is good news.
    message: Option<(&'static str, bool)>,
}

impl std::fmt::Debug for MySignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MySignIn")
            .field("idle", &self.idle)
            .field("message", &self.message)
            .finish_non_exhaustive()
    }
}

fn label() -> OrphanLabel {
    OrphanLabel {
        what: "orphan.my_sign_in",
        key: None,
    }
}

impl MySignIn {
    /// Reads the team size (for automatic sign-in).
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, idle: u16) -> MySignIn {
        let group = link.group().and_then(Result::ok);
        let keychain =
            !link.is_demo() && cl_settings::AutoSignIn::available(link.secrets().as_ref());
        let auto_on = link.auto_sign_in().is_on();
        let mut s = MySignIn {
            group,
            current: SecretText::default(),
            new_password: SecretText::default(),
            confirm: SecretText::default(),
            auto_password: SecretText::default(),
            idle,
            keychain,
            auto_on,
            active_users: None,
            users: None,
            change: None,
            proof: None,
            proving: None,
            message: None,
            link,
        };
        s.users = Some(engine::read(
            &s.link,
            s.group.as_ref(),
            |d| Ok(d.auth.active_users()),
            |s| async move {
                let users = s.team().users().await?;
                Ok(users.iter().filter(|u| u.active).count())
            },
        ));
        s.poll();
        s
    }

    fn offer(&self) -> AutoOffer {
        auto_offer(self.keychain, self.active_users)
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.users) {
            self.active_users = r.ok();
            // Q-P29-D2: a second active user ends automatic sign-in.
            if self.active_users.is_some_and(|n| n > 1) && self.auto_on {
                self.link.clear_auto_sign_in();
                self.auto_on = false;
            }
        }
        if let Some(r) = engine::poll(&mut self.change) {
            match r {
                Ok(()) => {
                    // The saved password no longer works (Q-P29o).
                    self.link.clear_auto_sign_in();
                    self.auto_on = false;
                    self.message = Some(("mysignin.changed", true));
                    tracing::info!("own password changed");
                }
                Err(e) => self.message = Some((sign_in_message(&e), false)),
            }
        }
        if let Some(task) = self.proof.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.proof = None;
            let password = self.proving.take();
            match (r, password, self.link.signed_in_email()) {
                (Ok(()), Some(p), Some(email)) => self.keep_auto(&email, &p),
                (Err(e), _, _) => self.message = Some((sign_in_message(&e), false)),
                _ => self.message = Some(("login.msg.failed", false)),
            }
        }
    }

    fn keep_auto(&mut self, email: &str, password: &Secret) {
        match self
            .link
            .auto_sign_in()
            .turn_on(self.link.secrets().as_ref(), email, password)
        {
            Ok(()) => {
                self.auto_on = true;
                self.message = Some(("mysignin.auto.turned_on", true));
                tracing::info!("automatic sign-in turned on");
            }
            Err(e) => {
                tracing::warn!(error = %e, "automatic sign-in could not be kept in the keychain");
                self.message = Some(("mysignin.auto.failed", false));
            }
        }
    }

    fn change_password(&mut self) {
        if self.current.is_empty() {
            self.message = Some(("mysignin.current_needed", false));
            return;
        }
        if let Some(k) = new_password_problem(self.new_password.text(), self.confirm.text()) {
            self.message = Some((k, false));
            return;
        }
        let current = self.current.to_secret();
        let new = self.new_password.to_secret();
        self.current.clear();
        self.new_password.clear();
        self.confirm.clear();
        self.message = None;
        let (c2, n2) = (current.clone(), new.clone());
        self.change = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label(),
            move |d| d.auth.change_password(&c2, &n2),
            move |s| async move { s.change_password(&current, &new).await },
        ));
        self.poll();
    }

    fn turn_auto_on(&mut self) {
        if self.offer() != AutoOffer::Offered {
            return;
        }
        if self.auto_password.is_empty() {
            self.message = Some(("mysignin.auto.password_needed", false));
            return;
        }
        let password = self.auto_password.to_secret();
        self.auto_password.clear();
        let Some(email) = self.link.signed_in_email() else {
            self.message = Some(("login.msg.failed", false));
            return;
        };
        self.message = None;
        let demo = self
            .link
            .demo_state()
            .map(|mut d| d.auth.check(&email, &password).map(drop));
        self.proving = Some(password.clone());
        if let Some(r) = demo {
            self.proof = Some(Task::ready(r));
        } else {
            let (Some(group), Some(client), Some(own)) = (
                self.group.as_ref(),
                self.link.client().cloned(),
                self.link.session(),
            ) else {
                self.proving = None;
                self.message = Some(("login.msg.failed", false));
                return;
            };
            self.proof = spawn_lifecycle(&self.link, group, move || async move {
                crate::shell::idle_lock::proof_of_password(&client, &own, &email, &password).await
            })
            .ok();
        }
        self.poll();
    }

    fn turn_auto_off(&mut self) {
        self.link.clear_auto_sign_in();
        self.auto_on = false;
        self.message = Some(("mysignin.auto.turned_off", true));
        tracing::info!("automatic sign-in turned off");
    }

    fn set_idle(&mut self, minutes: u16) {
        if minutes == self.idle {
            return;
        }
        self.idle = minutes;
        self.link.push(CompanyEvent::IdleLockMinutes(minutes));
        self.message = Some(("mysignin.idle.saved", true));
    }

    fn busy(&self) -> bool {
        self.change.is_some() || self.proof.is_some()
    }

    fn password_card(&mut self, ui: &mut Ui, tokens: &Tokens) -> bool {
        let mut go = false;
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("mysignin.password_card")).strong());
            form::password_secret(ui, &t("mysignin.current"), &mut self.current);
            form::password_secret(ui, &t("login.change.new"), &mut self.new_password);
            form::password_secret(ui, &t("login.change.confirm"), &mut self.confirm);
            form::hint(ui, tokens, &t("login.rule"));
            ui.add_enabled_ui(!self.busy(), |ui| {
                go = form::actions(ui, &t("mysignin.change"), &[]).is_some();
            });
        });
        go
    }

    fn auto_card(&mut self, ui: &mut Ui, tokens: &Tokens) -> Option<bool> {
        let mut act = None;
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("mysignin.auto_card")).strong());
            form::hint(ui, tokens, &t("mysignin.auto.intro"));
            let offer = self.offer();
            if self.auto_on {
                form::inline_note(ui, tokens, &t("mysignin.auto.is_on"));
                ui.add_enabled_ui(!self.busy(), |ui| {
                    if ui.button(t("mysignin.auto.off")).clicked() {
                        act = Some(false);
                    }
                });
            } else if offer == AutoOffer::Offered {
                form::password_secret(ui, &t("mysignin.auto.password"), &mut self.auto_password);
                ui.add_enabled_ui(!self.busy(), |ui| {
                    if ui.button(t("mysignin.auto.on")).clicked() {
                        act = Some(true);
                    }
                });
            } else {
                form::hint(ui, tokens, &t(offer.key()));
            }
        });
        act
    }

    fn idle_card(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mut chosen = self.idle;
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("mysignin.idle_card")).strong());
            form::hint(ui, tokens, &t("mysignin.idle.intro"));
            ui.horizontal_wrapped(|ui| {
                for m in cl_settings::IDLE_LOCK_STEPS {
                    let text = if m == 0 {
                        t("mysignin.idle.off")
                    } else {
                        crate::i18n::t_with("mysignin.idle.minutes", &[("n", &m.to_string())])
                    };
                    ui.radio_value(&mut chosen, m, text);
                }
            });
        });
        self.set_idle(chosen);
    }
}

impl Screen for MySignIn {
    fn title(&self) -> String {
        t("cmd.company.my_sign_in")
    }

    fn dirty(&self) -> bool {
        !(self.current.is_empty() && self.new_password.is_empty() && self.auto_password.is_empty())
    }

    fn can_save(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        let tokens = Tokens::of(ui.ctx());
        if self.busy() || self.users.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        form::heading(ui, &tokens, &t("cmd.company.my_sign_in"));
        if let Some(email) = self.link.signed_in_email() {
            form::hint(
                ui,
                &tokens,
                &crate::i18n::t_with("mysignin.who", &[("email", &email)]),
            );
        }
        ui.add_space(6.0);
        let change = self.password_card(ui, &tokens);
        ui.add_space(8.0);
        let auto = self.auto_card(ui, &tokens);
        ui.add_space(8.0);
        self.idle_card(ui, &tokens);
        if let Some((k, good)) = self.message {
            ui.add_space(6.0);
            if good {
                form::inline_note(ui, &tokens, &t(k));
            } else {
                form::inline_warning(ui, &tokens, &t(k));
            }
        }
        if self.busy() {
            form::progress_line(ui, &tokens, &t("login.working"));
        }
        if change {
            self.change_password();
        }
        match auto {
            Some(true) => self.turn_auto_on(),
            Some(false) => self.turn_auto_off(),
            None => {}
        }
    }

    fn state(&self) -> Value {
        json!({
            "idle_minutes": self.idle,
            "auto_on": self.auto_on,
            "auto_offer": self.offer().key(),
            "active_users": self.active_users,
            "message": self.message.map(|(k, _)| k),
            "busy": self.busy(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::company::demo::auth::OWNER_PASSWORD;

    fn screen() -> (tempfile::TempDir, MySignIn) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let s = MySignIn::new(link, 0);
        (dir, s)
    }

    #[test]
    fn automatic_sign_in_needs_a_keychain_and_one_user() {
        assert_eq!(auto_offer(true, Some(1)), AutoOffer::Offered);
        assert_eq!(auto_offer(false, Some(1)), AutoOffer::NoKeychain);
        assert_eq!(auto_offer(true, Some(2)), AutoOffer::Team);
        assert_eq!(auto_offer(true, None), AutoOffer::Unknown);
        for o in [
            AutoOffer::Offered,
            AutoOffer::NoKeychain,
            AutoOffer::Team,
            AutoOffer::Unknown,
        ] {
            assert!(crate::i18n::has(o.key()), "{}", o.key());
        }
        // The demo has no OS keychain: never offered there.
        let (_d, s) = screen();
        assert_eq!(s.offer(), AutoOffer::NoKeychain);
        assert_eq!(s.state()["active_users"], 3);
    }

    #[test]
    fn the_own_password_changes_with_the_current_one() {
        let (_d, mut s) = screen();
        s.new_password = SecretText::from("a-brand-new-password");
        s.confirm = SecretText::from("a-brand-new-password");
        s.change_password();
        assert_eq!(s.state()["message"], "mysignin.current_needed");
        s.current = SecretText::from("not-the-password");
        s.new_password = SecretText::from("a-brand-new-password");
        s.confirm = SecretText::from("a-brand-new-password");
        s.change_password();
        assert_eq!(s.state()["message"], "login.msg.current_wrong");
        assert!(s.current.is_empty() && s.new_password.is_empty(), "wiped");
        s.current = SecretText::from(OWNER_PASSWORD);
        s.new_password = SecretText::from("short");
        s.confirm = SecretText::from("short");
        s.change_password();
        assert_eq!(s.state()["message"], "login.msg.policy");
        s.current = SecretText::from(OWNER_PASSWORD);
        s.new_password = SecretText::from("a-brand-new-password");
        s.confirm = SecretText::from("a-brand-new-password");
        s.change_password();
        assert_eq!(s.state()["message"], "mysignin.changed");
        let mut d = s.link.demo_state().expect("demo");
        assert!(
            d.auth
                .check(
                    crate::company::demo::auth::OWNER_EMAIL,
                    &Secret::new("a-brand-new-password")
                )
                .is_ok()
        );
    }

    #[test]
    fn the_idle_time_goes_to_the_shell_alone() {
        let (_d, mut s) = screen();
        s.set_idle(15);
        assert_eq!(s.state()["idle_minutes"], 15);
        assert!(s.link.drain().contains(&CompanyEvent::IdleLockMinutes(15)));
        s.set_idle(15);
        assert!(s.link.drain().is_empty(), "no event without a change");
        for k in [
            "mysignin.idle.saved",
            "mysignin.auto.turned_on",
            "mysignin.auto.turned_off",
            "mysignin.auto.failed",
            "mysignin.auto.password_needed",
            "orphan.my_sign_in",
            "cmd.company.my_sign_in",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
    }
}
