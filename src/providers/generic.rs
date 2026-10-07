//! Generic OpenAI-compatible adapter (S2.7), loopback only.
//!
//! Unlike the CLI adapters, this one implements [`Provider`] directly over
//! HTTP: it POSTs the composed prompt to an unauthenticated
//! OpenAI-compatible endpoint on the same machine (Ollama, LM Studio) and
//! parses the single chat-completion choice. It stays **disabled by
//! default**: loopback HTTP is still a direct outbound call, so this adapter
//! ships behind the owner's pending AGENTS.md exception
//! (docs/research/phase2-architecture.md §3.3).
//!
//! The endpoint is restricted to `http://127.0.0.1:<port>/v1`,
//! `http://localhost:<port>/v1` and `http://[::1]:<port>/v1` (literal
//! `localhost` maps to `127.0.0.1` without DNS). There is no TLS, proxy,
//! redirect-following, authentication, tool support or connection pooling:
//! each call opens one socket, drives the Hyper connection future inside the
//! provider call, and lets cancellation drop the socket.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, LengthLimitError, Limited};
use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper::header::{ACCEPT, ACCEPT_ENCODING, CONNECTION, CONTENT_ENCODING, CONTENT_TYPE, HOST};
use hyper::{Request, StatusCode};
use hyper_util::rt::tokio::TokioIo;
use serde_saphyr::Location;
use tokio::net::TcpStream;
use tokio::time::Instant;

use super::{
    FormatInput, ProbeSpec, Provider, ProviderDescriptor, ProviderError, ProviderErrorCode,
    ProviderFuture, ProviderLocations, ProviderSettings, RawOption,
};
use crate::config::ConfigError;
use crate::process::ProcessRunner;

pub const ID: &str = "generic";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The only supported option: the validated loopback base URL.
const BASE_URL_KEY: &str = "base_url";

/// The generic adapter accepts no environment overrides: nothing safe rides
/// on the environment of a server Pumice does not spawn.
const ALLOWED_ENV: &[&str] = &[];

/// Fixed notice `check-config` and startup print while the provider is
/// enabled. Never sent to the endpoint and never part of formatted dictation.
pub const RISK_WARNING: &str = "Disabled by default. Connects only to an unauthenticated loopback HTTP endpoint. The local server controls inference routing and retention.";

/// Serialized request cap: a dictation near this size cannot be formatted in
/// the time budget anyway, and this keeps memory bounded.
const MAX_REQUEST_BYTES: usize = 10 * 1024 * 1024;
/// Successful response cap, counted as bytes actually received.
const MAX_SUCCESS_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
/// Error responses are classified by status alone; the body is only drained
/// up to this bound.
const MAX_ERROR_RESPONSE_BYTES: usize = 64 * 1024;
/// Connect cap, inside the call deadline.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub const DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ID,
    defaults,
    allowed_env: ALLOWED_ENV,
    validate_options,
    build,
    risk_warning: Some(RISK_WARNING),
    validate_settings,
    // This adapter has no CLI: detection neither resolves nor spawns
    // anything for it (no PATH lookup, no probe). Its endpoint is a network
    // target whose availability is checked at call time, when
    // `EndpointUnavailable` then returns the original text.
    probe: ProbeSpec::NotApplicable,
    // No binary exists. The value stays empty on purpose and is never
    // resolved: `NotApplicable` detection skips resolution entirely.
    default_binary: "",
    // No npm entrypoint: nothing to translate a `.cmd` shim for.
    npm_entrypoint: None,
    // There is nothing to install: readiness means a local server answering
    // at the configured `base_url`.
    install_hint: "start a local OpenAI-compatible server such as Ollama or LM Studio",
};

fn defaults() -> ProviderSettings {
    ProviderSettings {
        // Nothing is enabled unless the configuration entry says so; the
        // loader replaces this with the entry's explicit value.
        enabled: false,
        binary: None,
        // No default model: enablement requires an explicit one.
        model: String::new(),
        timeout: DEFAULT_TIMEOUT,
        env: BTreeMap::new(),
        options: BTreeMap::new(),
    }
}

