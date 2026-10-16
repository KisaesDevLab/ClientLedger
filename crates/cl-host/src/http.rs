//! A very small HTTP/1.1 client for the engine on loopback.
//!
//! The supervisor only needs `GET /health`, `GET /api/v1/desktop/info` and one
//! `POST`, all to `127.0.0.1`. `cl-api` (P05) is the real client; keeping this
//! one tiny avoids an HTTP stack in the supervisor. Every request carries
//! `X-CL-Token`; the engine's desktop gate also checks `Host`.

use cl_pg::Secret;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// Largest response body read; the endpoints used here return a few hundred bytes.
const MAX_RESPONSE: u64 = 1 << 20;

/// A parsed response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Response {
    pub status: u16,
    pub body: String,
}

/// One request/response exchange with the engine on `port`.
pub(crate) fn request(
    port: u16,
    method: &str,
    path: &str,
    token: &Secret,
    json_body: Option<&str>,
    timeout: Duration,
) -> std::io::Result<Response> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-CL-Token: {}\r\nAccept: application/json\r\nConnection: close\r\n",
        token.expose()
    );
    if let Some(body) = json_body {
        let _ = write!(
            head,
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if let Some(body) = json_body {
        stream.write_all(body.as_bytes())?;
    }
    let mut raw = Vec::new();
    stream.take(MAX_RESPONSE).read_to_end(&mut raw)?;
    parse_response(&raw).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed HTTP response")
    })
}

/// Split a raw HTTP/1.1 response into status and body, undoing chunked
/// transfer encoding.
pub(crate) fn parse_response(raw: &[u8]) -> Option<Response> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let body = &raw[split + 4..];
    let mut lines = head.lines();
    let status: u16 = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let chunked = lines.any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    let body = if chunked {
        decode_chunked(body)?
    } else {
        body.to_vec()
    };
    Some(Response {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn decode_chunked(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = data.windows(2).position(|w| w == b"\r\n")?;
        let size_text = std::str::from_utf8(&data[..eol]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        data = &data[eol + 2..];
        if size == 0 {
            return Some(out);
        }
        if data.len() < size + 2 {
            return None;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_responses_are_parsed() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "{}");
    }

    #[test]
    fn chunked_responses_are_decoded() {
        let raw = b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;x=1\r\n world\r\n0\r\n\r\n";
        let r = parse_response(raw).unwrap();
        assert_eq!(r.status, 201);
        assert_eq!(r.body, "hello world");
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse_response(b"nonsense").is_none());
        assert!(parse_response(b"HTTP/1.1 abc\r\n\r\n").is_none());
        assert!(
            parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n")
                .is_none()
        );
        assert!(
            parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nshort\r\n")
                .is_none()
        );
    }
}
