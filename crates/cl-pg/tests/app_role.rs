//! The engine's database role: owner of one database, nothing else (Q-P04a).

mod common;

use cl_pg::{Cluster, MemoryStore, Secret};

const ROLE: &str = "clientledger_app";
const DB: &str = "clientledger";

fn connect_app(
    port: u16,
    db: &str,
    password: &Secret,
) -> Result<postgres::Client, postgres::Error> {
    postgres::Config::new()
        .host("127.0.0.1")
        .port(port)
        .user(ROLE)
        .password(password.expose())
        .dbname(db)
        .connect_timeout(std::time::Duration::from_secs(10))
        .connect(postgres::NoTls)
}

#[test]
fn the_app_role_owns_its_database_and_has_no_other_power() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("approle");
    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &MemoryStore::new()).unwrap();
    let running = cluster.start().unwrap();
    let port = running.port;
    let pw = Secret::generate().unwrap();

    cluster.ensure_app_database(port, ROLE, DB, &pw).unwrap();

    // Attributes: a plain login role.
    let mut admin = common::connect(&cluster, port, "postgres");
    let row = admin
        .query_one(
            "SELECT rolsuper, rolcreaterole, rolcreatedb, rolreplication, rolbypassrls \
             FROM pg_roles WHERE rolname = $1",
            &[&ROLE],
        )
        .unwrap();
    for i in 0..5 {
        assert!(!row.get::<_, bool>(i), "attribute {i} must be off");
    }
    let owner: String = admin
        .query_one(
            "SELECT r.rolname FROM pg_database d JOIN pg_roles r ON r.oid = d.datdba WHERE d.datname = $1",
            &[&DB],
        )
        .unwrap()
        .get(0);
    assert_eq!(owner, ROLE);

    // It can do what migrations and the job queue need.
    let mut app = connect_app(port, DB, &pw).expect("app role can log in");
    app.batch_execute(
        "CREATE SCHEMA pgboss; CREATE TABLE pgboss.job (id int primary key); \
         CREATE TABLE public.t (id int primary key, v numeric(12,2)); \
         INSERT INTO public.t VALUES (1, 10.50); \
         CREATE INDEX t_v ON public.t (v);",
    )
    .expect("owner can run DDL in its own database");

    // And nothing else.
    for (what, sql) in [
        ("create a role", "CREATE ROLE evil SUPERUSER"),
        ("create a database", "CREATE DATABASE evil"),
        ("run a program", "COPY public.t FROM PROGRAM 'echo 1'"),
        ("read a server file", "COPY public.t FROM '/etc/passwd'"),
        ("become superuser", "ALTER ROLE clientledger_app SUPERUSER"),
        ("read pg_authid", "SELECT rolpassword FROM pg_authid"),
    ] {
        assert!(app.batch_execute(sql).is_err(), "app role must not {what}");
    }
    drop(app);

    // It cannot even connect to the system databases.
    for db in ["postgres", "template1"] {
        assert!(
            connect_app(port, db, &pw).is_err(),
            "app role must not connect to {db}"
        );
    }
    // Even the superuser cannot open the books: pg_hba only lets it into `postgres`.
    assert!(common::connect_with(port, DB, cluster.connection(port).password.expose()).is_err());
    // A wrong password is refused.
    assert!(connect_app(port, DB, &Secret::new("wrong")).is_err());
    cluster.stop().unwrap();
}

#[test]
fn running_it_again_repairs_attributes_memberships_and_the_password() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("approle-repair");
    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &MemoryStore::new()).unwrap();
    let running = cluster.start().unwrap();
    let port = running.port;
    let first = Secret::generate().unwrap();
    cluster.ensure_app_database(port, ROLE, DB, &first).unwrap();

    // Someone (or a restored backup) widens the role.
    let mut admin = common::connect(&cluster, port, "postgres");
    admin
        .batch_execute(
            "ALTER ROLE clientledger_app CREATEROLE CREATEDB REPLICATION BYPASSRLS; \
             GRANT pg_read_server_files, pg_execute_server_program TO clientledger_app; \
             ALTER DATABASE clientledger OWNER TO postgres;              ALTER ROLE clientledger_app SET search_path = evil;              ALTER DATABASE clientledger SET search_path = evil;              ALTER ROLE clientledger_app VALID UNTIL '2000-01-01';              CREATE ROLE other_grantor SUPERUSER;              SET ROLE other_grantor; \n             GRANT pg_write_server_files TO clientledger_app; \n             RESET ROLE",
        )
        .unwrap();

    let second = Secret::generate().unwrap();
    cluster
        .ensure_app_database(port, ROLE, DB, &second)
        .unwrap();

    let attrs = admin
        .query_one(
            "SELECT rolcreaterole OR rolcreatedb OR rolreplication OR rolbypassrls OR rolsuper \
             FROM pg_roles WHERE rolname = $1",
            &[&ROLE],
        )
        .unwrap();
    assert!(!attrs.get::<_, bool>(0));
    let memberships: i64 = admin
        .query_one(
            "SELECT count(*) FROM pg_auth_members m JOIN pg_roles u ON u.oid = m.member \
             WHERE u.rolname = $1",
            &[&ROLE],
        )
        .unwrap()
        .get(0);
    assert_eq!(memberships, 0);
    let owner: String = admin
        .query_one(
            "SELECT r.rolname FROM pg_database d JOIN pg_roles r ON r.oid = d.datdba WHERE d.datname = $1",
            &[&DB],
        )
        .unwrap()
        .get(0);
    assert_eq!(owner, ROLE);

    // Settings, expiry and a grant made by another role are gone too.
    let leftovers: i64 = admin
        .query_one(
            "SELECT (SELECT count(*) FROM pg_db_role_setting s WHERE s.setrole = r.oid                        OR s.setdatabase = (SELECT oid FROM pg_database WHERE datname = 'clientledger'))                     + (CASE WHEN r.rolvaliduntil IS NULL OR r.rolvaliduntil = 'infinity' THEN 0 ELSE 1 END)              FROM pg_roles r WHERE r.rolname = $1",
            &[&ROLE],
        )
        .unwrap()
        .get(0);
    assert_eq!(leftovers, 0);

    // The new password works and the old one does not.
    assert!(connect_app(port, DB, &second).is_ok());
    assert!(connect_app(port, DB, &first).is_err());
    cluster.stop().unwrap();
}

#[test]
fn hostile_names_and_passwords_are_refused_before_any_sql_runs() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("approle-names");
    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &MemoryStore::new()).unwrap();
    let running = cluster.start().unwrap();
    let good = Secret::generate().unwrap();
    for (role, db, pw) in [
        ("a\"; DROP ROLE postgres; --", DB, good.clone()),
        (ROLE, "x y", good.clone()),
        (
            ROLE,
            DB,
            Secret::new("pw'; ALTER ROLE postgres PASSWORD 'x"),
        ),
        (ROLE, DB, Secret::new("")),
    ] {
        let err = cluster
            .ensure_app_database(running.port, role, db, &pw)
            .unwrap_err();
        assert!(
            !err.to_string().contains("DROP ROLE postgres PASSWORD"),
            "{err}"
        );
    }
    cluster.stop().unwrap();
}
