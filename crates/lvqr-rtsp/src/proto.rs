//! RTSP/1.0 message parser and types (RFC 2326).

use std::collections::HashMap;
use std::fmt;
use std::str;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Options,
    Describe,
    Announce,
    Setup,
    Play,
    Pause,
    Record,
    Teardown,
    GetParameter,
    SetParameter,
}

impl Method {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "OPTIONS" => Some(Self::Options),
            "DESCRIBE" => Some(Self::Describe),
            "ANNOUNCE" => Some(Self::Announce),
            "SETUP" => Some(Self::Setup),
            "PLAY" => Some(Self::Play),
            "PAUSE" => Some(Self::Pause),
            "RECORD" => Some(Self::Record),
            "TEARDOWN" => Some(Self::Teardown),
            "GET_PARAMETER" => Some(Self::GetParameter),
            "SET_PARAMETER" => Some(Self::SetParameter),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Options => "OPTIONS",
            Self::Describe => "DESCRIBE",
            Self::Announce => "ANNOUNCE",
            Self::Setup => "SETUP",
            Self::Play => "PLAY",
            Self::Pause => "PAUSE",
            Self::Record => "RECORD",
            Self::Teardown => "TEARDOWN",
            Self::GetParameter => "GET_PARAMETER",
            Self::SetParameter => "SET_PARAMETER",
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub uri: String,
    pub version: RtspVersion,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Request {
    pub fn cseq(&self) -> Option<u32> {
        self.headers.get("CSeq").and_then(|v| v.parse().ok())
    }

    pub fn session_id(&self) -> Option<&str> {
        self.headers.get("Session").map(|v| {
            // Session header may contain ";timeout=N"
            v.split(';').next().unwrap_or(v).trim()
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtspVersion {
    V1_0,
}

impl fmt::Display for RtspVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V1_0 => f.write_str("RTSP/1.0"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Headers(HashMap<String, String>);

impl Headers {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        let lower = key.to_ascii_lowercase();
        self.0
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == lower)
            .map(|(_, v)| v.as_str())
    }

    pub fn insert(&mut self, key: String, value: String) {
        self.0.insert(key, value);
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub version: RtspVersion,
    pub status: u16,
    pub reason: &'static str,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, reason: &'static str) -> Self {
        Self {
            version: RtspVersion::V1_0,
            status,
            reason,
            headers: Headers::new(),
            body: Vec::new(),
        }
    }

    pub fn ok() -> Self {
        Self::new(200, "OK")
    }

    pub fn not_found() -> Self {
        Self::new(404, "Not Found")
    }

    pub fn method_not_allowed() -> Self {
        Self::new(405, "Method Not Allowed")
    }

    pub fn bad_request() -> Self {
        Self::new(400, "Bad Request")
    }

    pub fn session_not_found() -> Self {
        Self::new(454, "Session Not Found")
    }

    pub fn internal_error() -> Self {
        Self::new(500, "Internal Server Error")
    }

    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.headers.insert(key.to_string(), value.to_string());
        self
    }

    pub fn with_cseq(self, cseq: u32) -> Self {
        self.with_header("CSeq", &cseq.to_string())
    }

    pub fn with_body(mut self, content_type: &str, body: Vec<u8>) -> Self {
        self.headers
            .insert("Content-Length".to_string(), body.len().to_string());
        self.headers
            .insert("Content-Type".to_string(), content_type.to_string());
        self.body = body;
        self
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = format!("{} {} {}\r\n", self.version, self.status, self.reason).into_bytes();
        for (k, v) in &self.headers.0 {
            out.extend_from_slice(k.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(v.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

#[derive(Debug)]
pub enum ParseError {
    Incomplete,
    InvalidRequestLine,
    UnknownMethod(String),
    UnsupportedVersion,
    MalformedHeader,
    BodyTruncated,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete => f.write_str("incomplete message"),
            Self::InvalidRequestLine => f.write_str("invalid request line"),
            Self::UnknownMethod(m) => write!(f, "unknown method: {m}"),
            Self::UnsupportedVersion => f.write_str("unsupported RTSP version"),
            Self::MalformedHeader => f.write_str("malformed header"),
            Self::BodyTruncated => f.write_str("body truncated"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Try to parse one RTSP request from the front of `buf`.
/// Returns `Ok((request, consumed_bytes))` on success,
/// `Err(Incomplete)` if more data is needed, or a hard error
/// for malformed input. The caller should drain `consumed_bytes`
/// from the buffer on success.
pub fn parse_request(buf: &[u8]) -> Result<(Request, usize), ParseError> {
    let text = str::from_utf8(buf).map_err(|_| ParseError::InvalidRequestLine)?;
    let header_end = text.find("\r\n\r\n").ok_or(ParseError::Incomplete)?;
    let header_section = &text[..header_end];
    let mut lines = header_section.lines();

    // Request line: METHOD URI RTSP/1.0
    let request_line = lines.next().ok_or(ParseError::InvalidRequestLine)?;
    let mut parts = request_line.split_whitespace();
    let method_str = parts.next().ok_or(ParseError::InvalidRequestLine)?;
    let uri = parts.next().ok_or(ParseError::InvalidRequestLine)?;
    let version_str = parts.next().ok_or(ParseError::InvalidRequestLine)?;

    let method = Method::parse(method_str).ok_or_else(|| ParseError::UnknownMethod(method_str.to_string()))?;

    if version_str != "RTSP/1.0" {
        return Err(ParseError::UnsupportedVersion);
    }

    let mut headers = Headers::new();
    for line in lines {
        let (key, value) = line.split_once(':').ok_or(ParseError::MalformedHeader)?;
        headers.insert(key.trim().to_string(), value.trim().to_string());
    }

    let body_start = header_end + 4; // past \r\n\r\n
    let content_length: usize = headers.get("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);

    let total = body_start + content_length;
    if buf.len() < total {
        return Err(ParseError::Incomplete);
    }

    let body = buf[body_start..total].to_vec();

    Ok((
        Request {
            method,
            uri: uri.to_string(),
            version: RtspVersion::V1_0,
            headers,
            body,
        },
        total,
    ))
}

/// Parse an RTSP Transport header value into structured fields.
/// Example: "RTP/AVP/TCP;unicast;interleaved=0-1"
#[derive(Debug, Clone, Default)]
pub struct TransportSpec {
    pub protocol: String,
    pub unicast: bool,
    pub interleaved: Option<(u8, u8)>,
    pub client_port: Option<(u16, u16)>,
}

pub fn parse_transport(value: &str) -> TransportSpec {
    let mut spec = TransportSpec::default();
    for part in value.split(';') {
        let part = part.trim();
        if part.contains("RTP/") {
            spec.protocol = part.to_string();
        } else if part == "unicast" {
            spec.unicast = true;
        } else if let Some(rest) = part.strip_prefix("interleaved=") {
            if let Some((a, b)) = rest.split_once('-') {
                spec.interleaved = a.parse::<u8>().ok().zip(b.parse::<u8>().ok());
            }
        } else if let Some(rest) = part.strip_prefix("client_port=") {
            if let Some((a, b)) = rest.split_once('-') {
                spec.client_port = a.parse::<u16>().ok().zip(b.parse::<u16>().ok());
            }
        }
    }
    spec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_options_request() {
        let raw = b"OPTIONS rtsp://localhost:8554/stream RTSP/1.0\r\nCSeq: 1\r\n\r\n";
        let (req, consumed) = parse_request(raw).unwrap();
        assert_eq!(req.method, Method::Options);
        assert_eq!(req.uri, "rtsp://localhost:8554/stream");
        assert_eq!(req.version, RtspVersion::V1_0);
        assert_eq!(req.cseq(), Some(1));
        assert!(req.body.is_empty());
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn parse_describe_request() {
        let raw = b"DESCRIBE rtsp://localhost:8554/live/test RTSP/1.0\r\n\
                     CSeq: 2\r\n\
                     Accept: application/sdp\r\n\r\n";
        let (req, _) = parse_request(raw).unwrap();
        assert_eq!(req.method, Method::Describe);
        assert_eq!(req.headers.get("Accept"), Some("application/sdp"));
    }

    #[test]
    fn parse_setup_with_transport() {
        let raw = b"SETUP rtsp://localhost:8554/stream/track1 RTSP/1.0\r\n\
                     CSeq: 3\r\n\
                     Transport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\r\n";
        let (req, _) = parse_request(raw).unwrap();
        assert_eq!(req.method, Method::Setup);
        let transport = parse_transport(req.headers.get("Transport").unwrap());
        assert_eq!(transport.protocol, "RTP/AVP/TCP");
        assert!(transport.unicast);
        assert_eq!(transport.interleaved, Some((0, 1)));
    }

    #[test]
    fn parse_announce_with_sdp_body() {
        let sdp = "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=Stream\r\n";
        let raw = format!(
            "ANNOUNCE rtsp://localhost:8554/publish/test RTSP/1.0\r\n\
             CSeq: 1\r\n\
             Content-Type: application/sdp\r\n\
             Content-Length: {}\r\n\r\n\
             {}",
            sdp.len(),
            sdp
        );
        let (req, consumed) = parse_request(raw.as_bytes()).unwrap();
        assert_eq!(req.method, Method::Announce);
        assert_eq!(consumed, raw.len());
        assert_eq!(req.body, sdp.as_bytes());
    }

    #[test]
    fn parse_incomplete_returns_error() {
        let raw = b"OPTIONS rtsp://localhost:8554/s RTSP/1.0\r\nCSeq: 1\r\n";
        assert!(matches!(parse_request(raw), Err(ParseError::Incomplete)));
    }

    #[test]
    fn parse_unknown_method() {
        let raw = b"FOOBAR rtsp://localhost/s RTSP/1.0\r\nCSeq: 1\r\n\r\n";
        assert!(matches!(parse_request(raw), Err(ParseError::UnknownMethod(_))));
    }

    #[test]
    fn response_serializes_correctly() {
        let resp = Response::ok()
            .with_cseq(3)
            .with_header("Public", "DESCRIBE, SETUP, PLAY, TEARDOWN");
        let data = resp.serialize();
        let text = str::from_utf8(&data).unwrap();
        assert!(text.starts_with("RTSP/1.0 200 OK\r\n"));
        assert!(text.contains("CSeq: 3\r\n"));
        assert!(text.ends_with("\r\n\r\n"));
    }

    #[test]
    fn response_with_body_includes_content_length() {
        let body = b"v=0\r\no=test\r\n".to_vec();
        let resp = Response::ok().with_cseq(2).with_body("application/sdp", body.clone());
        let data = resp.serialize();
        let text = str::from_utf8(&data).unwrap();
        assert!(text.contains(&format!("Content-Length: {}", body.len())));
        assert!(text.ends_with("v=0\r\no=test\r\n"));
    }

    #[test]
    fn transport_parse_udp_client_port() {
        let spec = parse_transport("RTP/AVP;unicast;client_port=50000-50001");
        assert_eq!(spec.protocol, "RTP/AVP");
        assert!(spec.unicast);
        assert_eq!(spec.client_port, Some((50000, 50001)));
        assert_eq!(spec.interleaved, None);
    }

    #[test]
    fn session_id_strips_timeout() {
        let raw = b"PLAY rtsp://localhost/stream RTSP/1.0\r\n\
                     CSeq: 5\r\n\
                     Session: abc123;timeout=60\r\n\r\n";
        let (req, _) = parse_request(raw).unwrap();
        assert_eq!(req.session_id(), Some("abc123"));
    }
}
