//! The browser test page (p15-serve-transcriptions/D8): `GET /` and its
//! script and stylesheet, embedded in `pumice` at build time.
//!
//! The page lists `GET /v1/models`, records or uploads audio and calls
//! `POST /v1/audio/transcriptions` on its own origin, so it passes the
//! frontend's Host/Origin guard like any same-origin request. It is a test
//! page, not the product UI: no external assets, nothing stored, audio and
//! results only in the page's memory. Every file is answered with a strict
//! content security policy ([`CSP`]) that allows only these files, same-origin
//! requests and the page's own recording as a `blob:` for playback.

use bytes::Bytes;
use http_body_util::Full;
use hyper::header::{self, HeaderValue};
use hyper::{Response, StatusCode};

/// The page and its assets: path, content type, body.
pub const ASSETS: [(&str, &str, &str); 3] = [
    (
        "/",
        "text/html; charset=utf-8",
        include_str!("page/index.html"),
    ),
    (
        "/page.js",
        "text/javascript; charset=utf-8",
        include_str!("page/page.js"),
    ),
    (
        "/page.css",
        "text/css; charset=utf-8",
        include_str!("page/page.css"),
    ),
];

/// The page's content security policy: its own script and stylesheet only,
/// same-origin requests, no framing, no forms.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
    connect-src 'self'; img-src 'self' data:; media-src 'self' blob:; \
    frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// The embedded file served at `path`, as its content type and body.
pub fn asset(path: &str) -> Option<(&'static str, &'static str)> {
    ASSETS
        .iter()
        .find(|(asset, _, _)| *asset == path)
        .map(|(_, content_type, body)| (*content_type, *body))
}

/// A `200` with the file and the page's headers.
pub fn response(content_type: &'static str, body: &'static str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from_static(body.as_bytes())));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("microphone=(self)"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_loads_nothing_from_outside() {
        for (path, _, body) in ASSETS {
            let lower = body.to_ascii_lowercase();
            // Absolute and protocol-relative URLs, and every way to load
            // more.
            for pattern in [
                "http:",
                "https:",
                "ws:",
                "wss:",
                "ftp:",
                "\"//",
                "'//",
                "`//",
                "(//",
                "@import",
                "import(",
                "importscripts",
                "<iframe",
                "<form",
                "<object",
                "<embed",
            ] {
                assert!(!lower.contains(pattern), "{path} has {pattern}");
            }
        }
        // The stylesheet loads nothing (the script's `createObjectURL(` is
        // not a CSS `url(`).
        assert!(!asset("/page.css").unwrap().1.contains("url("));
        // The HTML refers only to the embedded files.
        let html = asset("/").unwrap().1;
        assert!(html.contains(r#"<script src="/page.js" defer></script>"#));
        assert!(html.contains(r#"<link rel="stylesheet" href="/page.css">"#));
        assert!(!html.contains("<script>"), "no inline script");
        assert!(!html.contains("<style"), "no inline style");
        assert!(!html.contains(" style="), "no style attribute");
    }

    #[test]
    fn the_page_renders_text_only_as_text() {
        let script = asset("/page.js").unwrap().1;
        for sink in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
        ] {
            assert!(!script.contains(sink), "page.js uses {sink}");
        }
        // Nothing is kept beyond the page's memory.
        for storage in [
            "localStorage",
            "sessionStorage",
            "indexedDB",
            "document.cookie",
        ] {
            assert!(!script.contains(storage), "page.js uses {storage}");
        }
    }

    /// `node --check` when Node.js is on the PATH (CI runners have it);
    /// skipped otherwise.
    #[test]
    fn the_script_parses() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/orchestrator/serve/page/page.js"
        );
        match std::process::Command::new("node")
            .args(["--check", path])
            .output()
        {
            Ok(output) => assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            ),
            Err(error) => eprintln!("node not available, syntax check skipped: {error}"),
        }
    }
}
