//! The secrets the engine needs, kept with the other `ClientLedger` secrets.
//!
//! One random 256-bit master secret lives in the OS keychain (or the
//! encrypted-file fallback, see `cl_pg::default_store_for`). The engine's
//! application-role password, `JWT_SECRET`, `ENCRYPTION_KEY` and
//! `PLAID_ENCRYPTION_KEY` are derived from it with distinct labels, so they are
//! stable across restarts without any of them being written to disk, to the
//! engine's data folder, or to a process command line.

use crate::{HostError, Result};
use cl_pg::{PasswordStore, Secret};
use std::path::Path;

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
    /// Read the master secret from `store`.
    ///
    /// A new secret is created only when `brand_new` is true (the database
    /// cluster did not exist yet, so nothing was ever encrypted with the old
    /// one). For an existing cluster a missing secret is an error: silently
    /// making a new one would change `ENCRYPTION_KEY` and `JWT_SECRET` and make
    /// earlier data unreadable (security review M2). A non-secret fingerprint
    /// in `fingerprint_file` detects a store that now holds a different secret.
    ///
    /// # Errors
    /// [`HostError::MasterSecretMissing`] and [`HostError::MasterSecretMismatch`]
    /// as described, or a store failure.
    pub fn open(
        store: &dyn PasswordStore,
        data_dir: &Path,
        brand_new: bool,
        fingerprint_file: &Path,
    ) -> Result<EngineSecrets> {
        let master = if let Some(m) = store.get()? {
            m
        } else if brand_new {
            let m = Secret::generate()?;
            store.set(&m)?;
            tracing::info!(store = store.kind(), "created the engine master secret");
            m
        } else {
            return Err(HostError::MasterSecretMissing {
                data_dir: data_dir.to_path_buf(),
            });
        };
        let fingerprint = master.derive("fingerprint").expose()[..16].to_string();
        match std::fs::read_to_string(fingerprint_file) {
            Ok(saved) if saved.trim() != fingerprint => {
                return Err(HostError::MasterSecretMismatch {
                    data_dir: data_dir.to_path_buf(),
                });
            }
            Ok(_) => {}
            Err(_) => {
                // First run, or the marker was deleted: record it (best effort).
                let _ = std::fs::write(
                    fingerprint_file,
                    format!(
                        "{fingerprint}
"
                    ),
                );
            }
        }
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

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cl-host-sec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_new_cluster_gets_a_master_secret_and_later_runs_reuse_it() {
        let d = scratch("new");
        let (store, fp) = (MemoryStore::new(), d.join("master.fp"));
        let a = EngineSecrets::open(&store, &d, true, &fp).unwrap();
        let b = EngineSecrets::open(&store, &d, false, &fp).unwrap();
        assert_eq!(a.jwt_secret, b.jwt_secret);
        assert_eq!(a.app_db_password, b.app_db_password);
        assert!(fp.exists());
        let other = EngineSecrets::open(&MemoryStore::new(), &d, true, &d.join("o.fp")).unwrap();
        assert_ne!(a.jwt_secret, other.jwt_secret);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn an_existing_cluster_never_gets_a_new_secret() {
        let d = scratch("missing");
        let store = MemoryStore::new();
        let err = EngineSecrets::open(&store, &d, false, &d.join("master.fp")).unwrap_err();
        assert!(
            matches!(err, HostError::MasterSecretMissing { .. }),
            "{err}"
        );
        assert_eq!(store.get().unwrap(), None, "nothing was created");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_different_secret_in_the_store_is_detected() {
        let d = scratch("mismatch");
        let fp = d.join("master.fp");
        EngineSecrets::open(&MemoryStore::new(), &d, true, &fp).unwrap();
        let swapped = MemoryStore::new();
        swapped.set(&Secret::new("someone else")).unwrap();
        let err = EngineSecrets::open(&swapped, &d, false, &fp).unwrap_err();
        assert!(
            matches!(err, HostError::MasterSecretMismatch { .. }),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_fingerprint_file_does_not_reveal_the_secret() {
        let d = scratch("fp");
        let store = MemoryStore::new();
        let fp = d.join("master.fp");
        let s = EngineSecrets::open(&store, &d, true, &fp).unwrap();
        let text = std::fs::read_to_string(&fp).unwrap();
        assert_eq!(text.trim().len(), 16);
        for v in s.all() {
            assert!(!v.expose().contains(text.trim()));
        }
        assert!(!text.contains(store.get().unwrap().unwrap().expose()));
        let _ = std::fs::remove_dir_all(d);
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
