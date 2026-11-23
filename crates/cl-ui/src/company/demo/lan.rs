//! The `--demo` office network (P33 D4): fixed, synthetic values only. It
//! never binds a socket, touches the keychain or `lan.json`, or sends a
//! multicast packet; the code's countdown is frozen so screenshots are
//! stable. The control channel's `dev.lan_*` commands move it between the
//! states the scenarios need (a pairing request, the host down,
//! maintenance, someone else's save).

use cl_lan::policy::{Candidate, InterfaceAddr, check_private};
use cl_lan::settings::Mode;

/// A synthetic host fingerprint (not a real certificate's).
pub const FINGERPRINT: &str = "3f9a1c0e7b2d4a6f8e1c3b5d7f9a0c2e4b6d8f0a1c3e5b7d9f1a3c5e7b9d0f2a";

/// A synthetic pairing code, shown while the code is on (frozen).
pub const CODE: &str = "K7QX2-M9VD4-HT3NW-B8RLC";

/// The synthetic address of the last wrong try when the demo code burns
/// (a documentation-style private address, no real computer).
pub const BURN_PEER: &str = "192.168.1.77";

/// The demo link as a workstation sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DemoLink {
    /// Talking to the host.
    #[default]
    Connected,
    /// The host computer doesn't answer.
    HostDown,
    /// The host's owner is doing maintenance (restore, import).
    Maintenance,
}

/// A paired workstation in the demo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoDevice {
    /// Its name.
    pub label: String,
    /// Its fingerprint (synthetic hex).
    pub fingerprint: String,
    /// When it was paired (shown as is).
    pub paired: String,
    /// People signed in from it.
    pub signed_in: u32,
}

/// A computer asking to join ("Allow this computer?").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoRequest {
    /// The name it gave.
    pub label: String,
    /// Its address.
    pub peer: String,
}

/// The demo office network.
#[derive(Clone, Debug)]
pub struct DemoLan {
    /// Off, host or workstation.
    pub mode: Mode,
    /// This computer's network connections, judged (Q-P33c).
    pub interfaces: Vec<Candidate>,
    /// The one the host listens on.
    pub chosen: Option<usize>,
    /// The pairing code is showing.
    pub code_on: bool,
    /// The code burned after too many wrong tries (two computers, the last
    /// at a synthetic address).
    pub code_burned: bool,
    /// The host certificate file is missing (review M2): sharing can't
    /// start until the owner makes a new certificate.
    pub cert_missing: bool,
    /// Someone waits for Allow.
    pub request: Option<DemoRequest>,
    /// Paired workstations.
    pub devices: Vec<DemoDevice>,
    /// Keep serving after the app closes and start at sign-in.
    pub start_at_sign_in: bool,
    /// Announce on the network so workstations find it.
    pub announce: bool,
    /// Hosts the workstation found.
    pub found: Vec<(String, String)>,
    /// The host this workstation is paired with.
    pub paired_host: Option<String>,
    /// The link (workstation).
    pub link: DemoLink,
    /// The next save of an open record meets someone else's change.
    pub edit_elsewhere: bool,
    /// Audit events recorded (host mode on or off, pairing, Remove).
    pub audit: Vec<&'static str>,
}

fn iface(name: &str, ip: [u8; 4]) -> Candidate {
    let ip = std::net::IpAddr::from(ip);
    Candidate {
        addr: InterfaceAddr {
            name: name.to_string(),
            ip,
        },
        refusal: check_private(ip).err(),
    }
}

impl Default for DemoLan {
    fn default() -> DemoLan {
        DemoLan {
            mode: Mode::Off,
            interfaces: vec![
                iface("Ethernet", [192, 168, 1, 20]),
                iface("Wi-Fi", [10, 0, 0, 15]),
                iface("VPN", [100, 64, 3, 9]),
                iface("Ethernet 2", [203, 0, 113, 5]),
            ],
            chosen: Some(0),
            code_on: false,
            code_burned: false,
            cert_missing: false,
            request: None,
            devices: vec![DemoDevice {
                label: "Front desk".into(),
                fingerprint: "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90"
                    .into(),
                paired: "Nov 3, 2026".into(),
                signed_in: 1,
            }],
            start_at_sign_in: false,
            announce: true,
            found: vec![("ClientLedger host 3F9A".into(), "192.168.1.20:47615".into())],
            paired_host: None,
            link: DemoLink::Connected,
            edit_elsewhere: false,
            audit: Vec::new(),
        }
    }
}

