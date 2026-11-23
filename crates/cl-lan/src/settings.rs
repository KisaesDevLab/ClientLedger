//! `lan.json`: this computer's LAN settings (plan D2 step 5). No secrets:
//! keys live in the keychain, and fingerprints are public.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::host::DEFAULT_PORT;
use crate::pinning::Fingerprint;
use crate::tls::write_private;

/// What this computer is on the office network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Single computer (the default).
    #[default]
    Off,
    /// Shares its books with workstations.
    Host,
    /// Uses another computer's books.
    Workstation,
}

/// The host's settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSettings {
    /// Interface name the owner chose.
    pub interface: String,
    /// Its address when chosen (re-checked at start).
    pub ip: IpAddr,
    /// The port.
    pub port: u16,
    /// Announce on the network (mDNS).
    pub announce: bool,
    /// Keep serving in the background after the app closes.
    #[serde(default)]
    pub background: bool,
}

/// The workstation's settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkstationSettings {
    /// The host's address.
    pub host: SocketAddr,
    /// The host's fingerprint (hex).
    pub host_fingerprint: String,
}

impl WorkstationSettings {
    /// The pinned fingerprint.
    #[must_use]
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        Fingerprint::from_hex(&self.host_fingerprint)
    }
}

/// The whole file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanSettings {
    /// Off, host or workstation.
    #[serde(default)]
    pub mode: Mode,
    /// Set when this computer has been a host.
    #[serde(default)]
    pub host: Option<HostSettings>,
    /// Set when this computer has been paired.
    #[serde(default)]
    pub workstation: Option<WorkstationSettings>,
}

impl LanSettings {
    /// Reads `lan.json`; missing or unreadable means "off".
    #[must_use]
    pub fn load(path: &Path) -> LanSettings {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Writes `lan.json` (owner-only).
    ///
    /// # Errors
    /// The I/O error kind.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let b = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        write_private(path, &b).map_err(|e| e.to_string())
    }

    /// A host settings value with the default port.
    #[must_use]
    pub fn host_on(interface: String, ip: IpAddr) -> HostSettings {
        HostSettings {
            interface,
            ip,
            port: DEFAULT_PORT,
            announce: true,
            background: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_default() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("lan.json");
        assert_eq!(LanSettings::load(&p), LanSettings::default());
        let s = LanSettings {
            mode: Mode::Workstation,
            host: Some(LanSettings::host_on(
                "eth0".into(),
                "192.168.1.2".parse().expect("ip"),
            )),
            workstation: Some(WorkstationSettings {
                host: "192.168.1.2:47615".parse().expect("addr"),
                host_fingerprint: Fingerprint::of_der(b"h").hex(),
            }),
        };
        s.save(&p).expect("save");
        assert_eq!(LanSettings::load(&p), s);
        assert!(
            s.workstation
                .as_ref()
                .and_then(WorkstationSettings::fingerprint)
                .is_some()
        );
        let text = std::fs::read_to_string(&p).expect("read");
        assert!(text.contains("\"mode\": \"workstation\""));
    }
}
