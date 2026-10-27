//! What the document intake remembers between runs (P17 WP5, Q-P17p;
//! v2 for A01 plan B6, D4, D5): `inbox-state.json` in the settings
//! folder, one list per company (keyed by a hash of the company's id).
//!
//! An entry is a file of an intake folder on its way into the company:
//! - **uploading**: its draft key and upload key were saved before the
//!   upload was sent, so a lost reply is resolved after a restart by
//!   listing that draft (found: arrived; none: retried with the same key).
//! - **arrived**: copied in as a draft attachment; waiting to be handled.
//! - **duplicate**: the same bytes were copied in before (the `seen`
//!   index); listed once with Discard / Copy anyway, never uploaded on its
//!   own.
//! - **linked**: a saved record holds the upload; the original still has
//!   to move to `Processed/` (retried).
//! - **discarding**: the owner discarded it; the original still has to move
//!   to `Discarded/`, then the engine's copy is deleted.
//! - **kept**: handled in a "leave originals where they are" folder; the
//!   file is hidden from the list by its identity.
//!
//! The status moves only forward ([`EntryStatus::can_become`]) and each
//! step is saved before the next side effect, so a crash at any point
//! resumes.
//!
//! The `seen` index (per company) holds the SHA-256 of every file copied
//! in, so the same bytes saved again are flagged once (Q-A01g).
//!
//! Version 1 (P17) files are upgraded when read: entries get no folder
//! (`None`: the legacy single folder, rewritten to the migrated folder's
//! id by the migration), a `linked` entry is `linked`, any other is
//! `arrived` with its last upload.
//!
//! Version 3 (A04 D1) adds each entry's engine document draft
//! ([`DraftLink`], `docDraft`, absent when there is none): the claim is
//! saved as `claimSent` before it is sent, then the entry `own`s the draft
//! that reads its upload, or `follows` the draft another computer made of
//! the same bytes (the entry is then `duplicate`; its own extra copy in the
//! engine is deleted, the entry and the seen hash are kept). Version 2
//! files are read with no drafts (their arrivals are claimed again, and a
//! claim already made is adopted). An older app reading a version 3 file
//! ignores `docDraft`; it never adds a status, so it reads every entry.
//!
//! Nothing here names a file: paths are kept only as a SHA-256 hash.
//!
//! Caps: [`MAX_ENTRIES`] entries and [`MAX_SEEN`] seen hashes per company,
//! and the file stays under [`MAX_TEXT_BYTES`] (below the settings
//! store's 4 MB read cap).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cl_core::ids::IntakeFolderId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::{MAX_BYTES, read_capped, write_atomic};

/// The format written.
pub const VERSION: u32 = 3;

/// Entries kept per company (finished ones go first).
pub const MAX_ENTRIES: usize = 2000;

/// Upload ids kept per entry (re-uploads of an abandoned form's file).
pub const MAX_UPLOADS: usize = 20;

/// Seen hashes kept per company (the oldest go first).
pub const MAX_SEEN: usize = 10_000;

/// The largest file written (bytes of JSON text).
pub const MAX_TEXT_BYTES: usize = 3 * 1024 * 1024;

const _: () = assert!((MAX_TEXT_BYTES as u64) < MAX_BYTES);

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

/// Where an entry stands (see the module notes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryStatus {
    /// The upload was sent (draft and upload keys saved first).
    Uploading,
    /// Copied in as a draft attachment.
    #[default]
    Arrived,
    /// The same bytes were copied in before.
    Duplicate,
    /// A saved record holds the upload; the original moves to `Processed/`.
    Linked,
    /// Discarded; the original moves to `Discarded/`, then the engine's
    /// copy is deleted.
    Discarding,
    /// Handled in a folder that keeps its originals.
    Kept,
}

impl EntryStatus {
    /// A short name for the control channel.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EntryStatus::Uploading => "uploading",
            EntryStatus::Arrived => "arrived",
            EntryStatus::Duplicate => "duplicate",
            EntryStatus::Linked => "linked",
            EntryStatus::Discarding => "discarding",
            EntryStatus::Kept => "kept",
        }
    }

    /// True when an entry may go from `self` to `next` (staying put is
    /// always allowed, so a retried step is harmless):
    /// - uploading → arrived (the upload is there), or back to duplicate
    ///   (Copy anyway that was never sent);
    /// - arrived → linked, discarding, kept, or duplicate (another
    ///   computer's draft holds the same bytes, A04 D1);
    /// - duplicate → uploading (Copy anyway), discarding, kept (hidden in a
    ///   folder that keeps its originals), linked (the draft it follows
    ///   was entered);
    /// - discarding → linked or kept: the upload turned out to be on a
    ///   saved record, so it is not deleted (A01 review B1);
    /// - linked and kept are only left by removing the entry.
    #[must_use]
    pub fn can_become(self, next: EntryStatus) -> bool {
        use EntryStatus::{Arrived, Discarding, Duplicate, Kept, Linked, Uploading};
        self == next
            || matches!(
                (self, next),
                (Uploading, Arrived | Duplicate)
                    | (Arrived, Linked | Discarding | Kept | Duplicate)
                    | (Duplicate, Uploading | Discarding | Kept | Linked)
                    | (Discarding, Linked | Kept)
            )
    }

    /// True while a side effect is still owed (upload, move, delete): such
    /// an entry is never dropped by the cap.
    #[must_use]
    pub fn pending(self) -> bool {
        matches!(
            self,
            EntryStatus::Uploading | EntryStatus::Linked | EntryStatus::Discarding
        )
    }
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// An entry's engine document draft (A04 D1, version 3). Ids are the
/// engine's draft ids (UUID text). Claim keys are never stored: keys can't
/// outlive the app, so a `ClaimSent` entry is claimed again after a
/// restart with a fresh key, and the draft that claim made is adopted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DraftLink {
    /// No draft (the engine has no document drafts, or not claimed yet).
    #[default]
    None,
    /// The claim was saved and sent; its answer isn't known yet. Treated
    /// as claimed: the entry never takes the plain P17 paths.
    ClaimSent,
    /// This computer's claim: the draft reads this entry's upload.
    Own(String),
    /// Another computer's draft holds the same bytes; this entry (a
    /// `duplicate`) follows it: entered there → `linked`, discarded there
    /// → `discarding`. Its own extra copy in the engine is deleted.
    Follows(String),
}

