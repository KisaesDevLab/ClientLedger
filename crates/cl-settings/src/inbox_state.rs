//! What the receipt inbox remembers between runs (P17 WP5, Q-P17p):
//! `inbox-state.json` in the settings folder, one list per company (keyed
//! by a hash of the company's id).
//!
//! An entry is a file of the inbox folder that was uploaded to the engine:
//! - **uploaded** (`linked == false`): the file went up as a form's draft
//!   ("New bill", "New check"). It stays in the inbox and is offered again
//!   until a saved record holds the upload; the owner may still abandon the
//!   form.
//! - **linked** (`linked == true`): a record holds the upload, but moving the
//!   original to `Filed/` failed. It is never offered again, and the move is
//!   retried.
//!
//! Entries are checked against the engine (every attachment id still
//! there?) when the inbox starts and after a restore, so ids a restored
//! company no longer has cannot hide a file for ever.
//!
//! Nothing here names a file: paths are kept only as a SHA-256 hash.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::{read_capped, write_atomic};

/// Entries kept per company (the oldest uploaded ones go first).
pub const MAX_ENTRIES: usize = 1000;

/// Upload ids kept per entry (re-uploads of an abandoned form's file).
pub const MAX_UPLOADS: usize = 20;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The key a company's entries are kept under: a hash of its id.
#[must_use]
pub fn company_key(tenant: &str) -> String {
    hex(&Sha256::digest(
        format!("clientledger-inbox:{tenant}").as_bytes(),
    ))
}

/// The hash a file's path is kept as (case-folded on Windows, where paths
/// are not case-sensitive).
#[must_use]
pub fn path_hash(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.into_owned()
    };
    hex(&Sha256::digest(text.as_bytes()))
}

/// Which file, as the inbox saw it when it was read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIdentity {
    /// [`path_hash`] of its path.
    pub path_hash: String,
    /// Size in bytes.
    pub size: u64,
    /// Modified time, milliseconds since 1970.
    pub modified_ms: i64,
    /// SHA-256 of its bytes, lower-case hex.
    pub sha256: String,
}

impl FileIdentity {
    /// True when a file at `path_hash` with this size and time is this one.
    #[must_use]
    pub fn matches(&self, path_hash: &str, size: u64, modified_ms: i64) -> bool {
        self.path_hash == path_hash && self.size == size && self.modified_ms == modified_ms
    }
}

/// One uploaded inbox file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxEntry {
    /// The file.
    #[serde(flatten)]
    pub file: FileIdentity,
    /// The engine's attachment ids of its uploads (UUID text), oldest first.
    pub attachments: Vec<String>,
    /// A saved record holds one of them; the original still has to move.
    pub linked: bool,
}

/// Every company's entries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InboxState {
    /// Format version.
    pub version: u32,
    /// Entries by [`company_key`].
    pub companies: BTreeMap<String, Vec<InboxEntry>>,
}

impl InboxState {
    /// Parses the file's text; anything unreadable is an empty state (the
    /// inbox then offers its files again, which loses nothing).
    #[must_use]
    pub fn parse(text: &str) -> InboxState {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// The file's text.
    #[must_use]
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// A company's entries.
    #[must_use]
    pub fn entries(&self, company: &str) -> &[InboxEntry] {
        self.companies.get(company).map_or(&[], Vec::as_slice)
    }

    /// The entry of the file at `path_hash` with this size and time.
    #[must_use]
    pub fn find(
        &self,
        company: &str,
        path_hash: &str,
        size: u64,
        modified_ms: i64,
    ) -> Option<&InboxEntry> {
        self.entries(company)
            .iter()
            .find(|e| e.file.matches(path_hash, size, modified_ms))
    }

    /// Records an upload of `file` (`attachment` is the engine's id).
    pub fn record_upload(&mut self, company: &str, file: &FileIdentity, attachment: &str) {
        let list = self.companies.entry(company.to_string()).or_default();
        if let Some(e) = list.iter_mut().find(|e| e.file == *file) {
            if !e.attachments.iter().any(|a| a == attachment) {
                e.attachments.push(attachment.to_string());
                if e.attachments.len() > MAX_UPLOADS {
                    e.attachments.remove(0);
                }
            }
            return;
        }
        list.push(InboxEntry {
            file: file.clone(),
            attachments: vec![attachment.to_string()],
            linked: false,
        });
        while list.len() > MAX_ENTRIES {
            // The oldest still-unlinked entry goes first; its file is only
            // offered again.
            let at = list.iter().position(|e| !e.linked).unwrap_or(0);
            list.remove(at);
        }
    }

    /// A saved record holds `file`'s upload; the move is pending.
    pub fn mark_linked(&mut self, company: &str, file: &FileIdentity) {
        if let Some(e) = self
            .companies
            .get_mut(company)
            .and_then(|l| l.iter_mut().find(|e| e.file == *file))
        {
            e.linked = true;
        }
    }

    /// Forgets `file` (it moved to `Filed/`, or it is gone).
    pub fn remove(&mut self, company: &str, file: &FileIdentity) {
        if let Some(l) = self.companies.get_mut(company) {
            l.retain(|e| e.file != *file);
            if l.is_empty() {
                self.companies.remove(company);
            }
        }
    }

    /// Forgets an upload the engine no longer has (after a restore, or a
    /// draft it cleaned up). An entry left with no uploads is dropped, so
    /// its file is offered again.
    pub fn drop_attachment(&mut self, company: &str, attachment: &str) {
        if let Some(l) = self.companies.get_mut(company) {
            for e in l.iter_mut() {
                e.attachments.retain(|a| a != attachment);
            }
            l.retain(|e| !e.attachments.is_empty());
            if l.is_empty() {
                self.companies.remove(company);
            }
        }
    }
}

/// Why the file could not be read or written.
#[derive(Debug, thiserror::Error)]
#[error("inbox state could not be {action}")]
pub struct InboxStateError {
    action: &'static str,
    #[source]
    source: std::io::Error,
}

/// `<settings dir>/inbox-state.json`.
#[derive(Debug, Clone)]
pub struct InboxStateStore {
    path: PathBuf,
}

impl InboxStateStore {
    /// The store in the settings folder `dir`.
    #[must_use]
    pub fn in_dir(dir: &Path) -> InboxStateStore {
        InboxStateStore {
            path: dir.join("inbox-state.json"),
        }
    }

