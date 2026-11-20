//! Which addresses the host may listen on and which peers it may serve
//! (Q-109, Q-P33c, review M4).
//!
//! - **Allowed:** RFC 1918 (10/8, 172.16/12, 192.168/16), IPv4 link-local
//!   169.254/16 and IPv6 unique-local `fc00::/7`.
//! - **Refused:** the unspecified addresses (`0.0.0.0`, `::`), loopback,
//!   multicast and broadcast, carrier-grade NAT 100.64/10 (often a VPN or a
//!   provider network, never "this office"), IPv6 link-local `fe80::/10`
//!   (needs a scope id and reaches every segment the cable does) and every
//!   public address.
//!
//! IPv4-mapped IPv6 (`::ffff:a.b.c.d`) is judged as the IPv4 address it
//! carries, so `::ffff:8.8.8.8` is public and `::ffff:10.0.0.5` private.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Why an address was refused. Shown to the owner in plain words by the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `0.0.0.0` or `::`: would listen on every network, the internet included.
    Unspecified,
    /// This computer only.
    Loopback,
    /// A public (internet) address.
    Public,
    /// 100.64.0.0/10: shared by internet providers and VPNs.
    CarrierNat,
    /// `fe80::/10`.
    LinkLocalV6,
    /// Multicast, broadcast or another special-purpose range.
    Special,
}

impl Refusal {
    /// A stable key for i18n lookups and tests.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Refusal::Unspecified => "unspecified",
            Refusal::Loopback => "loopback",
            Refusal::Public => "public",
            Refusal::CarrierNat => "carrier-nat",
            Refusal::LinkLocalV6 => "link-local-v6",
            Refusal::Special => "special",
        }
    }
}

/// The address with an IPv4-mapped IPv6 address turned back into IPv4.
#[must_use]
pub fn canonical(ip: IpAddr) -> IpAddr {
    ip.to_canonical()
}

/// The address a per-address limit counts by (review S1, M3): an IPv4
/// address itself, an IPv6 address by its /64, since one machine may use
/// any address in its /64.
#[must_use]
pub fn net_key(ip: IpAddr) -> IpAddr {
    match canonical(ip) {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 @ IpAddr::V4(_) => v4,
    }
}

/// Ok when the host may listen on `ip` and serve peers from it.
///
/// # Errors
/// The reason the address is not a private office network.
pub fn check_private(ip: IpAddr) -> Result<(), Refusal> {
    match canonical(ip) {
        IpAddr::V4(v4) => check_v4(v4),
        IpAddr::V6(v6) => check_v6(v6),
    }
}

fn check_v4(ip: Ipv4Addr) -> Result<(), Refusal> {
    let o = ip.octets();
    if ip.is_unspecified() {
        return Err(Refusal::Unspecified);
    }
    if ip.is_loopback() {
        return Err(Refusal::Loopback);
    }
    if ip.is_multicast() || ip.is_broadcast() || o[0] == 0 || o[0] >= 240 {
        return Err(Refusal::Special);
    }
    if o[0] == 100 && (o[1] & 0b1100_0000) == 64 {
        return Err(Refusal::CarrierNat);
    }
    if ip.is_private() || ip.is_link_local() {
        return Ok(());
    }
    Err(Refusal::Public)
}

fn check_v6(ip: Ipv6Addr) -> Result<(), Refusal> {
    let s = ip.segments();
    if ip.is_unspecified() {
        return Err(Refusal::Unspecified);
    }
    if ip.is_loopback() {
        return Err(Refusal::Loopback);
    }
    if ip.is_multicast() {
        return Err(Refusal::Special);
    }
    if (s[0] & 0xffc0) == 0xfe80 {
        return Err(Refusal::LinkLocalV6);
    }
    if (s[0] & 0xfe00) == 0xfc00 {
        return Ok(());
    }
    Err(Refusal::Public)
}