/// Only `base_url` is allowed, and it must parse as a loopback endpoint.
fn validate_options(options: &[RawOption<'_>]) -> Result<(), ConfigError> {
    for option in options {
        if option.key != BASE_URL_KEY {
            return Err(ConfigError::at(
                option.key_at,
                format!("providers.{ID}.options.{} is not supported", option.key),
            ));
        }
        if let Err(error) = parse_endpoint(option.value) {
            return Err(ConfigError::at(
                option.value_at,
                format!("providers.{ID}.options.{BASE_URL_KEY} {error}"),
            ));
        }
    }
    Ok(())
}

/// Full-settings validation: an enabled provider needs an explicit `model` and
/// a validated loopback `base_url`, spawns no CLI (`binary` must be unset) and
/// accepts no environment overrides. Missing fields point at the `enabled:`
/// line; malformed values point at their own value. A base URL naming Pumice's
/// own listening port is refused, so a dictation can never route back into
/// this service.
fn validate_settings(
    settings: &ProviderSettings,
    locations: &ProviderLocations,
) -> Result<(), ConfigError> {
    if !settings.enabled {
        return Ok(());
    }
    let enabled_at = locations.enabled.unwrap_or(Location::UNKNOWN);
    if settings.model.is_empty() {
        return Err(ConfigError::at(
            enabled_at,
            format!("providers.{ID}.model is required when the provider is enabled"),
        ));
    }
    if settings.binary.is_some() {
        return Err(ConfigError::at(
            locations.binary.unwrap_or(enabled_at),
            format!("providers.{ID} spawns no CLI; binary must not be set"),
        ));
    }
    if !settings.env.is_empty() {
        return Err(ConfigError::at(
            enabled_at,
            format!("providers.{ID} accepts no environment overrides"),
        ));
    }
    let Some(base_url) = settings.options.get(BASE_URL_KEY) else {
        return Err(ConfigError::at(
            enabled_at,
            format!(
                "providers.{ID}.options.{BASE_URL_KEY} is required when the provider is enabled"
            ),
        ));
    };
    let endpoint = parse_endpoint(base_url)
        .map_err(|error| ConfigError::at(base_url_at(locations), format!("providers.{ID}.options.{BASE_URL_KEY} {error}")))?;
    if endpoint.addr.is_ipv4() && endpoint.addr.ip().is_loopback() && endpoint.port() == locations.port
    {
        return Err(ConfigError::at(
            base_url_at(locations),
            format!(
                "providers.{ID}.options.{BASE_URL_KEY} points at Pumice's own port ({}), which would route requests back into this service",
                locations.port
            ),
        ));
    }
    Ok(())
}

fn base_url_at(locations: &ProviderLocations) -> Location {
    locations
        .options
        .get(BASE_URL_KEY)
        .copied()
        .unwrap_or(Location::UNKNOWN)
}

/// Validates again (direct library callers may skip the loader's checks) and
/// builds the provider; the process runner is unused because nothing spawns.
fn build(
    settings: &ProviderSettings,
    _runner: Arc<ProcessRunner>,
) -> Result<Arc<dyn Provider>, ConfigError> {
    validate_settings(settings, &ProviderLocations::default())?;
    let Some(base_url) = settings.options.get(BASE_URL_KEY) else {
        return Err(ConfigError::general(format!(
            "providers.{ID}.options.{BASE_URL_KEY} is required when the provider is enabled"
        )));
    };
    let endpoint = parse_endpoint(base_url).map_err(|error| {
        ConfigError::general(format!("providers.{ID}.options.{BASE_URL_KEY} {error}"))
    })?;
    Ok(Arc::new(GenericProvider::new(
        endpoint,
        settings.model.clone(),
        settings.timeout,
    )))
}

/// A validated loopback endpoint: where to connect, the authority to send as
/// the Host header, and the chat-completions path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopbackEndpoint {
    pub addr: SocketAddr,
    pub authority: String,
    pub chat_path: &'static str,
}

impl LoopbackEndpoint {
    fn port(&self) -> u16 {
        self.addr.port()
    }
}

