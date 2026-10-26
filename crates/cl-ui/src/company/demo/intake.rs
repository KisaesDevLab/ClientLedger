//! The demo "engine" for intake folders (A01, U-93 contract): the list
//! per company with its `updatedAt` stamp, the engine's folder rules and
//! its `409 STALE` answer. Synthetic; `--demo` only.

use std::collections::BTreeMap;

use cl_core::ids::TenantId;
use cl_core::intake::IntakeFolder;


/// Most folders per company (U-93).
pub const MAX_FOLDERS: usize = 20;

/// One company's stored list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    /// The folders, in order.
    pub folders: Vec<IntakeFolder>,
    /// The engine's opaque stamp (`null` until the first save).
    pub updated_at: Option<String>,
}

/// Why the demo engine refused a write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemoIntakeError {
    /// `409 STALE`: someone saved since `expectedUpdatedAt`.
    Stale,
    /// `400 VALIDATION` (the field that failed).
    Invalid(&'static str),
}

/// The demo engine's intake settings.
#[derive(Clone, Debug, Default)]
pub struct DemoIntake {
    stored: BTreeMap<TenantId, Stored>,
    seq: u64,
    /// The next write answers 409, as if another computer had saved first
    /// (tests and scenarios set it).
    pub stale_next: bool,
    /// The user's engine role in the demo companies (`None`: `owner`, the
    /// Admin until P29). Tests and scenarios set another role to see the
    /// read-only panel.
    pub role: Option<String>,
    /// The demo engine acts as one without `intake-settings-v1` (tests:
    /// the P17 single receipt-inbox folder, Q-A01q).
    pub needs_engine: bool,
}

impl DemoIntake {
    /// The engine's `GET /company/intake-settings`.
    #[must_use]
    pub fn get(&self, tenant: TenantId) -> Stored {
        self.stored.get(&tenant).cloned().unwrap_or_default()
    }

    /// The user's role (`owner` unless a test changed it).
    #[must_use]
    pub fn role(&self) -> String {
        self.role.clone().unwrap_or_else(|| "owner".to_owned())
    }

    /// Someone else saves `folders` (tests: the stamp moves on).
    pub fn save_elsewhere(&mut self, tenant: TenantId, folders: Vec<IntakeFolder>) {
        let stamp = self.next_stamp();
        self.stored.insert(
            tenant,
            Stored {
                folders,
                updated_at: Some(stamp),
            },
        );
    }

    fn next_stamp(&mut self) -> String {
        self.seq += 1;
        // Opaque to the client; only echoed back.
        format!("2026-10-25T08:00:00.{:03}Z", self.seq % 1000)
    }

    /// The engine's `PUT /company/intake-settings`: a full replace when
    /// `expected` is the stored stamp.
    ///
    /// # Errors
    /// [`DemoIntakeError`].
    pub fn put(
        &mut self,
        tenant: TenantId,
        folders: Vec<IntakeFolder>,
        expected: Option<&str>,
    ) -> Result<Stored, DemoIntakeError> {
        let current = self.get(tenant);
        if std::mem::take(&mut self.stale_next) || current.updated_at.as_deref() != expected {
            return Err(DemoIntakeError::Stale);
        }
        validate(&folders)?;
        self.save_elsewhere(tenant, folders);
        Ok(self.get(tenant))
    }
}

/// The engine's folder rules (zod, strict).
fn validate(folders: &[IntakeFolder]) -> Result<(), DemoIntakeError> {
    if folders.len() > MAX_FOLDERS {
        return Err(DemoIntakeError::Invalid("folders"));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut paths = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeSet::new();
    for f in folders {
        let name = f.name.trim();
        if name.is_empty() || name.chars().count() > 100 || name != f.name {
            return Err(DemoIntakeError::Invalid("name"));
        }
        if !names.insert(name.to_lowercase()) {
            return Err(DemoIntakeError::Invalid("name"));
        }
        if cl_company::intake::check_path_syntax(&f.path, &[]).is_err()
            || !paths.insert(cl_settings::migrate_inbox::same_path_key(&f.path))
        {
            return Err(DemoIntakeError::Invalid("path"));
        }
        if !ids.insert(f.id) {
            return Err(DemoIntakeError::Invalid("id"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::ids::IntakeFolderId;
    use cl_core::intake::IntakeKind;

    fn tenant(n: u8) -> TenantId {
        TenantId::parse(&format!("d0000000-0000-4000-8000-0000000000{n:02x}")).expect("id")
    }

    fn folder(n: u8, name: &str, path: &str) -> IntakeFolder {
        IntakeFolder {
            id: IntakeFolderId::parse(&format!("7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a{n:02x}"))
                .expect("id"),
            name: name.to_owned(),
            path: path.to_owned(),
            default_kind: IntakeKind::Bills,
            auto_read: false,
            keep_originals: false,
        }
    }

    #[test]
    fn put_checks_the_stamp_and_the_rules_per_company() {
        let mut d = DemoIntake::default();
        assert_eq!(d.get(tenant(1)), Stored::default());
        assert_eq!(d.role(), "owner");
        let saved = d
            .put(tenant(1), vec![folder(1, "Bills", r"C:\Bills")], None)
            .expect("first save");
        let stamp = saved.updated_at.clone().expect("stamp");
        assert!(
            d.get(tenant(2)).folders.is_empty(),
            "company B sees none of A"
        );
        // An old stamp: 409, nothing changes.
        assert_eq!(d.put(tenant(1), vec![], None), Err(DemoIntakeError::Stale));
        assert_eq!(d.get(tenant(1)), saved);
        // The scenario switch.
        d.stale_next = true;
        assert_eq!(
            d.put(tenant(1), vec![], Some(&stamp)),
            Err(DemoIntakeError::Stale)
        );
        assert!(!d.stale_next, "only once");
        for (bad, field) in [
            (vec![folder(1, "Bills", "Bills")], "path"),
            (vec![folder(1, " Bills", r"C:\B")], "name"),
            (
                vec![folder(1, "Bills", r"C:\B"), folder(2, "bills", r"C:\C")],
                "name",
            ),
            (
                vec![folder(1, "A", r"C:\B"), folder(2, "B", "c:/b")],
                "path",
            ),
            (vec![folder(1, "A", r"C:\B"), folder(1, "B", r"C:\C")], "id"),
        ] {
            assert_eq!(
                d.put(tenant(1), bad, Some(&stamp)),
                Err(DemoIntakeError::Invalid(field))
            );
        }
        let many: Vec<_> = (0..21)
            .map(|n| folder(n, &format!("F{n}"), &format!(r"C:\F{n}")))
            .collect();
        assert_eq!(
            d.put(tenant(1), many, Some(&stamp)),
            Err(DemoIntakeError::Invalid("folders"))
        );
        let again = d.put(tenant(1), vec![], Some(&stamp)).expect("ok");
        assert_ne!(again.updated_at.as_deref(), Some(stamp.as_str()));
    }
}
