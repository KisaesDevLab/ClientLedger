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

/// `Cluster::restore` needs `postgres --single`, which `PostgreSQL` refuses to
/// run with administrator rights on Windows (Q-P03d). True (after a notice)
/// for that error so the caller can skip.
fn elevated_windows(err: &PgError) -> bool {
    match err {
        PgError::Command { detail, .. } if detail.to_lowercase().contains("administrat") => {
            eprintln!("SKIPPED: PostgreSQL refuses to run elevated on Windows: {detail}");
            true
        }
        _ => false,
    }
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
    let (mut cluster, store) = new_cluster(&scratch, "pgdata", bins);
    let running = cluster.start().unwrap();
    let cleanup = cluster.clone();
    let _stop = Guard(&cleanup);
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

    let old_pw = store.get().unwrap().unwrap();
    let restored = match cluster.restore(&backup, &store, false) {
        Ok(r) => r,
        Err(e) if elevated_windows(&e) => return,
        Err(e) => panic!("restore failed: {e}"),
    };
    assert!(restored.files > 100);
    // The password was replaced and saved; the backup's password is dead.
    let new_pw = store.get().unwrap().unwrap();
    assert_ne!(new_pw, old_pw);
    // The wrecked database was set aside, not deleted.
    let previous = restored.previous.expect("previous data kept");
    assert!(previous.join("PG_VERSION").is_file());
    assert_ne!(previous, scratch.path("pgdata"));

    let again = cluster.start().unwrap();
    assert!(
        common::connect_with(again.port, "postgres", old_pw.expose()).is_err(),
        "the backup's password still works"
    );
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

#[cfg(unix)]
#[test]
fn backup_files_are_readable_by_their_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("mode");
    let (cluster, _store) = new_cluster(&scratch, "pgdata", bins);
    let dest = scratch.path("b").join("c.tar.gz");
    cluster.dump(&dest).unwrap();
    let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "backup mode is {mode:o}");
    // Writing over an old, more open file also ends up owner-only.
    fs::write(dest.with_extension("part"), b"stale").unwrap();
    fs::set_permissions(
        dest.with_extension("part"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    cluster.dump(&dest).unwrap();
    let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "backup mode is {mode:o}");
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
#[allow(clippy::too_many_lines)] // a table of cases
fn bad_archives_are_refused_and_change_nothing() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("bad");
    let (mut cluster, store) = new_cluster(&scratch, "pgdata", bins);
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
            "program at the top of the data folder",
            Box::new(|p| {
                archive(
                    p,
                    &[("data/PG_VERSION", b"16\n"), ("data/evil.so", b"x")],
                    Some(&manifest(here, "16", 2, 4)),
                );
            }),
        ),
        (
            "dll at the top of the data folder",
            Box::new(|p| {
                archive(
                    p,
                    &[("data/PG_VERSION", b"16\n"), ("data/evil.dll", b"x")],
                    Some(&manifest(here, "16", 2, 4)),
                );
            }),
        ),
        (
            "recovery signal file",
            Box::new(|p| {
                archive(
                    p,
                    &[("data/PG_VERSION", b"16\n"), ("data/recovery.signal", b"")],
                    Some(&manifest(here, "16", 2, 3)),
                );
            }),
        ),
        (
            "unknown folder",
            Box::new(|p| {
                archive(
                    p,
                    &[("data/PG_VERSION", b"16\n"), ("data/scripts/run.sh", b"x")],
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
        let err = cluster.restore(&file, &store, true).unwrap_err();
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

/// A source cluster with a table of 42 rows, backed up while stopped.
/// Returns the archive path and the source's own password.
fn make_source_backup(scratch: &common::Scratch, bins: PgBinaries) -> (std::path::PathBuf, String) {
    let (source, source_store) = new_cluster(scratch, "source", bins);
    let r = source.start().unwrap();
    let mut db = common::connect(&source, r.port, "postgres");
    db.batch_execute("create table t(id int); insert into t select generate_series(1, 42)")
        .unwrap();
    drop(db);
    let backup = scratch.path("source.tar.gz");
    source.dump(&backup).unwrap();
    source.stop().unwrap();
    let pw = source_store.get().unwrap().unwrap().expose().to_string();
    (backup, pw)
}

#[test]
fn a_restored_database_gets_a_new_password() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("pwreset");
    let (backup, source_pw) = make_source_backup(&scratch, bins.clone());

    // A different cluster (different password) receives it; the owner has
    // confirmed the file, so the registry check is waived.
    let (mut target, target_store) = new_cluster(&scratch, "target", bins);
    let old_pw = target_store.get().unwrap().unwrap();
    if let Err(e) = target.restore(&backup, &target_store, true) {
        if elevated_windows(&e) {
            return;
        }
        panic!("restore failed: {e}");
    }
    let new_pw = target_store.get().unwrap().unwrap();
    assert_ne!(new_pw, old_pw);
    let running = target.start().unwrap();
    let cleanup = target.clone();
    let _stop = Guard(&cleanup);
    // Neither the file's password nor the previous one works any more.
    assert!(common::connect_with(running.port, "postgres", &source_pw).is_err());
    assert!(common::connect_with(running.port, "postgres", old_pw.expose()).is_err());
    let mut db = common::connect(&target, running.port, "postgres");
    let n: i64 = db.query_one("select count(*) from t", &[]).unwrap().get(0);
    assert_eq!(n, 42);
}

#[test]
fn a_backup_this_install_did_not_write_needs_confirmation() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("unregistered");
    let (backup, _pw) = make_source_backup(&scratch, bins.clone());
    let (mut target, target_store) = new_cluster(&scratch, "target", bins);
    let pw_before = target_store.get().unwrap().unwrap();
    let version_before = fs::read(scratch.path("target").join("PG_VERSION")).unwrap();
    let err = target.restore(&backup, &target_store, false).unwrap_err();
    assert!(
        matches!(&err, PgError::Backup(m) if m.contains("not made by this")),
        "{err:?}"
    );
    assert_eq!(target_store.get().unwrap().unwrap(), pw_before);
    assert_eq!(
        fs::read(scratch.path("target").join("PG_VERSION")).unwrap(),
        version_before
    );
    assert!(!scratch.path("target.restoring").exists());
}

fn this_platform() -> &'static str {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    match (os, arch) {
        ("windows", _) => "windows-x64",
        ("macos", "aarch64") => "macos-arm64",
        ("macos", _) => "macos-x64",
        _ => "linux-x64",
    }
}

