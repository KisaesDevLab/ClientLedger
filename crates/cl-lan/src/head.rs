//! The raw request-head check in front of hyper (review B1 fix 4).
//!
//! hyper's HTTP/1 server quietly normalises some ambiguous framings before
//! a handler can see them (it drops `Content-Length` when
//! `Transfer-Encoding` is present, and folds identical duplicate
//! `Content-Length` lines). The proxy must *refuse* those shapes, so it
//! reads the first request head itself, checks it, and only then hands the
//! bytes to hyper. Each connection carries exactly one request (hyper runs
//! with keep-alive off), so the head checked here is the only head hyper
//! ever parses on that connection: a second, smuggled request on the same
//! connection is never read.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

/// Largest request head accepted.
pub const MAX_HEAD: usize = 16 * 1024;
/// Most header lines accepted.
const MAX_HEADERS: usize = 64;
/// Time allowed for the whole head to arrive (plan D1 step 3).
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a request was refused before it reached the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadRefusal {
    /// Not a well-formed HTTP/1.1 request (HTTP/2 prior knowledge included).
    Malformed,
    /// The head was larger than [`MAX_HEAD`].
    TooLarge,
    /// The head did not arrive in time.
    Timeout,
    /// `CONNECT`.
    Connect,
    /// The target is not a plain path (absolute-form, authority-form, `*`).
    NotOriginForm,
    /// An `Upgrade` header.
    Upgrade,
    /// An `Origin` header (a browser page tried to call in).
    Origin,
    /// Both `Content-Length` and `Transfer-Encoding`.
    LengthAndChunked,
    /// More than one `Content-Length`, or one that is not a number.
    BadContentLength,
    /// More than one `Transfer-Encoding`, or one other than `chunked`.
    BadTransferEncoding,
    /// The `Host` header is not this forwarder's loopback address.
    BadHost,
}

impl HeadRefusal {
    /// The engine-shaped error code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            HeadRefusal::Malformed => "LAN_BAD_REQUEST",
            HeadRefusal::TooLarge => "LAN_HEAD_TOO_LARGE",
            HeadRefusal::Timeout => "LAN_HEAD_TIMEOUT",
            HeadRefusal::Connect => "LAN_CONNECT_REFUSED",
            HeadRefusal::NotOriginForm => "LAN_TARGET_REFUSED",
            HeadRefusal::Upgrade => "LAN_UPGRADE_REFUSED",
            HeadRefusal::Origin => "LAN_ORIGIN_REFUSED",
            HeadRefusal::LengthAndChunked
            | HeadRefusal::BadContentLength
            | HeadRefusal::BadTransferEncoding => "LAN_FRAMING_REFUSED",
            HeadRefusal::BadHost => "LAN_HOST_REFUSED",
        }
    }

    /// The HTTP status.
    #[must_use]
    pub fn status(self) -> u16 {
        match self {
            HeadRefusal::TooLarge => 431,
            HeadRefusal::Timeout => 408,
            HeadRefusal::Origin | HeadRefusal::BadHost => 403,
            _ => 400,
        }
    }
}

/// What the proxy needs from a checked head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadInfo {
    /// `GET`, `POST`, ...
    pub method: String,
    /// The path without the query (logs never see the query, M3).
    pub path: String,
    /// The request asked for a server-sent event stream.
    pub wants_stream: bool,
}

/// Which `Host` values are acceptable.
#[derive(Clone, Copy, Debug)]
pub enum HostRule {
    /// The proxy rewrites `Host`; anything is accepted here.
    Any,
    /// The workstation forwarder: `127.0.0.1:<port>` or `localhost:<port>`.
    Loopback(u16),
}

