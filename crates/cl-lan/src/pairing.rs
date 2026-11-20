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
    /// Burned after too many wrong tries.
    Burned,
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
            Some(c) if c.burned => CodeView::Burned,
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

    /// Checks a secret. A right one spends the code.
    ///
    /// # Errors
    /// `NoCode`, `WrongCode`, `Burned`.
    pub fn try_secret(&self, secret_text: &str) -> Result<(), PairRefusal> {
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
        // Every attempt costs a try, right or wrong, before the compare.
        c.tries_left = c.tries_left.saturating_sub(1);
        let given = PairingCode::parse_secret(secret_text).unwrap_or(u64::MAX);
        if constant_time_eq_u64(given, c.value.secret) {
            *g = None;
            return Ok(());
        }
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

    #[test]
    fn right_secret_spends_the_code() {
        let p = Pairing::default();
        let code = p.new_code(&host()).expect("code");
        assert!(matches!(p.view(), CodeView::Active { tries_left: 5, .. }));
        assert_eq!(p.try_secret(&code.secret_text()), Ok(()));
        assert_eq!(p.view(), CodeView::None);
        assert_eq!(p.try_secret(&code.secret_text()), Err(PairRefusal::NoCode));
    }

    #[test]
    fn five_wrong_tries_burn_the_code() {
        let p = Pairing::default();
        let code = p.new_code(&host()).expect("code");
        let wrong = PairingCode {
            pin: code.pin,
            secret: code.secret ^ 1,
        }
        .secret_text();
        for _ in 0..4 {
            assert_eq!(p.try_secret(&wrong), Err(PairRefusal::WrongCode));
        }
        assert_eq!(p.try_secret(&wrong), Err(PairRefusal::Burned));
        assert_eq!(p.view(), CodeView::Burned);
        // Even the right secret no longer works.
        assert_eq!(p.try_secret(&code.secret_text()), Err(PairRefusal::Burned));
        // Garbage costs a try too.
        let p2 = Pairing::default();
        p2.new_code(&host()).expect("code");
        assert_eq!(p2.try_secret("!!"), Err(PairRefusal::WrongCode));
        assert!(matches!(p2.view(), CodeView::Active { tries_left: 4, .. }));
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
