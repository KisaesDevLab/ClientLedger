//! TLS 1.3 with mutual certificate pinning (Q-P33b, review S3, S5, S6).
//!
//! - Every certificate is a self-signed P-256 certificate made on the
//!   machine. The private key lives in the OS keychain (hex PKCS#8); the
//!   certificate (public) sits in the app data folder, owner-only.
//! - Both sides pin the peer's SHA-256 DER fingerprint **and** check the
//!   TLS 1.3 handshake signature against that certificate's key
//!   (`rustls::crypto::verify_tls13_signature`), so a replayed public
//!   certificate without its key is refused.
//! - Client certificates are mandatory. The host's verifier accepts any
//!   well-formed certificate at the handshake (an unpaired device must reach
//!   `/lan/hello` and `/lan/pair`); authorisation is the device registry,
//!   checked on every request against `peer_certificates()`.
//! - TLS 1.3 only, ALPN `http/1.1` only, no session tickets, no session
//!   cache, no resumption and no 0-RTT: every connection runs the verifiers
//!   again, so a removed device cannot come back on a ticket.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use cl_core::Secret;
use cl_settings::SecretStore;
use rustls::client::Resumption;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::NoServerSessionStorage;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, Error as TlsError,
    ServerConfig, SignatureScheme,
};
use zeroize::Zeroizing;

use crate::pinning::{Fingerprint, PairingCode, constant_time_eq};

/// The only application protocol either side speaks.
pub const ALPN: &[u8] = b"http/1.1";

/// Largest certificate a peer may present (ours are well under 1 KiB).
const MAX_CERT_LEN: usize = 4096;

/// The name the workstation puts in SNI. It is never checked: the pin is.
pub const SERVER_NAME: &str = "clientledger-host.invalid";

/// Errors of the TLS layer.
#[derive(Debug, thiserror::Error)]
pub enum TlsSetupError {
    /// rustls refused a configuration.
    #[error("TLS setup failed: {0}")]
    Tls(#[from] TlsError),
    /// Making a certificate failed.
    #[error("certificate generation failed: {0}")]
    Cert(String),
    /// The keychain refused.
    #[error("the keychain could not be used: {0}")]
    Keychain(String),
    /// Reading or writing the certificate file failed.
    #[error("certificate file: {0}")]
    Io(String),
    /// The host's key is in the keychain but its certificate file is gone
    /// (review M2). The paired workstations trust that certificate, so a
    /// new one would silently unpair them all: the owner decides (restore
    /// the file, or make a new certificate and pair every computer again).
    #[error("the office network certificate file is missing")]
    HostCertificateMissing,
}

/// Which identity: the host's server certificate or a workstation's
/// device certificate. Selects the keychain entry and the file name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The host computer.
    Host,
    /// A workstation.
    Device,
}

impl Role {
    fn secret_name(self) -> &'static str {
        match self {
            Role::Host => "lan-host-key",
            Role::Device => "lan-device-key",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Role::Host => "lan-host.crt",
            Role::Device => "lan-device.crt",
        }
    }
}

/// A certificate and its private key.
#[derive(Clone)]
pub struct Identity {
    cert: CertificateDer<'static>,
    key_pkcs8: Arc<Zeroizing<Vec<u8>>>,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("fingerprint", &self.fingerprint())
            .field("key", &"***")
            .finish()
    }
}

