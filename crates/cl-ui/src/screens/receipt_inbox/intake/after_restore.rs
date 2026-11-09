//! After a restore (PR-A1 S2, Q-PRA1c) and N7.
//!
//! - **Bring back**: when the open company was restored on this computer
//!   ([`cl_settings::RestoreNote`]) and the owner hasn't answered yet, the
//!   documents the replaced company copied in **after the backup was
//!   made** (its `seen` hashes newer than the backup) and filed in
//!   `Processed/` since are listed: they are not in the restored books and
//!   not in Review Documents. Review Documents asks "Bring them back to
//!   Review Documents?"; only a yes copies them back to the folder root
//!   (the inbox then reads them again); a no is remembered. Nothing moves
//!   without the owner's yes.
//! - **N7**: a document whose original already went to `Processed/` and
//!   was then discarded in Review Documents (after a Bank Feed Undo) is
//!   refiled in `Discarded/`, found by its bytes.
//!
//! File work runs on its own thread; nothing here logs a file name.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use cl_settings::inbox_state::company_key;

use super::Intake;
use crate::company::CompanyLink;

/// A handled original found again: (folder root, its path, SHA-256).
pub(crate) type Found = (PathBuf, PathBuf, String);

/// Where the bring-back stands.
#[derive(Default)]
pub(crate) struct AfterRestore {
    /// Looked once this session.
    looked: bool,
    look: Option<Receiver<Vec<Found>>>,
    /// Documents to offer back (banner up while not empty).
    pub found: Vec<Found>,
    copy: Option<Receiver<usize>>,
    /// Copied back after a yes (control channel).
    pub brought: Option<usize>,
    refile: Option<Receiver<usize>>,
    /// Originals refiled in `Discarded/` (N7).
    pub refiled: usize,
}

impl Intake {
    /// The roots of the folders watched on this computer.
    fn watched_roots(&self) -> Vec<PathBuf> {
        self.runs
            .iter()
            .filter_map(|r| r.watched.as_ref().map(|(p, _)| p.clone()))
            .collect()
    }

    /// Once the folders are watched: the documents to offer back, if this
    /// company was restored here and the owner hasn't answered.
    pub(super) fn look_after_restore(&mut self, link: &CompanyLink) {
        if self.after_restore.looked || self.after_restore.look.is_some() {
            return;
        }
        let roots = self.watched_roots();
        let Some(tenant) = self.tenant else {
            return;
        };
        if roots.is_empty() {
            return;
        }
        self.after_restore.looked = true;
        let Ok(list) = link.store().load() else {
            return;
        };
        let Some(note) = list.get(tenant).and_then(|c| c.restored.clone()) else {
            return;
        };
        let (Some(previous), Some(made)) = (note.previous, note.backup_made) else {
            return;
        };
        if note.bring_back_answered {
            return;
        }
        let made = made.unix_millis();
        let here: BTreeSet<&str> = self
            .state
            .seen
            .get(&self.company)
            .map(|s| s.iter().map(|f| f.sha256.as_str()).collect())
            .unwrap_or_default();
        let wanted: BTreeSet<String> = self
            .state
            .seen
            .get(&company_key(&previous.to_string()))
            .map(|s| {
                s.iter()
                    .filter(|f| f.at > made && !here.contains(f.sha256.as_str()))
                    .map(|f| f.sha256.clone())
                    .collect()
            })
            .unwrap_or_default();
        if wanted.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            for root in roots {
                for (sha, path) in cl_company::intake::find_processed(&root, &wanted) {
                    if !out.iter().any(|(_, _, s): &Found| *s == sha) {
                        out.push((root.clone(), path, sha));
                    }
                }
            }
            let _ = tx.send(out);
        });
        self.after_restore.look = Some(rx);
    }

    /// Each frame: the look, the copies and the refiles answer.
    pub(super) fn poll_after_restore(&mut self) {
        let a = &mut self.after_restore;
        if let Some(rx) = &a.look
            && let Ok(found) = rx.try_recv()
        {
            a.found = found;
            a.look = None;
        }
        if let Some(rx) = &a.copy
            && let Ok(n) = rx.try_recv()
        {
            a.brought = Some(n);
            a.copy = None;
        }
        if let Some(rx) = &a.refile
            && let Ok(n) = rx.try_recv()
        {
            a.refiled += n;
            a.refile = None;
        }
    }

    /// The owner answered "Bring them back to Review Documents?": yes
    /// copies them to their folders (the inbox reads them again); either
    /// way the question isn't asked again for this restore.
    pub(crate) fn answer_bring_back(&mut self, link: &CompanyLink, yes: bool) {
        let found = std::mem::take(&mut self.after_restore.found);
        if let Some(t) = self.tenant {
            let _ = link.store().update(|l| {
                if let Some(r) = l.get_mut(t).and_then(|c| c.restored.as_mut()) {
                    r.bring_back_answered = true;
                }
            });
        }
        if !yes || found.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let n = found
                .iter()
                .filter(|(root, path, sha)| cl_company::intake::copy_back(root, path, sha).is_ok())
                .count();
            let _ = tx.send(n);
        });
        self.after_restore.copy = Some(rx);
        self.poke();
    }

    /// N7: a discarded document whose original is in `Processed/` (no
    /// waiting entry holds it) goes to `Discarded/`, found by its bytes.
    pub(crate) fn refile_discarded(&mut self, sha256: &str) {
        let roots = self.watched_roots();
        if roots.is_empty() || sha256.is_empty() {
            return;
        }
        let wanted = BTreeSet::from([sha256.to_string()]);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut n = 0;
            for root in roots {
                for (sha, path) in cl_company::intake::find_processed(&root, &wanted) {
                    if cl_company::intake::refile_discarded(&root, &path, &sha).is_ok() {
                        n += 1;
                    }
                }
            }
            let _ = tx.send(n);
        });
        self.after_restore.refile = Some(rx);
    }

    /// The bring-back question is up (Review Documents' banner).
    #[cfg(test)]
    pub(crate) fn bring_back_count(&self) -> usize {
        self.after_restore.found.len()
    }

    /// The file names of the documents offered back (the banner names
    /// them; never logged).
    pub(crate) fn bring_back_names(&self) -> Vec<String> {
        self.after_restore
            .found
            .iter()
            .map(|(_, path, _)| {
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// File work in flight (tests wait for it).
    pub(crate) fn after_restore_busy(&self) -> bool {
        let a = &self.after_restore;
        a.look.is_some() || a.copy.is_some() || a.refile.is_some()
    }
}