/// Why an endpoint string was rejected. `Display` strings are fixed so an
/// error never echoes the configured value (config values may be private).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointError {
    /// Not exactly `http://` (https and every other scheme).
    Scheme,
    /// Whitespace, control characters, backslashes, userinfo,
    /// percent-encoding, zone ids, query or fragment.
    ForbiddenCharacters,
    /// The host is not exactly `127.0.0.1`, `localhost` or `[::1]`.
    Host,
    /// No explicit `:port`.
    PortMissing,
    /// The port is not decimal digits from 1 to 65535.
    PortInvalid,
    /// The path is not exactly `/v1` (one optional trailing slash allowed).
    Path,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            EndpointError::Scheme => "must be an http:// URL (https is not supported)",
            EndpointError::ForbiddenCharacters => {
                "must not contain whitespace, control characters, backslashes, userinfo, percent-encoding, zone ids, query or fragment"
            }
            EndpointError::Host => "host must be exactly 127.0.0.1, localhost or [::1]",
            EndpointError::PortMissing => {
                "must include an explicit port, as in http://127.0.0.1:11434/v1"
            }
            EndpointError::PortInvalid => "port must be decimal digits from 1 to 65535",
            EndpointError::Path => "path must be exactly /v1",
        })
    }
}

/// Parses one of the exact accepted spellings of a loopback OpenAI-compatible
/// endpoint: `http://127.0.0.1:<port>/v1`, `http://localhost:<port>/v1` or
/// `http://[::1]:<port>/v1`, with an optional trailing slash and an explicit
/// decimal port from 1 to 65535. Literal `localhost` maps to `127.0.0.1`
/// without DNS; `[::1]` stays IPv6. The raw string is validated so alternate
/// IP spellings (`127.1`, `0x7f.0.0.1`, `2130706433`), percent-encoding, zone
/// ids, userinfo, query, fragment, backslashes and control characters are all
/// rejected.
pub fn parse_endpoint(value: &str) -> Result<LoopbackEndpoint, EndpointError> {
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(EndpointError::ForbiddenCharacters);
    }
    let Some(rest) = value.strip_prefix("http://") else {
        return Err(EndpointError::Scheme);
    };
    // '%' (percent-encoding and zone ids), '\\', '@' (userinfo), '?' and '#'
    // are never part of an accepted spelling.
    if rest.bytes().any(|b| matches!(b, b'%' | b'\\' | b'@' | b'?' | b'#')) {
        return Err(EndpointError::ForbiddenCharacters);
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    if path != "/v1" {
        return Err(EndpointError::Path);
    }

    let (connect_host, authority_host, port) = parse_authority(authority)?;
    let port = parse_port(port)?;
    // `connect_host` is one of the exact validated spellings and `port` is in
    // range, so this parse cannot fail; the error branch stays as defense.
    let addr_text = if connect_host.contains(':') {
        format!("[{connect_host}]:{port}")
    } else {
        format!("{connect_host}:{port}")
    };
    let addr: SocketAddr = addr_text
        .parse()
        .map_err(|_| EndpointError::PortInvalid)?;
    Ok(LoopbackEndpoint {
        addr,
        authority: format!("{authority_host}:{port}"),
        chat_path: "/v1/chat/completions",
    })
}

/// Splits the authority into the host to connect to (`localhost` already
/// mapped to `127.0.0.1`), the authority form for the Host header (bracketed
/// for IPv6) and the raw port text.
fn parse_authority(authority: &str) -> Result<(&'static str, &'static str, &str), EndpointError> {
    if authority.is_empty() {
        return Err(EndpointError::Host);
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((inside, after)) = rest.split_once(']') else {
            return Err(EndpointError::Host);
        };
        if inside != "::1" {
            return Err(EndpointError::Host);
        }
        let Some(port) = after.strip_prefix(':') else {
            return Err(EndpointError::PortMissing);
        };
        return Ok(("::1", "[::1]", port));
    }
    let Some((host, port)) = authority.split_once(':') else {
        return Err(EndpointError::PortMissing);
    };
    match host {
        "127.0.0.1" => Ok(("127.0.0.1", "127.0.0.1", port)),
        "localhost" => Ok(("127.0.0.1", "127.0.0.1", port)),
        _ => Err(EndpointError::Host),
    }
}

fn parse_port(raw: &str) -> Result<u16, EndpointError> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(EndpointError::PortInvalid);
    }
    match raw.parse::<u16>() {
        Ok(0) | Err(_) => Err(EndpointError::PortInvalid),
        Ok(port) => Ok(port),
    }
}

