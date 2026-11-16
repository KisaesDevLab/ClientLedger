//! Automatic sign-in (Q-20, Q-P29o). Off by default.
//!
//! - The password is kept only in the operating-system keychain, under an
//!   entry tied to the app data folder ([`crate::KeyringSecrets::for_root`]);
//!   never in a file, a log or a backup. A memory or file secret store is
//!   never used: [`AutoSignIn::available`] says no.
//! - The email (not a secret) sits in `auto-sign-in.json` in the same
//!   folder, so the app knows the feature is on without reading the
//!   keychain.
//! - It is only for a workspace with one active user. The app clears it
//!   when the password changes, after a recovery or a temporary password,
//!   and when a second user appears; Sign out skips it for that run.

use std::path::{Path, PathBuf};

use cl_core::Secret;
use serde::{Deserialize, Serialize};

use crate::secrets::{SecretError, SecretStore};
use crate::store::{read_capped, write_atomic};

/// Keychain entry name of the saved password.
pub const AUTO_SIGN_IN_SECRET: &str = "auto-sign-in";

/// Name of the file that says automatic sign-in is on (email only).
pub const AUTO_SIGN_IN_FILE: &str = "auto-sign-in.json";

/// The keychain kind automatic sign-in needs ([`SecretStore::kind`]).
const OS_KEYCHAIN: &str = "os-keychain";

/// Entry written and removed once to check the keychain works.
const PROBE: &str = "auto-sign-in-probe";

/// Why automatic sign-in could not be turned on.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AutoSignInError {
    /// This computer has no usable keychain.
    #[error("no keychain on this computer")]
    NoKeychain,
    /// The keychain refused (message from the OS, never the value).
    #[error(transparent)]
    Keychain(#[from] SecretError),
    /// The settings file could not be written.
    #[error("the automatic sign-in file could not be written")]
    File,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    email: String,
}

/// Automatic sign-in for one app data folder.
#[derive(Debug, Clone)]
pub struct AutoSignIn {
    path: PathBuf,
}

impl AutoSignIn {
    /// The setting in the app data folder `dir`.
    #[must_use]
    pub fn in_dir(dir: &Path) -> AutoSignIn {
        AutoSignIn {
            path: dir.join(AUTO_SIGN_IN_FILE),
        }
    }

    /// True when `store` is the operating-system keychain and a probe
    /// entry can be written, read back and removed. Never offered
    /// otherwise.
    #[must_use]
    pub fn available(store: &dyn SecretStore) -> bool {
        if store.kind() != OS_KEYCHAIN {
            return false;
        }
        let probe = Secret::new("probe");
        let ok = store.set(PROBE, &probe).is_ok()
            && store
                .get(PROBE)
                .ok()
                .flatten()
                .is_some_and(|s| s.expose() == "probe");
        let _ = store.delete(PROBE);
        ok
    }

    /// The email automatic sign-in uses, when it is on.
    #[must_use]
    pub fn email(&self) -> Option<String> {
        let text = read_capped(&self.path).ok().flatten()?;
        let saved: Saved = serde_json::from_str(&text).ok()?;
        let email = saved.email.trim().to_string();
        (!email.is_empty()).then_some(email)
    }

