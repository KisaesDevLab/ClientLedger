//! Test-only SQL (P34, feature `test-support`): live tests seed faults the
//! integrity checks must find (a drifted cached balance) straight into the
//! engine's database. Never compiled into a build users run.

use std::str::FromStr;
use std::time::Duration;

use postgres::{Config, NoTls};

use crate::app_role::sql_error;
use crate::{PgError, Result, Secret};

/// Run `sql` (one or more statements) on the database `url` names, as the
/// role it names. The engine's database URL connects as the application
/// role, which owns every table (the superuser may only reach `postgres`,
/// `pg_hba`): enough to change rows and to disable the table's own
/// triggers for a seed.
///
/// # Errors
/// [`PgError::Sql`] when the URL is not a `postgres://` URL, the server
/// cannot be reached or a statement fails.
pub fn exec_sql_for_tests(url: &Secret, sql: &str) -> Result<()> {
    let mut cfg = Config::from_str(url.expose()).map_err(|_| PgError::Sql {
        context: "test SQL".into(),
        detail: "not a database URL".into(),
    })?;
    let mut client = cfg
        .connect_timeout(Duration::from_secs(10))
        .application_name("clientledger-test")
        .connect(NoTls)
        .map_err(|e| sql_error("connect for test SQL", &e))?;
    client
        .batch_execute(sql)
        .map_err(|e| sql_error("run test SQL", &e))
}