/// Formats dictations through one unauthenticated loopback OpenAI-compatible
/// endpoint. One fresh socket per call; nothing is kept between calls.
#[derive(Clone, Debug)]
pub struct GenericProvider {
    endpoint: LoopbackEndpoint,
    model: String,
    timeout: Duration,
}

impl GenericProvider {
    pub fn new(endpoint: LoopbackEndpoint, model: String, timeout: Duration) -> GenericProvider {
        GenericProvider {
            endpoint,
            model,
            timeout,
        }
    }

    async fn format_inner(
        &self,
        input: FormatInput<'_>,
        deadline: Instant,
    ) -> Result<String, ProviderError> {
        // Own timeout, capped by the caller's deadline (same rule as the CLI
        // providers).
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .map_or(deadline, |own| own.min(deadline));
        match tokio::time::timeout_at(deadline, self.exchange(input, deadline)).await {
            Ok(result) => result,
            Err(_) => Err(ProviderError::Timeout),
        }
    }

    /// One POST: serialize the body, connect, drive the Hyper connection
    /// future alongside the exchange, read the bounded body and parse it.
    /// Everything runs under `deadline`; dropping this future (cancellation)
    /// drops the socket, because the connection is driven here and never
    /// detached to a task.
    async fn exchange(
        &self,
        input: FormatInput<'_>,
        deadline: Instant,
    ) -> Result<String, ProviderError> {
        let body = serialize_request(&self.model, input)?;
        if Instant::now() >= deadline {
            return Err(ProviderError::Timeout);
        }

        let connect_cap = deadline.saturating_duration_since(Instant::now()).min(CONNECT_TIMEOUT);
        let stream = match tokio::time::timeout(
            connect_cap,
            TcpStream::connect(self.endpoint.addr),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => return Err(connect_error(&error)),
            // The connect cap, not the request deadline, ran out: the local
            // endpoint did not accept the connection. Windows retries a closed
            // loopback port for about 2 s before reporting a refusal, so this
            // is how "nothing is listening" usually surfaces there.
            Err(_) if connect_cap == CONNECT_TIMEOUT => {
                return Err(ProviderError::other(ProviderErrorCode::EndpointUnavailable));
            }
            Err(_) => return Err(ProviderError::Timeout),
        };
        if Instant::now() >= deadline {
            return Err(ProviderError::Timeout);
        }

        let (mut sender, connection) = ClientBuilder::new()
            .max_headers(64)
            .handshake(TokioIo::new(stream))
            .await
            .map_err(|_| ProviderError::other(ProviderErrorCode::EndpointUnavailable))?;
        tokio::pin!(connection);

        let request = build_request(&self.endpoint, body)?;
        let send_future = sender.send_request(request);
        tokio::pin!(send_future);
        // Race the connection driver with the request: the driver performs
        // the IO, so both must be polled. With `Connection: close` the driver
        // can finish in the same instant the response is dispatched; when it
        // finishes first, the request future still resolves (the dispatcher
        // signals pending requests when it ends), so it is awaited once more
        // instead of discarding a response that raced us.
        let response = tokio::select! {
            result = &mut connection => {
                match result {
                    // The socket died mid-exchange.
                    Err(_) => {
                        return Err(ProviderError::other(
                            ProviderErrorCode::EndpointUnavailable,
                        ))
                    }
                    Ok(()) => (&mut send_future)
                        .await
                        .map_err(|error| classify_send_error(&error))?,
                }
            }
            result = &mut send_future => result.map_err(|error| classify_send_error(&error))?,
        };

        let status = response.status();
        if status != StatusCode::OK {
            // Classification is by status alone; the body is drained only to
            // that bound and never inspected or copied.
            let _ = read_body(response.into_body(), MAX_ERROR_RESPONSE_BYTES, &mut connection).await;
            return Err(status_error(status));
        }
        reject_unsupported_encoding(response.headers())?;
        reject_sse(response.headers())?;

        let bytes = read_body(
            response.into_body(),
            MAX_SUCCESS_RESPONSE_BYTES,
            &mut connection,
        )
        .await?;
        parse_success_body(&bytes)
    }
}

impl Provider for GenericProvider {
    fn id(&self) -> &'static str {
        ID
    }

    fn format<'a>(&'a self, input: FormatInput<'a>, deadline: Instant) -> ProviderFuture<'a> {
        Box::pin(self.format_inner(input, deadline))
    }
}

