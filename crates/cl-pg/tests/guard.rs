//! Data directory guard (P03 step 3, Q-42). These need no `PostgreSQL` binaries:
//! the folder is refused before anything is created.

use cl_pg::{
    CloudService, Cluster, DataDirIssue, MemoryStore, PgBinaries, PgError, check_data_dir,
};
use std::fs;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cl-pg-guard-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn refused(path: &std::path::Path) -> DataDirIssue {
    match check_data_dir(path) {
        Err(issue) => issue,
        Ok(()) => panic!("{} should have been refused", path.display()),
    }
}

#[test]
fn sync_folder_names_are_refused() {
    let root = scratch("names");
    for (dir, service) in [
        ("OneDrive", CloudService::OneDrive),
        ("OneDrive - Acme Co", CloudService::OneDrive),
        ("Dropbox", CloudService::Dropbox),
        ("iCloud Drive", CloudService::ICloud),
        ("Google Drive", CloudService::GoogleDrive),
    ] {
        let data = root.join(dir).join("books").join("pgdata");
        assert_eq!(
            refused(&data),
            DataDirIssue::CloudSync {
                path: data.clone(),
                service
            }
        );
        // Nothing was created by the check.
        assert!(!root.join(dir).exists());
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_marker_file_in_a_parent_folder_is_refused() {
    let root = scratch("marker");
    fs::write(root.join(".dropbox"), b"").unwrap();
    let data = root.join("work").join("pgdata");
    assert!(matches!(
        refused(&data),
        DataDirIssue::CloudSync {
            service: CloudService::Dropbox,
            ..
        }
    ));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_symlink_into_a_sync_folder_is_followed() {
    // Symlinks need privileges on Windows; try and skip quietly if refused.
    let root = scratch("link");
    let real = root.join("Dropbox").join("books");
    fs::create_dir_all(&real).unwrap();
    let link = root.join("innocent");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&real, &link).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
    if made {
        assert!(matches!(
            refused(&link.join("pgdata")),
            DataDirIssue::CloudSync { .. }
        ));
    } else {
        eprintln!("symlink not permitted here; skipped");
    }
    let _ = fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
fn windows_network_paths_are_refused() {
    for p in [r"\\nas\books\pgdata", r"\\?\UNC\nas\books\pgdata"] {
        assert!(matches!(
            refused(std::path::Path::new(p)),
            DataDirIssue::NetworkShare { .. }
        ));
    }
}

#[test]
fn a_plain_local_folder_is_allowed() {
    let root = scratch("ok");
    assert_eq!(check_data_dir(&root.join("pgdata")), Ok(()));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_refuses_before_touching_anything() {
    let root = scratch("ensure");
    let data = root.join("Dropbox").join("pgdata");
    // Binaries are not needed: the guard runs first, so a fake install is enough.
    let fake = root.join("fake-pg");
    fs::create_dir_all(fake.join("bin")).unwrap();
    for n in ["postgres", "initdb", "pg_ctl"] {
        let name = if cfg!(windows) {
            format!("{n}.exe")
        } else {
            n.to_string()
        };
        fs::write(fake.join("bin").join(name), b"x").unwrap();
    }
    let bins = PgBinaries::from_root(&fake).unwrap();
    let store = MemoryStore::new();
    let err = Cluster::ensure(&data, bins, &store).unwrap_err();
    assert!(matches!(err, PgError::DataDir { .. }), "{err:?}");
    assert!(err.to_string().contains("Dropbox"));
    assert!(!data.exists());
    assert!(!root.join("Dropbox").exists());
    let _ = fs::remove_dir_all(root);
}
