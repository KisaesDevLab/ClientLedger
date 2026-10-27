//! Intake folders (A01): folders the owner points the app at, so new
//! bills and receipts dropped there are picked up.
//!
//! The list lives in the engine (`companies.intake_settings`, U-93,
//! capability `intake-settings-v1`); these are the shared value types for
//! one folder. The JSON shape matches the engine's strict folder schema:
//! `{id, name, path, defaultKind, autoRead, keepOriginals}`.
//!
//! A folder's name and path can identify a person or client, so `Debug`
//! redacts both: they must never reach a log line or a crash dialog.

use crate::ids::IntakeFolderId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// What a file dropped in the folder becomes by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntakeKind {
    /// A bill to pay (`"bills"`).
    Bills,
    /// A receipt for something already paid (`"receipts"`).
    Receipts,
    /// Let the document reader decide per file (`"auto"`).
    Auto,
    /// Vendor statements (`"statements"`, A06, Q-A06f): files open in Match
    /// Vendor Statement instead of becoming bill drafts. Offered only when
    /// the engine advertises `document-drafts-statement-v1` (U-94).
    Statements,
}

/// One intake folder, as stored in the engine.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntakeFolder {
    /// Client-generated id; stable across renames and path edits.
    pub id: IntakeFolderId,
    /// Owner-chosen display name (engine: trimmed, 1–100 chars, unique
    /// per company ignoring case).
    pub name: String,
    /// Folder path as the owner entered it (engine: 1–1024 chars, drive,
    /// UNC or absolute Unix form). Validated, not canonicalized, here.
    pub path: String,
    /// What new files become unless the owner picks otherwise.
    pub default_kind: IntakeKind,
    /// Read new files with the document reader automatically.
    pub auto_read: bool,
    /// Leave the original file in the folder after it is picked up.
    /// The engine defaults it to `false` when absent.
    #[serde(default)]
    pub keep_originals: bool,
}

impl fmt::Debug for IntakeFolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntakeFolder")
            .field("id", &self.id)
            .field("name", &"<redacted>")
            .field("path", &"<redacted>")
            .field("default_kind", &self.default_kind)
            .field("auto_read", &self.auto_read)
            .field("keep_originals", &self.keep_originals)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const ID: &str = "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a6b";

    fn sample() -> IntakeFolder {
        IntakeFolder {
            id: IntakeFolderId::parse(ID).unwrap(),
            name: "Smith Plumbing bills".to_owned(),
            path: r"\\fileserver\Scans\Smith".to_owned(),
            default_kind: IntakeKind::Bills,
            auto_read: true,
            keep_originals: false,
        }
    }

    #[test]
    fn kind_serializes_as_engine_strings() {
        for (kind, s) in [
            (IntakeKind::Bills, "bills"),
            (IntakeKind::Receipts, "receipts"),
            (IntakeKind::Auto, "auto"),
            (IntakeKind::Statements, "statements"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(s));
            assert_eq!(
                serde_json::from_value::<IntakeKind>(json!(s)).unwrap(),
                kind
            );
        }
        assert!(serde_json::from_value::<IntakeKind>(json!("Bills")).is_err());
        assert!(serde_json::from_value::<IntakeKind>(json!("bill")).is_err());
    }

    #[test]
    fn folder_matches_engine_json_shape() {
        let v = serde_json::to_value(sample()).unwrap();
        assert_eq!(
            v,
            json!({
                "id": ID,
                "name": "Smith Plumbing bills",
                "path": r"\\fileserver\Scans\Smith",
                "defaultKind": "bills",
                "autoRead": true,
                "keepOriginals": false,
            })
        );
        let Value::Object(map) = &v else {
            panic!("folder must serialize as an object")
        };
        assert_eq!(map.len(), 6);
    }

    #[test]
    fn folder_round_trips() {
        let mut f = sample();
        f.default_kind = IntakeKind::Auto;
        f.keep_originals = true;
        f.path = "/home/owner/Receipts".to_owned();
        let back: IntakeFolder = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn keep_originals_defaults_to_false() {
        let f: IntakeFolder = serde_json::from_value(json!({
            "id": ID,
            "name": "Receipts",
            "path": r"C:\Scans",
            "defaultKind": "receipts",
            "autoRead": false,
        }))
        .unwrap();
        assert!(!f.keep_originals);
        assert_eq!(f.default_kind, IntakeKind::Receipts);
    }

    #[test]
    fn folder_rejects_bad_id_and_missing_fields() {
        let bad_id = json!({
            "id": "not-a-uuid", "name": "x", "path": "/x",
            "defaultKind": "auto", "autoRead": false,
        });
        assert!(serde_json::from_value::<IntakeFolder>(bad_id).is_err());
        let no_path = json!({
            "id": ID, "name": "x", "defaultKind": "auto", "autoRead": false,
        });
        assert!(serde_json::from_value::<IntakeFolder>(no_path).is_err());
    }

    #[test]
    fn debug_redacts_name_and_path() {
        let s = format!("{:?}", sample());
        assert!(!s.contains("Smith"), "{s}");
        assert!(!s.contains("fileserver"), "{s}");
        assert!(!s.contains("Scans"), "{s}");
        assert!(s.contains("<redacted>"), "{s}");
        assert!(s.contains(ID), "{s}");
        assert!(s.contains("Bills"), "{s}");
        let pretty = format!("{:#?}", sample());
        assert!(!pretty.contains("Smith") && !pretty.contains("fileserver"));
    }
}
