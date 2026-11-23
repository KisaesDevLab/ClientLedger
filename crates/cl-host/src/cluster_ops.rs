//! Whole-database (cluster) backup and restore with the supervisor stopped
//! (P34 A4, Q-P34l): a cold copy through `cl-pg`, then the engine and the
//! database are started again. **Every path returns a restarted
//! [`Supervisor`]**, success or failure, so the app never loses its engine.
//!
//! After a restore the engine starts on the restored database; the
//! supervisor clears any maintenance hold the copy carried at that start
//! (P33 follow-up 8), and every LAN workstation sees the host go away and
//! come back.
//!
//! Errors are typed for the owner's choices: [`ClusterOpError::Unregistered`]
//! (this install did not write the file: ask "I trust this file", then call
//! again with `allow_unregistered`), [`ClusterOpError::NoSpace`] and
//! [`ClusterOpError::Damaged`]; anything else is [`ClusterOpError::Failed`].

use std::path::Path;
use std::sync::Arc;

use cl_pg::{Cluster, DumpInfo, PasswordStore, PgBinaries, PgError, RestoreInfo};

use crate::{HostConfig, HostError, Supervisor};

/// Why a whole-database backup or restore did not happen.
#[derive(Debug, thiserror::Error)]
pub enum ClusterOpError {
    /// The backup was not written by this install (Q-P03h); nothing changed.
    #[error("this backup file was not made on this computer")]
    Unregistered,
    /// Not enough free disk space; nothing changed.
    #[error("there is not enough free disk space")]
    NoSpace,
    /// The file is damaged or not a backup for this version; nothing changed.
    #[error("the backup file is damaged or does not fit this version: {0}")]
    Damaged(String),
    /// Anything else (the data folder, the keychain, stopping the server).
    #[error(transparent)]
    Failed(#[from] HostError),
}

impl From<PgError> for ClusterOpError {
    fn from(e: PgError) -> Self {
        match e {
            e if e.is_unregistered() => ClusterOpError::Unregistered,
            PgError::RestoreTooLarge { .. } => ClusterOpError::NoSpace,
            PgError::Io { ref source, .. } if is_disk_full(source) => ClusterOpError::NoSpace,
            PgError::Backup(m) => ClusterOpError::Damaged(m),
            other => ClusterOpError::Failed(HostError::from(other)),
        }
    }
}

fn is_disk_full(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::StorageFull
        || matches!(e.raw_os_error(), Some(code) if (cfg!(windows) && (code == 112 || code == 39)) || (cfg!(unix) && code == 28))
}

/// The cluster and its password store, as the supervisor would open them.
fn open_cluster(config: &HostConfig) -> Result<(Cluster, Arc<dyn PasswordStore>), ClusterOpError> {
    let data = config.paths.pg_data();
    let bins = match &config.pg_binaries {
        Some(b) => b.clone(),
        None => PgBinaries::discover(None).map_err(ClusterOpError::from)?,
    };
    let store: Arc<dyn PasswordStore> = match &config.pg_password_store {
        Some(s) => Arc::clone(s),
        None => Arc::from(cl_pg::default_store(&data).store),
    };
    let cluster = Cluster::ensure(&data, bins, store.as_ref())?;
    Ok((cluster, store))
}

fn restart(config: HostConfig, guard: crate::InstanceGuard) -> Supervisor {
    Supervisor::spawn(config, guard)
}

/// Back up the whole database to `dest` (a `.tar.gz` outside the data
/// folder): stop the supervisor (engine, then database), copy, verify,
/// record the file as this install's, start again. The returned supervisor
/// is starting; wait for it as at launch.
pub fn full_backup(
    sup: Supervisor,
    config: HostConfig,
    dest: &Path,
) -> (Supervisor, Result<DumpInfo, ClusterOpError>) {
    let guard = sup.stop_keeping_guard();
    let result = open_cluster(&config)
        .and_then(|(cluster, _)| cluster.dump(dest).map_err(ClusterOpError::from));
    (restart(config, guard), result)
}

/// Replace the whole database with the backup at `src`. The archive is
/// unpacked and checked first (format, platform, version, size limit);
/// the replaced data folder is kept beside the new one
/// ([`RestoreInfo::previous`]). `allow_unregistered` only after the owner
/// confirmed a file this install did not write. The returned supervisor
/// starts on the restored database (or the old one when nothing changed).
pub fn full_restore(
    sup: Supervisor,
    config: HostConfig,
    src: &Path,
    allow_unregistered: bool,
) -> (Supervisor, Result<RestoreInfo, ClusterOpError>) {
    let guard = sup.stop_keeping_guard();
    let result = open_cluster(&config).and_then(|(mut cluster, store)| {
        cluster
            .restore(src, store.as_ref(), allow_unregistered)
            .map_err(ClusterOpError::from)
    });
    (restart(config, guard), result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_errors_map_to_the_owner_choices() {
        let unreg = PgError::Backup("this backup file was not made by this copy of ClientLedger, so it cannot be restored without confirmation".into());
        assert!(matches!(
            ClusterOpError::from(unreg),
            ClusterOpError::Unregistered
        ));
        assert!(matches!(
            ClusterOpError::from(PgError::RestoreTooLarge { limit: 1 }),
            ClusterOpError::NoSpace
        ));
        assert!(matches!(
            ClusterOpError::from(PgError::Backup("damaged".into())),
            ClusterOpError::Damaged(_)
        ));
        assert!(matches!(
            ClusterOpError::from(PgError::StuckProcess { pid: 1 }),
            ClusterOpError::Failed(_)
        ));
        let full = std::io::Error::from(std::io::ErrorKind::StorageFull);
        assert!(is_disk_full(&full));
        assert!(!is_disk_full(&std::io::Error::other("x")));
    }
}