/// Checks a complete request head (up to and including the blank line).
///
/// # Errors
/// The first refusal found.
pub fn check_head(head: &[u8], host_rule: HostRule) -> Result<HeadInfo, HeadRefusal> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut headers);
    match req.parse(head) {
        Ok(httparse::Status::Complete(n)) if n == head.len() => {}
        _ => return Err(HeadRefusal::Malformed),
    }
    if req.version != Some(1) {
        return Err(HeadRefusal::Malformed);
    }
    let method = req.method.ok_or(HeadRefusal::Malformed)?;
    if method.eq_ignore_ascii_case("CONNECT") {
        return Err(HeadRefusal::Connect);
    }
    let target = req.path.ok_or(HeadRefusal::Malformed)?;
    if !target.starts_with('/') || target.starts_with("//") {
        return Err(HeadRefusal::NotOriginForm);
    }
    let mut content_length = 0usize;
    let mut transfer_encoding = 0usize;
    let mut hosts = Vec::new();
    let mut wants_stream = false;
    for h in req.headers.iter() {
        let name = h.name;
        if name.eq_ignore_ascii_case("upgrade") || name.eq_ignore_ascii_case("http2-settings") {
            return Err(HeadRefusal::Upgrade);
        }
        if name.eq_ignore_ascii_case("origin") {
            return Err(HeadRefusal::Origin);
        }
        if name.eq_ignore_ascii_case("content-length") {
            content_length += 1;
            let v = std::str::from_utf8(h.value).map_err(|_| HeadRefusal::BadContentLength)?;
            if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) || v.len() > 15 {
                return Err(HeadRefusal::BadContentLength);
            }
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            transfer_encoding += 1;
            if !h.value.eq_ignore_ascii_case(b"chunked") {
                return Err(HeadRefusal::BadTransferEncoding);
            }
        }
        if name.eq_ignore_ascii_case("host") {
            hosts.push(h.value);
        }
        if name.eq_ignore_ascii_case("accept")
            && std::str::from_utf8(h.value)
                .is_ok_and(|v| v.to_ascii_lowercase().contains("text/event-stream"))
        {
            wants_stream = true;
        }
    }
    if content_length > 0 && transfer_encoding > 0 {
        return Err(HeadRefusal::LengthAndChunked);
    }
    if content_length > 1 {
        return Err(HeadRefusal::BadContentLength);
    }
    if transfer_encoding > 1 {
        return Err(HeadRefusal::BadTransferEncoding);
    }
    if hosts.len() != 1 {
        return Err(HeadRefusal::Malformed);
    }
    if let HostRule::Loopback(port) = host_rule {
        let want_a = format!("127.0.0.1:{port}");
        let want_b = format!("localhost:{port}");
        let h = hosts[0];
        if !(h.eq_ignore_ascii_case(want_a.as_bytes()) || h.eq_ignore_ascii_case(want_b.as_bytes()))
        {
            return Err(HeadRefusal::BadHost);
        }
    }
    let path = target.split('?').next().unwrap_or("/").to_string();
    Ok(HeadInfo {
        method: method.to_string(),
        path,
        wants_stream,
    })
}

/// Reads bytes until the end of the first request head. Returns everything
/// read (the head and any body bytes that came with it) and the head length.
///
/// # Errors
/// `TooLarge`, `Timeout`, `Malformed` (the peer closed early).
pub async fn read_head<S: AsyncRead + Unpin>(s: &mut S) -> Result<(Vec<u8>, usize), HeadRefusal> {
    let fut = async {
        let mut buf = Vec::with_capacity(1024);
        let mut chunk = [0u8; 2048];
        loop {
            let n = s
                .read(&mut chunk)
                .await
                .map_err(|_| HeadRefusal::Malformed)?;
            if n == 0 {
                return Err(HeadRefusal::Malformed);
            }
            let start = buf.len().saturating_sub(3);
            buf.extend_from_slice(&chunk[..n]);
            if let Some(i) = find_blank_line(&buf[start..]) {
                let end = start + i + 4;
                if end > MAX_HEAD {
                    return Err(HeadRefusal::TooLarge);
                }
                return Ok((buf, end));
            }
            if buf.len() > MAX_HEAD {
                return Err(HeadRefusal::TooLarge);
            }
        }
    };
    tokio::time::timeout(HEAD_TIMEOUT, fut)
        .await
        .map_err(|_| HeadRefusal::Timeout)?
}

