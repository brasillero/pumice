//! Minimal HTTP/1.1 request parsing for the logging listener.
//!
//! Only what the prototype needs: a request line, headers, and a body read
//! with either `Content-Length` or `Transfer-Encoding: chunked`. One request
//! is handled per connection and the response always carries
//! `Connection: close`.

use std::io::{BufRead, BufReader, Read};

/// Maximum accepted request body size (10 MiB); larger bodies are rejected.
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Maximum accepted length of a single head line (request line or header).
const MAX_LINE_BYTES: usize = 64 * 1024;

/// The parsed head of an HTTP request.
#[derive(Debug, Clone)]
pub struct RequestHead {
    pub method: String,
    /// The request target as received, including any query string.
    pub target: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
}

impl RequestHead {
    /// Returns the first header value for `name`, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The request path without the query string, for routing.
    pub fn path(&self) -> &str {
        match self.target.find('?') {
            Some(i) => &self.target[..i],
            None => &self.target,
        }
    }
}

/// A fully parsed HTTP request.
#[derive(Debug, Clone)]
pub struct Request {
    pub head: RequestHead,
    pub body: Vec<u8>,
}

/// Result of reading one request from a connection.
pub enum ParseOutcome {
    /// A complete request was read.
    Ok(Request),
    /// The head parsed but the body exceeded [`MAX_BODY_BYTES`].
    BodyTooLarge(RequestHead),
    /// The request was malformed and could not be parsed.
    BadRequest(String),
    /// The client connected and disconnected without sending anything.
    Empty,
}

/// Reads one HTTP request from `reader`.
pub fn read_request<R: Read>(reader: &mut BufReader<R>) -> ParseOutcome {
    let head = match read_head(reader) {
        Ok(Some(head)) => head,
        Ok(None) => return ParseOutcome::Empty,
        Err(message) => return ParseOutcome::BadRequest(message),
    };
    match read_body(reader, &head) {
        Ok(body) => ParseOutcome::Ok(Request { head, body }),
        Err(BodyError::TooLarge) => ParseOutcome::BodyTooLarge(head),
        Err(BodyError::Bad(message)) => ParseOutcome::BadRequest(message),
    }
}

enum BodyError {
    TooLarge,
    Bad(String),
}

impl From<String> for BodyError {
    fn from(message: String) -> Self {
        BodyError::Bad(message)
    }
}

fn read_head<R: Read>(reader: &mut BufReader<R>) -> Result<Option<RequestHead>, String> {
    let request_line = match read_line(reader)? {
        Some(line) => line,
        None => return Ok(None),
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let version = parts.next().unwrap_or_default().to_string();
    if method.is_empty() || target.is_empty() || version.is_empty() {
        return Err(format!("malformed request line: {request_line:?}"));
    }

    let mut headers = Vec::new();
    loop {
        let line = read_line(reader)?.ok_or_else(|| "unexpected EOF in headers".to_string())?;
        if line.is_empty() {
            break;
        }
        if let Some(colon) = line.find(':') {
            let name = line[..colon].trim();
            let value = line[colon + 1..].trim();
            if !name.is_empty() {
                headers.push((name.to_string(), value.to_string()));
            }
        }
        // Lines without a colon are tolerated and skipped.
    }

    Ok(Some(RequestHead {
        method,
        target,
        version,
        headers,
    }))
}

fn read_body<R: Read>(reader: &mut BufReader<R>, head: &RequestHead) -> Result<Vec<u8>, BodyError> {
    if let Some(te) = head.header("transfer-encoding") {
        let chunked = te
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("chunked"));
        if !chunked {
            return Err(BodyError::Bad(format!(
                "unsupported transfer-encoding: {te}"
            )));
        }
        return read_chunked(reader);
    }
    if let Some(content_length) = head.header("content-length") {
        let n: usize = content_length
            .trim()
            .parse()
            .map_err(|_| BodyError::Bad(format!("invalid content-length: {content_length}")))?;
        if n > MAX_BODY_BYTES {
            return Err(BodyError::TooLarge);
        }
        let mut body = vec![0u8; n];
        reader
            .read_exact(&mut body)
            .map_err(|e| BodyError::Bad(format!("failed to read request body: {e}")))?;
        return Ok(body);
    }
    Ok(Vec::new())
}

fn read_chunked<R: Read>(reader: &mut BufReader<R>) -> Result<Vec<u8>, BodyError> {
    let mut body = Vec::new();
    loop {
        let line = read_line(reader)?
            .ok_or_else(|| BodyError::Bad("unexpected EOF in chunked body".to_string()))?;
        let size_field = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_field, 16)
            .map_err(|_| BodyError::Bad(format!("invalid chunk size: {size_field:?}")))?;
        if size == 0 {
            // Consume optional trailer fields up to the blank line.
            loop {
                let trailer = read_line(reader)?
                    .ok_or_else(|| BodyError::Bad("unexpected EOF in trailers".to_string()))?;
                if trailer.is_empty() {
                    return Ok(body);
                }
            }
        }
        if body.len() + size > MAX_BODY_BYTES {
            return Err(BodyError::TooLarge);
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader
            .read_exact(&mut body[start..])
            .map_err(|e| BodyError::Bad(format!("failed to read chunk: {e}")))?;
        let mut crlf = [0u8; 2];
        reader
            .read_exact(&mut crlf)
            .map_err(|e| BodyError::Bad(format!("failed to read chunk ending: {e}")))?;
        if crlf != *b"\r\n" {
            return Err(BodyError::Bad("malformed chunk ending".to_string()));
        }
    }
}