/// Ok when a connecting peer may be served by a listener bound to
/// `listen`: the peer must be private by the same rule. A loopback listener
/// (only the `lan-test-listener` build makes one) serves loopback peers.
///
/// # Errors
/// The peer's refusal.
pub fn check_peer(listen: IpAddr, peer: IpAddr) -> Result<(), Refusal> {
    let peer = canonical(peer);
    if canonical(listen).is_loopback() && peer.is_loopback() && cfg!(feature = "lan-test-listener")
    {
        return Ok(());
    }
    check_private(peer)
}

/// One address of one network interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceAddr {
    /// The OS name (`Ethernet`, `en0`, `wlan0`).
    pub name: String,
    /// The address.
    pub ip: IpAddr,
}

/// An interface address with the policy verdict, for the setup screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The address.
    pub addr: InterfaceAddr,
    /// `None` when the host may listen on it.
    pub refusal: Option<Refusal>,
}

/// Judges every address. Loopback addresses are dropped (never offered).
#[must_use]
pub fn candidates(addrs: &[InterfaceAddr]) -> Vec<Candidate> {
    addrs
        .iter()
        .filter(|a| !canonical(a.ip).is_loopback())
        .map(|a| Candidate {
            addr: a.clone(),
            refusal: check_private(a.ip).err(),
        })
        .collect()
}

/// Supplies the machine's interface addresses (real or injected in tests).
pub trait Interfaces: Send + Sync + 'static {
    /// The current addresses.
    fn list(&self) -> Vec<InterfaceAddr>;
}

/// The operating system's interfaces (through `if-addrs`).
#[derive(Debug, Default, Clone, Copy)]
pub struct OsInterfaces;

