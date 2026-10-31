//! Company-local preferences (P09): choices made for one company that the
//! engine has no column for. They sit on the company's `companies.json`
//! record and travel in a backup's `manifest.host` (Q-P09c), never in the
//! engine. Nothing here is a secret or an amount.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use cl_core::Timestamp;
use cl_core::ids::{AccountId, IntakeFolderId, TagGroupId};
use cl_core::intake::IntakeFolder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ai::{AiLocalPrefs, AiProvider, DEFAULT_MONTHLY_TOKEN_CAP as DEFAULT_LOCAL_CAP};
use crate::email_templates::EmailPrefs;
use crate::smtp::SmtpSecurity;

/// Default for "bills due within N days" on Home (Q-169).
pub const BILLS_DUE_DAYS_DEFAULT: u16 = 7;
/// Largest bills-due window accepted.
pub const BILLS_DUE_DAYS_MAX: u16 = 365;

/// The cloud-AI consent as this app recorded it (Q-136): the engine's audit
/// row carries date, user and disclosure version; the provider is kept
/// here until U-30 adds it to the engine row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConsentNote {
    /// The provider the owner agreed to send redacted data to.
    pub provider: AiProvider,
    /// Disclosure version accepted.
    pub disclosure_version: u32,
    /// When (UTC).
    pub accepted_at: Option<Timestamp>,
}

impl Default for AiConsentNote {
    fn default() -> Self {
        AiConsentNote {
            provider: AiProvider::Ollama,
            disclosure_version: 1,
            accepted_at: None,
        }
    }
}

/// Everything local to one company.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanyPrefs {
    /// P17's single folder watched for new receipts and bills (Q-54);
    /// `None` = off. **Legacy** since A01: intake folders live in the
    /// engine (U-93); the migration ([`crate::migrate_inbox`]) reads this
    /// once and then clears it. It keeps its P17 name so P17 records still
    /// read; the code rename to `legacy_receipt_inbox_dir` waits until the
    /// inbox service (A01 WP-C) stops reading it.
    pub receipt_inbox_dir: Option<PathBuf>,
    /// This computer's confirmations of the company's intake folders
    /// ("Set this company's folder for this computer", Q-A01b), by folder
    /// id. A folder is watched only while its note's `path_hash` matches
    /// the folder's current path. Machine-local: never archived.
    pub intake_here: BTreeMap<IntakeFolderId, HereNote>,
    /// Home shows bills due within this many days (Q-169).
    pub bills_due_days: u16,
    /// Local AI switches, cap and count.
    pub ai: AiLocalPrefs,
    /// Recorded cloud consent (see [`AiConsentNote`]).
    pub ai_consent: Option<AiConsentNote>,
    /// SMTP connection security (the rest of SMTP lives in the engine).
    pub smtp_security: SmtpSecurity,
    /// Show account numbers in lists and pickers (Q-79, on by default).
    pub account_numbers: bool,
    /// Accounts marked sensitive (Q-157). Kept here until the engine stores
    /// the flag (U-33); hiding them from other roles waits for P29.
    pub sensitive_accounts: BTreeSet<AccountId>,
    /// Tag groups a line must use for the account types listed (engine
    /// account-type names such as `expense`, Q-180). Kept here and checked
    /// by the forms until the engine stores and enforces the rule (U-42).
    pub required_tag_groups: BTreeMap<TagGroupId, BTreeSet<String>>,
    /// The owner hid Home's setup checklist (Q-63). It travels with the
    /// company, so a restored company does not show it again.
    pub setup_checklist_dismissed: bool,
    /// Company Information's Remove (Q-P19an): forms and checks print
    /// without the uploaded logo until a new one is uploaded (the engine
    /// can't delete it yet, U-114).
    pub hide_logo: bool,
    /// Email templates and BCC-self (P20, Q-P20j). Travels with backups.
    pub email: EmailPrefs,
}

