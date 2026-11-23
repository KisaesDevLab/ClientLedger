//! Host-side pairing: one code at a time, 10 minutes, 5 tries in total
//! and then burned, and an "Allow this device?" click (Q-P33b, review M2).
//!
//! The device that is added is the one whose TLS client certificate made
//! the pair request: the request body never names a fingerprint.

use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use crate::pinning::{Fingerprint, PairingCode, constant_time_eq_u64};

/// How long a code is valid.
pub const CODE_LIFETIME: Duration = Duration::from_mins(10);
/// Wrong secrets allowed for one code, in total, before it is burned.
pub const CODE_TRIES: u8 = 5;
/// Wrong secrets one address may spend on one code (review M3): fewer
/// than [`CODE_TRIES`], so no single machine can burn the code the owner
/// is reading out. An address over its budget is refused before the
/// compare and spends nothing.
pub const PEER_TRIES: u8 = 2;
/// How long a workstation waits for the host owner's click.
pub const APPROVAL_WAIT: Duration = Duration::from_secs(120);

/// Why a pair request was refused (the workstation shows a plain message).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairRefusal {
    /// No code is showing on the host, or it expired.
    NoCode,
    /// The code was wrong (tries remain).
    WrongCode,
    /// Too many wrong tries: the code is burned; make a new one.
    Burned,
    /// Another device is waiting for the owner's click.
    Busy,
    /// The owner clicked Don't allow.
    Denied,
    /// Nobody answered on the host in time.
    TimedOut,
    /// This computer used up its tries on the code showing; the code
    /// itself still works for others (review M3).
    Throttled,
}

impl PairRefusal {
    /// The engine-shaped error code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            PairRefusal::NoCode => "LAN_PAIR_NO_CODE",
            PairRefusal::WrongCode => "LAN_PAIR_WRONG_CODE",
            PairRefusal::Burned => "LAN_PAIR_CODE_BURNED",
            PairRefusal::Busy => "LAN_PAIR_BUSY",
            PairRefusal::Denied => "LAN_PAIR_DENIED",
            PairRefusal::TimedOut => "LAN_PAIR_TIMED_OUT",
            PairRefusal::Throttled => "LAN_PAIR_THROTTLED",
        }
    }

    /// Parses [`PairRefusal::code`].
    #[must_use]
    pub fn from_code(s: &str) -> Option<PairRefusal> {
        [
            PairRefusal::NoCode,
            PairRefusal::WrongCode,
            PairRefusal::Burned,
            PairRefusal::Busy,
            PairRefusal::Denied,
            PairRefusal::TimedOut,
            PairRefusal::Throttled,
        ]
        .into_iter()
        .find(|r| r.code() == s)
    }
}

/// What the host's setup screen shows about the code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeView {
    /// No code.
    None,
    /// Showing; `seconds_left` until it expires.
    Active {
        /// `XXXXX-XXXXX-XXXXX-XXXXX`.
        display: String,
        /// Seconds until it stops working.
        seconds_left: u64,
        /// Wrong tries left.
        tries_left: u8,
    },
    /// Burned after too many wrong tries (the reason the host screen
    /// gives): how many computers spent them, and the address of the one
    /// that spent the last.
    Burned {
        /// Computers (addresses; one IPv6 /64 counts once) that tried.
        computers: usize,
        /// The address of the last wrong try.
        last_peer: Option<IpAddr>,
    },
}

/// A device waiting for the owner's click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDevice {
    /// Identifies this request for [`Pairing::decide`].
    pub id: u64,
    /// The device certificate's fingerprint.
    pub fingerprint: Fingerprint,
    /// The name the workstation suggested.
    pub label: String,
    /// Its address on the network.
    pub peer: IpAddr,
}

struct Code {
    value: PairingCode,
    expires: Instant,
    tries_left: u8,
    burned: bool,
    /// Wrong tries per address ([`crate::policy::net_key`]) on this code.
    by_peer: std::collections::HashMap<IpAddr, u8>,
    /// The real address of the last wrong try (shown when the code burns).
    last_wrong: Option<IpAddr>,
}

struct Pending {
    device: PendingDevice,
    answer: oneshot::Sender<bool>,
}

/// The pairing state of one host.
#[derive(Default)]
pub struct Pairing {
    code: Mutex<Option<Code>>,
    pending: Mutex<Option<Pending>>,
    next_id: Mutex<u64>,
}

impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pairing(..)")
    }
}