/// Serializes the chat-completions body with a hard size cap, so a huge
/// dictation fails as `InputTooLarge` instead of exhausting memory.
fn serialize_request(model: &str, input: FormatInput<'_>) -> Result<Vec<u8>, ProviderError> {
    let user = [
        input.user_prompt.before_text,
        input.text,
        input.user_prompt.after_text,
    ]
    .concat();
    let payload = serde_json::json!({
        "model": model,
        "stream": false,
        "messages": [
            {"role": "system", "content": input.system_prompt},
            {"role": "user", "content": user},
        ],
    });
    let mut writer = CappedWriter::new(MAX_REQUEST_BYTES);
    match serde_json::to_writer(&mut writer, &payload) {
        Ok(()) => Ok(writer.into_inner()),
        Err(_) if writer.exceeded() => {
            Err(ProviderError::other(ProviderErrorCode::InputTooLarge))
        }
        Err(_) => Err(ProviderError::other(
            ProviderErrorCode::InvalidConfiguration,
        )),
    }
}

/// Maps a failed request to a safe error: bytes that are not a complete HTTP
/// response are a malformed endpoint answer; anything else means the
/// transport died. The hyper error text never leaves this function.
fn classify_send_error(error: &hyper::Error) -> ProviderError {
    if error.is_parse() || error.is_incomplete_message() {
        ProviderError::other(ProviderErrorCode::InvalidOutput)
    } else {
        ProviderError::other(ProviderErrorCode::EndpointUnavailable)
    }
}

/// Builds the exact request: origin-form target, the validated authority as
/// the Host header, and no authentication, tools or compression negotiation.
fn build_request(
    endpoint: &LoopbackEndpoint,
    body: Vec<u8>,
) -> Result<Request<Full<Bytes>>, ProviderError> {
    Request::builder()
        .method(hyper::Method::POST)
        .uri(endpoint.chat_path)
        .header(HOST, &endpoint.authority)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .header(ACCEPT_ENCODING, "identity")
        .header(CONNECTION, "close")
        .body(Full::new(Bytes::from(body)))
        .map_err(|_| ProviderError::other(ProviderErrorCode::InvalidConfiguration))
}

/// Maps a failed TCP connect to a safe error. "Connection refused" and its
/// siblings mean nothing is listening; anything else is a plain I/O failure.
/// The OS error text never leaves this function.
fn connect_error(error: &std::io::Error) -> ProviderError {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected
        | ErrorKind::AddrInUse
        | ErrorKind::AddrNotAvailable => {
            ProviderError::other(ProviderErrorCode::EndpointUnavailable)
        }
        ErrorKind::TimedOut => ProviderError::Timeout,
        _ => ProviderError::other(ProviderErrorCode::Io),
    }
}

/// Maps a non-200 status to a safe error; the response body (if any) is never
/// part of the classification.
fn status_error(status: StatusCode) -> ProviderError {
    match status.as_u16() {
        408 | 504 => ProviderError::Timeout,
        401 | 403 => ProviderError::other(ProviderErrorCode::AuthenticationRejected),
        429 => ProviderError::RateLimited { retry_after: None },
        other => ProviderError::other(ProviderErrorCode::HttpStatus(other)),
    }
}

/// A compressed response is never decoded: Pumice sends
/// `Accept-Encoding: identity`, so anything else means the endpoint ignored
/// the negotiation.
fn reject_unsupported_encoding(headers: &hyper::HeaderMap) -> Result<(), ProviderError> {
    match headers.get(CONTENT_ENCODING) {
        None => Ok(()),
        Some(value) if value == "identity" => Ok(()),
        Some(_) => Err(ProviderError::other(ProviderErrorCode::InvalidOutput)),
    }
}

/// An SSE stream is not a chat completion; `stream: false` was negotiated.
fn reject_sse(headers: &hyper::HeaderMap) -> Result<(), ProviderError> {
    let is_sse = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"));
    if is_sse {
        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
    }
    Ok(())
}