impl DraftLink {
    /// True for [`DraftLink::None`].
    #[must_use]
    pub fn is_none(&self) -> bool {
        *self == DraftLink::None
    }

    /// True for every link but [`DraftLink::None`]: the entry is claimed
    /// (or its claim's answer is not known), so it never takes the plain
    /// P17 save or link paths (M2 b).
    #[must_use]
    pub fn is_claimed(&self) -> bool {
        !self.is_none()
    }

    /// The draft id of [`DraftLink::Own`] or [`DraftLink::Follows`].
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        match self {
            DraftLink::Own(id) | DraftLink::Follows(id) => Some(id),
            DraftLink::None | DraftLink::ClaimSent => None,
        }
    }

    /// A short name for the control channel.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            DraftLink::None => "none",
            DraftLink::ClaimSent => "claim_sent",
            DraftLink::Own(_) => "own",
            DraftLink::Follows(_) => "follows",
        }
    }
}

/// One intake file on its way into the company.
///
/// `attachments` and `linked` are the P17 fields (still used by the P17
/// receipt inbox screen); `linked` always equals `status == Linked`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxEntry {
    /// The intake folder (`None`: the P17 single folder, until migrated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<IntakeFolderId>,
    /// The file.
    #[serde(flatten)]
    pub file: FileIdentity,
    /// The engine's attachment ids of its uploads (UUID text), oldest
    /// first; the last is the current one ([`InboxEntry::attachment`]).
    #[serde(default)]
    pub attachments: Vec<String>,
    /// A saved record holds one of them; the original still has to move.
    #[serde(default, skip_serializing_if = "is_false")]
    pub linked: bool,
    /// Where it stands.
    #[serde(default)]
    pub status: EntryStatus,
    /// The draft key of its upload (UUID text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// The idempotency key of its upload (UUID text), reused on a retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_key: Option<String>,
    /// Its engine document draft (version 3; see [`DraftLink`]).
    #[serde(default, skip_serializing_if = "DraftLink::is_none")]
    pub doc_draft: DraftLink,
}

impl InboxEntry {
    fn new(folder: Option<IntakeFolderId>, file: &FileIdentity, status: EntryStatus) -> Self {
        InboxEntry {
            folder,
            file: file.clone(),
            attachments: Vec::new(),
            linked: status == EntryStatus::Linked,
            status,
            draft: None,
            upload_key: None,
            doc_draft: DraftLink::None,
        }
    }

    /// The current upload's attachment id.
    #[must_use]
    pub fn attachment(&self) -> Option<&str> {
        self.attachments.last().map(String::as_str)
    }

    /// True while it still owes a side effect (an upload, a move, a delete,
    /// a claim whose answer isn't known, or deleting its extra copy while
    /// it follows another computer's draft): never dropped for room.
    #[must_use]
    pub fn owes(&self) -> bool {
        self.status.pending()
            || self.doc_draft == DraftLink::ClaimSent
            || matches!(self.doc_draft, DraftLink::Follows(_)) && !self.attachments.is_empty()
    }

    fn set(&mut self, status: EntryStatus) {
        self.status = status;
        self.linked = status == EntryStatus::Linked;
    }
}

/// A file copied in before (by its bytes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenFile {
    /// SHA-256 of its bytes, lower-case hex.
    pub sha256: String,
    /// The attachment it became (UUID text).
    pub attachment: String,
    /// When, milliseconds since 1970 ("Same as a file copied on <date>").
    pub at: i64,
}

/// Every company's entries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InboxState {
    /// Format version (1: P17; 2: A01; 3: A04 drafts).
    pub version: u32,
    /// Entries by [`company_key`].
    pub companies: BTreeMap<String, Vec<InboxEntry>>,
    /// Files copied in, by [`company_key`], oldest first.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub seen: BTreeMap<String, Vec<SeenFile>>,
}

impl InboxState {
    /// Parses the file's text, upgrading a version 1 file; anything
    /// unreadable is an empty state (the inbox then offers its files
    /// again, which loses nothing).
    #[must_use]
    pub fn parse(text: &str) -> InboxState {
        let mut s: InboxState = serde_json::from_str(text).unwrap_or_default();
        if s.version < 2 {
            for e in s.companies.values_mut().flatten() {
                let status = if e.linked {
                    EntryStatus::Linked
                } else {
                    EntryStatus::Arrived
                };
                e.set(status);
            }
        } else {
            for e in s.companies.values_mut().flatten() {
                e.linked = e.status == EntryStatus::Linked;
            }
        }
        s.version = VERSION;
        s.sanitize();
        s
    }

