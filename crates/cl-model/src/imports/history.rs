//! The import History list (P31): one row per batch, newest first, the
//! actions each row offers, and the backups that must not be pruned.
//!
//! P31 adds one retention rule to the shared before-import backups: never
//! prune a backup that an unverified or still-undoable batch references
//! ([`protected_backups`]).

use std::collections::BTreeSet;

use cl_core::CalendarDate;

/// Where a batch stands (the engine's `status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BatchStatus {
    /// Grouped, not committed.
    Draft,
    /// The commit job is running.
    Committing,
    /// Posted.
    Committed,
    /// The commit failed; nothing was posted.
    Failed,
    /// The undo job is running.
    Undoing,
    /// Undone.
    Undone,
}

impl BatchStatus {
    /// Reads the engine's status.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<BatchStatus> {
        Some(match s {
            "draft" => BatchStatus::Draft,
            "committing" => BatchStatus::Committing,
            "committed" => BatchStatus::Committed,
            "failed" => BatchStatus::Failed,
            "undoing" => BatchStatus::Undoing,
            "undone" => BatchStatus::Undone,
            _ => return None,
        })
    }

    /// A stable key (`imports.status.<key>`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            BatchStatus::Draft => "draft",
            BatchStatus::Committing => "committing",
            BatchStatus::Committed => "committed",
            BatchStatus::Failed => "failed",
            BatchStatus::Undoing => "undoing",
            BatchStatus::Undone => "undone",
        }
    }

    /// A job is running (Check status polls).
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, BatchStatus::Committing | BatchStatus::Undoing)
    }
}

/// One History row, as the screen built it from the engine's batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The batch id (text).
    pub id: String,
    /// The source key ([`super::wizard::Source::key`]).
    pub source: &'static str,
    /// Where it stands.
    pub status: BatchStatus,
    /// When it was made.
    pub created: Option<CalendarDate>,
    /// The conversion date.
    pub conversion_date: Option<CalendarDate>,
    /// The tie-out was verified.
    pub verified: bool,
    /// The before-import backup's file name.
    pub backup_ref: Option<String>,
}

/// What a row offers. Buttons a non-Admin can't use are hidden (plan §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools, reason = "independent buttons")]
pub struct Actions {
    /// Open the tie-out.
    pub tieout: bool,
    /// Mark verified (Admin).
    pub verify: bool,
    /// Undo (Admin, after a backup).
    pub undo: bool,
    /// Retry a failed commit with a new key (Admin).
    pub retry: bool,
    /// Read the batch again while a job runs.
    pub check_status: bool,
    /// Restore the before-import backup instead.
    pub restore: bool,
}

/// The actions of `e` for a user who is (or is not) the company Admin.
#[must_use]
pub fn actions(e: &Entry, admin: bool) -> Actions {
    let committed = e.status == BatchStatus::Committed;
    Actions {
        tieout: committed,
        verify: admin && committed && !e.verified,
        undo: admin && committed,
        retry: admin && e.status == BatchStatus::Failed,
        check_status: e.status.is_running(),
        restore: admin && committed && e.backup_ref.is_some(),
    }
}

/// Newest first (by creation date, then id for a stable order).
pub fn sort_newest_first(entries: &mut [Entry]) {
    entries.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
}

/// The backups retention must keep: those of batches still undoable
/// (committed, verified or not), running, or failed (the backup is kept
/// after a failure, plan §4). Undone batches and drafts need none.
#[must_use]
pub fn protected_backups(entries: &[Entry]) -> BTreeSet<String> {
    entries
        .iter()
        .filter(|e| !matches!(e.status, BatchStatus::Undone | BatchStatus::Draft))
        .filter_map(|e| e.backup_ref.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, status: BatchStatus, verified: bool, created: &str) -> Entry {
        Entry {
            id: id.into(),
            source: "qbd",
            status,
            created: CalendarDate::parse_wire(created).ok(),
            conversion_date: None,
            verified,
            backup_ref: Some(format!("{id}.clbackup")),
        }
    }

    #[test]
    fn statuses_round_trip() {
        for s in [
            "draft",
            "committing",
            "committed",
            "failed",
            "undoing",
            "undone",
        ] {
            assert_eq!(BatchStatus::from_wire(s).map(BatchStatus::key), Some(s));
        }
        assert_eq!(BatchStatus::from_wire("posted"), None);
        assert!(BatchStatus::Undoing.is_running());
    }

    #[test]
    fn admins_get_the_buttons_others_only_look() {
        let e = entry("a", BatchStatus::Committed, false, "2026-11-01");
        let a = actions(&e, true);
        assert!(a.tieout && a.verify && a.undo && a.restore && !a.retry);
        let viewer = actions(&e, false);
        assert!(viewer.tieout && !viewer.verify && !viewer.undo && !viewer.restore);
        let verified = entry("b", BatchStatus::Committed, true, "2026-11-01");
        assert!(!actions(&verified, true).verify);
        assert!(
            actions(&verified, true).undo,
            "undo until something is used"
        );
        let failed = entry("c", BatchStatus::Failed, false, "2026-11-01");
        assert!(actions(&failed, true).retry && !actions(&failed, true).undo);
        let running = entry("d", BatchStatus::Committing, false, "2026-11-01");
        assert_eq!(
            actions(&running, true),
            Actions {
                check_status: true,
                ..Actions::default()
            }
        );
        assert_eq!(
            actions(&entry("e", BatchStatus::Undone, false, "2026-11-01"), true),
            Actions::default()
        );
    }

    #[test]
    fn newest_first() {
        let mut l = vec![
            entry("a", BatchStatus::Undone, false, "2026-10-01"),
            entry("b", BatchStatus::Committed, false, "2026-11-01"),
            entry("c", BatchStatus::Failed, false, "2026-10-15"),
        ];
        sort_newest_first(&mut l);
        let ids: Vec<_> = l.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["b", "c", "a"]);
    }

    #[test]
    fn backups_of_unverified_or_undoable_batches_are_kept() {
        let l = vec![
            entry("committed", BatchStatus::Committed, true, "2026-11-01"),
            entry("unverified", BatchStatus::Committed, false, "2026-11-01"),
            entry("failed", BatchStatus::Failed, false, "2026-11-01"),
            entry("done", BatchStatus::Undone, true, "2026-11-01"),
            entry("draft", BatchStatus::Draft, false, "2026-11-01"),
        ];
        let kept = protected_backups(&l);
        let want: BTreeSet<String> = ["committed", "unverified", "failed"]
            .iter()
            .map(|s| format!("{s}.clbackup"))
            .collect();
        assert_eq!(kept, want);
    }
}
