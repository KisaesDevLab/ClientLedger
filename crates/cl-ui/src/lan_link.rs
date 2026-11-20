//! The app's LAN link (P33 D2): wires cl-lan to the supervisor and the
//! keychain. Used by the window and by the headless `--lan-serve` host.
//!
//! - [`LanHost`]: the proxy on the chosen interface, fed from the
//!   supervisor's snapshot (port, workstation token, generation, engine
//!   commit) by a small pump thread; announces on mDNS when asked.
//! - [`LanWorkstation`]: the loopback forwarder and heartbeat; its
//!   [`EndpointSource`] replaces the local engine's for cl-api.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cl_api::EndpointSource;
use cl_host::HostState;
use cl_lan::client::{LinkState, Workstation, WorkstationConfig};
use cl_lan::discovery::Discovery;
use cl_lan::host::{HostProxy, ProxyConfig, ProxyStatus, Upstream};
use cl_lan::maintenance::Gate;
use cl_lan::pairing::Pairing;
use cl_lan::pinning::Fingerprint;
use cl_lan::policy::Interfaces;
use cl_lan::registry::Registry;
use cl_lan::settings::{HostSettings, WorkstationSettings};
use cl_lan::tls::{Identity, Role};
use cl_settings::SecretStore;

use crate::engine_link::SnapshotSource;

/// Folder for the LAN files (certificate, workstation list, `lan.json`).
#[must_use]
pub fn lan_dir(root: &Path) -> PathBuf {
    root.join("lan")
}

/// `lan.json` under the app data root.
#[must_use]
pub fn settings_path(root: &Path) -> PathBuf {
    lan_dir(root).join("lan.json")
}

/// The app version compared between host and workstations (Q-P33p).
#[must_use]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The engine commit this build ships with, when the build knows it.
#[must_use]
pub fn bundled_engine_commit() -> Option<String> {
    option_env!("CL_ENGINE_COMMIT").map(str::to_string)
}

/// Why the LAN link could not start.
#[derive(Debug, thiserror::Error)]
pub enum LanStartError {
    /// The async runtime could not start.
    #[error("the network service could not start")]
    Runtime,
    /// Certificate or keychain trouble.
    #[error("the security certificate could not be prepared: {0}")]
    Identity(String),
    /// The workstation link could not start.
    #[error("the connection to the host could not start: {0}")]
    Link(String),
    /// `lan.json` has no pinned host.
    #[error("this computer is not paired with a host")]
    NotPaired,
}

fn runtime() -> Result<tokio::runtime::Runtime, LanStartError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("cl-lan")
        .enable_all()
        .build()
        .map_err(|_| LanStartError::Runtime)
}

/// Turns a supervisor snapshot into the proxy's upstream.
#[must_use]
pub fn upstream_of(snap: &cl_host::HostSnapshot) -> Option<Upstream> {
    if snap.status.state != HostState::Running {
        return None;
    }
    let ep = snap.endpoint.as_ref()?;
    let lan = ep.lan_token.as_ref()?;
    Some(Upstream {
        port: ep.port,
        lan_token: cl_core::Secret::new(lan.expose()),
        generation: ep.generation,
        engine_commit: snap.status.engine_commit.clone().unwrap_or_default(),
    })
}

/// The host side, running.
pub struct LanHost {
    rt: tokio::runtime::Runtime,
    proxy: Option<HostProxy>,
    gate: Arc<Gate>,
    registry: Arc<Registry>,
    pairing: Arc<Pairing>,
    identity: Identity,
    discovery: Arc<dyn Discovery>,
    pump_stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for LanHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanHost")
            .field("fingerprint", &self.identity.fingerprint())
            .finish_non_exhaustive()
    }
}