    /// What a file edited by hand could put in that the app never writes
    /// (security review L9): ids that aren't UUIDs go (an entry left with
    /// no upload but needing one is dropped, so its file is offered
    /// again), and every cap that saving keeps holds on load too.
    fn sanitize(&mut self) {
        let uuid = |s: &str| cl_core::ids::parse_uuid(s).is_ok();
        for list in self.companies.values_mut() {
            list.retain_mut(|e| {
                e.attachments.retain(|a| uuid(a));
                let over = e.attachments.len().saturating_sub(MAX_UPLOADS);
                e.attachments.drain(..over);
                e.draft = e.draft.take().filter(|d| uuid(d));
                e.upload_key = e.upload_key.take().filter(|k| uuid(k));
                // A draft id that isn't a UUID: an arrival is claimed again
                // (the engine names its draft, which is adopted); anything
                // else forgets it.
                if e.doc_draft.id().is_some_and(|d| !uuid(d)) {
                    e.doc_draft = if e.status == EntryStatus::Arrived {
                        DraftLink::ClaimSent
                    } else {
                        DraftLink::None
                    };
                }
                // Only an arrival waits for a claim's answer, and only a
                // duplicate follows another computer's draft.
                match (&e.doc_draft, e.status) {
                    (DraftLink::ClaimSent, s) if s != EntryStatus::Arrived => {
                        e.doc_draft = DraftLink::None;
                    }
                    (DraftLink::Follows(_), s)
                        if !matches!(
                            s,
                            EntryStatus::Duplicate
                                | EntryStatus::Linked
                                | EntryStatus::Discarding
                                | EntryStatus::Kept
                        ) =>
                    {
                        e.doc_draft = DraftLink::None;
                    }
                    _ => {}
                }
                // Waiting for the owner means having an upload to show.
                !(e.status == EntryStatus::Arrived && e.attachments.is_empty())
            });
            cap_entries(list);
        }
        self.companies.retain(|_, l| !l.is_empty());
        for list in self.seen.values_mut() {
            list.retain(|f| uuid(&f.attachment));
            let over = list.len().saturating_sub(MAX_SEEN);
            list.drain(..over);
        }
        self.seen.retain(|_, l| !l.is_empty());
        self.fit();
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

    /// A company's entries of one folder (`None`: the P17 folder).
    pub fn entries_in(
        &self,
        company: &str,
        folder: Option<IntakeFolderId>,
    ) -> impl Iterator<Item = &InboxEntry> {
        self.entries(company)
            .iter()
            .filter(move |e| e.folder == folder)
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

    /// The entry of exactly `file`.
    #[must_use]
    pub fn entry(&self, company: &str, file: &FileIdentity) -> Option<&InboxEntry> {
        self.entries(company).iter().find(|e| e.file == *file)
    }

    fn entry_mut(&mut self, company: &str, file: &FileIdentity) -> Option<&mut InboxEntry> {
        self.companies
            .get_mut(company)
            .and_then(|l| l.iter_mut().find(|e| e.file == *file))
    }

    fn push(&mut self, company: &str, entry: InboxEntry) {
        let list = self.companies.entry(company.to_string()).or_default();
        list.push(entry);
        cap_entries(list);
    }

    /// Records an upload of `file` (`attachment` is the engine's id) — the
    /// P17 receipt inbox: the entry is `arrived` (the legacy folder).
    pub fn record_upload(&mut self, company: &str, file: &FileIdentity, attachment: &str) {
        if let Some(e) = self.entry_mut(company, file) {
            if !e.attachments.iter().any(|a| a == attachment) {
                e.attachments.push(attachment.to_string());
                if e.attachments.len() > MAX_UPLOADS {
                    e.attachments.remove(0);
                }
            }
            return;
        }
        let mut e = InboxEntry::new(None, file, EntryStatus::Arrived);
        e.attachments.push(attachment.to_string());
        self.push(company, e);
    }

    /// A saved record holds `file`'s upload; the move is pending.
    pub fn mark_linked(&mut self, company: &str, file: &FileIdentity) {
        if let Some(e) = self.entry_mut(company, file) {
            // A P17 entry (arrived) or a v2 one at any step before the move.
            if e.status.can_become(EntryStatus::Linked) || e.status == EntryStatus::Uploading {
                e.set(EntryStatus::Linked);
            }
        }
    }

    /// Saves, before the upload is sent, that `file` of `folder` goes up
    /// into draft `draft` with idempotency key `upload_key` (a new
    /// `uploading` entry, or a `duplicate` the owner chose to copy anyway).
    /// Returns false (nothing changed) when the entry is elsewhere.
    pub fn begin_upload(
        &mut self,
        company: &str,
        folder: Option<IntakeFolderId>,
        file: &FileIdentity,
        draft: &str,
        upload_key: &str,
    ) -> bool {
        if let Some(e) = self.entry_mut(company, file) {
            // A copy of another computer's draft never goes up again.
            if !e.status.can_become(EntryStatus::Uploading) || e.doc_draft.is_claimed() {
                return false;
            }
            e.set(EntryStatus::Uploading);
            e.draft = Some(draft.to_string());
            e.upload_key = Some(upload_key.to_string());
            return true;
        }
        let mut e = InboxEntry::new(folder, file, EntryStatus::Uploading);
        e.draft = Some(draft.to_string());
        e.upload_key = Some(upload_key.to_string());
        self.push(company, e);
        true
    }

    /// The upload of `file` is in the engine as `attachment` (`at`:
    /// milliseconds since 1970): the entry is `arrived` and its bytes are
    /// seen. Returns false when there is no uploading entry for it.
    pub fn arrived(
        &mut self,
        company: &str,
        file: &FileIdentity,
        attachment: &str,
        at: i64,
    ) -> bool {
        let Some(e) = self.entry_mut(company, file) else {
            return false;
        };
        if e.status != EntryStatus::Uploading && e.status != EntryStatus::Arrived {
            return false;
        }
        e.set(EntryStatus::Arrived);
        if !e.attachments.iter().any(|a| a == attachment) {
            e.attachments.push(attachment.to_string());
            if e.attachments.len() > MAX_UPLOADS {
                e.attachments.remove(0);
            }
        }
        let sha = e.file.sha256.clone();
        self.record_seen(company, &sha, attachment, at);
        true
    }

    /// Lists `file` of `folder` as the same bytes as a file copied in
    /// before (one entry; never uploaded on its own). False when it already
    /// has an entry.
    pub fn mark_duplicate(
        &mut self,
        company: &str,
        folder: Option<IntakeFolderId>,
        file: &FileIdentity,
    ) -> bool {
        if self.entry(company, file).is_some() {
            return false;
        }
        self.push(
            company,
            InboxEntry::new(folder, file, EntryStatus::Duplicate),
        );
        true
    }

    /// Moves `file`'s entry to `status` when [`EntryStatus::can_become`]
    /// allows it; false otherwise (or when there is no entry).
    pub fn set_status(&mut self, company: &str, file: &FileIdentity, status: EntryStatus) -> bool {
        match self.entry_mut(company, file) {
            Some(e) if e.status.can_become(status) => {
                e.set(status);
                true
            }
            _ => false,
        }
    }

    // ---- A04 D1: document drafts --------------------------------------

    /// Saves, before the claim is sent, that `file`'s upload is being
    /// claimed as a document draft. Only an arrival with no draft (or one
    /// whose earlier claim is unanswered) is claimed; false otherwise.
    pub fn begin_claim(&mut self, company: &str, file: &FileIdentity) -> bool {
        match self.entry_mut(company, file) {
            Some(e)
                if e.status == EntryStatus::Arrived
                    && matches!(e.doc_draft, DraftLink::None | DraftLink::ClaimSent)
                    && !e.attachments.is_empty() =>
            {
                e.doc_draft = DraftLink::ClaimSent;
                true
            }
            _ => false,
        }
    }

    /// The claim made draft `draft` of `file`'s upload (or the engine named
    /// the draft an earlier claim of this entry's own upload made): the
    /// entry owns it. False when it isn't an arrival waiting for that.
    pub fn claimed(&mut self, company: &str, file: &FileIdentity, draft: &str) -> bool {
        match self.entry_mut(company, file) {
            Some(e)
                if e.status == EntryStatus::Arrived
                    && match &e.doc_draft {
                        DraftLink::None | DraftLink::ClaimSent => true,
                        DraftLink::Own(id) => id == draft,
                        DraftLink::Follows(_) => false,
                    } =>
            {
                e.doc_draft = DraftLink::Own(draft.to_string());
                true
            }
            _ => false,
        }
    }

    /// Another computer's live draft `draft` holds the same bytes: the
    /// arrival becomes a `duplicate` that follows it (its own copy in the
    /// engine is deleted next; the entry and the seen hash stay). False
    /// when it isn't an arrival waiting for its claim.
    pub fn claimed_elsewhere(&mut self, company: &str, file: &FileIdentity, draft: &str) -> bool {
        match self.entry_mut(company, file) {
            Some(e)
                if e.status == EntryStatus::Arrived
                    && matches!(e.doc_draft, DraftLink::None | DraftLink::ClaimSent) =>
            {
                e.set(EntryStatus::Duplicate);
                e.doc_draft = DraftLink::Follows(draft.to_string());
                true
            }
            _ => false,
        }
    }

    /// The engine no longer has the entry's draft (a restore, or drafts
    /// reset): the link goes. An arrival is then claimed again; a
    /// duplicate stays a plain duplicate.
    pub fn unclaim(&mut self, company: &str, file: &FileIdentity) {
        if let Some(e) = self.entry_mut(company, file) {
            e.doc_draft = DraftLink::None;
        }
    }

    /// The extra copy of an entry that follows another computer's draft
    /// was deleted: its uploads are forgotten, the entry and the seen hash
    /// stay. False (nothing changed) for any other entry.
    pub fn copy_deleted(&mut self, company: &str, file: &FileIdentity) -> bool {
        match self.entry_mut(company, file) {
            Some(e) if matches!(e.doc_draft, DraftLink::Follows(_)) => {
                e.attachments.clear();
                true
            }
            _ => false,
        }
    }

    /// An arrival's upload now sits alone under draft key `draft` (UUID
    /// text): a P17 upload re-keyed before its claim or its form.
    pub fn set_draft_key(&mut self, company: &str, file: &FileIdentity, draft: &str) -> bool {
        match self.entry_mut(company, file) {
            Some(e) if e.status == EntryStatus::Arrived => {
                e.draft = Some(draft.to_string());
                true
            }
            _ => false,
        }
    }

    /// The entry whose uploads include `attachment`.
    #[must_use]
    pub fn by_attachment(&self, company: &str, attachment: &str) -> Option<&InboxEntry> {
        self.entries(company)
            .iter()
            .find(|e| e.attachments.iter().any(|a| a == attachment))
    }

    /// Forgets `file` (it moved, or it is gone).
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
    /// its file is offered again; the bytes are no longer "seen" either.
    pub fn drop_attachment(&mut self, company: &str, attachment: &str) {
        if let Some(l) = self.companies.get_mut(company) {
            l.retain_mut(|e| {
                let had = e.attachments.iter().any(|a| a == attachment);
                e.attachments.retain(|a| a != attachment);
                !(had && e.attachments.is_empty())
            });
            if l.is_empty() {
                self.companies.remove(company);
            }
        }
        if let Some(s) = self.seen.get_mut(company) {
            s.retain(|f| f.attachment != attachment);
            if s.is_empty() {
                self.seen.remove(company);
            }
        }
    }

    /// The file copied in before with these bytes.
    #[must_use]
    pub fn seen(&self, company: &str, sha256: &str) -> Option<&SeenFile> {
        self.seen
            .get(company)
            .and_then(|s| s.iter().rev().find(|f| f.sha256 == sha256))
    }

    /// Remembers that bytes `sha256` were copied in as `attachment`.
    pub fn record_seen(&mut self, company: &str, sha256: &str, attachment: &str, at: i64) {
        let list = self.seen.entry(company.to_string()).or_default();
        if list
            .iter()
            .any(|f| f.sha256 == sha256 && f.attachment == attachment)
        {
            return;
        }
        list.push(SeenFile {
            sha256: sha256.to_string(),
            attachment: attachment.to_string(),
            at,
        });
        if list.len() > MAX_SEEN {
            let over = list.len() - MAX_SEEN;
            list.drain(..over);
        }
    }

    /// The migration (D5): entries of the P17 folder (`folder: None`)
    /// become entries of the migrated folder `id`. Returns how many.
    pub fn assign_legacy_folder(&mut self, company: &str, id: IntakeFolderId) -> usize {
        let mut n = 0;
        if let Some(l) = self.companies.get_mut(company) {
            for e in l.iter_mut().filter(|e| e.folder.is_none()) {
                e.folder = Some(id);
                n += 1;
            }
        }
        n
    }

    /// Drops the oldest seen hashes, then the oldest finished entries,
    /// until the text fits [`MAX_TEXT_BYTES`]. Pending entries are kept.
    pub fn fit(&mut self) {
        while self.to_text().len() > MAX_TEXT_BYTES {
            if let Some(list) = self
                .seen
                .values_mut()
                .max_by_key(|l| l.len())
                .filter(|l| !l.is_empty())
            {
                let n = list.len().div_ceil(10);
                list.drain(..n);
                self.seen.retain(|_, l| !l.is_empty());
                continue;
            }
            let Some(list) = self
                .companies
                .values_mut()
                .filter(|l| l.iter().any(|e| !e.owes()))
                .max_by_key(|l| l.len())
            else {
                return;
            };
            let n = list.len().div_ceil(10);
            let mut dropped = 0;
            list.retain(|e| {
                if dropped < n && !e.owes() {
                    dropped += 1;
                    false
                } else {
                    true
                }
            });
            self.companies.retain(|_, l| !l.is_empty());
        }
    }
}

/// Keeps a company's entries within [`MAX_ENTRIES`]: the oldest finished
/// entry goes first (its file is only offered again). One still owing a
/// side effect (a move, a delete, an upload) is never dropped, even when
/// that leaves the list over the cap; such entries are bounded by the
/// files the folders list.
fn cap_entries(list: &mut Vec<InboxEntry>) {
    while list.len() > MAX_ENTRIES {
        let Some(at) = list.iter().position(|e| !e.owes()) else {
            return;
        };
        list.remove(at);
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

    /// Loads the state (empty when missing; a version 1 file is upgraded).
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

    /// Saves the state atomically as the current version, within the size cap.
    ///
    /// # Errors
    /// When the file cannot be written.
    pub fn save(&self, state: &InboxState) -> Result<(), InboxStateError> {
        let mut state = state.clone();
        state.version = VERSION;
        state.fit();
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

    const FOLDER: &str = "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a6b";
    const A1: &str = "d0000000-0000-4000-8000-0000000000a1";
    const A2: &str = "d0000000-0000-4000-8000-0000000000a2";
    const B1: &str = "d0000000-0000-4000-8000-0000000000b1";
    const ATT1: &str = "d0000000-0000-4000-8000-000000000a77";
    const DRAFT1: &str = "d0000000-0000-4000-8000-00000000d001";
    const KEY1: &str = "d0000000-0000-4000-8000-00000000c001";

    fn folder() -> IntakeFolderId {
        IntakeFolderId::parse(FOLDER).expect("id")
    }

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
                s.record_upload(&c, &f, A1);
                s.record_upload(&c, &f, A1);
                s.record_upload(&c, &f, A2);
            })
            .expect("save");
        let s = store.load().expect("load");
        let e = s
            .find(&c, &f.path_hash, f.size, f.modified_ms)
            .expect("entry");
        assert_eq!(e.attachments, [A1, A2]);
        assert_eq!(e.attachment(), Some(A2));
        assert!(!e.linked);
        assert_eq!(e.status, EntryStatus::Arrived);
        assert!(
            s.find(&c, &f.path_hash, f.size + 1, f.modified_ms)
                .is_none()
        );
        assert!(s.entries(&company_key("other")).is_empty(), "per company");

        store.update(|s| s.mark_linked(&c, &f)).expect("save");
        let e = store.load().expect("load").entries(&c)[0].clone();
        assert!(e.linked);
        assert_eq!(e.status, EntryStatus::Linked);
        store.update(|s| s.remove(&c, &f)).expect("save");
        assert!(store.load().expect("load").companies.is_empty());
    }

    #[test]
    fn stale_uploads_are_dropped() {
        let mut s = InboxState::default();
        let c = company_key("t");
        s.record_upload(&c, &file(1), A1);
        s.record_upload(&c, &file(1), A2);
        s.record_upload(&c, &file(2), B1);
        s.mark_linked(&c, &file(2));
        s.record_seen(&c, &file(2).sha256, B1, 1);
        s.drop_attachment(&c, A1);
        assert_eq!(s.entries(&c)[0].attachments, [A2]);
        // A restored company without the linked upload: the file is
        // offered again instead of hidden for ever, and isn't a duplicate.
        s.drop_attachment(&c, B1);
        assert_eq!(s.entries(&c).len(), 1);
        assert!(s.seen(&c, &file(2).sha256).is_none());
        s.drop_attachment(&c, A2);
        assert!(s.companies.is_empty());
    }

    #[test]
    fn dropping_an_attachment_keeps_entries_without_one() {
        let mut s = InboxState::default();
        let c = company_key("t");
        assert!(s.begin_upload(&c, Some(folder()), &file(1), "d1", "k1"));
        assert!(s.mark_duplicate(&c, Some(folder()), &file(2)));
        s.drop_attachment(&c, "zzz");
        assert_eq!(s.entries(&c).len(), 2);
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
            .update(|s| {
                s.begin_upload(&c, Some(folder()), &f, DRAFT1, KEY1);
                s.arrived(&c, &f, A1, 5);
            })
            .expect("save");
        let text = std::fs::read_to_string(store.path()).expect("read");
        assert!(!text.contains("Secret"), "{text}");
        assert!(!text.contains("inbox"), "{text}");
        assert!(text.contains("\"version\": 3"), "{text}");
        std::fs::write(store.path(), "not json").expect("write");
        let empty = store.load().expect("load");
        assert!(empty.companies.is_empty() && empty.seen.is_empty());
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
        let mut uploading = file(9);
        uploading.path_hash = "uploading".into();
        s.begin_upload(&c, Some(folder()), &uploading, "d", "k");
        for n in 0..=MAX_ENTRIES {
            let mut f = file(1);
            f.path_hash = format!("p{n}");
            s.record_upload(&c, &f, "a");
        }
        assert_eq!(s.entries(&c).len(), MAX_ENTRIES);
        assert!(s.entries(&c)[0].linked, "a linked entry is kept");
        assert_eq!(
            s.entries(&c)[1].status,
            EntryStatus::Uploading,
            "so is an upload in flight"
        );
        let f = file(3);
        for n in 0..(MAX_UPLOADS + 2) {
            s.record_upload(&c, &f, &format!("u{n}"));
        }
        let e = s.entries(&c).iter().find(|e| e.file.sha256 == f.sha256);
        assert_eq!(e.map(|e| e.attachments.len()), Some(MAX_UPLOADS));
    }

    /// Entries that still owe a side effect are never dropped to make
    /// room, even when every entry does.
    #[test]
    fn pending_entries_are_never_dropped_for_room() {
        let mut s = InboxState::default();
        let c = company_key("t");
        for n in 0..=MAX_ENTRIES {
            let mut f = file(1);
            f.path_hash = format!("p{n}");
            s.begin_upload(&c, Some(folder()), &f, DRAFT1, KEY1);
        }
        assert_eq!(s.entries(&c).len(), MAX_ENTRIES + 1);
        assert_eq!(s.entries(&c)[0].file.path_hash, "p0");
    }

    /// A file edited by hand can't make the inbox act on ids the app
    /// never wrote, or hold more than the caps (security review L9).
    #[test]
    fn a_tampered_file_is_cleaned_on_load() {
        let mut big = InboxState {
            version: VERSION,
            ..InboxState::default()
        };
        let c = company_key("t");
        for n in 0..(MAX_SEEN + 3) {
            big.seen.entry(c.clone()).or_default().push(SeenFile {
                sha256: format!("{n:064}"),
                attachment: A1.into(),
                at: 1,
            });
        }
        let mut one = file(1);
        let mut entry = InboxEntry::new(Some(folder()), &one, EntryStatus::Discarding);
        entry.attachments = (0..(MAX_UPLOADS + 4)).map(|_| A2.to_string()).collect();
        entry.attachments.push("../../etc/passwd".into());
        entry.attachments.push(B1.into());
        entry.draft = Some("not a uuid".into());
        entry.upload_key = Some(KEY1.into());
        big.companies.entry(c.clone()).or_default().push(entry);
        one.path_hash = "arrived-without-upload".into();
        let mut bad = InboxEntry::new(Some(folder()), &one, EntryStatus::Arrived);
        bad.attachments = vec!["x".into()];
        big.companies.entry(c.clone()).or_default().push(bad);
        for n in 0..(MAX_ENTRIES + 5) {
            let mut dup = file(2);
            dup.path_hash = format!("dup{n}");
            big.companies
                .entry(c.clone())
                .or_default()
                .push(InboxEntry::new(
                    Some(folder()),
                    &dup,
                    EntryStatus::Duplicate,
                ));
        }
        let s = InboxState::parse(&serde_json::to_string(&big).expect("json"));
        assert_eq!(s.seen[&c].len(), MAX_SEEN);
        let list = s.entries(&c);
        assert_eq!(list.len(), MAX_ENTRIES);
        let first = &list[0];
        assert_eq!(first.status, EntryStatus::Discarding);
        assert_eq!(first.attachments.len(), MAX_UPLOADS);
        assert_eq!(first.attachment(), Some(B1), "the bad id is gone");
        assert!(
            first
                .attachments
                .iter()
                .all(|a| cl_core::ids::parse_uuid(a).is_ok())
        );
        assert_eq!(
            (first.draft.as_deref(), first.upload_key.as_deref()),
            (None, Some(KEY1))
        );
        assert!(
            !list
                .iter()
                .any(|e| e.file.path_hash == "arrived-without-upload"),
            "nothing to show: offered again"
        );
    }

    #[test]
    fn seen_is_capped_oldest_first() {
        let mut s = InboxState::default();
        let c = company_key("t");
        for n in 0..(MAX_SEEN + 5) {
            s.record_seen(&c, &format!("{n:064}"), "a", i64::try_from(n).expect("n"));
        }
        assert_eq!(s.seen[&c].len(), MAX_SEEN);
        assert!(s.seen(&c, &format!("{:064}", 0)).is_none());
        assert!(s.seen(&c, &format!("{:064}", MAX_SEEN + 4)).is_some());
        s.record_seen(&c, &format!("{:064}", MAX_SEEN + 4), "a", 9);
        assert_eq!(s.seen[&c].len(), MAX_SEEN, "no repeat");
    }

    #[test]
    fn the_file_stays_under_the_text_cap() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = InboxStateStore::in_dir(dir.path());
        let mut s = InboxState::default();
        // Three companies at the seen cap would pass 3 MB.
        for t in 0..3 {
            let c = company_key(&format!("t{t}"));
            for n in 0..MAX_SEEN {
                s.record_seen(
                    &c,
                    &format!("{n:064}"),
                    "d0000000-0000-4000-8000-000000000001",
                    1,
                );
            }
            let mut f = file(1);
            f.path_hash = format!("pending{t}");
            s.begin_upload(&c, None, &f, "d", "k");
        }
        assert!(s.to_text().len() > MAX_TEXT_BYTES);
        store.save(&s).expect("save");
        let len = std::fs::metadata(store.path()).expect("meta").len();
        assert!(len <= MAX_TEXT_BYTES as u64, "{len}");
        let back = store.load().expect("load");
        for t in 0..3 {
            let c = company_key(&format!("t{t}"));
            assert_eq!(back.entries(&c).len(), 1, "pending entries survive");
        }
    }

