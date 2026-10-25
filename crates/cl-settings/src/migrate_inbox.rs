//! Moving P17's single receipt-inbox folder into the company's intake
//! folders (A01 D5, Q-A01q). This module only plans; the glue in
//! `cl-company::intake::migrate` carries the plan out, in this order:
//!
//! 1. PUT the folder list to the engine (on a 409: re-read and re-plan once);
//! 2. save this computer's [`HereNote`] for the folder (auto-confirmed:
//!    the owner chose this folder on this computer);
//! 3. clear the legacy field.
//!
//! A crash between steps repeats harmlessly: the next plan finds the path
//! already in the engine and only confirms it. Nothing in the folder moves.

use std::collections::BTreeMap;
use std::path::Path;

use cl_core::ids::IntakeFolderId;
use cl_core::intake::{IntakeFolder, IntakeKind};

use crate::company_prefs::{HereNote, path_hash};

/// Name of the folder the migration adds.
pub const MIGRATED_NAME: &str = "Receipts";
/// Name used when [`MIGRATED_NAME`] is already taken.
pub const MIGRATED_NAME_TAKEN: &str = "Receipts (this computer)";
/// Most folders the engine keeps per company (U-93).
pub const MAX_FOLDERS: usize = 20;

/// Why the legacy folder can't be moved over yet. The legacy field is
/// kept, the P17 inbox keeps using it, and Home shows a Needs attention
/// line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationBlock {
    /// The company already has the most folders the engine keeps.
    TooManyFolders,
    /// The legacy path is not valid UTF-8, so the engine can't store it.
    NotUtf8,
}

/// What the migration should do. Every field "off" means there is
/// nothing to do.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MigrationPlan {
    /// A folder to append to the engine's list (step 1).
    pub add_folder: Option<IntakeFolder>,
    /// The folder to confirm on this computer (step 2).
    pub confirm: Option<IntakeFolderId>,
    /// Clear the legacy field (step 3).
    pub clear_legacy: bool,
    /// Why nothing can be done yet.
    pub blocked: Option<MigrationBlock>,
}

impl MigrationPlan {
    /// True when the plan changes nothing.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.add_folder.is_none() && self.confirm.is_none() && !self.clear_legacy
    }
}