/// Copy the backup at `from` to `to`, replacing (or adding) the named files
/// under `data/` and fixing the manifest so the archive stays consistent.
fn tamper(from: &Path, to: &Path, edits: &[(&str, &[u8])]) {
    let mut archive =
        tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(from).unwrap()));
    let mut entries: Vec<(tar::Header, Vec<u8>)> = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
        entries.push((entry.header().clone(), data));
    }
    let name_of = |h: &tar::Header| h.path().unwrap().to_string_lossy().into_owned();
    entries.retain(|(h, _)| name_of(h) != "manifest.json");
    let mut seen = Vec::new();
    for (h, data) in &mut entries {
        let name = name_of(h);
        for (edit, bytes) in edits {
            if name == format!("data/{edit}") {
                *data = bytes.to_vec();
                h.set_size(data.len() as u64);
                h.set_cksum();
                seen.push(*edit);
            }
        }
    }
    for (edit, bytes) in edits {
        if !seen.contains(edit) {
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(0o600);
            h.set_entry_type(tar::EntryType::Regular);
            let raw = format!("data/{edit}");
            h.as_old_mut().name[..raw.len()].copy_from_slice(raw.as_bytes());
            h.set_cksum();
            entries.push((h, bytes.to_vec()));
        }
    }
    let regular = || entries.iter().filter(|(h, _)| h.entry_type().is_file());
    let files = regular().count() as u64;
    let bytes: u64 = regular().map(|(_, d)| d.len() as u64).sum();
    let gz =
        flate2::write::GzEncoder::new(fs::File::create(to).unwrap(), flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    for (h, data) in &entries {
        b.append(h, &data[..]).unwrap();
    }
    let m = manifest(this_platform(), "16", files, bytes);
    let mut h = tar::Header::new_gnu();
    h.set_size(m.len() as u64);
    h.set_mode(0o644);
    b.append_data(&mut h, "manifest.json", m.as_bytes())
        .unwrap();
    b.into_inner().unwrap().finish().unwrap().flush().unwrap();
}

#[test]
fn a_crafted_configuration_in_a_backup_is_neutralised() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("crafted");
    let (backup, _pw) = make_source_backup(&scratch, bins.clone());
    let crafted = scratch.path("crafted.tar.gz");
    tamper(
        &backup,
        &crafted,
        &[
            (
                "postgresql.auto.conf",
                b"shared_preload_libraries = 'evil'\narchive_mode = on\narchive_command = 'calc'\n",
            ),
            (
                "postgresql.conf",
                b"max_connections = 37\nshared_preload_libraries = 'evil2'\nssl = on\nport = 1\nunix_socket_directories = '/tmp'\nhba_file = '/tmp/x'\n",
            ),
            (
                "clientledger.conf",
                b"shared_preload_libraries = 'evil3'\nlisten_addresses = '*'\n",
            ),
            ("pg_hba.conf", b"host all all 0.0.0.0/0 trust\n"),
            ("pg_ident.conf", b"evil evil postgres\n"),
        ],
    );
    let (mut target, target_store) = new_cluster(&scratch, "target", bins);
    if let Err(e) = target.restore(&crafted, &target_store, true) {
        if elevated_windows(&e) {
            return;
        }
        panic!("restore failed: {e}");
    }
    let data = scratch.path("target");
    let auto = fs::read_to_string(data.join("postgresql.auto.conf")).unwrap();
    assert!(
        !auto.contains("evil") && !auto.contains("archive"),
        "{auto}"
    );
    let conf = fs::read_to_string(data.join("postgresql.conf")).unwrap();
    assert!(
        !conf.contains("evil") && !conf.contains("ssl") && !conf.contains("hba_file"),
        "{conf}"
    );
    assert!(
        conf.contains("max_connections = 37"),
        "harmless initdb choices are kept"
    );
    let hba = fs::read_to_string(data.join("pg_hba.conf")).unwrap();
    assert!(!hba.contains("trust") && !hba.contains("0.0.0.0"), "{hba}");
    assert!(
        !fs::read_to_string(data.join("pg_ident.conf"))
            .unwrap()
            .contains("evil")
    );
    assert!(
        !fs::read_to_string(data.join("clientledger.conf"))
            .unwrap()
            .contains("evil")
    );

    // A server that loaded 'evil' would not start. This one does, and says so.
    let running = target.start().unwrap();
    let cleanup = target.clone();
    let _stop = Guard(&cleanup);
    let mut db = common::connect(&target, running.port, "postgres");
    let libs: String = db
        .query_one("show shared_preload_libraries", &[])
        .unwrap()
        .get(0);
    assert_eq!(libs, "");
    let mode: String = db.query_one("show archive_mode", &[]).unwrap().get(0);
    assert_eq!(mode, "off");
    let conns: String = db.query_one("show max_connections", &[]).unwrap().get(0);
    assert_eq!(conns, "37");
    common::assert_only_loopback(running.port);
}