impl Interfaces for OsInterfaces {
    fn list(&self) -> Vec<InterfaceAddr> {
        if_addrs::get_if_addrs()
            .map(|v| {
                v.into_iter()
                    .map(|i| InterfaceAddr {
                        ip: i.ip(),
                        name: i.name,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Fixed addresses for tests.
#[derive(Debug, Default)]
pub struct FakeInterfaces(pub std::sync::Mutex<Vec<InterfaceAddr>>);

impl FakeInterfaces {
    /// Replaces the list (simulates a DHCP change or a cable pull).
    pub fn set(&self, v: Vec<InterfaceAddr>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = v;
    }
}

impl Interfaces for FakeInterfaces {
    fn list(&self) -> Vec<InterfaceAddr> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// What the periodic re-check (every 30 s, M4) decided for a listener on
/// `name` / `ip`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recheck {
    /// Nothing changed.
    Keep,
    /// The interface now has another private address: listen there.
    Rebind(IpAddr),
    /// The interface is gone or no longer private: stop listening.
    Stop(Option<Refusal>),
}

/// Decides what the listener on interface `name`, bound to `ip`, should do.
#[must_use]
pub fn recheck(name: &str, ip: IpAddr, now: &[InterfaceAddr]) -> Recheck {
    let same_family = |a: &InterfaceAddr| a.ip.is_ipv4() == ip.is_ipv4();
    if let Some(a) = now.iter().find(|a| a.name == name && a.ip == ip) {
        return match check_private(a.ip) {
            Ok(()) => Recheck::Keep,
            Err(r) => Recheck::Stop(Some(r)),
        };
    }
    let mut first_refusal = None;
    for a in now.iter().filter(|a| a.name == name && same_family(a)) {
        match check_private(a.ip) {
            Ok(()) => return Recheck::Rebind(a.ip),
            Err(r) => first_refusal = first_refusal.or(Some(r)),
        }
    }
    Recheck::Stop(first_refusal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test address")
    }

    #[test]
    fn private_ranges_are_allowed() {
        for a in [
            "10.0.0.1",
            "10.255.255.254",
            "172.16.0.1",
            "172.31.255.1",
            "192.168.1.20",
            "169.254.10.10",
            "fd12:3456::1",
            "fc00::1",
            "::ffff:10.0.0.5",
            "::ffff:192.168.0.9",
        ] {
            assert_eq!(check_private(ip(a)), Ok(()), "{a}");
        }
    }

    #[test]
    fn public_and_unsafe_addresses_are_refused() {
        let cases = [
            ("0.0.0.0", Refusal::Unspecified),
            ("::", Refusal::Unspecified),
            ("127.0.0.1", Refusal::Loopback),
            ("::1", Refusal::Loopback),
            ("8.8.8.8", Refusal::Public),
            ("172.32.0.1", Refusal::Public),
            ("172.15.0.1", Refusal::Public),
            ("192.169.0.1", Refusal::Public),
            ("100.64.0.1", Refusal::CarrierNat),
            ("100.127.255.254", Refusal::CarrierNat),
            ("fe80::1", Refusal::LinkLocalV6),
            ("febf::1", Refusal::LinkLocalV6),
            ("2001:4860::8888", Refusal::Public),
            ("::ffff:8.8.8.8", Refusal::Public),
            ("::ffff:100.64.1.1", Refusal::CarrierNat),
            ("::ffff:0.0.0.0", Refusal::Unspecified),
            ("224.0.0.251", Refusal::Special),
            ("255.255.255.255", Refusal::Special),
            ("ff02::fb", Refusal::Special),
        ];
        for (a, want) in cases {
            assert_eq!(check_private(ip(a)), Err(want), "{a}");
        }
        // Just outside 100.64/10 is public, not CGNAT.
        assert_eq!(check_private(ip("100.128.0.1")), Err(Refusal::Public));
        assert_eq!(check_private(ip("100.63.255.255")), Err(Refusal::Public));
    }

    #[test]
    fn peers_follow_the_same_rule() {
        assert_eq!(check_peer(ip("192.168.1.2"), ip("192.168.1.9")), Ok(()));
        assert_eq!(
            check_peer(ip("192.168.1.2"), ip("::ffff:192.168.1.9")),
            Ok(())
        );
        assert_eq!(
            check_peer(ip("192.168.1.2"), ip("203.0.113.9")),
            Err(Refusal::Public)
        );
        assert_eq!(
            check_peer(ip("192.168.1.2"), ip("::ffff:203.0.113.9")),
            Err(Refusal::Public)
        );
        // A private listener never serves loopback peers.
        assert_eq!(
            check_peer(ip("192.168.1.2"), ip("127.0.0.1")),
            Err(Refusal::Loopback)
        );
    }

    #[test]
    fn candidates_hide_loopback_and_judge_the_rest() {
        let list = vec![
            InterfaceAddr {
                name: "lo".into(),
                ip: ip("127.0.0.1"),
            },
            InterfaceAddr {
                name: "eth0".into(),
                ip: ip("192.168.1.5"),
            },
            InterfaceAddr {
                name: "wwan".into(),
                ip: ip("100.70.1.1"),
            },
        ];
        let c = candidates(&list);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].refusal, None);
        assert_eq!(c[1].refusal, Some(Refusal::CarrierNat));
    }

    #[test]
    fn recheck_keeps_rebinds_and_stops() {
        let eth = |a: &str| InterfaceAddr {
            name: "eth0".into(),
            ip: ip(a),
        };
        let bound = ip("192.168.1.5");
        assert_eq!(recheck("eth0", bound, &[eth("192.168.1.5")]), Recheck::Keep);
        assert_eq!(
            recheck("eth0", bound, &[eth("192.168.1.77")]),
            Recheck::Rebind(ip("192.168.1.77"))
        );
        assert_eq!(
            recheck("eth0", bound, &[eth("203.0.113.4")]),
            Recheck::Stop(Some(Refusal::Public))
        );
        assert_eq!(recheck("eth0", bound, &[]), Recheck::Stop(None));
        // An IPv6 address on the same interface is not a substitute.
        assert_eq!(
            recheck("eth0", bound, &[eth("fd00::5")]),
            Recheck::Stop(None)
        );
    }
}