impl Identity {
    /// A fresh self-signed P-256 identity.
    ///
    /// # Errors
    /// `Cert` when rcgen fails.
    pub fn generate() -> Result<Identity, TlsSetupError> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(|e| TlsSetupError::Cert(e.to_string()))?;
        let mut params = rcgen::CertificateParams::new(vec![SERVER_NAME.to_string()])
            .map_err(|e| TlsSetupError::Cert(e.to_string()))?;
        // A generic subject: no machine, company or owner name (S11).
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "ClientLedger");
        params.distinguished_name = dn;
        let cert = params
            .self_signed(&key)
            .map_err(|e| TlsSetupError::Cert(e.to_string()))?;
        Ok(Identity {
            cert: cert.der().clone(),
            key_pkcs8: Arc::new(Zeroizing::new(key.serialize_der())),
        })
    }

    /// An identity from a DER certificate and a PKCS#8 key. Not checked
    /// here: rustls refuses a key that does not match when a config is
    /// built from it.
    #[must_use]
    pub fn from_der(cert: Vec<u8>, key_pkcs8: Vec<u8>) -> Identity {
        Identity {
            cert: CertificateDer::from(cert),
            key_pkcs8: Arc::new(Zeroizing::new(key_pkcs8)),
        }
    }

    /// The PKCS#8 private key (tests that build a deliberately wrong
    /// identity; never logged).
    #[must_use]
    pub fn key_pkcs8(&self) -> &[u8] {
        &self.key_pkcs8
    }

    /// The certificate's fingerprint.
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of_der(&self.cert)
    }

    /// The certificate (DER).
    #[must_use]
    pub fn cert_der(&self) -> &[u8] {
        &self.cert
    }

    fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_pkcs8.as_slice().to_vec()))
    }

    /// Loads this role's identity, or makes and saves a new one when none
    /// is saved or the saved key does not match the saved certificate.
    /// Returns the identity and whether it is new.
    ///
    /// # Errors
    /// `Keychain`, `Io`, `Cert`; `HostCertificateMissing` when the host's
    /// key is saved but its certificate file is gone (review M2: never a
    /// silent new host identity; [`Identity::regenerate`] is the owner's
    /// explicit way out).
    pub fn load_or_create(
        store: &dyn SecretStore,
        dir: &Path,
        role: Role,
    ) -> Result<(Identity, bool), TlsSetupError> {
        if let Some(id) = Self::load(store, dir, role)? {
            return Ok((id, false));
        }
        // `save` writes the certificate before the key, so a saved key
        // with no certificate file means the file was lost or removed.
        if role == Role::Host
            && !cert_path(dir, role).exists()
            && store
                .get(role.secret_name())
                .map_err(|e| TlsSetupError::Keychain(e.to_string()))?
                .is_some()
        {
            return Err(TlsSetupError::HostCertificateMissing);
        }
        let id = Self::generate()?;
        id.save(store, dir, role)?;
        Ok((id, true))
    }

    /// Replaces this role's identity with a new one (the host's
    /// "Regenerate"; every workstation must pair again).
    ///
    /// # Errors
    /// `Keychain`, `Io`, `Cert`.
    pub fn regenerate(
        store: &dyn SecretStore,
        dir: &Path,
        role: Role,
    ) -> Result<Identity, TlsSetupError> {
        let id = Self::generate()?;
        id.save(store, dir, role)?;
        Ok(id)
    }

    fn load(
        store: &dyn SecretStore,
        dir: &Path,
        role: Role,
    ) -> Result<Option<Identity>, TlsSetupError> {
        let Some(key_hex) = store
            .get(role.secret_name())
            .map_err(|e| TlsSetupError::Keychain(e.to_string()))?
        else {
            return Ok(None);
        };
        let Some(key) = from_hex(key_hex.expose()) else {
            return Ok(None);
        };
        let Ok(cert) = fs::read(cert_path(dir, role)) else {
            return Ok(None);
        };
        let id = Identity {
            cert: CertificateDer::from(cert),
            key_pkcs8: Arc::new(key),
        };
        // A key that does not belong to the certificate (a half-finished
        // regenerate) is replaced rather than served.
        let provider = provider();
        if rustls::sign::CertifiedKey::from_der(vec![id.cert.clone()], id.key(), &provider).is_err()
        {
            return Ok(None);
        }
        Ok(Some(id))
    }

    fn save(&self, store: &dyn SecretStore, dir: &Path, role: Role) -> Result<(), TlsSetupError> {
        // The certificate first: a crash between the two leaves a mismatch
        // that `load` detects and repairs.
        write_private(&cert_path(dir, role), &self.cert)?;
        store
            .set(role.secret_name(), &Secret::new(to_hex(&self.key_pkcs8)))
            .map_err(|e| TlsSetupError::Keychain(e.to_string()))
    }
}

fn cert_path(dir: &Path, role: Role) -> PathBuf {
    dir.join(role.file_name())
}

/// Writes a file readable by the owner only (Unix mode 0600; on Windows the
/// app data folder's ACL already limits it to the user), through a
/// temporary file and a rename.
///
/// # Errors
/// `Io`.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), TlsSetupError> {
    let io = |e: std::io::Error| TlsSetupError::Io(e.kind().to_string());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io)?;
    }
    let tmp = path.with_extension("tmp");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).map_err(io)?;
    f.write_all(bytes).map_err(io)?;
    f.sync_all().map_err(io)?;
    drop(f);
    fs::rename(&tmp, path).map_err(io)
}

fn to_hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

fn from_hex(s: &str) -> Option<Zeroizing<Vec<u8>>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Zeroizing::new(Vec::with_capacity(s.len() / 2));
    for pair in s.as_bytes().chunks(2) {
        let t = std::str::from_utf8(pair).ok()?;
        out.push(u8::from_str_radix(t, 16).ok()?);
    }
    Some(out)
}

/// The ring provider. Never the process default: configs pass it
/// explicitly so no other crate's choice applies here.
#[must_use]
pub fn provider() -> CryptoProvider {
    rustls::crypto::ring::default_provider()
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    provider().signature_verification_algorithms
}

