//! What both hops share: header rebuilding, one fresh upstream connection
//! per request with no retry, and engine-shaped error responses.

use std::net::SocketAddr;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};

/// A response body: streamed from upstream, or a small local JSON value.
pub type Body = BoxBody<Bytes, hyper::Error>;

/// Hop-by-hop headers (RFC 9110 §7.6.1) plus the proxy ones.
const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// True for headers that carry trust or routing and must never come from
/// the client side: they are dropped and, where needed, set by this hop.
fn is_trust_header(name: &str) -> bool {
    name.starts_with("x-cl-")
        || name.starts_with("x-forwarded-")
        || name == "forwarded"
        || name == "x-real-ip"
        || name == "host"
}

/// Copies `src` without hop-by-hop headers, headers named in `Connection`,
/// trust headers and `Content-Length` when `keep_length` is false.
#[must_use]
pub fn rebuild_headers(src: &HeaderMap, keep_length: bool) -> HeaderMap {
    let named: Vec<String> = src
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let mut out = HeaderMap::with_capacity(src.len());
    for (name, value) in src {
        let n = name.as_str();
        if HOP_BY_HOP.contains(&n)
            || named.iter().any(|x| x == n)
            || is_trust_header(n)
            || (!keep_length && n == "content-length")
        {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// Sets a header from a string this crate made (never user input).
pub fn set(headers: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        headers.insert(HeaderName::from_static(name), v);
    }
}

/// An engine-shaped JSON error response that closes the connection.
#[must_use]
pub fn error_response(status: u16, code: &str, message: &str) -> Response<Body> {
    json_response(status, crate::head::error_body(code, message))
}

/// A JSON response that closes the connection.
#[must_use]
pub fn json_response(status: u16, body: String) -> Response<Body> {
    let mut r = Response::new(
        Full::new(Bytes::from(body))
            .map_err(|never| match never {})
            .boxed(),
    );
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    set(r.headers_mut(), "content-type", "application/json");
    set(r.headers_mut(), "connection", "close");
    r
}

/// Turns an upstream response into the downstream one: streamed body,
/// hop-by-hop headers dropped, connection closed after it.
#[must_use]
pub fn relay_response(resp: Response<Incoming>) -> Response<Body> {
    let (mut parts, body) = resp.into_parts();
    let keep_length = !parts.headers.contains_key(header::TRANSFER_ENCODING);
    let mut headers = rebuild_headers(&parts.headers, keep_length);
    // Response trust headers do not exist; `host` never appears. Restore
    // anything the engine sends under an `x-cl-` name (none today) is not
    // needed by clients, so they stay dropped.
    set(&mut headers, "connection", "close");
    parts.headers = headers;
    Response::from_parts(parts, body.boxed())
}

/// Why an upstream exchange failed.
#[derive(Debug)]
pub enum UpstreamError {
    /// Nothing was sent: the connection (or TLS handshake) failed.
    NotSent,
    /// The request may have reached the other side.
    Sent,
}

/// Sends one request over a fresh connection `io` (no pool, no retry) and
/// returns the response head; the body streams afterwards.
///
/// # Errors
/// `NotSent` when the HTTP handshake fails, `Sent` afterwards.
pub async fn send_once<IO, B>(io: IO, req: Request<B>) -> Result<Response<Incoming>, UpstreamError>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: hyper::body::Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let (mut sender, conn) = hyper::client::conn::http1::Builder::new()
        .handshake::<_, B>(TokioIo::new(io))
        .await
        .map_err(|_| UpstreamError::NotSent)?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    sender
        .send_request(req)
        .await
        .map_err(|_| UpstreamError::Sent)
}

/// Builds the upstream request: same method, path and query, rebuilt
/// headers, plus `extra` headers set by this hop and `Host`.
///
/// # Errors
/// When the parts cannot form a request (never for a parsed request).
pub fn upstream_request(
    req: Request<Incoming>,
    host: &str,
    extra: &[(&'static str, &str)],
) -> Result<Request<Incoming>, hyper::http::Error> {
    let (parts, body) = req.into_parts();
    let keep_length = !parts.headers.contains_key(header::TRANSFER_ENCODING);
    let mut headers = rebuild_headers(&parts.headers, keep_length);
    for (k, v) in extra {
        set(&mut headers, k, v);
    }
    set(&mut headers, "host", host);
    set(&mut headers, "connection", "close");
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", hyper::http::uri::PathAndQuery::as_str)
        .to_string();
    let mut out = Request::builder()
        .method(parts.method)
        .uri(path)
        .version(hyper::Version::HTTP_11)
        .body(body)?;
    *out.headers_mut() = headers;
    Ok(out)
}

/// `127.0.0.1:port`.
#[must_use]
pub fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuild_drops_hop_by_hop_trust_and_connection_named_headers() {
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("connection", "keep-alive, X-CL-Device, x-custom"),
            ("keep-alive", "1"),
            ("x-custom", "drop-me"),
            ("x-cl-token", "spoof"),
            ("X-CL-Device", "spoof"),
            ("x-forwarded-for", "1.2.3.4"),
            ("forwarded", "for=1.2.3.4"),
            ("x-real-ip", "1.2.3.4"),
            ("host", "evil"),
            ("te", "trailers"),
            ("authorization", "Bearer x"),
            ("content-type", "application/json"),
            ("content-length", "2"),
            ("idempotency-key", "k"),
        ] {
            h.append(
                HeaderName::from_bytes(k.as_bytes()).expect("name"),
                HeaderValue::from_static(v),
            );
        }
        let out = rebuild_headers(&h, true);
        let names: Vec<&str> = out.keys().map(HeaderName::as_str).collect();
        assert_eq!(
            names,
            vec![
                "authorization",
                "content-type",
                "content-length",
                "idempotency-key"
            ]
        );
        assert!(!rebuild_headers(&h, false).contains_key("content-length"));
    }
}
