//! Finding the host on the office network (Q-P33d, review S11).
//!
//! - The host announces `_clientledger._tcp.local.` on the **chosen
//!   interface only**, under a generic instance name, with a TXT record that
//!   holds the protocol version and nothing else (no machine, company or
//!   owner name).
//! - Discovery is a convenience: a manual address always works, and
//!   announcing can be turned off.
//! - Tests use [`FakeDiscovery`]; nothing multicasts under
//!   `CL_OFFLINE_TESTS`.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use crate::PROTOCOL;

/// The DNS-SD service type.
pub const SERVICE_TYPE: &str = "_clientledger._tcp.local.";

/// A host seen on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The generic instance name (`ClientLedger host 1A2B`).
    pub name: String,
    /// Where to connect.
    pub addr: SocketAddr,
}

/// Errors of discovery.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// The mDNS service could not start.
    #[error("network discovery is not available: {0}")]
    Unavailable(String),
    /// Refused because tests run offline.
    #[error("network discovery is off during offline tests")]
    Offline,
}

/// Announces a host and finds hosts.
pub trait Discovery: Send + Sync {
    /// Starts announcing `port` on `ip` under a name derived from `tag`
    /// (a short, non-identifying string such as the fingerprint prefix).
    ///
    /// # Errors
    /// `Unavailable`, `Offline`.
    fn announce(&self, ip: IpAddr, port: u16, tag: &str) -> Result<(), DiscoveryError>;

    /// Stops announcing.
    fn stop(&self);

    /// Looks for hosts for up to `wait`.
    ///
    /// # Errors
    /// `Unavailable`, `Offline`.
    fn browse(&self, wait: Duration) -> Result<Vec<Found>, DiscoveryError>;
}

/// The instance name: generic, plus 4 characters so two hosts differ.
#[must_use]
pub fn instance_name(tag: &str) -> String {
    let short: String = tag
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(4)
        .collect();
    format!("ClientLedger host {short}")
}

/// True when tests asked for no network.
#[must_use]
pub fn offline_tests() -> bool {
    std::env::var_os("CL_OFFLINE_TESTS").is_some_and(|v| v == "1")
}

/// mDNS through `mdns-sd`.
#[derive(Default)]
pub struct MdnsDiscovery {
    daemon: Mutex<Option<mdns_sd::ServiceDaemon>>,
    registered: Mutex<Option<String>>,
}

impl std::fmt::Debug for MdnsDiscovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MdnsDiscovery(..)")
    }
}

impl MdnsDiscovery {
    fn with_daemon<T>(
        &self,
        only: Option<IpAddr>,
        f: impl FnOnce(&mdns_sd::ServiceDaemon) -> Result<T, DiscoveryError>,
    ) -> Result<T, DiscoveryError> {
        if offline_tests() {
            return Err(DiscoveryError::Offline);
        }
        let mut g = self.daemon.lock().unwrap_or_else(PoisonError::into_inner);
        if g.is_none() {
            let d = mdns_sd::ServiceDaemon::new()
                .map_err(|e| DiscoveryError::Unavailable(e.to_string()))?;
            // Queued before the daemon's first poll: nothing is sent on any
            // interface except the chosen one.
            if let Some(ip) = only {
                d.disable_interface(mdns_sd::IfKind::All)
                    .map_err(|e| DiscoveryError::Unavailable(e.to_string()))?;
                d.enable_interface(mdns_sd::IfKind::Addr(ip))
                    .map_err(|e| DiscoveryError::Unavailable(e.to_string()))?;
            }
            *g = Some(d);
        }
        match g.as_ref() {
            Some(d) => f(d),
            None => Err(DiscoveryError::Unavailable(String::new())),
        }
    }
}

