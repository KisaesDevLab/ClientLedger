//! The secrets the engine needs, kept with the other `ClientLedger` secrets.
//!
//! One random 256-bit master secret lives in the OS keychain (or the
//! encrypted-file fallback, see `cl_pg::default_store_for`). The engine's
//! application-role password, `JWT_SECRET`, `ENCRYPTION_KEY` and
//! `PLAID_ENCRYPTION_KEY` are derived from it with distinct labels, so they are
//! stable across restarts without any of them being written to disk, to the
//! engine's data folder, or to a process command line.

use crate::Result;
use cl_pg::{PasswordStore, Secret};

/// Keychain purpose name for the master secret.
pub(crate) const MASTER_PURPOSE: &str = "host-master";

/// Everything the engine process is given besides the launch token.
#[derive(Debug, Clone)]
pub struct EngineSecrets {
    /// Password of the engine's database role.
    pub app_db_password: Secret,
    /// `JWT_SECRET`.
    pub jwt_secret: Secret,
    /// `ENCRYPTION_KEY` (64 hex characters).
    pub encryption_key: Secret,
    /// `PLAID_ENCRYPTION_KEY` (64 hex characters).
    pub plaid_encryption_key: Secret,
}

impl EngineSecrets {
    /// Read the master secret from `store`, creating and saving one first if
    /// there is none.
    ///
    /// # Errors
    /// Fails when the store cannot be read or written.
    pub fn load_or_create(store: &dyn PasswordStore) -> Result<EngineSecrets> {
        let master = if let Some(m) = store.get()? {
            m
        } else {
            let m = Secret::generate()?;
            store.set(&m)?;
            tracing::info!(store = store.kind(), "created the engine master secret");
            m
        };
        Ok(Self::from_master(&master))
    }

    /// Derive all engine secrets from `master`.
    #[must_use]
    pub fn from_master(master: &Secret) -> EngineSecrets {
        EngineSecrets {
            app_db_password: master.derive("engine-db-role-password"),
            jwt_secret: master.derive("engine-jwt-secret"),
            encryption_key: master.derive("engine-encryption-key"),
            plaid_encryption_key: master.derive("engine-plaid-encryption-key"),
        }
    }

    /// Every value that must never appear in a log or error message.
    pub(crate) fn all(&self) -> [&Secret; 4] {
        [
            &self.app_db_password,
            &self.jwt_secret,
            &self.encryption_key,
            &self.plaid_encryption_key,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_pg::MemoryStore;

    #[test]
    fn secrets_are_created_once_and_then_stable() {
        let store = MemoryStore::new();
        let a = EngineSecrets::load_or_create(&store).unwrap();
        let b = EngineSecrets::load_or_create(&store).unwrap();
        assert_eq!(a.jwt_secret, b.jwt_secret);
        assert_eq!(a.app_db_password, b.app_db_password);
        let other = EngineSecrets::load_or_create(&MemoryStore::new()).unwrap();
        assert_ne!(a.jwt_secret, other.jwt_secret);
    }

    #[test]
    fn the_derived_secrets_differ_and_are_the_right_shape() {
        let s = EngineSecrets::from_master(&Secret::new("m"));
        let all = s.all();
        for (i, a) in all.iter().enumerate() {
            assert_eq!(a.expose().len(), 64);
            assert!(a.expose().bytes().all(|c| c.is_ascii_hexdigit()));
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn debug_output_shows_no_secret() {
        let s = EngineSecrets::from_master(&Secret::new("m"));
        let text = format!("{s:?}");
        for v in s.all() {
            assert!(!text.contains(v.expose()));
        }
    }
}