impl DemoLan {
    /// Turns sharing on (on the chosen connection) or off. Off forgets the
    /// code and any waiting request.
    pub fn set_host(&mut self, on: bool) {
        if on {
            self.mode = Mode::Host;
            self.audit.push("lan_host_mode_on");
        } else {
            if self.mode == Mode::Host {
                self.audit.push("lan_host_mode_off");
            }
            self.mode = Mode::Off;
            self.code_on = false;
            self.code_burned = false;
            self.request = None;
        }
    }

    /// Makes a new host certificate: every paired computer is removed and
    /// pairs again (review M2, S7).
    pub fn regenerate(&mut self) {
        for _ in 0..self.devices.len() {
            self.audit.push("lan_device_removed");
        }
        self.devices.clear();
        self.cert_missing = false;
        self.audit.push("lan_host_key_regenerated");
    }

    /// Allow or Don't allow the waiting computer. Allow pairs it; either
    /// way the code is spent (Q-P33ag).
    pub fn decide(&mut self, allow: bool) {
        if let Some(r) = self.request.take() {
            if allow {
                self.devices.push(DemoDevice {
                    label: r.label,
                    fingerprint: "5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d"
                        .into(),
                    paired: "Today".into(),
                    signed_in: 0,
                });
                self.audit.push("lan_device_paired");
            }
            self.code_on = false;
        }
    }

    /// Removes a workstation (and ends its sign-ins).
    pub fn remove(&mut self, i: usize) {
        if i < self.devices.len() {
            self.devices.remove(i);
            self.audit.push("lan_device_removed");
        }
    }

    /// Pairs this workstation with `addr` (the demo accepts the frozen
    /// code only).
    ///
    /// # Errors
    /// The i18n key of the refusal.
    pub fn pair(&mut self, addr: &str, code: &str) -> Result<(), &'static str> {
        if addr.trim().is_empty() {
            return Err("lan.ws.no_address");
        }
        let typed: String = code
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let want: String = CODE.chars().filter(char::is_ascii_alphanumeric).collect();
        if typed != want {
            return Err("lan.pair.refused.wrong_code");
        }
        self.mode = Mode::Workstation;
        self.paired_host = Some(addr.trim().to_string());
        self.link = DemoLink::Connected;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interfaces_are_judged_and_pairing_takes_the_code_and_a_click() {
        let mut l = DemoLan::default();
        let ok: Vec<bool> = l.interfaces.iter().map(|c| c.refusal.is_none()).collect();
        assert_eq!(ok, vec![true, true, false, false]);
        l.set_host(true);
        l.code_on = true;
        l.request = Some(DemoRequest {
            label: "Back office".into(),
            peer: "192.168.1.31".into(),
        });
        l.decide(true);
        assert_eq!(l.devices.len(), 2);
        assert!(!l.code_on);
        l.remove(0);
        assert_eq!(l.devices.len(), 1);
        l.set_host(false);
        assert_eq!(
            l.audit,
            vec![
                "lan_host_mode_on",
                "lan_device_paired",
                "lan_device_removed",
                "lan_host_mode_off"
            ]
        );
        assert_eq!(l.pair("", CODE), Err("lan.ws.no_address"));
        assert_eq!(
            l.pair("192.168.1.20:47615", "nope"),
            Err("lan.pair.refused.wrong_code")
        );
        assert_eq!(l.pair("192.168.1.20:47615", &CODE.to_lowercase()), Ok(()));
        assert_eq!(l.mode, Mode::Workstation);
    }
}