impl LanHost {
    /// Starts the proxy for `settings`, fed from `snapshots`.
    ///
    /// # Errors
    /// `Runtime`, `Identity`.
    pub fn start(
        snapshots: Arc<dyn SnapshotSource>,
        settings: &HostSettings,
        root: &Path,
        store: &dyn SecretStore,
        interfaces: Arc<dyn Interfaces>,
        discovery: Arc<dyn Discovery>,
    ) -> Result<LanHost, LanStartError> {
        let rt = runtime()?;
        let dir = lan_dir(root);
        let (identity, _) = Identity::load_or_create(store, &dir, Role::Host)
            .map_err(|e| LanStartError::Identity(e.to_string()))?;
        let registry = Arc::new(Registry::load(dir.join("lan-devices.json")));
        let pairing = Arc::new(Pairing::default());
        let gate = Arc::new(Gate::default());
        let pump_stop = Arc::new(AtomicBool::new(false));
        {
            let (gate, stop) = (Arc::clone(&gate), Arc::clone(&pump_stop));
            let _ = std::thread::Builder::new()
                .name("cl-lan-gate".into())
                .spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        gate.set_upstream(upstream_of(&snapshots.snapshot()));
                        std::thread::sleep(Duration::from_millis(250));
                    }
                });
        }
        let proxy = rt
            .block_on(HostProxy::start(
                ProxyConfig {
                    interface: settings.interface.clone(),
                    ip: settings.ip,
                    port: settings.port,
                    app_version: app_version(),
                },
                identity.clone(),
                Arc::clone(&registry),
                Arc::clone(&pairing),
                Arc::clone(&gate) as Arc<dyn cl_lan::host::HostUpstream>,
                interfaces,
            ))
            .map_err(|e| LanStartError::Identity(e.to_string()))?;
        if settings.announce
            && let ProxyStatus::Listening(addr) = proxy.status()
            && let Err(e) =
                discovery.announce(addr.ip(), addr.port(), &identity.fingerprint().prefix())
        {
            tracing::warn!(error = %e, "LAN announce failed; the manual address still works");
        }
        tracing::info!("LAN host mode on");
        Ok(LanHost {
            rt,
            proxy: Some(proxy),
            gate,
            registry,
            pairing,
            identity,
            discovery,
            pump_stop,
        })
    }

    /// The proxy status.
    #[must_use]
    pub fn status(&self) -> ProxyStatus {
        self.proxy.as_ref().map_or(
            ProxyStatus::Stopped(cl_lan::host::Stopped::Shutdown),
            HostProxy::status,
        )
    }

    /// The host certificate's fingerprint.
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        self.identity.fingerprint()
    }

    /// Pairing state (codes, the Allow prompt).
    #[must_use]
    pub fn pairing(&self) -> &Arc<Pairing> {
        &self.pairing
    }

    /// Paired workstations.
    #[must_use]
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// The gate closed around restores.
    #[must_use]
    pub fn gate(&self) -> &Arc<Gate> {
        &self.gate
    }

    /// Stops the proxy and the announcement.
    pub fn stop(&mut self) {
        self.pump_stop.store(true, Ordering::SeqCst);
        self.discovery.stop();
        if let Some(p) = self.proxy.take() {
            self.rt.block_on(p.shutdown());
        }
        tracing::info!("LAN host mode off");
    }
}

impl Drop for LanHost {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The workstation side, running.
pub struct LanWorkstation {
    _rt: tokio::runtime::Runtime,
    link: Workstation,
}

impl std::fmt::Debug for LanWorkstation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanWorkstation")
            .field("link", &self.link)
            .finish_non_exhaustive()
    }
}

impl LanWorkstation {
    /// Starts the forwarder and heartbeat for the paired host.
    ///
    /// # Errors
    /// `NotPaired`, `Runtime`, `Identity`, `Link`.
    pub fn start(
        settings: &WorkstationSettings,
        root: &Path,
        store: &dyn SecretStore,
    ) -> Result<LanWorkstation, LanStartError> {
        let fp = settings.fingerprint().ok_or(LanStartError::NotPaired)?;
        let rt = runtime()?;
        let (device, _) = Identity::load_or_create(store, &lan_dir(root), Role::Device)
            .map_err(|e| LanStartError::Identity(e.to_string()))?;
        let link = rt
            .block_on(Workstation::start(
                WorkstationConfig {
                    host: settings.host,
                    host_fingerprint: fp,
                    app_version: app_version(),
                    engine_commit: bundled_engine_commit(),
                },
                &device,
            ))
            .map_err(|e| LanStartError::Link(e.to_string()))?;
        Ok(LanWorkstation { _rt: rt, link })
    }

    /// cl-api's endpoint for this link.
    #[must_use]
    pub fn endpoint_source(&self) -> Arc<dyn EndpointSource> {
        Arc::new(self.link.endpoint_source())
    }

    /// The link state now.
    #[must_use]
    pub fn state(&self) -> LinkState {
        self.link.state()
    }

    /// The host address.
    #[must_use]
    pub fn host(&self, settings: &WorkstationSettings) -> SocketAddr {
        settings.host
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_host::{Endpoint, HostSnapshot, HostStatus, Secret};

    fn snap(state: HostState, lan: Option<&str>) -> HostSnapshot {
        HostSnapshot {
            status: HostStatus {
                state,
                database_pid: Some(1),
                engine_pid: Some(2),
                restarts: cl_host::RestartCounts {
                    database: 0,
                    engine: 0,
                },
                generation: 3,
                notices: Vec::new(),
                engine_commit: Some("abc".into()),
                schema_version: None,
            },
            endpoint: Some(Endpoint {
                port: 4100,
                token: Secret::new("host-token"),
                generation: 3,
                lan_token: lan.map(Secret::new),
            }),
            generation: 3,
        }
    }

    #[test]
    fn upstream_carries_only_the_workstation_token() {
        let up = upstream_of(&snap(HostState::Running, Some("lan-token"))).expect("up");
        assert_eq!(up.lan_token.expose(), "lan-token");
        assert_eq!(up.port, 4100);
        assert_eq!(up.engine_commit, "abc");
        assert!(!format!("{up:?}").contains("host-token"));
        // Not host mode, or not running: no upstream.
        assert!(upstream_of(&snap(HostState::Running, None)).is_none());
        assert!(upstream_of(&snap(HostState::Starting, Some("lan-token"))).is_none());
    }
}
