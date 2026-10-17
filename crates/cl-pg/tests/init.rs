//! Cluster creation (P03 step 2).

mod common;

use cl_pg::{Cluster, MemoryStore, PasswordStore, PgError, is_initialized};
use std::fs;

#[test]
fn fresh_ensure_creates_a_cluster_and_saves_the_password() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("init");
    let store = MemoryStore::new();
    assert!(!is_initialized(&scratch.data_dir()));

    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &store).unwrap();
    assert!(is_initialized(cluster.data_dir()));
    let saved = store.get().unwrap().expect("password saved in the store");
    assert_eq!(saved.expose().len(), 64);

    // UTF8 database encoding and SCRAM only.
    let hba = fs::read_to_string(scratch.data_dir().join("pg_hba.conf")).unwrap();
    let rules: Vec<&str> = hba.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(rules.len(), 2, "{hba}");
    for r in &rules {
        assert!(
            r.contains("127.0.0.1/32") && r.contains("scram-sha-256"),
            "{r}"
        );
        assert!(!r.contains(" all "), "{r}");
    }
    let conf = fs::read_to_string(scratch.data_dir().join("postgresql.conf")).unwrap();
    assert!(conf.contains("include_if_exists = 'clientledger.conf'"));
}

#[test]
fn the_password_never_lands_in_the_data_directory() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("nopw");
    let store = MemoryStore::new();
    Cluster::ensure(&scratch.data_dir(), bins, &store).unwrap();
    let pw = store.get().unwrap().unwrap();
    for f in common::files_under(&scratch.root) {
        let bytes = fs::read(&f).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(pw.expose()),
            "password found in {}",
            f.display()
        );
    }
    // The temporary initdb password file is gone.
    let leftovers: Vec<_> = fs::read_dir(&scratch.root)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".clpg-init"))
        .collect();
    assert_eq!(leftovers.len(), 0);
}

#[test]
fn ensure_on_an_existing_cluster_reuses_it_and_its_password() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("reuse-init");
    let store = MemoryStore::new();
    Cluster::ensure(&scratch.data_dir(), bins.clone(), &store).unwrap();
    let pw = store.get().unwrap().unwrap();
    let marker = fs::read(scratch.data_dir().join("PG_VERSION")).unwrap();

    // A hand-edited login file is put back.
    fs::write(
        scratch.data_dir().join("pg_hba.conf"),
        "host all all 0.0.0.0/0 trust\n",
    )
    .unwrap();
    Cluster::ensure(&scratch.data_dir(), bins, &store).unwrap();
    assert_eq!(store.get().unwrap().unwrap(), pw);
    assert_eq!(
        fs::read(scratch.data_dir().join("PG_VERSION")).unwrap(),
        marker
    );
    let hba = fs::read_to_string(scratch.data_dir().join("pg_hba.conf")).unwrap();
    assert!(!hba.contains("trust") && !hba.contains("0.0.0.0"), "{hba}");
}

#[test]
fn existing_cluster_without_its_password_is_reported_not_reset() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("nopw-existing");
    Cluster::ensure(&scratch.data_dir(), bins.clone(), &MemoryStore::new()).unwrap();
    let err = Cluster::ensure(&scratch.data_dir(), bins, &MemoryStore::new()).unwrap_err();
    assert!(matches!(err, PgError::PasswordMissing { .. }), "{err:?}");
}

#[test]
fn a_non_empty_folder_that_is_not_a_cluster_is_left_alone() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("nonempty");
    fs::create_dir_all(scratch.data_dir()).unwrap();
    let precious = scratch.data_dir().join("precious.txt");
    fs::write(&precious, "keep me").unwrap();
    let err = Cluster::ensure(&scratch.data_dir(), bins, &MemoryStore::new()).unwrap_err();
    assert!(matches!(err, PgError::Io { .. }), "{err:?}");
    assert_eq!(fs::read_to_string(precious).unwrap(), "keep me");
}