fn find_blank_line(b: &[u8]) -> Option<usize> {
    b.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Writes an engine-shaped error response and closes.
pub async fn write_refusal<S: AsyncWrite + Unpin>(
    s: &mut S,
    status: u16,
    code: &str,
    message: &str,
) {
    let body = error_body(code, message);
    let reason = match status {
        400 => "Bad Request",
        403 => "Forbidden",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes()).await;
    let _ = s.write_all(body.as_bytes()).await;
    let _ = s.shutdown().await;
}

/// The engine's error shape: `{"error":{"message":..,"code":..}}`.
#[must_use]
pub fn error_body(code: &str, message: &str) -> String {
    serde_json::json!({ "error": { "message": message, "code": code } }).to_string()
}

/// A stream that first yields bytes already read, then the rest.
#[derive(Debug)]
pub struct Prefixed<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    /// Wraps `inner`, replaying `prefix` first.
    pub fn new(prefix: Vec<u8>, inner: S) -> Prefixed<S> {
        Prefixed {
            prefix,
            pos: 0,
            inner,
        }
    }

    /// The wrapped stream.
    pub fn inner(&self) -> &S {
        &self.inner
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.pos < this.prefix.len() {
            let n = (this.prefix.len() - this.pos).min(buf.remaining());
            buf.put_slice(&this.prefix[this.pos..this.pos + n]);
            this.pos += n;
            if this.pos == this.prefix.len() {
                this.prefix = Vec::new();
                this.pos = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        s.replace('\n', "\r\n").into_bytes()
    }

    fn check(s: &str) -> Result<HeadInfo, HeadRefusal> {
        check_head(&h(s), HostRule::Any)
    }

    #[test]
    fn plain_requests_pass_and_drop_the_query() {
        let i =
            check("POST /api/v1/x?q=secret HTTP/1.1\nHost: a\nContent-Length: 2\n\n").expect("ok");
        assert_eq!(i.method, "POST");
        assert_eq!(i.path, "/api/v1/x");
        assert!(!i.wants_stream);
        let s = check("GET /s HTTP/1.1\nHost: a\nAccept: text/event-stream\n\n").expect("ok");
        assert!(s.wants_stream);
        assert!(check("POST /x HTTP/1.1\nHost: a\nTransfer-Encoding: chunked\n\n").is_ok());
    }

    #[test]
    fn smuggling_shapes_are_refused() {
        let cases = [
            (
                "POST /x HTTP/1.1\nHost: a\nContent-Length: 4\nTransfer-Encoding: chunked\n\n",
                HeadRefusal::LengthAndChunked,
            ),
            (
                "POST /x HTTP/1.1\nHost: a\nTransfer-Encoding: chunked\nContent-Length: 4\n\n",
                HeadRefusal::LengthAndChunked,
            ),
            (
                "POST /x HTTP/1.1\nHost: a\nContent-Length: 4\nContent-Length: 4\n\n",
                HeadRefusal::BadContentLength,
            ),
            (
                "POST /x HTTP/1.1\nHost: a\nContent-Length: +4\n\n",
                HeadRefusal::BadContentLength,
            ),
            (
                "POST /x HTTP/1.1\nHost: a\nTransfer-Encoding: gzip, chunked\n\n",
                HeadRefusal::BadTransferEncoding,
            ),
            (
                "POST /x HTTP/1.1\nHost: a\nTransfer-Encoding: chunked\nTransfer-Encoding: chunked\n\n",
                HeadRefusal::BadTransferEncoding,
            ),
            (
                "GET /x HTTP/1.1\nHost: a\nUpgrade: websocket\nConnection: Upgrade\n\n",
                HeadRefusal::Upgrade,
            ),
            (
                "GET /x HTTP/1.1\nHost: a\nHTTP2-Settings: AAA\n\n",
                HeadRefusal::Upgrade,
            ),
            (
                "GET http://evil/x HTTP/1.1\nHost: a\n\n",
                HeadRefusal::NotOriginForm,
            ),
            (
                "GET //evil/x HTTP/1.1\nHost: a\n\n",
                HeadRefusal::NotOriginForm,
            ),
            (
                "OPTIONS * HTTP/1.1\nHost: a\n\n",
                HeadRefusal::NotOriginForm,
            ),
            (
                "CONNECT evil:443 HTTP/1.1\nHost: evil:443\n\n",
                HeadRefusal::Connect,
            ),
            (
                "GET /x HTTP/1.1\nHost: a\nOrigin: http://evil\n\n",
                HeadRefusal::Origin,
            ),
            ("PRI * HTTP/2.0\n\nSM\n\n", HeadRefusal::Malformed),
            ("GET /x HTTP/1.0\nHost: a\n\n", HeadRefusal::Malformed),
            ("GET /x HTTP/1.1\n\n", HeadRefusal::Malformed),
            (
                "GET /x HTTP/1.1\nHost: a\nHost: b\n\n",
                HeadRefusal::Malformed,
            ),
            (
                "GET /x HTTP/1.1\nHost: a\n folded\n\n",
                HeadRefusal::Malformed,
            ),
        ];
        for (s, want) in cases {
            assert_eq!(check(s), Err(want), "{s:?}");
        }
    }

    #[test]
    fn forwarder_host_rule() {
        let ok = h("GET /x HTTP/1.1\nHost: 127.0.0.1:4100\n\n");
        assert!(check_head(&ok, HostRule::Loopback(4100)).is_ok());
        let lh = h("GET /x HTTP/1.1\nHost: localhost:4100\n\n");
        assert!(check_head(&lh, HostRule::Loopback(4100)).is_ok());
        for bad in ["evil.example:4100", "127.0.0.1:4101", "127.0.0.1"] {
            let b = h(&format!("GET /x HTTP/1.1\nHost: {bad}\n\n"));
            assert_eq!(
                check_head(&b, HostRule::Loopback(4100)),
                Err(HeadRefusal::BadHost),
                "{bad}"
            );
        }
    }

    #[tokio::test]
    async fn read_head_returns_head_and_extra_bytes() {
        let data = h("POST /x HTTP/1.1\nHost: a\nContent-Length: 3\n\n");
        let mut all = data.clone();
        all.extend_from_slice(b"abc");
        let mut r = &all[..];
        let (buf, n) = read_head(&mut r).await.expect("head");
        assert_eq!(n, data.len());
        assert_eq!(&buf[n..], b"abc");
        let big = vec![b'a'; MAX_HEAD + 10];
        let mut r = &big[..];
        assert_eq!(read_head(&mut r).await, Err(HeadRefusal::TooLarge));
        let mut r: &[u8] = b"GET / HTTP/1.1\r\n";
        assert_eq!(read_head(&mut r).await, Err(HeadRefusal::Malformed));
    }

    #[tokio::test]
    async fn prefixed_replays_then_reads() {
        let mut p = Prefixed::new(b"abc".to_vec(), &b"def"[..]);
        let mut out = String::new();
        p.read_to_string(&mut out).await.expect("read");
        assert_eq!(out, "abcdef");
    }
}