    #[test]
    fn version_1_files_are_upgraded() {
        let v1 = r#"{
          "version": 1,
          "companies": {
            "c1": [
              {"pathHash":"p1","size":10,"modifiedMs":5,"sha256":"aa","attachments":["d0000000-0000-4000-8000-0000000000a1","d0000000-0000-4000-8000-0000000000a2"],"linked":false},
              {"pathHash":"p2","size":11,"modifiedMs":6,"sha256":"bb","attachments":["d0000000-0000-4000-8000-0000000000b1"],"linked":true}
            ]
          }
        }"#;
        let s = InboxState::parse(v1);
        assert_eq!(s.version, VERSION);
        let e = s.entries("c1");
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].folder, e[0].status), (None, EntryStatus::Arrived));
        assert_eq!(e[0].attachment(), Some(A2));
        assert_eq!(e[0].draft, None);
        assert_eq!((e[1].status, e[1].linked), (EntryStatus::Linked, true));
        // Saved as v2 and read back the same.
        let again = InboxState::parse(&s.to_text());
        assert_eq!(again, s);
        // A v2 file is read as written (the status wins over a stale flag).
        let v2 = r#"{"version":2,"companies":{"c":[{"pathHash":"p","size":1,"modifiedMs":1,
          "sha256":"s","status":"discarding","linked":true,"folder":"7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a6b"}]}}"#;
        let s = InboxState::parse(v2);
        let e = &s.entries("c")[0];
        assert_eq!((e.status, e.linked), (EntryStatus::Discarding, false));
        assert_eq!(e.folder, Some(folder()));
    }

    #[test]
    fn the_migration_assigns_the_legacy_folder() {
        let mut s = InboxState::default();
        let c = company_key("t");
        s.record_upload(&c, &file(1), "a");
        s.begin_upload(&c, Some(folder()), &file(2), "d", "k");
        assert_eq!(s.entries_in(&c, None).count(), 1);
        assert_eq!(s.assign_legacy_folder(&c, folder()), 1);
        assert_eq!(s.assign_legacy_folder(&c, folder()), 0, "once");
        assert_eq!(s.entries_in(&c, Some(folder())).count(), 2);
    }

    #[test]
    fn the_status_moves_only_forward() {
        use EntryStatus::{Arrived, Discarding, Duplicate, Kept, Linked, Uploading};
        let all = [Uploading, Arrived, Duplicate, Linked, Discarding, Kept];
        let allowed = [
            (Uploading, Arrived),
            (Uploading, Duplicate),
            (Arrived, Linked),
            (Arrived, Discarding),
            (Arrived, Kept),
            (Arrived, Duplicate),
            (Duplicate, Uploading),
            (Duplicate, Discarding),
            (Duplicate, Kept),
            (Duplicate, Linked),
            (Discarding, Linked),
            (Discarding, Kept),
        ];
        for from in all {
            for to in all {
                let want = from == to || allowed.contains(&(from, to));
                assert_eq!(from.can_become(to), want, "{from:?} -> {to:?}");
            }
        }
        let names: std::collections::HashSet<_> = all.iter().map(|s| s.as_str()).collect();
        assert_eq!(names.len(), all.len());
        assert_eq!(
            all.iter().filter(|s| s.pending()).collect::<Vec<_>>(),
            [&Uploading, &Linked, &Discarding]
        );
    }

    /// Upload → link → move, saved after every step; a "crash" (reload) at
    /// each step resumes where it stopped.
    #[test]
    fn every_step_survives_a_restart() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = InboxStateStore::in_dir(dir.path());
        let c = company_key("t");
        let f = file(4);
        store
            .update(|s| s.begin_upload(&c, Some(folder()), &f, DRAFT1, KEY1))
            .expect("save");
        // Crash: the upload reply was lost. The draft and key are there to
        // check and to retry with.
        let s = store.load().expect("load");
        let e = s.entry(&c, &f).expect("entry");
        assert_eq!(e.status, EntryStatus::Uploading);
        assert_eq!(
            (e.draft.as_deref(), e.upload_key.as_deref()),
            (Some(DRAFT1), Some(KEY1))
        );
        assert!(s.seen(&c, &f.sha256).is_none(), "not seen until it arrived");
        store
            .update(|s| assert!(s.arrived(&c, &f, ATT1, 77)))
            .expect("save");
        let s = store.load().expect("load");
        assert_eq!(
            s.entry(&c, &f).map(|e| e.status),
            Some(EntryStatus::Arrived)
        );
        assert_eq!(
            s.seen(&c, &f.sha256).map(|x| (x.attachment.as_str(), x.at)),
            Some((ATT1, 77))
        );
        // The same bytes under another name: one duplicate entry.
        let mut again = f.clone();
        again.path_hash = "copy".into();
        store
            .update(|s| {
                assert!(s.mark_duplicate(&c, Some(folder()), &again));
                assert!(
                    !s.mark_duplicate(&c, Some(folder()), &again),
                    "flagged once"
                );
            })
            .expect("save");
        store
            .update(|s| assert!(s.set_status(&c, &f, EntryStatus::Linked)))
            .expect("save");
        let s = store.load().expect("load");
        let e = s.entry(&c, &f).expect("entry");
        assert!(
            e.linked && e.status == EntryStatus::Linked,
            "the move is owed"
        );
        // Linked never goes back; the move removes the entry.
        store
            .update(|s| {
                assert!(!s.set_status(&c, &f, EntryStatus::Arrived));
                assert!(!s.begin_upload(&c, Some(folder()), &f, "d2", "k2"));
                s.remove(&c, &f);
            })
            .expect("save");
        let s = store.load().expect("load");
        assert!(s.entry(&c, &f).is_none());
        assert!(s.seen(&c, &f.sha256).is_some(), "still seen");
        // Discard path of the duplicate.
        store
            .update(|s| assert!(s.set_status(&c, &again, EntryStatus::Discarding)))
            .expect("save");
        assert_eq!(
            store
                .load()
                .expect("load")
                .entry(&c, &again)
                .map(|e| e.status),
            Some(EntryStatus::Discarding)
        );
    }

    // ---- A04 D1: version 3 ---------------------------------------------

    const DRAFT_A: &str = "d0000000-0000-4000-8000-0000000dd0a1";
    const DRAFT_B: &str = "d0000000-0000-4000-8000-0000000dd0b1";

    fn arrived(s: &mut InboxState, c: &str, f: &FileIdentity) {
        assert!(s.begin_upload(c, Some(folder()), f, DRAFT1, KEY1));
        assert!(s.arrived(c, f, ATT1, 9));
    }

    /// The claim is saved before it is sent; its answer makes the entry own
    /// the draft, or follow another computer's (a duplicate), and every
    /// step survives a restart.
    #[test]
    fn claims_are_saved_and_survive_a_restart() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = InboxStateStore::in_dir(dir.path());
        let c = company_key("t");
        let (one, two) = (file(1), file(2));
        store
            .update(|s| {
                arrived(s, &c, &one);
                assert!(s.begin_upload(&c, Some(folder()), &two, DRAFT1, KEY1));
                assert!(s.arrived(&c, &two, A2, 9));
                assert!(s.begin_claim(&c, &one));
                assert!(s.begin_claim(&c, &two));
            })
            .expect("save");
        // Crash before either answer: both are still being claimed (never
        // the plain P17 paths), and the cap keeps them.
        let s = store.load().expect("load");
        for f in [&one, &two] {
            let e = s.entry(&c, f).expect("entry");
            assert_eq!(e.doc_draft, DraftLink::ClaimSent);
            assert!(e.doc_draft.is_claimed() && e.owes());
        }
        store
            .update(|s| {
                assert!(s.begin_claim(&c, &one), "claimed again after a restart");
                assert!(s.claimed(&c, &one, DRAFT_A));
                assert!(s.claimed(&c, &one, DRAFT_A), "the same answer again");
                assert!(!s.claimed(&c, &one, DRAFT_B), "never another draft");
                assert!(!s.begin_claim(&c, &one), "owned: no new claim");
                assert!(s.claimed_elsewhere(&c, &two, DRAFT_B));
                assert!(!s.claimed(&c, &two, DRAFT_B), "follows, never owns");
            })
            .expect("save");
        let s = store.load().expect("load");
        let e1 = s.entry(&c, &one).expect("one");
        assert_eq!(
            (e1.status, &e1.doc_draft),
            (EntryStatus::Arrived, &DraftLink::Own(DRAFT_A.into()))
        );
        assert!(!e1.owes());
        let e2 = s.entry(&c, &two).expect("two");
        assert_eq!(
            (e2.status, &e2.doc_draft),
            (EntryStatus::Duplicate, &DraftLink::Follows(DRAFT_B.into()))
        );
        assert!(e2.owes(), "its extra copy is still to delete");
        // Copy anyway never sends a followed file up again.
        let mut s = s;
        assert!(!s.begin_upload(&c, Some(folder()), &two, "d", "k"));
        assert!(!s.copy_deleted(&c, &one), "only a follower's copy");
        assert!(s.copy_deleted(&c, &two));
        let e2 = s.entry(&c, &two).expect("two");
        assert!(e2.attachments.is_empty() && !e2.owes());
        assert!(s.seen(&c, &two.sha256).is_some(), "still seen");
        // Entered on the other computer: the original moves.
        assert!(s.set_status(&c, &two, EntryStatus::Linked));
        assert_eq!(
            s.by_attachment(&c, ATT1).map(|e| e.file.clone()),
            Some(one.clone())
        );
        s.unclaim(&c, &one);
        assert_eq!(
            s.entry(&c, &one).map(|e| e.doc_draft.clone()),
            Some(DraftLink::None)
        );
        assert!(s.set_draft_key(&c, &one, DRAFT_B));
        assert_eq!(
            s.entry(&c, &one).and_then(|e| e.draft.as_deref()),
            Some(DRAFT_B)
        );
    }

    #[test]
    fn only_arrivals_with_an_upload_are_claimed() {
        let mut s = InboxState::default();
        let c = company_key("t");
        assert!(s.begin_upload(&c, Some(folder()), &file(1), DRAFT1, KEY1));
        assert!(!s.begin_claim(&c, &file(1)), "not arrived yet");
        assert!(s.mark_duplicate(&c, Some(folder()), &file(2)));
        assert!(!s.begin_claim(&c, &file(2)), "a local duplicate");
        assert!(!s.claimed_elsewhere(&c, &file(2), DRAFT_A));
        assert!(!s.begin_claim(&c, &file(3)), "no entry");
    }

    /// T17: a version 2 file reads with no drafts; the text written names
    /// the link only when there is one, and reads back the same.
    #[test]
    fn version_2_files_are_upgraded_to_3() {
        let v2 = r#"{"version":2,"companies":{"c":[
          {"pathHash":"p","size":1,"modifiedMs":1,"sha256":"s","status":"arrived",
           "attachments":["d0000000-0000-4000-8000-0000000000a1"],
           "draft":"d0000000-0000-4000-8000-00000000d001",
           "folder":"7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a6b"}]}}"#;
        let mut s = InboxState::parse(v2);
        assert_eq!(s.version, VERSION);
        let e = &s.entries("c")[0];
        assert_eq!(
            (e.status, &e.doc_draft),
            (EntryStatus::Arrived, &DraftLink::None)
        );
        assert!(!s.to_text().contains("docDraft"), "none: not written");
        let f = e.file.clone();
        assert!(s.begin_claim("c", &f));
        let text = s.to_text();
        assert!(text.contains("\"docDraft\": \"claimSent\""), "{text}");
        assert!(s.claimed("c", &f, DRAFT_A));
        let text = s.to_text();
        assert!(text.contains(DRAFT_A), "{text}");
        assert_eq!(InboxState::parse(&text), s);
    }

    /// T17: an older app (A01, version 2) reads a version 3 file: every
    /// entry and status survives; it only doesn't see the drafts.
    #[test]
    fn an_older_app_reads_version_3() {
        /// The A01 entry, as that app declares it (no `docDraft`).
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct V2Entry {
            #[serde(flatten)]
            file: FileIdentity,
            #[serde(default)]
            attachments: Vec<String>,
            #[serde(default)]
            status: EntryStatus,
        }
        #[derive(Deserialize)]
        struct V2State {
            version: u32,
            companies: BTreeMap<String, Vec<V2Entry>>,
        }
        let mut s = InboxState::default();
        let c = company_key("t");
        let (one, two) = (file(1), file(2));
        arrived(&mut s, &c, &one);
        assert!(s.begin_upload(&c, Some(folder()), &two, DRAFT1, KEY1));
        assert!(s.arrived(&c, &two, A2, 9));
        assert!(s.begin_claim(&c, &one) && s.claimed(&c, &one, DRAFT_A));
        assert!(s.begin_claim(&c, &two) && s.claimed_elsewhere(&c, &two, DRAFT_B));
        s.version = VERSION;
        let old: V2State = serde_json::from_str(&s.to_text()).expect("an A01 app reads it");
        assert_eq!(old.version, 3);
        let list = &old.companies[&c];
        assert_eq!(list.len(), 2);
        assert_eq!(
            list.iter()
                .map(|e| (e.file.clone(), e.status))
                .collect::<Vec<_>>(),
            [(one, EntryStatus::Arrived), (two, EntryStatus::Duplicate)]
        );
        assert_eq!(list[0].attachments, [ATT1]);
    }

    /// A file edited by hand: a draft id that isn't a UUID is claimed again
    /// (an arrival) or forgotten; links on the wrong status go.
    #[test]
    fn tampered_links_are_cleaned() {
        let mut s = InboxState::default();
        let c = company_key("t");
        for n in 1..=4 {
            arrived(&mut s, &c, &file(n));
        }
        let list = s.companies.get_mut(&c).expect("list");
        list[0].doc_draft = DraftLink::Own("../etc".into());
        list[1].doc_draft = DraftLink::Follows("x".into());
        list[1].set(EntryStatus::Duplicate);
        list[2].doc_draft = DraftLink::Follows(DRAFT_A.into());
        list[3].doc_draft = DraftLink::ClaimSent;
        list[3].set(EntryStatus::Linked);
        s.version = VERSION;
        let s = InboxState::parse(&s.to_text());
        let links: Vec<DraftLink> = s.entries(&c).iter().map(|e| e.doc_draft.clone()).collect();
        assert_eq!(
            links,
            [
                DraftLink::ClaimSent,
                DraftLink::None,
                DraftLink::None,
                DraftLink::None
            ]
        );
        assert_eq!(DraftLink::Own(DRAFT_A.into()).id(), Some(DRAFT_A));
        assert_eq!(DraftLink::ClaimSent.as_str(), "claim_sent");
    }

    /// Entries waiting for a claim's answer are kept like any other
    /// entry that owes a side effect.
    #[test]
    fn unanswered_claims_are_never_dropped_for_room() {
        let mut s = InboxState::default();
        let c = company_key("t");
        let mut first = file(1);
        first.path_hash = "claiming".into();
        arrived(&mut s, &c, &first);
        assert!(s.begin_claim(&c, &first));
        for n in 0..=MAX_ENTRIES {
            let mut f = file(2);
            f.path_hash = format!("p{n}");
            s.record_upload(&c, &f, A1);
        }
        assert_eq!(s.entries(&c).len(), MAX_ENTRIES);
        assert_eq!(s.entries(&c)[0].file.path_hash, "claiming");
    }
}