/// Reads a response body to its end, counting bytes actually received and
/// failing past `limit`. The connection future is polled beside the body: it
/// is the IO driver and must keep running while frames arrive. When the
/// connection ends first (the server closed, which is legal once the whole
/// body was received), the remaining buffered frames are drained without it;
/// an error there means the body was truncated.
async fn read_body(
    body: hyper::body::Incoming,
    limit: usize,
    connection: &mut std::pin::Pin<
        &mut hyper::client::conn::http1::Connection<TokioIo<TcpStream>, Full<Bytes>>,
    >,
) -> Result<Vec<u8>, ProviderError> {
    let mut limited = Limited::new(body, limit);
    let mut bytes = Vec::new();
    loop {
        tokio::select! {
            result = &mut *connection => {
                return match result {
                    // Graceful close: everything the endpoint sent is
                    // buffered; consume it without the driver.
                    Ok(()) => drain_body(&mut limited, bytes).await,
                    Err(_) => Err(ProviderError::other(ProviderErrorCode::InvalidOutput)),
                };
            }
            frame = limited.frame() => {
                match frame {
                    Some(Ok(frame)) => {
                        if let Ok(data) = frame.into_data() {
                            bytes.extend_from_slice(&data);
                        } else {
                            // Trailers are not negotiated.
                            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
                        }
                    }
                    Some(Err(error)) => {
                        if error.downcast_ref::<LengthLimitError>().is_some() {
                            return Err(ProviderError::other(ProviderErrorCode::OutputTooLarge));
                        }
                        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
                    }
                    None => return Ok(bytes),
                }
            }
        }
    }
}

/// Consumes already-buffered body frames after the connection future ended;
/// anything but a clean end is a truncated body.
async fn drain_body(
    limited: &mut Limited<hyper::body::Incoming>,
    mut bytes: Vec<u8>,
) -> Result<Vec<u8>, ProviderError> {
    while let Some(frame) = limited.frame().await {
        match frame {
            Ok(frame) if frame.is_data() => {
                bytes.extend_from_slice(&frame.into_data().expect("is_data"));
            }
            Ok(_) => return Err(ProviderError::other(ProviderErrorCode::InvalidOutput)),
            Err(error) => {
                return Err(if error.downcast_ref::<LengthLimitError>().is_some() {
                    ProviderError::other(ProviderErrorCode::OutputTooLarge)
                } else {
                    ProviderError::other(ProviderErrorCode::InvalidOutput)
                })
            }
        }
    }
    Ok(bytes)
}

/// Parses a 200 chat completion: exactly one choice, string content, a
/// successful finish, and no tool or function call. The content is returned
/// unchanged for pipeline cleanup.
fn parse_success_body(bytes: &[u8]) -> Result<String, ProviderError> {
    let invalid = || ProviderError::other(ProviderErrorCode::InvalidOutput);
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ProviderError::other(ProviderErrorCode::InvalidOutput))?;
    let choices = value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(invalid)?;
    if choices.len() != 1 {
        return Err(invalid());
    }
    let choice = &choices[0];
    let message = choice.get("message").ok_or_else(invalid)?;

    if message
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|calls| !calls.is_empty())
        {
            return Err(ProviderError::other(
                ProviderErrorCode::UnexpectedToolActivity,
            ));
        }
    if message
        .get("function_call")
        .is_some_and(|call| !call.is_null())
    {
        return Err(ProviderError::other(
            ProviderErrorCode::UnexpectedToolActivity,
        ));
    }
    match choice
        .get("finish_reason")
        .and_then(serde_json::Value::as_str)
    {
        // Absent is tolerated: `stop` is the only successful completion, and
        // anything else (truncation, content filters, tool calls) is refused.
        None | Some("stop") => {}
        Some(_) => return Err(invalid()),
    }
    message
        .get("content")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(invalid)
}

/// A `Vec<u8>` writer that fails once `limit` bytes are exceeded, so an
/// oversized request errors instead of growing without bound.
struct CappedWriter {
    buf: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl CappedWriter {
    fn new(limit: usize) -> CappedWriter {
        CappedWriter {
            buf: Vec::new(),
            limit,
            exceeded: false,
        }
    }

    fn exceeded(&self) -> bool {
        self.exceeded
    }

    fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

impl std::io::Write for CappedWriter {
    fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
        if self.buf.len() + chunk.len() > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request body exceeds the limit",
            ));
        }
        self.buf.extend_from_slice(chunk);
        Ok(chunk.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