/// The key two folder paths are compared by, as the engine compares them
/// for uniqueness: letter case ignored and `/` treated as `\`.
#[must_use]
pub fn same_path_key(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

fn name_taken(engine: &[IntakeFolder], name: &str) -> bool {
    let want = name.trim().to_lowercase();
    engine.iter().any(|f| f.name.trim().to_lowercase() == want)
}

/// A name for the migrated folder no other folder uses.
fn free_name(engine: &[IntakeFolder]) -> String {
    if !name_taken(engine, MIGRATED_NAME) {
        return MIGRATED_NAME.to_owned();
    }
    if !name_taken(engine, MIGRATED_NAME_TAKEN) {
        return MIGRATED_NAME_TAKEN.to_owned();
    }
    // At most MAX_FOLDERS names exist, so one of the first MAX_FOLDERS + 2
    // numbers is free.
    (2..MAX_FOLDERS + 3)
        .map(|n| format!("{MIGRATED_NAME_TAKEN} {n}"))
        .find(|n| !name_taken(engine, n))
        .unwrap_or_else(|| MIGRATED_NAME_TAKEN.to_owned())
}

/// Plans the migration of `legacy` (the P17 folder on this computer) into
/// the engine's `engine` folders, given this computer's confirmations
/// `here`. `fresh_id` is the id a new folder gets (the caller mints it, so
/// the plan stays pure).
///
/// - No legacy folder (or an empty one): nothing, or only clear it.
/// - The engine already has a folder with the same path: confirm that one
///   (unless it already is) and clear the legacy field.
/// - Otherwise add "Receipts" (or "Receipts (this computer)" when the name
///   is taken) with kind auto, auto-read off and originals moved, confirm
///   it and clear the legacy field.
#[must_use]
pub fn plan(
    legacy: Option<&Path>,
    engine: &[IntakeFolder],
    here: &BTreeMap<IntakeFolderId, HereNote>,
    fresh_id: IntakeFolderId,
) -> MigrationPlan {
    let Some(legacy) = legacy else {
        return MigrationPlan::default();
    };
    if legacy.as_os_str().is_empty() {
        return MigrationPlan {
            clear_legacy: true,
            ..MigrationPlan::default()
        };
    }
    let Some(text) = legacy.to_str() else {
        return MigrationPlan {
            blocked: Some(MigrationBlock::NotUtf8),
            ..MigrationPlan::default()
        };
    };
    let key = same_path_key(text);
    if let Some(found) = engine.iter().find(|f| same_path_key(&f.path) == key) {
        let confirmed = here
            .get(&found.id)
            .is_some_and(|n| n.path_hash == path_hash(&found.path));
        return MigrationPlan {
            add_folder: None,
            confirm: (!confirmed).then_some(found.id),
            clear_legacy: true,
            blocked: None,
        };
    }
    if engine.len() >= MAX_FOLDERS {
        return MigrationPlan {
            blocked: Some(MigrationBlock::TooManyFolders),
            ..MigrationPlan::default()
        };
    }
    MigrationPlan {
        add_folder: Some(IntakeFolder {
            id: fresh_id,
            name: free_name(engine),
            path: text.to_owned(),
            default_kind: IntakeKind::Auto,
            auto_read: false,
            keep_originals: false,
        }),
        confirm: Some(fresh_id),
        clear_legacy: true,
        blocked: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> IntakeFolderId {
        IntakeFolderId::parse(&format!("7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a{n:02x}")).expect("id")
    }

    fn folder(n: u8, name: &str, path: &str) -> IntakeFolder {
        IntakeFolder {
            id: id(n),
            name: name.to_owned(),
            path: path.to_owned(),
            default_kind: IntakeKind::Bills,
            auto_read: true,
            keep_originals: true,
        }
    }

    fn note(path: &str) -> HereNote {
        HereNote {
            path_hash: path_hash(path),
            canonical_key: "k".repeat(64),
            since_ms: None,
            confirmed_at: None,
        }
    }

    const LEGACY: &str = r"C:\Users\Owner\Receipts";

    #[test]
    fn nothing_to_migrate() {
        let none = plan(None, &[], &BTreeMap::new(), id(99));
        assert!(none.is_noop());
        assert_eq!(none, MigrationPlan::default());
        let empty = plan(Some(Path::new("")), &[], &BTreeMap::new(), id(99));
        assert_eq!(
            empty,
            MigrationPlan {
                clear_legacy: true,
                ..MigrationPlan::default()
            }
        );
    }

    #[test]
    fn a_new_receipts_folder_is_added_and_confirmed() {
        let p = plan(Some(Path::new(LEGACY)), &[], &BTreeMap::new(), id(99));
        let added = p.add_folder.clone().expect("added");
        assert_eq!(added.id, id(99));
        assert_eq!(added.name, "Receipts");
        assert_eq!(added.path, LEGACY);
        assert_eq!(added.default_kind, IntakeKind::Auto);
        assert!(!added.auto_read);
        assert!(!added.keep_originals);
        assert_eq!(p.confirm, Some(id(99)));
        assert!(p.clear_legacy);
        assert_eq!(p.blocked, None);
        assert!(!p.is_noop());
    }

    #[test]
    fn the_same_path_in_the_engine_is_only_confirmed() {
        let engine = [
            folder(1, "Bills", r"D:\Bills"),
            folder(2, "Shoebox", "c:/users/owner/receipts"),
        ];
        let p = plan(Some(Path::new(LEGACY)), &engine, &BTreeMap::new(), id(99));
        assert_eq!(
            p,
            MigrationPlan {
                add_folder: None,
                confirm: Some(id(2)),
                clear_legacy: true,
                blocked: None,
            }
        );
        // Already confirmed here (a crash after step 2): only clear.
        let here = BTreeMap::from([(id(2), note("c:/users/owner/receipts"))]);
        let p = plan(Some(Path::new(LEGACY)), &engine, &here, id(99));
        assert_eq!(p.confirm, None);
        assert!(p.clear_legacy);
        assert!(p.add_folder.is_none());
        // A note for an older path does not count.
        let stale = BTreeMap::from([(id(2), note(r"E:\Old"))]);
        assert_eq!(
            plan(Some(Path::new(LEGACY)), &engine, &stale, id(99)).confirm,
            Some(id(2))
        );
    }

    #[test]
    fn a_taken_name_gets_this_computer() {
        let engine = [folder(1, " receipts ", r"D:\Other")];
        let p = plan(Some(Path::new(LEGACY)), &engine, &BTreeMap::new(), id(99));
        assert_eq!(
            p.add_folder.expect("added").name,
            "Receipts (this computer)"
        );
        let engine = [
            folder(1, "Receipts", r"D:\A"),
            folder(2, "RECEIPTS (THIS COMPUTER)", r"D:\B"),
        ];
        let p = plan(Some(Path::new(LEGACY)), &engine, &BTreeMap::new(), id(99));
        assert_eq!(
            p.add_folder.expect("added").name,
            "Receipts (this computer) 2"
        );
    }

    #[test]
    fn a_full_list_blocks_and_keeps_the_legacy_folder() {
        let engine: Vec<IntakeFolder> = (0..20)
            .map(|n| folder(n, &format!("F{n}"), &format!(r"D:\F{n}")))
            .collect();
        let p = plan(Some(Path::new(LEGACY)), &engine, &BTreeMap::new(), id(99));
        assert!(p.is_noop());
        assert!(!p.clear_legacy, "the P17 folder keeps working");
        assert_eq!(p.blocked, Some(MigrationBlock::TooManyFolders));
        // Even full, a folder already there is confirmed.
        let mut engine = engine;
        engine[5].path = LEGACY.to_owned();
        let p = plan(Some(Path::new(LEGACY)), &engine, &BTreeMap::new(), id(99));
        assert_eq!(p.confirm, Some(id(5)));
        assert!(p.clear_legacy);
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_path_is_blocked() {
        use std::os::unix::ffi::OsStrExt as _;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/home/o/\xff"));
        let p = plan(Some(bad), &[], &BTreeMap::new(), id(99));
        assert_eq!(p.blocked, Some(MigrationBlock::NotUtf8));
        assert!(!p.clear_legacy);
    }

    #[test]
    fn same_path_key_ignores_case_and_slashes() {
        assert_eq!(same_path_key("C:/A/b"), same_path_key(r"c:\a\B"));
        assert_ne!(same_path_key(r"C:\A"), same_path_key(r"C:\A\"));
    }
}