impl Pairing {
    /// Shows a new code (replacing any other).
    ///
    /// # Errors
    /// The OS random source failed.
    pub fn new_code(&self, host: &Fingerprint) -> Result<PairingCode, getrandom::Error> {
        let code = PairingCode::generate(host)?;
        *self.code.lock().unwrap_or_else(PoisonError::into_inner) = Some(Code {
            value: code,
            expires: Instant::now() + CODE_LIFETIME,
            tries_left: CODE_TRIES,
            burned: false,
            by_peer: std::collections::HashMap::new(),
            last_wrong: None,
        });
        Ok(code)
    }

    /// Hides the code.
    pub fn cancel_code(&self) {
        *self.code.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// What the setup screen shows.
    #[must_use]
    pub fn view(&self) -> CodeView {
        let g = self.code.lock().unwrap_or_else(PoisonError::into_inner);
        match g.as_ref() {
            None => CodeView::None,
            Some(c) if c.burned => CodeView::Burned {
                computers: c.by_peer.len(),
                last_peer: c.last_wrong,
            },
            Some(c) => {
                let left = c.expires.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    CodeView::None
                } else {
                    CodeView::Active {
                        display: c.value.display(),
                        seconds_left: left.as_secs(),
                        tries_left: c.tries_left,
                    }
                }
            }
        }
    }

    /// Checks a secret from `peer`. A right one spends the code.
    ///
    /// # Errors
    /// `NoCode`, `WrongCode`, `Burned`; `Throttled` when `peer` already
    /// spent its [`PEER_TRIES`] on this code (nothing more is spent).
    pub fn try_secret(&self, peer: IpAddr, secret_text: &str) -> Result<(), PairRefusal> {
        let mut g = self.code.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(c) = g.as_mut() else {
            return Err(PairRefusal::NoCode);
        };
        if c.burned {
            return Err(PairRefusal::Burned);
        }
        if Instant::now() >= c.expires {
            *g = None;
            return Err(PairRefusal::NoCode);
        }
        let key = crate::policy::net_key(peer);
        let spent = c.by_peer.get(&key).copied().unwrap_or(0);
        if spent >= PEER_TRIES {
            return Err(PairRefusal::Throttled);
        }
        // Every attempt costs a try, right or wrong, before the compare.
        c.tries_left = c.tries_left.saturating_sub(1);
        c.by_peer.insert(key, spent + 1);
        let given = PairingCode::parse_secret(secret_text).unwrap_or(u64::MAX);
        if constant_time_eq_u64(given, c.value.secret) {
            *g = None;
            return Ok(());
        }
        c.last_wrong = Some(peer);
        if c.tries_left == 0 {
            c.burned = true;
            return Err(PairRefusal::Burned);
        }
        Err(PairRefusal::WrongCode)
    }

    /// Queues the device for the owner's click and returns the answer
    /// receiver.
    ///
    /// # Errors
    /// `Busy` when another device is waiting.
    pub fn request(
        &self,
        fingerprint: Fingerprint,
        label: String,
        peer: IpAddr,
    ) -> Result<(u64, oneshot::Receiver<bool>), PairRefusal> {
        let mut p = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if p.as_ref().is_some_and(|x| !x.answer.is_closed()) {
            return Err(PairRefusal::Busy);
        }
        let id = {
            let mut n = self.next_id.lock().unwrap_or_else(PoisonError::into_inner);
            *n += 1;
            *n
        };
        let (tx, rx) = oneshot::channel();
        *p = Some(Pending {
            device: PendingDevice {
                id,
                fingerprint,
                label,
                peer,
            },
            answer: tx,
        });
        Ok((id, rx))
    }

    /// The device waiting for a click, if any.
    #[must_use]
    pub fn pending(&self) -> Option<PendingDevice> {
        let p = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        p.as_ref()
            .filter(|x| !x.answer.is_closed())
            .map(|x| x.device.clone())
    }

    /// The owner's answer. False when that request is no longer waiting.
    pub fn decide(&self, id: u64, allow: bool) -> bool {
        let mut p = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if p.as_ref().is_some_and(|x| x.device.id == id)
            && let Some(x) = p.take()
        {
            return x.answer.send(allow).is_ok();
        }
        false
    }