impl Default for CompanyPrefs {
    fn default() -> Self {
        CompanyPrefs {
            receipt_inbox_dir: None,
            intake_here: BTreeMap::new(),
            bills_due_days: BILLS_DUE_DAYS_DEFAULT,
            ai: AiLocalPrefs::default(),
            ai_consent: None,
            smtp_security: SmtpSecurity::default(),
            account_numbers: true,
            sensitive_accounts: BTreeSet::new(),
            required_tag_groups: BTreeMap::new(),
            setup_checklist_dismissed: false,
            hide_logo: false,
            email: EmailPrefs::default(),
        }
    }
}

/// This computer's confirmation of one intake folder (A01 D1). It holds
/// hashes only, never the path itself.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HereNote {
    /// [`path_hash`] of the folder path the Admin confirmed. A path edited
    /// elsewhere, or a restored backup with another path, no longer
    /// matches, and the folder waits for a new confirmation.
    pub path_hash: String,
    /// The folder's pinned root (`canonical_key`, A01 D2 rule 4): the
    /// watcher pauses the folder when the root resolves elsewhere.
    pub canonical_key: String,
    /// "Only files added from now on" (Q-A01h): files older than this
    /// (Unix ms) are listed with a Bring in button, not copied.
    pub since_ms: Option<i64>,
    /// When the Admin confirmed it (UTC).
    pub confirmed_at: Option<Timestamp>,
}

impl std::fmt::Debug for HereNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Even hashes of a path stay out of logs.
        f.debug_struct("HereNote")
            .field("path_hash", &"<hash>")
            .field("canonical_key", &"<hash>")
            .field("since_ms", &self.since_ms)
            .field("confirmed_at", &self.confirmed_at)
            .finish()
    }
}