    /// The file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the state (empty when missing).
    ///
    /// # Errors
    /// When the file exists but cannot be read.
    pub fn load(&self) -> Result<InboxState, InboxStateError> {
        let text = read_capped(&self.path).map_err(|source| InboxStateError {
            action: "read",
            source,
        })?;
        Ok(text.map(|t| InboxState::parse(&t)).unwrap_or_default())
    }

    /// Saves the state atomically.
    ///
    /// # Errors
    /// When the file cannot be written.
    pub fn save(&self, state: &InboxState) -> Result<(), InboxStateError> {
        let mut state = state.clone();
        state.version = 1;
        write_atomic(&self.path, &state.to_text()).map_err(|source| InboxStateError {
            action: "saved",
            source,
        })
    }

    /// Loads, applies `f`, saves.
    ///
    /// # Errors
    /// From either side.
    pub fn update<T>(&self, f: impl FnOnce(&mut InboxState) -> T) -> Result<T, InboxStateError> {
        let mut state = self.load()?;
        let out = f(&mut state);
        self.save(&state)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(n: u8) -> FileIdentity {
        FileIdentity {
            path_hash: path_hash(Path::new(&format!("/inbox/r{n}.pdf"))),
            size: 100,
            modified_ms: 1_700_000_000_000,
            sha256: format!("{n:064}"),
        }
    }

    #[test]
    fn uploads_link_and_go() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = InboxStateStore::in_dir(dir.path());
        let c = company_key("d0000000-0000-4000-8000-000000000001");
        let f = file(1);
        store
            .update(|s| {
                s.record_upload(&c, &f, "a1");
                s.record_upload(&c, &f, "a1");
                s.record_upload(&c, &f, "a2");
            })
            .expect("save");
        let s = store.load().expect("load");
        let e = s
            .find(&c, &f.path_hash, f.size, f.modified_ms)
            .expect("entry");
        assert_eq!(e.attachments, ["a1", "a2"]);
        assert!(!e.linked);
        assert!(
            s.find(&c, &f.path_hash, f.size + 1, f.modified_ms)
                .is_none()
        );
        assert!(s.entries(&company_key("other")).is_empty(), "per company");

        store.update(|s| s.mark_linked(&c, &f)).expect("save");
        assert!(store.load().expect("load").entries(&c)[0].linked);
        store.update(|s| s.remove(&c, &f)).expect("save");
        assert!(store.load().expect("load").companies.is_empty());
    }

    #[test]
    fn stale_uploads_are_dropped() {
        let mut s = InboxState::default();
        let c = company_key("t");
        s.record_upload(&c, &file(1), "a1");
        s.record_upload(&c, &file(1), "a2");
        s.record_upload(&c, &file(2), "b1");
        s.mark_linked(&c, &file(2));
        s.drop_attachment(&c, "a1");
        assert_eq!(s.entries(&c)[0].attachments, ["a2"]);
        // A restored company without the linked upload: the file is
        // offered again instead of hidden for ever.
        s.drop_attachment(&c, "b1");
        assert_eq!(s.entries(&c).len(), 1);
        s.drop_attachment(&c, "a2");
        assert!(s.companies.is_empty());
    }

    #[test]
    fn the_file_names_nothing_and_survives_garbage() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = InboxStateStore::in_dir(dir.path());
        let c = company_key("t");
        let f = FileIdentity {
            path_hash: path_hash(Path::new("/inbox/Secret Vendor invoice.pdf")),
            size: 1,
            modified_ms: 1,
            sha256: "ab".into(),
        };
        store
            .update(|s| s.record_upload(&c, &f, "a"))
            .expect("save");
        let text = std::fs::read_to_string(store.path()).expect("read");
        assert!(!text.contains("Secret"), "{text}");
        assert!(!text.contains("inbox"), "{text}");
        std::fs::write(store.path(), "not json").expect("write");
        assert_eq!(store.load().expect("load"), InboxState::default());
        assert_eq!(path_hash(Path::new("/a")), path_hash(Path::new("/a")));
        assert_ne!(path_hash(Path::new("/a")), path_hash(Path::new("/b")));
    }

    #[test]
    fn entries_are_capped() {
        let mut s = InboxState::default();
        let c = company_key("t");
        let mut first = file(0);
        first.path_hash = "linked".into();
        s.record_upload(&c, &first, "x");
        s.mark_linked(&c, &first);
        for n in 0..=MAX_ENTRIES {
            let mut f = file(1);
            f.path_hash = format!("p{n}");
            s.record_upload(&c, &f, "a");
        }
        assert_eq!(s.entries(&c).len(), MAX_ENTRIES);
        assert!(s.entries(&c)[0].linked, "a linked entry is kept");
        let f = file(3);
        for n in 0..(MAX_UPLOADS + 2) {
            s.record_upload(&c, &f, &format!("u{n}"));
        }
        let e = s.entries(&c).iter().find(|e| e.file.sha256 == f.sha256);
        assert_eq!(e.map(|e| e.attachments.len()), Some(MAX_UPLOADS));
    }
}