/// Reads one CRLF-terminated line, without the terminator.
///
/// Returns `Ok(None)` on a clean EOF before any byte is read.
fn read_line<R: Read>(reader: &mut BufReader<R>) -> Result<Option<String>, String> {
    let mut buf = Vec::new();
    let read = reader
        .read_until(b'\n', &mut buf)
        .map_err(|e| format!("error reading request: {e}"))?;
    if read == 0 {
        return Ok(None);
    }
    if buf.len() > MAX_LINE_BYTES {
        return Err("head line exceeds maximum length".to_string());
    }
    while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
        buf.pop();
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// Whether a header's value must be redacted in logs: `Authorization`, or any
/// header whose name contains `key` or `token` (case-insensitive).
pub fn is_sensitive_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "authorization" || lower.contains("key") || lower.contains("token")
}

/// The value to log for a header, redacting sensitive ones.
pub fn display_header_value(name: &str, value: &str) -> String {
    if is_sensitive_header(name) {
        format!("<redacted, {} chars>", value.chars().count())
    } else {
        value.to_string()
    }
}

/// Formats the request head for the log block.
pub fn format_head(remote: &str, head: &RequestHead) -> String {
    let mut out = String::new();
    out.push_str("===== request =====\n");
    out.push_str(&format!(
        "time: {}\n",
        crate::time::format_rfc3339(crate::time::now_unix_secs())
    ));
    out.push_str(&format!("remote: {remote}\n"));
    out.push_str(&format!(
        "request: {} {} {}\n",
        head.method, head.target, head.version
    ));
    out.push_str("headers:\n");
    for (name, value) in &head.headers {
        out.push_str(&format!(
            "  {name}: {}\n",
            display_header_value(name, value)
        ));
    }
    out
}

/// Formats the request body for the log block: pretty-printed when it parses
/// as JSON, raw text otherwise.
pub fn format_body(body: &[u8]) -> String {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_default(),
        Err(_) => String::from_utf8_lossy(body).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &[u8]) -> ParseOutcome {
        let mut reader = BufReader::new(raw);
        read_request(&mut reader)
    }

    #[test]
    fn parses_request_with_content_length() {
        let raw = b"POST /v1/chat/completions?x=1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello";
        let outcome = parse(raw);
        let ParseOutcome::Ok(req) = outcome else {
            panic!("expected Ok");
        };
        assert_eq!(req.head.method, "POST");
        assert_eq!(req.head.target, "/v1/chat/completions?x=1");
        assert_eq!(req.head.path(), "/v1/chat/completions");
        assert_eq!(req.head.version, "HTTP/1.1");
        assert_eq!(req.head.header("host"), Some("localhost"));
        assert_eq!(req.body, b"hello");
    }

    #[test]
    fn decodes_chunked_body() {
        let raw = b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        let ParseOutcome::Ok(req) = parse(raw) else {
            panic!("expected Ok");
        };
        assert_eq!(req.body, b"Wikipedia");
    }

    #[test]
    fn decodes_chunked_body_with_extensions_and_trailers() {
        let raw = b"POST / HTTP/1.1\r\nTransfer-Encoding: Chunked\r\n\r\n3;foo=bar\r\nabc\r\n0\r\nX-Trailer: yes\r\n\r\n";
        let ParseOutcome::Ok(req) = parse(raw) else {
            panic!("expected Ok");
        };
        assert_eq!(req.body, b"abc");
    }

    #[test]
    fn rejects_body_above_cap_via_content_length() {
        let raw = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        let outcome = parse(raw.as_bytes());
        assert!(matches!(outcome, ParseOutcome::BodyTooLarge(_)));
    }

    #[test]
    fn rejects_body_above_cap_via_chunked() {
        let raw = format!(
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n",
            MAX_BODY_BYTES + 1
        );
        let outcome = parse(raw.as_bytes());
        assert!(matches!(outcome, ParseOutcome::BodyTooLarge(_)));
    }

    #[test]
    fn rejects_invalid_content_length() {
        let raw = b"POST / HTTP/1.1\r\nContent-Length: abc\r\n\r\n";
        assert!(matches!(parse(raw), ParseOutcome::BadRequest(_)));
    }

    #[test]
    fn empty_connection_yields_empty() {
        let outcome = parse(b"");
        assert!(matches!(outcome, ParseOutcome::Empty));
    }

    #[test]
    fn redacts_sensitive_headers() {
        assert!(is_sensitive_header("Authorization"));
        assert!(is_sensitive_header("authorization"));
        assert!(is_sensitive_header("X-Api-Key"));
        assert!(is_sensitive_header("x-auth-TOKEN"));
        assert!(!is_sensitive_header("Content-Type"));
        assert!(!is_sensitive_header("Accept"));
        assert_eq!(
            display_header_value("Authorization", "Bearer abc"),
            "<redacted, 10 chars>"
        );
        assert_eq!(
            display_header_value("X-Api-Key", "supersecretvalue"),
            "<redacted, 16 chars>"
        );
        assert_eq!(
            display_header_value("Content-Type", "application/json"),
            "application/json"
        );
    }

    #[test]
    fn formats_body_pretty_when_json() {
        let formatted = format_body(br#"{"a":1}"#);
        assert!(formatted.contains("\"a\": 1"));
        assert_eq!(format_body(b"plain text"), "plain text");
    }
}