    /// Clears a request that ended without an answer (timeout, dropped
    /// connection).
    pub fn forget(&self, id: u64) {
        let mut p = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if p.as_ref().is_some_and(|x| x.device.id == id) {
            *p = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Fingerprint {
        Fingerprint::of_der(b"host")
    }

    fn peer(n: u8) -> IpAddr {
        IpAddr::from([192, 168, 1, n])
    }

    fn wrong(code: &PairingCode) -> String {
        PairingCode {
            pin: code.pin,
            secret: code.secret ^ 1,
        }
        .secret_text()
    }

    #[test]
    fn right_secret_spends_the_code() {
        let p = Pairing::default();
        let code = p.new_code(&host()).expect("code");
        assert!(matches!(p.view(), CodeView::Active { tries_left: 5, .. }));
        assert_eq!(p.try_secret(peer(9), &code.secret_text()), Ok(()));
        assert_eq!(p.view(), CodeView::None);
        assert_eq!(
            p.try_secret(peer(9), &code.secret_text()),
            Err(PairRefusal::NoCode)
        );
    }

    #[test]
    fn five_wrong_tries_burn_the_code() {
        let p = Pairing::default();
        let code = p.new_code(&host()).expect("code");
        let bad = wrong(&code);
        // Three machines between them (two tries each at most).
        for n in [1, 1, 2, 2] {
            assert_eq!(p.try_secret(peer(n), &bad), Err(PairRefusal::WrongCode));
        }
        assert_eq!(p.try_secret(peer(3), &bad), Err(PairRefusal::Burned));
        // The host screen says why: three computers, the last at .3.
        assert_eq!(
            p.view(),
            CodeView::Burned {
                computers: 3,
                last_peer: Some(peer(3)),
            }
        );
        // Even the right secret no longer works.
        assert_eq!(
            p.try_secret(peer(4), &code.secret_text()),
            Err(PairRefusal::Burned)
        );
        // Garbage costs a try too.
        let p2 = Pairing::default();
        p2.new_code(&host()).expect("code");
        assert_eq!(p2.try_secret(peer(1), "!!"), Err(PairRefusal::WrongCode));
        assert!(matches!(p2.view(), CodeView::Active { tries_left: 4, .. }));
    }

    /// Review M3: one unpaired machine can't burn the code: after its own
    /// tries it is refused without spending any, and the right computer
    /// still pairs with the same code.
    #[test]
    fn one_machine_cannot_burn_the_code() {
        let p = Pairing::default();
        let code = p.new_code(&host()).expect("code");
        let bad = wrong(&code);
        let attacker = peer(66);
        for _ in 0..PEER_TRIES {
            assert_eq!(p.try_secret(attacker, &bad), Err(PairRefusal::WrongCode));
        }
        for _ in 0..50 {
            assert_eq!(p.try_secret(attacker, &bad), Err(PairRefusal::Throttled));
        }
        // Its right guess would be refused too: the budget is per code.
        assert_eq!(
            p.try_secret(attacker, &code.secret_text()),
            Err(PairRefusal::Throttled)
        );
        assert!(matches!(p.view(), CodeView::Active { tries_left: 3, .. }));
        // Another address of the same IPv6 /64 is the same machine.
        let v6 = |s: &str| s.parse::<IpAddr>().expect("ip");
        for _ in 0..PEER_TRIES {
            let _ = p.try_secret(v6("fd00:1:2:3::1"), &bad);
        }
        assert_eq!(
            p.try_secret(v6("fd00:1:2:3::99"), &bad),
            Err(PairRefusal::Throttled)
        );
        assert_eq!(p.try_secret(peer(20), &code.secret_text()), Ok(()));
        // A new code gives everyone fresh tries.
        let code = p.new_code(&host()).expect("code");
        assert_eq!(p.try_secret(attacker, &code.secret_text()), Ok(()));
    }

    #[tokio::test]
    async fn approval_flow() {
        let p = Pairing::default();
        let fp = Fingerprint::of_der(b"dev");
        let peer: IpAddr = "192.168.1.9".parse().expect("ip");
        let (id, rx) = p.request(fp, "Desk".into(), peer).expect("req");
        assert_eq!(
            p.request(fp, "Again".into(), peer).map(|_| ()),
            Err(PairRefusal::Busy)
        );
        assert_eq!(p.pending().map(|d| d.id), Some(id));
        assert!(!p.decide(id + 1, true));
        assert!(p.decide(id, true));
        assert_eq!(rx.await, Ok(true));
        assert_eq!(p.pending(), None);
        // A dropped waiter frees the slot.
        let (id2, rx2) = p.request(fp, "x".into(), peer).expect("req");
        drop(rx2);
        assert_eq!(p.pending(), None);
        assert!(p.request(fp, "y".into(), peer).is_ok());
        p.forget(id2);
        assert_eq!(
            PairRefusal::from_code("LAN_PAIR_DENIED"),
            Some(PairRefusal::Denied)
        );
    }
}
