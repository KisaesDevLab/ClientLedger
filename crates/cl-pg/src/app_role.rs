//! The application role: what the engine connects as (Q-P04a).
//!
//! The cluster superuser (`postgres`) stays inside `cl-pg` and the keychain.
//! The engine instead gets a role that owns one database and nothing else:
//! it can run migrations and create schemas and tables in that database, but
//! it is not a superuser, cannot create roles or databases, cannot replicate
//! or bypass row security, and belongs to no other role (in particular not to
//! `pg_execute_server_program`, `pg_read_server_files` or
//! `pg_write_server_files`, which would let it run programs or read files as
//! the database server).

use crate::{Cluster, PgError, Result, Secret};
use postgres::{Client, Config, NoTls};
use std::time::Duration;

/// Attributes the application role must have. Applied on every start, so a
/// restored or hand-edited cluster is brought back in line.
const ROLE_ATTRIBUTES: &str = "LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT CONNECTION LIMIT -1";

/// A safe, unquoted-looking SQL name: lowercase letters, digits, underscores.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name.len() <= 63
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn quote(name: &str) -> String {
    format!("\"{name}\"")
}

fn sql_error(context: &str, e: &postgres::Error) -> PgError {
    // The server's message names the object and the problem, never the
    // password we sent; the full Debug form could echo a statement, so only
    // the SQLSTATE and the short message are kept.
    let detail = e.as_db_error().map_or_else(
        || e.to_string(),
        |d| format!("{} ({})", d.message(), d.code().code()),
    );
    PgError::Sql {
        context: context.to_string(),
        detail,
    }
}

impl Cluster {
    /// Make sure the application role `role` and the database `database` it
    /// owns exist, with exactly the privileges described in this module's
    /// documentation, and that the role's password is `password`.
    ///
    /// Safe to run on every start. Needs the server running on `port`.
    ///
    /// # Errors
    /// Fails when a name is not a plain lowercase SQL name, the password is not
    /// alphanumeric, or the server refuses a statement.
    pub fn ensure_app_database(
        &self,
        port: u16,
        role: &str,
        database: &str,
        password: &Secret,
    ) -> Result<()> {
        if !valid_name(role) || !valid_name(database) {
            return Err(PgError::Sql {
                context: "prepare the application database".into(),
                detail: "role and database names must be lowercase letters, digits, underscores"
                    .into(),
            });
        }
        let pw = password.expose();
        if pw.is_empty() || !pw.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(PgError::Sql {
                context: "prepare the application database".into(),
                detail: "the application role password must be letters and digits only".into(),
            });
        }
        let mut client = Config::new()
            .host("127.0.0.1")
            .port(port)
            .user("postgres")
            .password(self.password.expose())
            .dbname("postgres")
            .connect_timeout(Duration::from_secs(10))
            .application_name("clientledger-host")
            .connect(NoTls)
            .map_err(|e| sql_error("connect as the database administrator", &e))?;
        apply(&mut client, role, database, pw)
    }

    /// `postgres://<role>:<password>@127.0.0.1:<port>/<database>`.
    #[must_use]
    pub fn app_url(port: u16, role: &str, database: &str, password: &Secret) -> Secret {
        Secret::new(format!(
            "postgres://{role}:{}@127.0.0.1:{port}/{database}",
            password.expose()
        ))
    }
}

fn apply(client: &mut Client, role: &str, database: &str, password: &str) -> Result<()> {
    let role_q = quote(role);
    let db_q = quote(database);
    let step = |client: &mut Client, context: &str, sql: &str| {
        client
            .batch_execute(sql)
            .map_err(|e| sql_error(context, &e))
    };

    let exists = client
        .query_opt("SELECT 1 FROM pg_roles WHERE rolname = $1", &[&role])
        .map_err(|e| sql_error("look up the application role", &e))?
        .is_some();
    // Name and password were validated above, so they are safe to inline.
    // Statements are never logged (`log_statement=none`, Q-P03).
    let verb = if exists { "ALTER" } else { "CREATE" };
    step(
        client,
        "set up the application role",
        &format!("{verb} ROLE {role_q} WITH {ROLE_ATTRIBUTES} PASSWORD '{password}'"),
    )?;

    // No membership in any other role (pg_execute_server_program and friends).
    let memberships: Vec<String> = client
        .query(
            "SELECT g.rolname FROM pg_auth_members m \
             JOIN pg_roles g ON g.oid = m.roleid \
             JOIN pg_roles u ON u.oid = m.member WHERE u.rolname = $1",
            &[&role],
        )
        .map_err(|e| sql_error("look up role memberships", &e))?
        .iter()
        .map(|r| r.get::<_, String>(0))
        .collect();
    for granted in memberships {
        step(
            client,
            "remove a role membership",
            &format!("REVOKE {} FROM {role_q}", quote(&granted)),
        )?;
    }

    let owner: Option<String> = client
        .query_opt(
            "SELECT r.rolname FROM pg_database d JOIN pg_roles r ON r.oid = d.datdba \
             WHERE d.datname = $1",
            &[&database],
        )
        .map_err(|e| sql_error("look up the application database", &e))?
        .map(|r| r.get(0));
    match owner.as_deref() {
        None => step(
            client,
            "create the application database",
            &format!("CREATE DATABASE {db_q} OWNER {role_q} ENCODING 'UTF8'"),
        )?,
        Some(o) if o == role => {}
        Some(_) => step(
            client,
            "give the application database to its role",
            &format!("ALTER DATABASE {db_q} OWNER TO {role_q}"),
        )?,
    }

    // Nobody but the owner may use the application database, and the role has
    // no business in the system databases.
    step(
        client,
        "restrict database access",
        &format!(
            "REVOKE ALL ON DATABASE {db_q} FROM PUBLIC; \
             REVOKE ALL ON DATABASE postgres FROM PUBLIC; \
             REVOKE ALL ON DATABASE template1 FROM PUBLIC"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_must_be_plain() {
        assert!(valid_name("clientledger_app"));
        assert!(valid_name("_x1"));
        for bad in [
            "",
            "Upper",
            "a-b",
            "a b",
            "a\"b",
            "1abc",
            "a;drop",
            &"x".repeat(64),
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_app_url_hides_its_password_when_printed() {
        let url = Cluster::app_url(5444, "app", "books", &Secret::new("abc123"));
        assert_eq!(url.expose(), "postgres://app:abc123@127.0.0.1:5444/books");
        assert!(!format!("{url:?}").contains("abc123"));
    }

    #[test]
    fn the_role_cannot_be_a_superuser_by_construction() {
        for must in [
            "NOSUPERUSER",
            "NOCREATEROLE",
            "NOREPLICATION",
            "NOBYPASSRLS",
            "NOCREATEDB",
        ] {
            assert!(ROLE_ATTRIBUTES.contains(must), "{must}");
        }
    }
}
