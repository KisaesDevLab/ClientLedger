//! `cl-lan`: LAN multi-user (P33, Q-108–Q-111, Q-P33a onward).
//!
//! The engine never leaves `127.0.0.1`. On the host, [`host::HostProxy`]
//! listens on one private interface ([`policy`]), terminates TLS 1.3 with
//! mutual pinning ([`tls`], [`pinning`]) and relays each request to the
//! engine with the workstation-class token. On each workstation,
//! [`client::Workstation`] runs a loopback forwarder that cl-api talks to,
//! and implements cl-api's `EndpointSource`.
//!
//! This is the only crate that binds a non-loopback address (Q-P33x,
//! enforced by `cargo xtask layering`).
#![forbid(unsafe_code)]

pub mod client;
pub mod discovery;
pub mod head;
pub mod host;
pub mod maintenance;
pub mod pairing;
pub mod pinning;
pub mod policy;
pub mod registry;
pub mod relay;
pub mod settings;
pub mod tls;

/// The LAN protocol version (hello and the mDNS TXT record).
pub const PROTOCOL: u32 = 1;
