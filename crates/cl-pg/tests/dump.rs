//! Whole-cluster backup and restore (P03 step 5).

mod common;

use cl_pg::{Cluster, MemoryStore, PasswordStore, PgBinaries, PgError};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

struct Guard<'a>(&'a Cluster);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let _ = self.0.stop();
    }
}

/// Two databases, several tables, an index, a sequence, text with awkward
/// characters, and enough rows to span many pages.
fn populate(cluster: &Cluster, port: u16) {
    let mut db = common::connect(cluster, port, "postgres");
    db.batch_execute("create database company").unwrap();
    db.batch_execute(
        "create table accounts(id serial primary key, name text not null, balance numeric(14,2));
         insert into accounts(name, balance)
           select 'Acct ' || g || ' caf\u{e9} \u{2603}', (g * 1.37)::numeric(14,2) from generate_series(1, 3000) g;
         create index accounts_name on accounts(name);
         create table notes(id int primary key, body text);
         insert into notes select g, repeat('x', 200) || g from generate_series(1, 500) g;",
    )
    .unwrap();
    drop(db);
    let mut db = common::connect(cluster, port, "company");
    db.batch_execute(
        "create table invoices(id bigint primary key, total numeric(14,2), memo text);
         insert into invoices select g, g * 10.05, 'memo ' || md5(g::text) from generate_series(1, 2000) g;
         create table empty_table(id int);",
    )
    .unwrap();
}

fn all_checksums(cluster: &Cluster, port: u16) -> BTreeMap<String, (i64, String)> {
    let mut all = common::table_checksums(cluster, port, "postgres");
    all.extend(common::table_checksums(cluster, port, "company"));
    all
}

fn new_cluster(scratch: &common::Scratch, name: &str, bins: PgBinaries) -> (Cluster, MemoryStore) {
    let store = MemoryStore::new();
    let cluster = Cluster::ensure(&scratch.path(name), bins, &store).unwrap();
    (cluster, store)
}

#[test]
fn dump_then_restore_gives_identical_tables() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("roundtrip");
    let (cluster, store) = new_cluster(&scratch, "pgdata", bins);
    let running = cluster.start().unwrap();
    let _stop = Guard(&cluster);
    populate(&cluster, running.port);
    let before = all_checksums(&cluster, running.port);
    assert_eq!(before["postgres.accounts"].0, 3000);
    assert_eq!(before["company.invoices"].0, 2000);
    assert_eq!(before["company.empty_table"].0, 0);

    // Back up while running: the server stops, is archived, and comes back.
    let backup = scratch.path("backups").join("cluster.tar.gz");
    let info = cluster.dump(&backup).unwrap();
    assert!(backup.is_file());
    assert_eq!(info.path, backup);
    assert!(info.files > 100 && info.bytes > 1_000_000, "{info:?}");
    assert_eq!(info.sha256.len(), 64);
    let after_dump = info.restarted.expect("server restarted after the backup");
    assert_eq!(
        all_checksums(&cluster, after_dump.port),
        before,
        "dump changed data"
    );

    // The backup never holds the cleartext password.
    let pw = store.get().unwrap().unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
        fs::File::open(&backup).unwrap(),
    ));
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(pw.expose()),
            "password in {:?}",
            entry.path().unwrap()
        );
    }

    // Wreck the data, then restore.
    let mut db = common::connect(&cluster, after_dump.port, "company");
    db.batch_execute("drop table invoices; insert into empty_table values (1)")
        .unwrap();
    drop(db);
    let mut db = common::connect(&cluster, after_dump.port, "postgres");
    db.batch_execute("delete from accounts where id % 2 = 0; drop table notes")
        .unwrap();
    drop(db);
    assert_ne!(all_checksums(&cluster, after_dump.port), before);

    let restored = cluster.restore(&backup).unwrap();
    assert!(restored.files > 100);
    // The wrecked database was set aside, not deleted.
    let previous = restored.previous.expect("previous data kept");
    assert!(previous.join("PG_VERSION").is_file());
    assert_ne!(previous, scratch.path("pgdata"));

    let again = cluster.start().unwrap();
    assert_eq!(all_checksums(&cluster, again.port), before);
    // Sequences continue where they were.
    let mut db = common::connect(&cluster, again.port, "postgres");
    let next: i64 = db
        .query_one(
            "insert into accounts(name, balance) values ('new', 1) returning id::bigint",
            &[],
        )
        .unwrap()
        .get(0);
    assert_eq!(next, 3001);
}

#[test]
fn dumping_a_stopped_cluster_leaves_it_stopped() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("stopped");
    let (cluster, _store) = new_cluster(&scratch, "pgdata", bins);
    let info = cluster.dump(&scratch.path("b").join("c.tar.gz")).unwrap();
    assert!(info.restarted.is_none());
    assert_eq!(cluster.running_port(), None);
}

#[test]
fn the_backup_cannot_be_written_into_the_data_folder() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("inside");
    let (cluster, _store) = new_cluster(&scratch, "pgdata", bins);
    let err = cluster
        .dump(&scratch.path("pgdata").join("backup.tar.gz"))
        .unwrap_err();
    assert!(matches!(err, PgError::Backup(_)), "{err:?}");
}

