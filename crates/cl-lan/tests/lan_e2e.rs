//! End-to-end tests of the LAN path on one machine: a fake engine on
//! loopback, the host proxy on loopback (the `lan-test-listener` build),
//! and a workstation forwarder. Covers pairing, the injected headers, the
//! smuggling refusals (T2), pinning both ways with the right certificate
//! and the wrong key (T4), revocation (T11), `LAN_NOT_SENT` (T13), the
//! maintenance gate and streaming through both hops.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use cl_core::Secret;
use cl_lan::client::{self, CallError, LinkState, Workstation, WorkstationConfig};
use cl_lan::host::{HostProxy, ProxyConfig, ProxyStatus, Upstream};
use cl_lan::maintenance::{Closed, Gate};
use cl_lan::pairing::{PairRefusal, Pairing};
use cl_lan::pinning::{Fingerprint, PairingCode};
use cl_lan::policy::FakeInterfaces;
use cl_lan::registry::Registry;
use cl_lan::tls::{self, HostPin, Identity};
use http_body_util::{BodyExt, Full};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const LAN_TOKEN: &str = "lan-token-0123456789abcdef0123456789abcdef0123456789abcdef01234567";
const VERSION: &str = "9.9.9";
const COMMIT: &str = "c0ffee";

/// What the fake engine saw.
#[derive(Clone, Debug, Default)]
struct Seen {
    requests: Vec<serde_json::Value>,
}

async fn fake_engine(seen: Arc<Mutex<Seen>>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = l.local_addr().expect("addr").port();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else {
                continue;
            };
            let seen = seen.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let seen = seen.clone();
                    async move {
                        let h = |n: &str| {
                            req.headers()
                                .get_all(n)
                                .iter()
                                .map(|v| v.to_str().unwrap_or("?").to_string())
                                .collect::<Vec<_>>()
                        };
                        let rec = serde_json::json!({
                            "method": req.method().as_str(),
                            "path": req.uri().to_string(),
                            "token": h("x-cl-token"),
                            "device": h("x-cl-device"),
                            "xff": h("x-forwarded-for"),
                            "host": h("host"),
                            "origin": h("origin"),
                            "custom": h("x-custom"),
                            "forwarded": h("forwarded"),
                        });
                        seen.lock().expect("lock").requests.push(rec.clone());
                        if req.uri().path() == "/api/v1/stream" {
                            let (tx, rx) =
                                tokio::sync::mpsc::channel::<Result<Frame<Bytes>, Infallible>>(4);
                            tokio::spawn(async move {
                                let _ =
                                    tx.send(Ok(Frame::data(Bytes::from("data: one\n\n")))).await;
                                tokio::time::sleep(Duration::from_secs(30)).await;
                                let _ =
                                    tx.send(Ok(Frame::data(Bytes::from("data: two\n\n")))).await;
                            });
                            let mut r = Response::new(ChanBody(rx).boxed());
                            r.headers_mut()
                                .insert("content-type", "text/event-stream".parse().expect("hv"));
                            return Ok::<_, Infallible>(r);
                        }
                        let body = req
                            .into_body()
                            .collect()
                            .await
                            .map_or(0, |b| b.to_bytes().len());
                        let mut out = rec;
                        out["bodyLen"] = body.into();
                        let r = Response::new(
                            Full::new(Bytes::from(out.to_string()))
                                .map_err(|n| match n {})
                                .boxed(),
                        );
                        Ok(r)
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    port
}

/// A body fed from a channel (the engine's event stream).
struct ChanBody(tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, Infallible>>);

impl hyper::body::Body for ChanBody {
    type Data = Bytes;
    type Error = Infallible;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.0.poll_recv(cx)
    }
}
struct Lab {
    seen: Arc<Mutex<Seen>>,
    gate: Arc<Gate>,
    registry: Arc<Registry>,
    pairing: Arc<Pairing>,
    host_id: Identity,
    proxy: Option<HostProxy>,
    addr: SocketAddr,
}