impl Discovery for MdnsDiscovery {
    fn announce(&self, ip: IpAddr, port: u16, tag: &str) -> Result<(), DiscoveryError> {
        let name = instance_name(tag);
        let fullname = self.with_daemon(Some(ip), |d| {
            let proto = PROTOCOL.to_string();
            let props = [("v", proto.as_str())];
            let info = mdns_sd::ServiceInfo::new(
                SERVICE_TYPE,
                &name,
                "clientledger-host.local.",
                ip,
                port,
                &props[..],
            )
            .map_err(|e| DiscoveryError::Unavailable(e.to_string()))?;
            let full = info.get_fullname().to_string();
            d.register(info)
                .map_err(|e| DiscoveryError::Unavailable(e.to_string()))?;
            Ok(full)
        })?;
        *self
            .registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(fullname);
        Ok(())
    }

    fn stop(&self) {
        let d = self
            .daemon
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let reg = self
            .registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(d) = d {
            if let Some(full) = reg {
                let _ = d.unregister(&full);
            }
            let _ = d.shutdown();
        }
    }

    fn browse(&self, wait: Duration) -> Result<Vec<Found>, DiscoveryError> {
        let rx = self.with_daemon(None, |d| {
            d.browse(SERVICE_TYPE)
                .map_err(|e| DiscoveryError::Unavailable(e.to_string()))
        })?;
        let deadline = std::time::Instant::now() + wait;
        let mut out: Vec<Found> = Vec::new();
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            let Ok(ev) = rx.recv_timeout(left) else {
                break;
            };
            if let mdns_sd::ServiceEvent::ServiceResolved(info) = ev {
                let v_ok = info
                    .get_property_val_str("v")
                    .is_some_and(|v| v == PROTOCOL.to_string());
                if !v_ok {
                    continue;
                }
                let name = info
                    .get_fullname()
                    .trim_end_matches(SERVICE_TYPE)
                    .trim_end_matches('.')
                    .to_string();
                for ip in info.get_addresses_v4() {
                    let ip = IpAddr::V4(ip);
                    if crate::policy::check_private(ip).is_ok() {
                        let f = Found {
                            name: name.clone(),
                            addr: SocketAddr::new(ip, info.get_port()),
                        };
                        if !out.contains(&f) {
                            out.push(f);
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

/// In-memory discovery for tests and the demo backend.
#[derive(Debug, Default)]
pub struct FakeDiscovery {
    announced: Mutex<Option<Found>>,
    others: Mutex<Vec<Found>>,
}

impl FakeDiscovery {
    /// Adds a host that `browse` will return.
    pub fn add(&self, f: Found) {
        self.others
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(f);
    }

    /// What `announce` last published.
    #[must_use]
    pub fn announced(&self) -> Option<Found> {
        self.announced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Discovery for FakeDiscovery {
    fn announce(&self, ip: IpAddr, port: u16, tag: &str) -> Result<(), DiscoveryError> {
        *self
            .announced
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Found {
            name: instance_name(tag),
            addr: SocketAddr::new(ip, port),
        });
        Ok(())
    }

    fn stop(&self) {
        *self
            .announced
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn browse(&self, _wait: Duration) -> Result<Vec<Found>, DiscoveryError> {
        let mut v = self
            .others
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(a) = self.announced() {
            v.push(a);
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_name_is_generic() {
        assert_eq!(instance_name("1A2B3C4D"), "ClientLedger host 1A2B");
        assert_eq!(instance_name("Ab's PC!"), "ClientLedger host AbsP");
    }

    #[test]
    fn fake_round_trip() {
        let d = FakeDiscovery::default();
        d.announce("192.168.1.5".parse().expect("ip"), 47615, "ABCD")
            .expect("announce");
        let found = d.browse(Duration::from_millis(1)).expect("browse");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].addr.port(), 47615);
        d.stop();
        assert_eq!(
            d.browse(Duration::from_millis(1)).expect("browse"),
            Vec::new()
        );
    }

    #[test]
    fn mdns_refuses_under_offline_tests() {
        if offline_tests() {
            let d = MdnsDiscovery::default();
            assert!(matches!(
                d.browse(Duration::from_millis(1)),
                Err(DiscoveryError::Offline)
            ));
        }
    }
}