fn check_shape(
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
) -> Result<(), TlsError> {
    if !intermediates.is_empty() || end_entity.len() > MAX_CERT_LEN {
        return Err(TlsError::InvalidCertificate(CertificateError::BadEncoding));
    }
    Ok(())
}

/// What the workstation pins the host to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPin {
    /// After pairing: the full fingerprint.
    Full(Fingerprint),
    /// During pairing: the 60-bit prefix carried by the code.
    Pairing(u64),
}

/// The workstation's view of the host certificate: pinned, with the
/// handshake signature checked. Records the fingerprint it accepted so
/// pairing can store the full value.
#[derive(Debug)]
pub struct PinnedHostVerifier {
    pin: HostPin,
    algs: WebPkiSupportedAlgorithms,
    seen: Mutex<Option<Fingerprint>>,
}

impl PinnedHostVerifier {
    /// A verifier for this pin.
    #[must_use]
    pub fn new(pin: HostPin) -> PinnedHostVerifier {
        PinnedHostVerifier {
            pin,
            algs: algorithms(),
            seen: Mutex::new(None),
        }
    }

    /// The fingerprint of the certificate the last handshake accepted.
    #[must_use]
    pub fn seen(&self) -> Option<Fingerprint> {
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ServerCertVerifier for PinnedHostVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        check_shape(end_entity, intermediates)?;
        let fp = Fingerprint::of_der(end_entity);
        let ok = match self.pin {
            HostPin::Full(want) => constant_time_eq(want.bytes(), fp.bytes()),
            HostPin::Pairing(pin) => PairingCode { pin, secret: 0 }.matches_host(&fp),
        };
        if !ok {
            return Err(TlsError::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(fp);
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Err(TlsError::General("TLS 1.2 is not offered".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// The host's view of device certificates: one is required and its
/// handshake signature is checked; which devices may do what is decided per
/// request by the registry.
#[derive(Debug)]
pub struct DeviceCertVerifier {
    algs: WebPkiSupportedAlgorithms,
}

impl Default for DeviceCertVerifier {
    fn default() -> Self {
        DeviceCertVerifier { algs: algorithms() }
    }
}

impl ClientCertVerifier for DeviceCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        check_shape(end_entity, intermediates)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Err(TlsError::General("TLS 1.2 is not offered".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// The host's TLS configuration.
///
/// # Errors
/// `Tls` when rustls refuses the identity.
pub fn server_config(host: &Identity) -> Result<Arc<ServerConfig>, TlsSetupError> {
    let mut cfg = ServerConfig::builder_with_provider(Arc::new(provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(DeviceCertVerifier::default()))
        .with_single_cert(vec![host.cert.clone()], host.key())?;
    cfg.session_storage = Arc::new(NoServerSessionStorage {});
    cfg.send_tls13_tickets = 0;
    cfg.max_early_data_size = 0;
    cfg.send_half_rtt_data = false;
    cfg.alpn_protocols = vec![ALPN.to_vec()];
    Ok(Arc::new(cfg))
}

/// A workstation's TLS configuration and the verifier (to read the
/// accepted fingerprint after a pairing handshake).
///
/// # Errors
/// `Tls` when rustls refuses the identity.
pub fn client_config(
    device: &Identity,
    pin: HostPin,
) -> Result<(Arc<ClientConfig>, Arc<PinnedHostVerifier>), TlsSetupError> {
    let verifier = Arc::new(PinnedHostVerifier::new(pin));
    let mut cfg = ClientConfig::builder_with_provider(Arc::new(provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_client_auth_cert(vec![device.cert.clone()], device.key())?;
    cfg.resumption = Resumption::disabled();
    cfg.enable_early_data = false;
    cfg.alpn_protocols = vec![ALPN.to_vec()];
    Ok((Arc::new(cfg), verifier))
}

/// The server name the workstation sends.
#[must_use]
pub fn server_name() -> ServerName<'static> {
    // A constant valid DNS name; the conversion cannot fail.
    ServerName::try_from(SERVER_NAME).unwrap_or(ServerName::IpAddress(
        std::net::IpAddr::from([127, 0, 0, 1]).into(),
    ))
}

/// The fingerprint of a connection's peer certificate, if any.
#[must_use]
pub fn peer_fingerprint(certs: Option<&[CertificateDer<'_>]>) -> Option<Fingerprint> {
    certs
        .and_then(|c| c.first())
        .map(|c| Fingerprint::of_der(c))
}

/// An identity built from parts (tests that need a certificate with
/// someone else's key).
#[cfg(test)]
pub(crate) fn mixed(cert_of: &Identity, key_of: &Identity) -> Identity {
    Identity {
        cert: cert_of.cert.clone(),
        key_pkcs8: key_of.key_pkcs8.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemStore(Mutex<HashMap<String, Secret>>);

    impl SecretStore for MemStore {
        fn get(&self, name: &str) -> Result<Option<Secret>, cl_settings::SecretError> {
            Ok(self.0.lock().expect("lock").get(name).cloned())
        }
        fn set(&self, name: &str, value: &Secret) -> Result<(), cl_settings::SecretError> {
            self.0
                .lock()
                .expect("lock")
                .insert(name.into(), value.clone());
            Ok(())
        }
        fn delete(&self, name: &str) -> Result<(), cl_settings::SecretError> {
            self.0.lock().expect("lock").remove(name);
            Ok(())
        }
        fn kind(&self) -> &'static str {
            "memory"
        }
    }

    #[test]
    fn identity_persists_and_regenerates() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = MemStore::default();
        let (a, new_a) = Identity::load_or_create(&store, dir.path(), Role::Host).expect("a");
        assert!(new_a);
        let (b, new_b) = Identity::load_or_create(&store, dir.path(), Role::Host).expect("b");
        assert!(!new_b);
        assert_eq!(a.fingerprint(), b.fingerprint());
        let c = Identity::regenerate(&store, dir.path(), Role::Host).expect("c");
        assert_ne!(a.fingerprint(), c.fingerprint());
        // The key is never in the certificate file.
        let on_disk = fs::read(dir.path().join("lan-host.crt")).expect("read");
        let key_hex = store.get("lan-host-key").expect("get").expect("some");
        assert!(!to_hex(&on_disk).contains(key_hex.expose()));
        assert!(!format!("{c:?}").contains(key_hex.expose()));
    }

    /// Review M2: a host whose certificate file is gone (key still in the
    /// keychain) does not quietly become a new host; regenerate is the
    /// explicit way out. A workstation's own device identity is remade.
    #[test]
    fn a_lost_host_certificate_is_an_error_not_a_new_identity() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = MemStore::default();
        let (first, _) = Identity::load_or_create(&store, dir.path(), Role::Host).expect("a");
        fs::remove_file(dir.path().join("lan-host.crt")).expect("rm");
        assert!(matches!(
            Identity::load_or_create(&store, dir.path(), Role::Host),
            Err(TlsSetupError::HostCertificateMissing)
        ));
        assert!(!dir.path().join("lan-host.crt").exists(), "nothing made");
        let remade = Identity::regenerate(&store, dir.path(), Role::Host).expect("regenerate");
        assert_ne!(first.fingerprint(), remade.fingerprint());
        let (loaded, new) = Identity::load_or_create(&store, dir.path(), Role::Host).expect("c");
        assert!(!new);
        assert_eq!(loaded.fingerprint(), remade.fingerprint());
        // Device: no pairing rests on a lost device file's old identity
        // (the host no longer knows it either way), so it is remade.
        let (device, _) = Identity::load_or_create(&store, dir.path(), Role::Device).expect("d");
        fs::remove_file(dir.path().join(Role::Device.file_name())).expect("rm");
        let (again, new) = Identity::load_or_create(&store, dir.path(), Role::Device).expect("e");
        assert!(new);
        assert_ne!(device.fingerprint(), again.fingerprint());
    }

    #[test]
    fn a_key_that_does_not_match_its_certificate_is_replaced() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = MemStore::default();
        let (a, _) = Identity::load_or_create(&store, dir.path(), Role::Device).expect("a");
        let other = Identity::generate().expect("gen");
        store
            .set("lan-device-key", &Secret::new(to_hex(&other.key_pkcs8)))
            .expect("set");
        let (b, new_b) = Identity::load_or_create(&store, dir.path(), Role::Device).expect("b");
        assert!(new_b);
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn configs_turn_off_resumption_tickets_and_early_data() {
        let host = Identity::generate().expect("host");
        let dev = Identity::generate().expect("dev");
        let s = server_config(&host).expect("server");
        assert_eq!(s.send_tls13_tickets, 0);
        assert_eq!(s.max_early_data_size, 0);
        assert!(!s.ticketer.enabled());
        assert_eq!(s.alpn_protocols, vec![ALPN.to_vec()]);
        let (c, _) = client_config(&dev, HostPin::Full(host.fingerprint())).expect("client");
        assert!(!c.enable_early_data);
        assert_eq!(c.alpn_protocols, vec![ALPN.to_vec()]);
    }

    #[test]
    fn mixed_identity_is_refused_by_rustls() {
        let a = Identity::generate().expect("a");
        let b = Identity::generate().expect("b");
        let m = mixed(&a, &b);
        assert!(server_config(&m).is_err());
    }
}