async fn lab() -> Lab {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let engine_port = fake_engine(seen.clone()).await;
    let gate = Arc::new(Gate::default());
    gate.set_upstream(Some(Upstream {
        port: engine_port,
        lan_token: Secret::new(LAN_TOKEN),
        generation: 4,
        engine_commit: COMMIT.into(),
    }));
    let registry = Arc::new(Registry::in_memory());
    let pairing = Arc::new(Pairing::default());
    let host_id = Identity::generate().expect("host id");
    let proxy = HostProxy::start(
        ProxyConfig {
            interface: "lo".into(),
            ip: "127.0.0.1".parse().expect("ip"),
            port: 0,
            app_version: VERSION.into(),
        },
        host_id.clone(),
        registry.clone(),
        pairing.clone(),
        gate.clone(),
        Arc::new(FakeInterfaces::default()),
    )
    .await
    .expect("proxy");
    let ProxyStatus::Listening(addr) = proxy.status() else {
        panic!("not listening: {:?}", proxy.status());
    };
    Lab {
        seen,
        gate,
        registry,
        pairing,
        host_id,
        proxy: Some(proxy),
        addr,
    }
}

/// Pairs a fresh device, clicking Allow on the host.
async fn pair(lab: &Lab) -> Identity {
    let device = Identity::generate().expect("device");
    let code = lab
        .pairing
        .new_code(&lab.host_id.fingerprint())
        .expect("code");
    let p = lab.pairing.clone();
    let clicker = tokio::spawn(async move {
        for _ in 0..200 {
            if let Some(d) = p.pending() {
                assert_eq!(d.label, "Front desk");
                return p.decide(d.id, true);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    });
    let fp = client::pair(lab.addr, code, &device, "Front desk")
        .await
        .expect("pair");
    assert!(clicker.await.expect("join"));
    assert_eq!(fp, lab.host_id.fingerprint());
    assert!(lab.registry.contains(&device.fingerprint()));
    device
}

async fn workstation(lab: &Lab, device: &Identity) -> Workstation {
    let ws = Workstation::start(
        WorkstationConfig {
            host: lab.addr,
            host_fingerprint: lab.host_id.fingerprint(),
            app_version: VERSION.into(),
            engine_commit: COMMIT.into(),
        },
        device,
    )
    .await
    .expect("ws");
    let st = ws.check_now().await;
    assert!(
        matches!(st, LinkState::Connected { generation: 4, .. }),
        "{st:?}"
    );
    ws
}

/// Sends raw bytes and reads until the peer closes.
async fn raw<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    s: &mut S,
    bytes: &[u8],
) -> String {
    s.write_all(bytes).await.expect("write");
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

async fn via_forwarder(ws: &Workstation, extra_headers: &str, path: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", ws.port()))
        .await
        .expect("connect");
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-CL-Token: {}\r\n{extra_headers}Content-Length: 2\r\n\r\n{{}}",
        ws.port(),
        ws.local_token().expose()
    );
    raw(&mut s, req.as_bytes()).await
}

async fn tls_to_proxy(lab: &Lab, device: &Identity) -> tokio_rustls::client::TlsStream<TcpStream> {
    let (cfg, _) =
        tls::client_config(device, HostPin::Full(lab.host_id.fingerprint())).expect("cfg");
    let tcp = TcpStream::connect(lab.addr).await.expect("tcp");
    tokio_rustls::TlsConnector::from(cfg)
        .connect(tls::server_name(), tcp)
        .await
        .expect("tls")
}

fn body_json(resp: &str) -> serde_json::Value {
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
}

#[tokio::test(flavor = "multi_thread")]
async fn paired_request_reaches_the_engine_with_only_the_workstation_token() {
    let lab = lab().await;
    let device = pair(&lab).await;
    let ws = workstation(&lab, &device).await;
    let resp = via_forwarder(
        &ws,
        "X-CL-Device: spoofed\r\nX-Forwarded-For: 1.2.3.4\r\nForwarded: for=1.2.3.4\r\nConnection: x-custom\r\nX-Custom: drop\r\n",
        "/api/v1/journal-entries?q=secret",
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let j = body_json(&resp);
    assert_eq!(j["token"], serde_json::json!([LAN_TOKEN]));
    assert_eq!(j["device"], serde_json::json!([device.fingerprint().hex()]));
    assert_eq!(j["xff"], serde_json::json!(["127.0.0.1"]));
    assert_eq!(j["custom"], serde_json::json!([]));
    assert_eq!(j["forwarded"], serde_json::json!([]));
    assert_eq!(j["path"], "/api/v1/journal-entries?q=secret");
    assert_eq!(j["bodyLen"], 2);
    let host = j["host"][0].as_str().unwrap_or("");
    assert!(host.starts_with("127.0.0.1:"), "{host}");
    // The workstation's own token never reaches the engine.
    assert!(!resp.contains(ws.local_token().expose()));
}

#[tokio::test(flavor = "multi_thread")]
async fn unpaired_devices_reach_hello_and_pair_only() {
    let lab = lab().await;
    let stranger = Identity::generate().expect("id");
    let h = client::hello(
        lab.addr,
        HostPin::Full(lab.host_id.fingerprint()),
        &stranger,
    )
    .await
    .expect("hello");
    assert!(!h.paired);
    assert_eq!(h.fingerprint_prefix, lab.host_id.fingerprint().prefix());
    assert_eq!(h.app_version, None);
    assert_eq!(h.engine_commit, None);
    assert_eq!(h.generation, None);
    let mut s = tls_to_proxy(&lab, &stranger).await;
    let resp = raw(&mut s, b"GET /api/v1/accounts HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
    assert!(resp.contains("LAN_NOT_PAIRED"));
    assert_eq!(lab.seen.lock().expect("lock").requests.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_code_and_denied_pairing_are_refused() {
    let lab = lab().await;
    let device = Identity::generate().expect("id");
    let code = lab
        .pairing
        .new_code(&lab.host_id.fingerprint())
        .expect("code");
    let wrong = PairingCode {
        pin: code.pin,
        secret: code.secret ^ 7,
    };
    assert_eq!(
        client::pair(lab.addr, wrong, &device, "x").await,
        Err(CallError::Pair(PairRefusal::WrongCode))
    );
    // A code whose pin is not this host's never even connects.
    let other = PairingCode {
        pin: code.pin ^ 1,
        secret: code.secret,
    };
    assert_eq!(
        client::pair(lab.addr, other, &device, "x").await,
        Err(CallError::Unreachable)
    );
    let p = lab.pairing.clone();
    let clicker = tokio::spawn(async move {
        for _ in 0..200 {
            if let Some(d) = p.pending() {
                return p.decide(d.id, false);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    });
    assert_eq!(
        client::pair(lab.addr, code, &device, "x").await,
        Err(CallError::Pair(PairRefusal::Denied))
    );
    assert!(clicker.await.expect("join"));
    assert!(!lab.registry.contains(&device.fingerprint()));
}

#[tokio::test(flavor = "multi_thread")]
async fn smuggling_shapes_are_refused_and_never_reach_the_engine() {
    let lab = lab().await;
    let device = pair(&lab).await;
    let cases: [&[u8]; 7] = [
        b"POST /api/v1/x HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        b"POST /api/v1/x HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
        b"GET /api/v1/x HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        b"GET http://127.0.0.1/api/v1/x HTTP/1.1\r\nHost: x\r\n\r\n",
        b"CONNECT 127.0.0.1:80 HTTP/1.1\r\nHost: 127.0.0.1:80\r\n\r\n",
        b"GET /api/v1/x HTTP/1.1\r\nHost: x\r\nOrigin: http://evil\r\n\r\n",
        b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n",
    ];
    for c in cases {
        let mut s = tls_to_proxy(&lab, &device).await;
        let resp = raw(&mut s, c).await;
        assert!(
            resp.starts_with("HTTP/1.1 400") || resp.starts_with("HTTP/1.1 403"),
            "{:?} -> {resp}",
            String::from_utf8_lossy(c)
        );
    }
    assert_eq!(lab.seen.lock().expect("lock").requests.len(), 0);

    // A second request on the same connection carrying a spoofed token is
    // never read: the engine sees exactly one request, with the LAN token.
    let mut s = tls_to_proxy(&lab, &device).await;
    let resp = raw(
        &mut s,
        b"GET /api/v1/a HTTP/1.1\r\nHost: x\r\n\r\nGET /api/v1/b HTTP/1.1\r\nHost: x\r\nX-CL-Token: host-token\r\n\r\n",
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert_eq!(resp.matches("HTTP/1.1").count(), 1, "{resp}");
    let seen = lab.seen.lock().expect("lock").clone();
    assert_eq!(seen.requests.len(), 1);
    assert_eq!(seen.requests[0]["path"], "/api/v1/a");
    assert_eq!(seen.requests[0]["token"], serde_json::json!([LAN_TOKEN]));
}

#[tokio::test(flavor = "multi_thread")]
async fn forwarder_checks_token_origin_and_host() {
    let lab = lab().await;
    let device = pair(&lab).await;
    let ws = workstation(&lab, &device).await;
    let mut s = TcpStream::connect(("127.0.0.1", ws.port()))
        .await
        .expect("c");
    let r = raw(
        &mut s,
        format!(
            "GET /api/v1/x HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-CL-Token: wrong\r\n\r\n",
            ws.port()
        )
        .as_bytes(),
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    let r = via_forwarder(&ws, "Origin: http://evil.example\r\n", "/api/v1/x").await;
    assert!(r.starts_with("HTTP/1.1 403"), "{r}");
    let mut s = TcpStream::connect(("127.0.0.1", ws.port()))
        .await
        .expect("c");
    let r = raw(
        &mut s,
        format!(
            "GET /api/v1/x HTTP/1.1\r\nHost: evil.example:{}\r\nX-CL-Token: {}\r\n\r\n",
            ws.port(),
            ws.local_token().expose()
        )
        .as_bytes(),
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 403"), "{r}");
    assert_eq!(lab.seen.lock().expect("lock").requests.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn pin_mismatch_is_refused() {
    let lab = lab().await;
    let device = pair(&lab).await;
    let wrong = Fingerprint::of_der(b"not the host");
    assert_eq!(
        client::hello(lab.addr, HostPin::Full(wrong), &device).await,
        Err(CallError::Unreachable)
    );
}

fn certified(cert: &Identity, key_of: &Identity) -> Arc<CertifiedKey> {
    let provider = tls::provider();
    let key = provider
        .key_provider
        .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            key_of.key_pkcs8().to_vec(),
        )))
        .expect("key");
    Arc::new(CertifiedKey::new(
        vec![CertificateDer::from(cert.cert_der().to_vec())],
        key,
    ))
}

#[derive(Debug)]
struct Fixed(Arc<CertifiedKey>);
impl rustls::server::ResolvesServerCert for Fixed {
    fn resolve(&self, _: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}
impl rustls::client::ResolvesClientCert for Fixed {
    fn resolve(&self, _: &[&[u8]], _: &[rustls::SignatureScheme]) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
    fn has_certs(&self) -> bool {
        true
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_certificate_with_the_wrong_key_is_refused() {
    let host = Identity::generate().expect("host");
    let attacker = Identity::generate().expect("attacker");
    let device = Identity::generate().expect("device");
    // A server presenting the host's public certificate, signing with
    // another key.
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(tls::provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("v")
        .with_client_cert_verifier(Arc::new(tls::DeviceCertVerifier::default()))
        .with_cert_resolver(Arc::new(Fixed(certified(&host, &attacker))));
    let mut cfg = cfg;
    cfg.alpn_protocols = vec![tls::ALPN.to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            let a = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut t) = a.accept(s).await {
                    let _ = t
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                        .await;
                }
            });
        }
    });
    assert_eq!(
        client::hello(addr, HostPin::Full(host.fingerprint()), &device).await,
        Err(CallError::Unreachable)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn device_certificate_with_the_wrong_key_is_refused() {
    let lab = lab().await;
    let device = pair(&lab).await;
    let attacker = Identity::generate().expect("attacker");
    let verifier = Arc::new(tls::PinnedHostVerifier::new(HostPin::Full(
        lab.host_id.fingerprint(),
    )));
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(tls::provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("v")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_cert_resolver(Arc::new(Fixed(certified(&device, &attacker))));
    cfg.alpn_protocols = vec![tls::ALPN.to_vec()];
    let tcp = TcpStream::connect(lab.addr).await.expect("tcp");
    let result = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(tls::server_name(), tcp)
        .await;
    // TLS 1.3: the client may finish before the server rejects its
    // CertificateVerify, so also require that no request gets through.
    if let Ok(mut s) = result {
        let resp = raw(&mut s, b"GET /api/v1/x HTTP/1.1\r\nHost: x\r\n\r\n").await;
        assert!(!resp.contains("200"), "{resp}");
    }
    assert_eq!(lab.seen.lock().expect("lock").requests.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn removed_device_is_refused_at_once_and_its_stream_closes() {
    let lab = lab().await;
    let device = pair(&lab).await;
    // An open event stream through the proxy.
    let mut s = tls_to_proxy(&lab, &device).await;
    s.write_all(b"GET /api/v1/stream HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\n\r\n")
        .await
        .expect("w");
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
        .await
        .expect("first event in time")
        .expect("read");
    let first = String::from_utf8_lossy(&buf[..n]).into_owned();
    assert!(first.contains("200"), "{first}");
    // Streaming: the first event arrived although the engine holds the
    // stream open for 30 s.
    let mut got = first.clone();
    if !got.contains("data: one") {
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
            .await
            .expect("t")
            .expect("r");
        got.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    assert!(got.contains("data: one"), "{got}");
    assert!(lab.registry.remove(&device.fingerprint()).expect("remove"));
    // The stream is closed by the host within moments.
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match s.read(&mut buf).await {
                Ok(0) | Err(_) => return true,
                Ok(_) => {}
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(closed, "stream still open after Remove");
    let mut s2 = tls_to_proxy(&lab, &device).await;
    let r = raw(&mut s2, b"GET /api/v1/x HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(r.contains("LAN_NOT_PAIRED"), "{r}");
}

#[tokio::test(flavor = "multi_thread")]
async fn gate_answers_maintenance_and_engine_down() {
    let lab = lab().await;
    let device = pair(&lab).await;
    lab.gate.close(Closed::Maintenance);
    let mut s = tls_to_proxy(&lab, &device).await;
    let r = raw(
        &mut s,
        b"POST /api/v1/x HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
    )
    .await;
    assert!(
        r.starts_with("HTTP/1.1 503") && r.contains("\"MAINTENANCE\""),
        "{r}"
    );
    lab.gate.close(Closed::EngineDown);
    let mut s = tls_to_proxy(&lab, &device).await;
    let r = raw(&mut s, b"GET /api/v1/x HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(r.contains("HOST_ENGINE_DOWN"), "{r}");
    assert_eq!(lab.seen.lock().expect("lock").requests.len(), 0);
    lab.gate.open();
    let h = client::hello(lab.addr, HostPin::Full(lab.host_id.fingerprint()), &device)
        .await
        .expect("hello");
    assert_eq!(h.engine.as_deref(), Some("ready"));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_gone_answers_lan_not_sent_and_the_link_goes_down() {
    let mut lab = lab().await;
    let device = pair(&lab).await;
    let ws = workstation(&lab, &device).await;
    if let Some(p) = lab.proxy.take() {
        p.shutdown().await;
    }
    // Before the heartbeat notices: the connect fails, nothing was sent.
    let r = via_forwarder(&ws, "", "/api/v1/journal-entries").await;
    assert!(
        r.starts_with("HTTP/1.1 503") && r.contains("LAN_NOT_SENT"),
        "{r}"
    );
    assert_eq!(ws.check_now().await, LinkState::HostDown);
    let r = via_forwarder(&ws, "", "/api/v1/journal-entries").await;
    assert!(r.contains("LAN_NOT_SENT"), "{r}");
    assert_eq!(lab.seen.lock().expect("lock").requests.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn version_mismatch_blocks_the_link() {
    let lab = lab().await;
    let device = pair(&lab).await;
    let ws = Workstation::start(
        WorkstationConfig {
            host: lab.addr,
            host_fingerprint: lab.host_id.fingerprint(),
            app_version: "9.9.8".into(),
            engine_commit: COMMIT.into(),
        },
        &device,
    )
    .await
    .expect("ws");
    assert!(matches!(
        ws.check_now().await,
        LinkState::UpdateNeeded { .. }
    ));
    let r = via_forwarder(&ws, "", "/api/v1/x").await;
    assert!(r.contains("LAN_UPDATE_NEEDED"), "{r}");
    assert!(matches!(
        cl_api::EndpointSource::current(&ws.endpoint_source()),
        cl_api::EndpointState::Down { permanent: true }
    ));
}