    /// True when automatic sign-in is on.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.email().is_some()
    }

    /// The saved email and password, when it is on and the keychain still
    /// has the password.
    #[must_use]
    pub fn credentials(&self, store: &dyn SecretStore) -> Option<(String, Secret)> {
        if store.kind() != OS_KEYCHAIN {
            return None;
        }
        let email = self.email()?;
        let password = store
            .get(AUTO_SIGN_IN_SECRET)
            .ok()
            .flatten()
            .filter(|p| !p.is_empty())?;
        Some((email, password))
    }

    /// Turns automatic sign-in on with `email` and `password`.
    ///
    /// # Errors
    /// `NoKeychain` when `store` is not the operating-system keychain;
    /// `Keychain`; `File`. On any error nothing stays saved.
    pub fn turn_on(
        &self,
        store: &dyn SecretStore,
        email: &str,
        password: &Secret,
    ) -> Result<(), AutoSignInError> {
        if store.kind() != OS_KEYCHAIN {
            return Err(AutoSignInError::NoKeychain);
        }
        store.set(AUTO_SIGN_IN_SECRET, password)?;
        let text = serde_json::to_string(&Saved {
            email: email.trim().to_string(),
        })
        .map_err(|_| AutoSignInError::File)?;
        if write_atomic(&self.path, &text).is_err() {
            let _ = store.delete(AUTO_SIGN_IN_SECRET);
            return Err(AutoSignInError::File);
        }
        Ok(())
    }

    /// Turns automatic sign-in off: the keychain entry and the file are
    /// removed. Safe to call when it is off.
    ///
    /// # Errors
    /// `Keychain` when the entry could not be removed (the file is removed
    /// anyway, so the app stops using it).
    pub fn clear(&self, store: &dyn SecretStore) -> Result<(), AutoSignInError> {
        let _ = std::fs::remove_file(&self.path);
        store.delete(AUTO_SIGN_IN_SECRET)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemorySecrets;

    /// A keychain stand-in that says it is the OS keychain.
    struct FakeKeychain(MemorySecrets);

    impl SecretStore for FakeKeychain {
        fn get(&self, name: &str) -> Result<Option<Secret>, SecretError> {
            self.0.get(name)
        }
        fn set(&self, name: &str, value: &Secret) -> Result<(), SecretError> {
            self.0.set(name, value)
        }
        fn delete(&self, name: &str) -> Result<(), SecretError> {
            self.0.delete(name)
        }
        fn kind(&self) -> &'static str {
            OS_KEYCHAIN
        }
    }

    #[test]
    fn never_offered_without_the_os_keychain() {
        let dir = tempfile::tempdir().expect("tmp");
        let memory = MemorySecrets::new();
        assert!(!AutoSignIn::available(&memory));
        let a = AutoSignIn::in_dir(dir.path());
        assert_eq!(
            a.turn_on(&memory, "pat@example.test", &Secret::new("pw-123456789012")),
            Err(AutoSignInError::NoKeychain)
        );
        assert!(!a.is_on());
        assert!(!dir.path().join(AUTO_SIGN_IN_FILE).exists());
    }

    #[test]
    fn on_keeps_the_password_only_in_the_keychain_and_clear_removes_both() {
        let dir = tempfile::tempdir().expect("tmp");
        let kc = FakeKeychain(MemorySecrets::new());
        assert!(AutoSignIn::available(&kc));
        assert!(kc.get(PROBE).expect("get").is_none(), "probe removed");
        let a = AutoSignIn::in_dir(dir.path());
        assert!(!a.is_on());
        a.turn_on(&kc, " pat@example.test ", &Secret::new("CANARY-password-1"))
            .expect("on");
        let file = std::fs::read_to_string(dir.path().join(AUTO_SIGN_IN_FILE)).expect("file");
        assert!(!file.contains("CANARY"), "no password in the file");
        let (email, pw) = a.credentials(&kc).expect("saved");
        assert_eq!(email, "pat@example.test");
        assert_eq!(pw.expose(), "CANARY-password-1");
        a.clear(&kc).expect("off");
        assert!(!a.is_on());
        assert!(a.credentials(&kc).is_none());
        assert!(kc.get(AUTO_SIGN_IN_SECRET).expect("get").is_none());
        a.clear(&kc).expect("clearing twice is fine");
    }

    #[test]
    fn a_missing_keychain_entry_means_off() {
        let dir = tempfile::tempdir().expect("tmp");
        let kc = FakeKeychain(MemorySecrets::new());
        let a = AutoSignIn::in_dir(dir.path());
        a.turn_on(&kc, "pat@example.test", &Secret::new("pw-123456789012"))
            .expect("on");
        kc.delete(AUTO_SIGN_IN_SECRET).expect("gone");
        assert!(a.credentials(&kc).is_none());
        assert!(a.credentials(&MemorySecrets::new()).is_none());
    }
}
