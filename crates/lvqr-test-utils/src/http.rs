//! Shared HTTP helpers for integration tests (session 129).
//!
//! LVQR integration tests historically open a raw `TcpStream`, hand-write
//! the HTTP/1.1 GET, read the response to EOF, and parse the status line
//! by hand. Every test file reimplemented its own variant. This module
//! centralizes the primitive so adopters get:
//!
//! * One timeout policy (configurable via [`HttpGetOptions::timeout`]).
//! * Structured responses that expose status, headers, and body.
//! * Case-insensitive header lookup via [`HttpResponse::header`].
//! * Builder-style options for the common extensions (bearer token,
//!   range request, arbitrary extra headers).
//!
//! The raw-TCP approach stays rather than pulling a full HTTP client
//! (e.g. `reqwest`) so CI does not pay the TLS build-graph cost for
//! localhost integration tests. Every callsite already accepts EOF-
//! terminated responses because the request sets `Connection: close`.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Response from an integration-test HTTP GET. Status, decoded header
/// list, and raw body bytes; test callers pick the view they need.
#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Look up a response header case-insensitively. Returns the first
    /// match in wire order. `None` when the header is absent.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Lossy UTF-8 view of the response body. For binary routes (e.g.
    /// `/playback/file/*` which returns fMP4 bytes), prefer reading
    /// [`HttpResponse::body`] directly.
    pub fn body_text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }
}

/// Optional extensions for an HTTP GET request. Passed to
/// [`http_get_with`]; the bare [`http_get`] convenience wrapper uses
/// the `Default` values (no bearer, no range, default timeout).
#[derive(Debug, Clone)]
pub struct HttpGetOptions<'a> {
    /// Emit `Authorization: Bearer <token>` when `Some`.
    pub bearer: Option<&'a str>,
    /// Emit `Range: <spec>` when `Some` (e.g. `"bytes=0-1023"`).
    pub range: Option<&'a str>,
    /// Any additional headers beyond `Host` + `Connection: close` +
    /// the `bearer` / `range` pair above. Names are emitted verbatim.
    pub extra_headers: Vec<(&'a str, &'a str)>,
    /// Per-connect + per-read timeout ceiling. Defaults to 5 s; raise
    /// for routes that wait on a live publisher to produce a segment.
    pub timeout: Duration,
}

impl<'a> Default for HttpGetOptions<'a> {
    fn default() -> Self {
        Self {
            bearer: None,
            range: None,
            extra_headers: Vec::new(),
            timeout: Duration::from_secs(5),
        }
    }
}

impl<'a> HttpGetOptions<'a> {
    /// Convenience: option set with only the bearer header.
    pub fn with_bearer(bearer: &'a str) -> Self {
        Self {
            bearer: Some(bearer),
            ..Self::default()
        }
    }

    /// Convenience: option set with only the range header.
    pub fn with_range(range: &'a str) -> Self {
        Self {
            range: Some(range),
            ..Self::default()
        }
    }
}

/// Open a TCP connection, send `GET <path> HTTP/1.1`, and return the
/// parsed response. Uses the default [`HttpGetOptions`] (no bearer, no
/// range, 5 s timeout).
pub async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    http_get_with(addr, path, HttpGetOptions::default()).await
}

/// Open a TCP connection, send `GET <path> HTTP/1.1` with the requested
/// extensions, and return the parsed response. Panics with a clear
/// message on connect / write / read / parse failure so integration
/// tests surface a readable trace instead of a swallowed `?`.
pub async fn http_get_with(addr: SocketAddr, path: &str, opts: HttpGetOptions<'_>) -> HttpResponse {
    let mut stream = tokio::time::timeout(opts.timeout, TcpStream::connect(addr))
        .await
        .expect("http_get: connect timed out")
        .expect("http_get: connect failed");

    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(token) = opts.bearer {
        req.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(spec) = opts.range {
        req.push_str(&format!("Range: {spec}\r\n"));
    }
    for (k, v) in &opts.extra_headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).await.expect("http_get: write failed");

    let mut raw = Vec::with_capacity(4096);
    tokio::time::timeout(opts.timeout, stream.read_to_end(&mut raw))
        .await
        .expect("http_get: read timed out")
        .expect("http_get: read failed");

    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http_get: response missing header terminator");
    let header_text = std::str::from_utf8(&raw[..split]).expect("http_get: headers are not utf-8");
    let mut lines = header_text.lines();
    let status_line = lines.next().expect("http_get: response missing status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("http_get: could not parse status line: {status_line:?}"));
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (k, v) = line.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let body = raw[split + 4..].to_vec();
    HttpResponse { status, headers, body }
}

/// Convenience wrapper: returns only the status code. Equivalent to
/// `http_get(addr, path).await.status` for tests that assert solely on
/// the auth / route dispatch outcome.
pub async fn http_get_status(addr: SocketAddr, path: &str) -> u16 {
    http_get(addr, path).await.status
}
