//! The host's list of paired workstations (review S3, M2).
//!
//! Devices are known by their certificate fingerprint only; the label is
//! what the owner typed or the workstation suggested, shown in the
//! Workstations list. The list is checked on **every** request against the
//! connection's peer certificate, so Remove takes effect at the next
//! request, and Remove also closes the device's open connections.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::pinning::Fingerprint;
use crate::tls::write_private;

/// Longest label kept.
pub const MAX_LABEL: usize = 40;

/// One paired workstation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Hex fingerprint (64 lowercase characters).
    pub fingerprint: String,
    /// A short name for people ("Front desk").
    pub label: String,
    /// When it was paired, RFC 3339 UTC.
    pub paired_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    version: u32,
    devices: Vec<Device>,
}

/// The paired devices, persisted to a small JSON file (public data: no keys).
#[derive(Debug)]
pub struct Registry {
    path: Option<PathBuf>,
    devices: Mutex<BTreeMap<Fingerprint, Device>>,
    /// Bumped on every Remove; open connections watch it.
    removed: watch::Sender<u64>,
}

/// Errors of the registry file.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// Reading or writing failed.
    #[error("the workstation list could not be saved: {0}")]
    Io(String),
}

/// Keeps printable characters only and caps the length.
#[must_use]
pub fn clean_label(s: &str) -> String {
    let t: String = s
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL)
        .collect();
    let t = t.trim().to_string();
    if t.is_empty() {
        "Workstation".to_string()
    } else {
        t
    }
}

impl Registry {
    /// An in-memory registry (tests).
    #[must_use]
    pub fn in_memory() -> Registry {
        Registry {
            path: None,
            devices: Mutex::new(BTreeMap::new()),
            removed: watch::channel(0).0,
        }
    }

    /// Loads the list from `path` (missing or unreadable → empty).
    #[must_use]
    pub fn load(path: PathBuf) -> Registry {
        let devices = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<File>(&b).ok())
            .map(|f| {
                f.devices
                    .into_iter()
                    .filter_map(|d| Fingerprint::from_hex(&d.fingerprint).map(|fp| (fp, d)))
                    .collect()
            })
            .unwrap_or_default();
        Registry {
            path: Some(path),
            devices: Mutex::new(devices),
            removed: watch::channel(0).0,
        }
    }

    fn save(&self, devices: &BTreeMap<Fingerprint, Device>) -> Result<(), RegistryError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let f = File {
            version: 1,
            devices: devices.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&f).map_err(|e| RegistryError::Io(e.to_string()))?;
        write_private(path, &bytes).map_err(|e| RegistryError::Io(e.to_string()))
    }

    /// True when `fp` is paired.
    #[must_use]
    pub fn contains(&self, fp: &Fingerprint) -> bool {
        self.devices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(fp)
    }

    /// Adds (or relabels) a device.
    ///
    /// # Errors
    /// `Io` when the file cannot be written (the device is not added).
    pub fn add(
        &self,
        fp: Fingerprint,
        label: &str,
        paired_at: String,
    ) -> Result<(), RegistryError> {
        let mut g = self.devices.lock().unwrap_or_else(PoisonError::into_inner);
        let mut next = g.clone();
        next.insert(
            fp,
            Device {
                fingerprint: fp.hex(),
                label: clean_label(label),
                paired_at,
            },
        );
        self.save(&next)?;
        *g = next;
        Ok(())
    }

    /// Removes a device and closes its connections. True when it was there.
    ///
    /// # Errors
    /// `Io` when the file cannot be written (the device stays listed, but
    /// its connections are closed and it is refused until restart).
    pub fn remove(&self, fp: &Fingerprint) -> Result<bool, RegistryError> {
        let mut g = self.devices.lock().unwrap_or_else(PoisonError::into_inner);
        let had = g.remove(fp).is_some();
        let saved = self.save(&g);
        drop(g);
        self.removed.send_modify(|n| *n += 1);
        saved.map(|()| had)
    }

    /// Removes every device (after Regenerate).
    ///
    /// # Errors
    /// `Io`.
    pub fn clear(&self) -> Result<(), RegistryError> {
        let mut g = self.devices.lock().unwrap_or_else(PoisonError::into_inner);
        g.clear();
        let saved = self.save(&g);
        drop(g);
        self.removed.send_modify(|n| *n += 1);
        saved
    }

    /// The paired devices, oldest pairing first.
    #[must_use]
    pub fn list(&self) -> Vec<Device> {
        let mut v: Vec<Device> = self
            .devices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| a.paired_at.cmp(&b.paired_at));
        v
    }

    /// A receiver that changes whenever a device is removed.
    #[must_use]
    pub fn removals(&self) -> watch::Receiver<u64> {
        self.removed.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_remove_and_persist() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("lan-devices.json");
        let a = Fingerprint::of_der(b"a");
        let b = Fingerprint::of_der(b"b");
        {
            let r = Registry::load(path.clone());
            r.add(a, "Front\u{7} desk", "2026-11-19T00:00:00Z".into())
                .expect("add");
            r.add(b, "", "2026-11-19T00:00:01Z".into()).expect("add");
            assert!(r.contains(&a));
            assert_eq!(r.list()[0].label, "Front desk");
            assert_eq!(r.list()[1].label, "Workstation");
        }
        let r = Registry::load(path.clone());
        assert!(r.contains(&a) && r.contains(&b));
        let mut rx = r.removals();
        assert!(r.remove(&a).expect("remove"));
        assert!(rx.has_changed().expect("open"));
        rx.mark_unchanged();
        assert!(!r.contains(&a));
        assert!(!Registry::load(path).contains(&a));
        r.clear().expect("clear");
        assert_eq!(r.list(), Vec::new());
    }

    #[test]
    fn label_is_capped() {
        assert_eq!(clean_label(&"x".repeat(100)).len(), MAX_LABEL);
    }
}
