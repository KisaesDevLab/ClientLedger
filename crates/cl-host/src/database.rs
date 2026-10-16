//! The database as the supervisor sees it.
//!
//! [`DatabaseService`] is the small surface the supervisor needs, so its
//! restart logic can be tested with a fake. [`PgService`] is the real thing:
//! the bundled `PostgreSQL` through `cl-pg`, plus the engine's own
//! non-superuser database role (Q-P04a).

use crate::secrets::EngineSecrets;
use crate::{AppPaths, HostError, Result};
use cl_pg::{Cluster, PasswordStore, PgBinaries, Secret};
use std::path::PathBuf;
use std::sync::Arc;

/// Name of the role the engine connects as.
pub const APP_ROLE: &str = "clientledger_app";
/// Name of the database that role owns.
pub const APP_DATABASE: &str = "clientledger";

/// Where the engine should connect.
#[derive(Debug, Clone)]
pub struct DbEndpoint {
    /// Loopback port of the server.
    pub port: u16,
    /// Process id of the server, or 0 when unknown.
    pub pid: u32,
    /// `DATABASE_URL` for the engine (contains the role password).
    pub url: Secret,
}

/// A database the supervisor can start, watch, and stop.
pub trait DatabaseService: Send {
    /// Start (or adopt) the server and make sure the engine's database and
    /// role exist.
    ///
    /// # Errors
    /// Fails when the server cannot be started or prepared.
    fn start(&mut self) -> Result<DbEndpoint>;

    /// True while the server answers.
    fn is_alive(&mut self, endpoint: &DbEndpoint) -> bool;

    /// Stop the server and wait for it.
    ///
    /// # Errors
    /// Fails when the server cannot be stopped.
    fn stop(&mut self) -> Result<()>;

    /// Log files to mention in a fault report.
    fn log_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// What a [`DatabaseFactory`] returns: the service and any notices for the owner.
pub type DatabaseParts = (Box<dyn DatabaseService>, Vec<String>);

/// Builds the database service on the supervisor's thread (creating a new
/// cluster takes a few seconds, so it must not run on the UI thread).
pub type DatabaseFactory = Box<dyn FnOnce(&EngineSecrets) -> Result<DatabaseParts> + Send>;

/// The bundled `PostgreSQL` through `cl-pg`.
pub struct PgService {
    cluster: Cluster,
    app_password: Secret,
    startup_log: PathBuf,
    data_log_dir: PathBuf,
}

impl std::fmt::Debug for PgService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgService")
            .field("data_dir", &self.cluster.data_dir())
            .finish_non_exhaustive()
    }
}

impl PgService {
    /// Open (creating on first use) the cluster for `paths`.
    ///
    /// `pg_store` holds the superuser password; `None` selects the OS keychain
    /// with the encrypted-file fallback.
    ///
    /// # Errors
    /// Fails when the binaries are missing or the cluster cannot be opened.
    pub fn open(
        paths: &AppPaths,
        bins: Option<PgBinaries>,
        pg_store: Option<Arc<dyn PasswordStore>>,
        app_password: Secret,
    ) -> Result<(PgService, Vec<String>)> {
        let bins = match bins {
            Some(b) => b,
            None => PgBinaries::discover(None)?,
        };
        let data_dir = paths.pg_data();
        let mut notices = Vec::new();
        let store: Arc<dyn PasswordStore> = if let Some(s) = pg_store {
            s
        } else {
            let choice = cl_pg::default_store(&data_dir);
            if let Some(n) = &choice.notice {
                notices.push(n.to_string());
            }
            Arc::from(choice.store)
        };
        let cluster = Cluster::ensure(&data_dir, bins, store.as_ref())?;
        let mut startup_log = data_dir.clone().into_os_string();
        startup_log.push(".startup.log");
        Ok((
            PgService {
                data_log_dir: data_dir.join("log"),
                startup_log: PathBuf::from(startup_log),
                cluster,
                app_password,
            },
            notices,
        ))
    }
}

impl DatabaseService for PgService {
    fn start(&mut self) -> Result<DbEndpoint> {
        let running = self.cluster.start()?;
        self.cluster.ensure_app_database(
            running.port,
            APP_ROLE,
            APP_DATABASE,
            &self.app_password,
        )?;
        Ok(DbEndpoint {
            port: running.port,
            pid: running.pid,
            url: Cluster::app_url(running.port, APP_ROLE, APP_DATABASE, &self.app_password),
        })
    }

    fn is_alive(&mut self, endpoint: &DbEndpoint) -> bool {
        cl_pg::ports::is_ready(endpoint.port)
    }

    fn stop(&mut self) -> Result<()> {
        self.cluster.stop().map_err(HostError::from)
    }

    fn log_paths(&self) -> Vec<PathBuf> {
        vec![self.startup_log.clone(), self.data_log_dir.clone()]
    }
}