/// The hash a [`HereNote`] keeps of a folder path: SHA-256 (hex) of the
/// path exactly as the engine stores it, so any edit of the path (even of
/// letter case or slashes) needs a new confirmation.
#[must_use]
pub fn path_hash(path: &str) -> String {
    Sha256::digest(format!("clientledger-intake-path:{path}").as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// Why a receipt-inbox folder was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FolderError {
    /// Not an absolute path.
    #[error("not an absolute path")]
    NotAbsolute,
    /// A network (UNC) path.
    #[error("network path")]
    Network,
    /// Not an existing folder.
    #[error("not an existing folder")]
    Missing,
}

/// Checks a receipt-inbox folder: absolute, not a UNC/network path, and an
/// existing directory (security M5).
///
/// # Errors
/// [`FolderError`].
pub fn check_inbox_folder(p: &std::path::Path) -> Result<(), FolderError> {
    let text = p.to_string_lossy();
    if text.starts_with("\\\\") || text.starts_with("//") {
        return Err(FolderError::Network);
    }
    if !p.is_absolute() {
        return Err(FolderError::NotAbsolute);
    }
    if !p.is_dir() {
        return Err(FolderError::Missing);
    }
    Ok(())
}

impl CompanyPrefs {
    /// What travels in a backup's `manifest.host`: the choices that make
    /// sense on another computer. The receipt-inbox folder (a path on this
    /// machine), the intake-folder confirmations (Q-A01b), this month's
    /// token count and the consent note (recorded where the owner agreed)
    /// stay out (security M5).
    #[must_use]
    pub fn for_archive(&self) -> CompanyPrefs {
        CompanyPrefs {
            receipt_inbox_dir: None,
            intake_here: BTreeMap::new(),
            bills_due_days: self.bills_due_days,
            ai: AiLocalPrefs {
                suggest_rules: self.ai.suggest_rules,
                monthly_token_cap: self.ai.monthly_token_cap,
                usage: crate::ai::TokenUsage::default(),
            },
            ai_consent: None,
            smtp_security: self.smtp_security,
            account_numbers: self.account_numbers,
            sensitive_accounts: self.sensitive_accounts.clone(),
            required_tag_groups: self.required_tag_groups.clone(),
            setup_checklist_dismissed: self.setup_checklist_dismissed,
            hide_logo: self.hide_logo,
            email: self.email.clone(),
        }
    }

    /// Merges preferences read from an archive over what this computer
    /// already had for the company: machine-local fields (receipt inbox,
    /// intake confirmations, token count, consent note) come from `local`;
    /// the cap is the
    /// stricter of the two and an archive can never lift a cap to "none".
    #[must_use]
    pub fn merged_from_archive(
        archive: &CompanyPrefs,
        local: Option<&CompanyPrefs>,
    ) -> CompanyPrefs {
        let local_cap = local.map_or(Some(DEFAULT_LOCAL_CAP), |l| l.ai.monthly_token_cap);
        let cap = match (local_cap, archive.ai.monthly_token_cap) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let mut out = CompanyPrefs {
            receipt_inbox_dir: local.and_then(|l| l.receipt_inbox_dir.clone()),
            intake_here: local.map(|l| l.intake_here.clone()).unwrap_or_default(),
            bills_due_days: archive.bills_due_days,
            ai: AiLocalPrefs {
                suggest_rules: archive.ai.suggest_rules,
                monthly_token_cap: cap,
                usage: local.map(|l| l.ai.usage.clone()).unwrap_or_default(),
            },
            ai_consent: local.and_then(|l| l.ai_consent.clone()),
            smtp_security: archive.smtp_security,
            account_numbers: archive.account_numbers,
            sensitive_accounts: archive.sensitive_accounts.clone(),
            required_tag_groups: archive.required_tag_groups.clone(),
            setup_checklist_dismissed: archive.setup_checklist_dismissed,
            hide_logo: archive.hide_logo,
            email: archive.email.clone(),
        };
        out.normalize();
        out
    }

    /// Snaps fields into range.
    pub fn normalize(&mut self) {
        self.bills_due_days = self.bills_due_days.clamp(1, BILLS_DUE_DAYS_MAX);
        self.required_tag_groups
            .retain(|_, kinds| !kinds.is_empty());
        if self
            .receipt_inbox_dir
            .as_ref()
            .is_some_and(|p| p.as_os_str().is_empty())
        {
            self.receipt_inbox_dir = None;
        }
        self.intake_here
            .retain(|_, n| !n.path_hash.is_empty() && !n.canonical_key.is_empty());
        self.email.normalize();
    }

    /// Whether this computer confirmed `folder` at its current path (the
    /// folder may be touched and watched).
    #[must_use]
    pub fn is_confirmed(&self, folder: &IntakeFolder) -> bool {
        self.intake_here
            .get(&folder.id)
            .is_some_and(|n| n.path_hash == path_hash(&folder.path))
    }

    /// Drops the confirmations of folders no longer in `folders` (removed
    /// in Preferences or elsewhere).
    pub fn retain_intake_for(&mut self, folders: &[IntakeFolder]) {
        self.intake_here
            .retain(|id, _| folders.iter().any(|f| f.id == *id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_and_normalize() {
        let d = CompanyPrefs::default();
        assert_eq!(d.bills_due_days, 7);
        assert!(
            d.account_numbers,
            "account numbers are on by default (Q-79)"
        );
        assert!(d.sensitive_accounts.is_empty());
        assert_eq!(
            d.ai.monthly_token_cap,
            Some(crate::ai::DEFAULT_MONTHLY_TOKEN_CAP)
        );
        let text = serde_json::to_string(&d).expect("json");
        assert_eq!(
            serde_json::from_str::<CompanyPrefs>(&text).expect("parse"),
            d
        );
        assert_eq!(
            serde_json::from_str::<CompanyPrefs>("{}").expect("empty"),
            d
        );
        let mut p = CompanyPrefs {
            bills_due_days: 0,
            receipt_inbox_dir: Some(PathBuf::new()),
            ai_consent: Some(AiConsentNote {
                provider: AiProvider::Anthropic,
                disclosure_version: 2,
                accepted_at: None,
            }),
            ..CompanyPrefs::default()
        };
        p.normalize();
        assert_eq!(p.bills_due_days, 1);
        assert_eq!(p.receipt_inbox_dir, None);
        p.bills_due_days = 9999;
        p.normalize();
        assert_eq!(p.bills_due_days, BILLS_DUE_DAYS_MAX);
        assert_eq!(AiConsentNote::default().disclosure_version, 1);
    }

    #[test]
    fn archive_copy_leaves_machine_local_fields_behind_and_merge_keeps_the_stricter_cap() {
        let mut local = CompanyPrefs {
            receipt_inbox_dir: Some(PathBuf::from("C:/Receipts")),
            ..CompanyPrefs::default()
        };
        local.ai.record("2026-10", 10, 5);
        local.ai.monthly_token_cap = Some(300_000);
        local.ai_consent = Some(AiConsentNote::default());
        let travelling = local.for_archive();
        assert_eq!(travelling.receipt_inbox_dir, None);
        assert_eq!(travelling.ai.usage.tokens, 0);
        assert_eq!(travelling.ai_consent, None);
        assert_eq!(travelling.ai.monthly_token_cap, Some(300_000));
        // An archive with no cap cannot lift the local cap; a lower one wins.
        let mut archive = travelling.clone();
        archive.ai.monthly_token_cap = None;
        archive.bills_due_days = 21;
        let merged = CompanyPrefs::merged_from_archive(&archive, Some(&local));
        assert_eq!(merged.ai.monthly_token_cap, Some(300_000));
        assert_eq!(merged.bills_due_days, 21);
        assert_eq!(
            merged.receipt_inbox_dir.as_deref(),
            Some(std::path::Path::new("C:/Receipts"))
        );
        assert_eq!(merged.ai.usage.tokens, 15);
        assert!(merged.ai_consent.is_some());
        // The account-number choice and sensitive marks travel (Q-79, Q-157).
        let mut chart = local.clone();
        chart.account_numbers = false;
        let sensitive =
            cl_core::ids::AccountId::parse("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
        chart.sensitive_accounts.insert(sensitive);
        let back = CompanyPrefs::merged_from_archive(&chart.for_archive(), Some(&local));
        assert!(!back.account_numbers);
        assert!(back.sensitive_accounts.contains(&sensitive));
        // Required tag groups travel too (Q-180, U-42); empty rules drop.
        let g = TagGroupId::parse("6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
        let empty = TagGroupId::parse("6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02").expect("id");
        chart
            .required_tag_groups
            .insert(g, BTreeSet::from(["expense".to_string()]));
        chart.required_tag_groups.insert(empty, BTreeSet::new());
        let back = CompanyPrefs::merged_from_archive(&chart.for_archive(), Some(&local));
        assert_eq!(back.required_tag_groups.len(), 1);
        assert!(back.required_tag_groups[&g].contains("expense"));
        let old: CompanyPrefs = serde_json::from_str(r#"{"bills_due_days":7}"#).expect("old");
        assert!(
            old.account_numbers,
            "records from before P11 keep numbers on"
        );
        // Hiding Home's setup checklist travels with the company (Q-63, P16).
        assert!(!old.setup_checklist_dismissed, "shown until hidden");
        let mut hidden = local.clone();
        hidden.setup_checklist_dismissed = true;
        assert!(hidden.for_archive().setup_checklist_dismissed);
        let back = CompanyPrefs::merged_from_archive(&hidden.for_archive(), Some(&local));
        assert!(back.setup_checklist_dismissed);
        archive.ai.monthly_token_cap = Some(1000);
        assert_eq!(
            CompanyPrefs::merged_from_archive(&archive, Some(&local))
                .ai
                .monthly_token_cap,
            Some(1000)
        );
        // No local record: the default cap applies even when the archive says none.
        archive.ai.monthly_token_cap = None;
        let fresh = CompanyPrefs::merged_from_archive(&archive, None);
        assert_eq!(fresh.ai.monthly_token_cap, Some(DEFAULT_LOCAL_CAP));
        assert_eq!(fresh.receipt_inbox_dir, None);
        let dir = tempfile::tempdir().expect("tmp");
        assert_eq!(check_inbox_folder(dir.path()), Ok(()));
        assert_eq!(
            check_inbox_folder(std::path::Path::new("Receipts")),
            Err(FolderError::NotAbsolute)
        );
        assert_eq!(
            check_inbox_folder(std::path::Path::new("\\\\server\\share")),
            Err(FolderError::Network)
        );
        assert_eq!(
            check_inbox_folder(&dir.path().join("nope")),
            Err(FolderError::Missing)
        );
    }

    fn folder(id: &str, path: &str) -> IntakeFolder {
        IntakeFolder {
            id: IntakeFolderId::parse(id).expect("id"),
            name: "Receipts".to_owned(),
            path: path.to_owned(),
            default_kind: cl_core::intake::IntakeKind::Auto,
            auto_read: false,
            keep_originals: false,
        }
    }

    fn note(path: &str) -> HereNote {
        HereNote {
            path_hash: path_hash(path),
            canonical_key: "k".repeat(64),
            since_ms: Some(1_700_000_000_000),
            confirmed_at: Some(crate::recent::now()),
        }
    }

    const A: &str = "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a6b";
    const B: &str = "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a6c";

    #[test]
    fn intake_confirmations_stay_on_this_computer() {
        let mut local = CompanyPrefs::default();
        let f = folder(A, r"\\fileserver\Scans\Smith");
        local.intake_here.insert(f.id, note(&f.path));
        assert!(local.is_confirmed(&f));
        // Never archived (Q-A01b): a backup carries no confirmation.
        let travelling = local.for_archive();
        assert!(travelling.intake_here.is_empty());
        let text = serde_json::to_string(&travelling).expect("json");
        assert!(!text.contains(&path_hash(&f.path)), "{text}");
        // Restored on the same computer: the local confirmations stay.
        let mut archive = travelling.clone();
        archive.intake_here.insert(f.id, note(r"C:\Elsewhere"));
        let merged = CompanyPrefs::merged_from_archive(&archive, Some(&local));
        assert_eq!(merged.intake_here, local.intake_here);
        assert!(merged.is_confirmed(&f));
        // Restored on another computer: nothing is confirmed.
        let fresh = CompanyPrefs::merged_from_archive(&archive, None);
        assert!(fresh.intake_here.is_empty());
        assert!(!fresh.is_confirmed(&f));
    }

    #[test]
    fn a_changed_path_needs_a_new_confirmation() {
        let mut p = CompanyPrefs::default();
        let mut f = folder(A, r"C:\Scans");
        p.intake_here.insert(f.id, note(&f.path));
        assert!(p.is_confirmed(&f));
        for edited in [r"c:\Scans", "C:/Scans", r"C:\Scans\", r"D:\Scans"] {
            f.path = edited.to_owned();
            assert!(!p.is_confirmed(&f), "{edited}");
        }
        // Another folder id is never confirmed by this note.
        assert!(!p.is_confirmed(&folder(B, r"C:\Scans")));
        assert_ne!(path_hash("a"), path_hash("A"));
        assert_eq!(path_hash("a").len(), 64);
    }

    #[test]
    fn removed_folders_lose_their_confirmation_and_empty_notes_drop() {
        let mut p = CompanyPrefs::default();
        let a = folder(A, r"C:\A");
        let b = folder(B, r"C:\B");
        p.intake_here.insert(a.id, note(&a.path));
        p.intake_here.insert(b.id, note(&b.path));
        p.retain_intake_for(std::slice::from_ref(&a));
        assert_eq!(p.intake_here.len(), 1);
        assert!(p.is_confirmed(&a));
        p.intake_here.insert(b.id, HereNote::default());
        p.normalize();
        assert_eq!(p.intake_here.len(), 1, "a note without hashes is dropped");
    }

    #[test]
    fn intake_notes_round_trip_and_debug_hides_the_hashes() {
        let mut p = CompanyPrefs::default();
        let a = folder(A, r"\\fileserver\Scans\Smith");
        p.intake_here.insert(a.id, note(&a.path));
        let text = serde_json::to_string(&p).expect("json");
        assert!(!text.contains("fileserver"), "{text}");
        assert!(text.contains(A), "{text}");
        let back: CompanyPrefs = serde_json::from_str(&text).expect("parse");
        assert_eq!(back, p);
        let debug = format!("{:?}", p.intake_here);
        assert!(!debug.contains(&path_hash(&a.path)), "{debug}");
        assert!(!debug.contains("kkkk"), "{debug}");
        // P17 records (no intake field) still read.
        let old: CompanyPrefs =
            serde_json::from_str(r#"{"receipt_inbox_dir":"C:/Receipts"}"#).expect("old");
        assert!(old.intake_here.is_empty());
        assert!(old.receipt_inbox_dir.is_some());
    }
}