/// A minimal archive: `data/<name>` files plus an optional manifest.
fn archive(path: &Path, files: &[(&str, &[u8])], manifest: Option<&str>) {
    let gz =
        flate2::write::GzEncoder::new(fs::File::create(path).unwrap(), flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    for (name, data) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        // Write the name directly so unsafe names can be tested.
        let raw = name.as_bytes();
        h.as_old_mut().name[..raw.len()].copy_from_slice(raw);
        h.set_cksum();
        b.append(&h, *data).unwrap();
    }
    if let Some(m) = manifest {
        let mut h = tar::Header::new_gnu();
        h.set_size(m.len() as u64);
        h.set_mode(0o644);
        b.append_data(&mut h, "manifest.json", m.as_bytes())
            .unwrap();
    }
    b.into_inner().unwrap().finish().unwrap().flush().unwrap();
}

/// A label and a function that writes the bad archive to a path.
type Case<'a> = (&'a str, Box<dyn Fn(&Path) + 'a>);

fn manifest(platform: &str, major: &str, files: u64, bytes: u64) -> String {
    format!(
        "{{\"format\": 1, \"pgMajor\": \"{major}\", \"platform\": \"{platform}\", \"files\": {files}, \"bytes\": {bytes}}}"
    )
}

#[test]
fn bad_archives_are_refused_and_change_nothing() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("bad");
    let (cluster, _store) = new_cluster(&scratch, "pgdata", bins);
    let version_before = fs::read(scratch.path("pgdata").join("PG_VERSION")).unwrap();
    let here = {
        // The same platform name the crate uses.
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        match (os, arch) {
            ("windows", _) => "windows-x64",
            ("macos", "aarch64") => "macos-arm64",
            ("macos", _) => "macos-x64",
            _ => "linux-x64",
        }
    };

    let cases: Vec<Case> = vec![
        (
            "not an archive",
            Box::new(|p| fs::write(p, b"hello").unwrap()),
        ),
        (
            "truncated",
            Box::new(|p| {
                archive(
                    p,
                    &[("data/PG_VERSION", b"16\n")],
                    Some(&manifest(here, "16", 1, 3)),
                );
                let bytes = fs::read(p).unwrap();
                fs::write(p, &bytes[..bytes.len() / 2]).unwrap();
            }),
        ),
        (
            "no manifest",
            Box::new(|p| archive(p, &[("data/PG_VERSION", b"16\n")], None)),
        ),
        (
            "wrong counts",
            Box::new(|p| {
                archive(
                    p,
                    &[("data/PG_VERSION", b"16\n")],
                    Some(&manifest(here, "16", 9, 3)),
                );
            }),
        ),
        (
            "other platform",
            Box::new(|p| {
                archive(
                    p,
                    &[("data/PG_VERSION", b"16\n")],
                    Some(&manifest("plan9-x64", "16", 1, 3)),
                );
            }),
        ),
        (
            "other major version",
            Box::new(|p| {
                archive(
                    p,
                    &[("data/PG_VERSION", b"15\n")],
                    Some(&manifest(here, "15", 1, 3)),
                );
            }),
        ),
        (
            "path traversal",
            Box::new(|p| {
                archive(
                    p,
                    &[("data/PG_VERSION", b"16\n"), ("data/../../evil.txt", b"x")],
                    Some(&manifest(here, "16", 2, 4)),
                );
            }),
        ),
        (
            "stray top-level file",
            Box::new(|p| {
                archive(p, &[("evil.txt", b"x")], Some(&manifest(here, "16", 0, 0)));
            }),
        ),
    ];
    for (label, make) in cases {
        let file = scratch.path("bad.tar.gz");
        make(&file);
        let err = cluster.restore(&file).unwrap_err();
        assert!(matches!(err, PgError::Backup(_)), "{label}: {err:?}");
        assert_eq!(
            fs::read(scratch.path("pgdata").join("PG_VERSION")).unwrap(),
            version_before,
            "{label}: data changed"
        );
        assert!(
            !scratch.path("pgdata.restoring").exists(),
            "{label}: staging left"
        );
        assert!(!scratch.root.parent().unwrap().join("evil.txt").exists());
    }
    // And the cluster still starts.
    let running = cluster.start().unwrap();
    let _stop = Guard(&cluster);
    common::connect(&cluster, running.port, "postgres");
}

#[test]
fn a_restored_database_with_another_password_can_be_reset() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("pwreset");

    // Source cluster with a table, backed up while stopped.
    let (source, _source_store) = new_cluster(&scratch, "source", bins.clone());
    let r = source.start().unwrap();
    let mut db = common::connect(&source, r.port, "postgres");
    db.batch_execute("create table t(id int); insert into t select generate_series(1, 42)")
        .unwrap();
    drop(db);
    let backup = scratch.path("source.tar.gz");
    source.dump(&backup).unwrap();
    source.stop().unwrap();

    // A different cluster (different password) receives it.
    let (mut target, target_store) = new_cluster(&scratch, "target", bins);
    let old_pw = target_store.get().unwrap().unwrap();
    target.restore(&backup).unwrap();
    let running = target.start().unwrap();
    let cleanup = target.clone();
    let _stop = Guard(&cleanup);
    assert!(
        common::connect_with(running.port, "postgres", old_pw.expose()).is_err(),
        "restored data carries the source password, not the target's"
    );
    target.stop().unwrap();

    match target.reset_password(&target_store) {
        Ok(()) => {}
        Err(PgError::Command { detail, .. }) if detail.to_lowercase().contains("administrat") => {
            eprintln!("SKIPPED: PostgreSQL refuses to run elevated on Windows: {detail}");
            return;
        }
        Err(e) => panic!("reset failed: {e}"),
    }
    let new_pw = target_store.get().unwrap().unwrap();
    assert_ne!(new_pw, old_pw);
    let running = target.start().unwrap();
    let mut db = common::connect(&target, running.port, "postgres");
    let n: i64 = db.query_one("select count(*) from t", &[]).unwrap().get(0);
    assert_eq!(n, 42);
}
