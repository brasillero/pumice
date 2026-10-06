//! `pumice doctor` (S2.8 part 2): what is installed, what is missing and how
//! to install it — plus the only quota-bearing diagnostic, `--login-check`.
//!
//! The default report renders from [`ProviderStatus`] plus the loaded config
//! as a pure function, so the presentation is unit-testable without spawning
//! anything. Detection runs fresh on every invocation (never the startup
//! cache) and never spends quota: only bounded version probes run.
//!
//! `--login-check` spends quota: it composes one fixed tiny dictation through
//! the selected provider's normal adapter — same restrictions, fresh empty
//! workspace and configured timeout — never the fallback chain, and it never
//! prints the response: only `ok (<ms> ms)` or a text-free error category.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use crate::config::{Config, ConfigError, ConfigSource};
use crate::process::ProcessRunner;
use crate::prompts::compose_with_settings;
use crate::providers::discovery::{self, Found, ProviderStatus, Version};
use crate::providers::{self, Provider, ProviderError, ProviderErrorCode, antigravity};
use crate::request::{ChatCompletionRequest, Content, Message, extract_request};

/// The fixed tiny dictation the login check formats.
pub const LOGIN_CHECK_SAMPLE: &str = "pumice login check";

/// Fixed refusal for `--login-check --provider antigravity`: dormant until a
/// supported per-launch tool policy exists, so no real call may ever run.
pub const ANTIGRAVITY_REFUSAL: &str =
    "antigravity cannot be login-checked: it cannot be enabled yet (see docs)";

/// Renders the full doctor report: the config source, one line per registered
/// provider and the readiness summary. Pure over the config and the detection
/// statuses; never prints dictated text or probe output.
pub fn render(config: &Config, source: &ConfigSource, statuses: &[ProviderStatus]) -> String {
    let mut out = String::new();
    match source {
        ConfigSource::File(path) => out.push_str(&format!("config: {}\n", path.display())),
        ConfigSource::BuiltInDefaults => out.push_str("config: built-in defaults\n"),
    }
    for status in statuses {
        out.push_str(&provider_line(status));
        out.push('\n');
        if config
            .providers
            .get(status.id)
            .is_some_and(|settings| settings.enabled)
            && let Some(warning) =
                providers::descriptor(status.id).and_then(|descriptor| descriptor.risk_warning)
        {
            out.push_str(&format!("  warning: {}: {warning}\n", status.id));
        }
    }
    let (ready, total) = enabled_ready(statuses);
    out.push_str(&format!("{ready} of {total} enabled providers ready\n"));
    out
}

/// One doctor line for `status`: id, enabled state, found state (with the
/// install hint when the provider is missing) and, for the dormant
/// Antigravity adapter, the plain note that it cannot be enabled yet. The
/// generic loopback adapter has no executable: its state names the local
/// endpoint and defers availability to call time.
fn provider_line(status: &ProviderStatus) -> String {
    let install_hint = providers::descriptor(status.id).map_or("", |d| d.install_hint);
    let state = match (&status.found, &status.version) {
        (Found::Found(_), Version::Parsed(version)) => format!("found {version}"),
        (Found::Found(_), Version::Unavailable) => "found (version unavailable)".to_owned(),
        (Found::Found(_), Version::Skipped) => "found".to_owned(),
        (Found::Missing, _) => format!("missing (install: {install_hint})"),
        (Found::UnsupportedShim, _) => format!("unsupported launcher (install: {install_hint})"),
        (Found::NotApplicable, _) => "local endpoint (checked at call time)".to_owned(),
    };
    let enabled = if status.enabled {
        "enabled"
    } else {
        "disabled"
    };
    let mut line = format!("{}: {enabled}, {state}", status.id);
    // Antigravity stays dormant regardless of detection (S2.5): say so.
    if status.id == antigravity::ID {
        line.push_str(", cannot be enabled yet (see docs)");
    }
    line
}

/// Counts `(ready, total)` over the enabled providers: an enabled provider
/// is ready when detection found its executable; the generic loopback
/// adapter has no executable and is ready by definition, since its endpoint
/// is checked at call time.
pub fn enabled_ready(statuses: &[ProviderStatus]) -> (usize, usize) {
    let mut ready = 0;
    let mut total = 0;
    for status in statuses {
        if !status.enabled {
            continue;
        }
        total += 1;
        if matches!(status.found, Found::Found(_) | Found::NotApplicable) {
            ready += 1;
        }
    }
    (ready, total)
}

/// Why a login check could not start; every variant carries a fixed,
/// text-free message.
pub enum LoginCheckError {
    /// Antigravity is refused before anything runs.
    Refused(&'static str),
    /// The ID names no registered provider.
    UnknownProvider,
    /// The provider is registered but disabled in the configuration.
    Disabled,
    /// Detection did not find the provider's executable.
    NotInstalled,
    /// The adapter refused to build (defense in depth: full-settings
    /// validation already ran at config load).
    BuildFailed(ConfigError),
}

impl LoginCheckError {
    /// The fixed output line and process exit code for this failure: 2 for
    /// usage-style problems, 1 for a provider that is not ready.
    pub fn line_and_code(&self, id: &str) -> (String, u8) {
        match self {
            LoginCheckError::Refused(message) => ((*message).to_owned(), 2),
            LoginCheckError::UnknownProvider => {
                (format!("error: provider \"{id}\" is not registered"), 2)
            }
            LoginCheckError::Disabled => (
                format!("error: cannot check provider \"{id}\": it is disabled"),
                1,
            ),
            LoginCheckError::NotInstalled => (
                format!("error: cannot check provider \"{id}\": it is not installed"),
                1,
            ),
            LoginCheckError::BuildFailed(error) => (error.to_string(), 2),
        }
    }
}

/// The outcome of the single real formatting call.
pub enum LoginCheck {
    /// The call returned text (discarded); elapsed time of the call.
    Ok(Duration),
    /// The call failed with a safe, text-free error.
    Failed(ProviderError),
}

/// The quota warning printed before the call runs.
pub fn quota_notice(id: &str) -> String {
    format!("this runs one real formatting call and spends {id} quota")
}

/// Validates the login-check preconditions and builds the selected provider:
/// refused, unknown, disabled and not-installed IDs stop here, before
/// anything is spawned.
pub async fn prepare_login_check(
    config: &Config,
    id: &str,
    runner: &ProcessRunner,
) -> Result<Arc<dyn Provider>, LoginCheckError> {
    if id == antigravity::ID {
        return Err(LoginCheckError::Refused(ANTIGRAVITY_REFUSAL));
    }
    let descriptor = providers::descriptor(id).ok_or(LoginCheckError::UnknownProvider)?;
    let settings = config
        .providers
        .get(id)
        .ok_or(LoginCheckError::UnknownProvider)?;
    if !settings.enabled {
        return Err(LoginCheckError::Disabled);
    }
    // Fresh detection for exactly this provider, through the same resolution
    // path a formatting call uses; the bounded version probe spends no quota.
    // NotApplicable providers (generic) have no executable to find: their
    // endpoint availability is what the login-check call itself establishes.
    let statuses = discovery::detect(config, &[*descriptor], runner).await;
    if !matches!(statuses[0].found, Found::Found(_) | Found::NotApplicable) {
        return Err(LoginCheckError::NotInstalled);
    }
    (descriptor.build)(settings, Arc::new(*runner)).map_err(LoginCheckError::BuildFailed)
}

/// Runs exactly one formatting call through `provider`: the normal adapter
/// with its configured restrictions and timeout inside a fresh empty
/// workspace. Never the fallback chain; the returned text is discarded, so
/// cleanup does not run and the response never reaches the output.
pub async fn run_login_call(config: &Config, provider: &Arc<dyn Provider>) -> LoginCheck {
    let request = extract_request(ChatCompletionRequest {
        model: None,
        messages: vec![Message {
            role: "user".to_owned(),
            content: Content::Text(LOGIN_CHECK_SAMPLE.to_owned()),
        }],
        stream: None,
    })
    .expect("the fixed login-check sample always extracts");
    let prompts = compose_with_settings(&request, &config.prompts);
    let now = Instant::now();
    // The provider still applies its own (configured) timeout inside this
    // deadline. An overflowing total timeout cannot happen in practice; any
    // finite deadline keeps the provider cap meaningful.
    let deadline = now
        .checked_add(config.total_timeout)
        .unwrap_or_else(|| now + Duration::from_secs(86_400));
    let started = std::time::Instant::now();
    match provider.format(prompts.format_input(), deadline).await {
        Ok(_) => LoginCheck::Ok(started.elapsed()),
        Err(error) => LoginCheck::Failed(error),
    }
}

/// Text-free category for a failed login check; never carries captured
/// output, diagnostics or dictated text. Returns an owned string because the
/// `HTTP status n` category embeds the status code.
pub fn error_category(error: ProviderError) -> String {
    match error {
        ProviderError::NotInstalled => "not installed".to_owned(),
        ProviderError::NotLoggedIn => "not logged in".to_owned(),
        ProviderError::Timeout => "timeout".to_owned(),
        ProviderError::QuotaExceeded { .. } => "quota exhausted".to_owned(),
        ProviderError::RateLimited { .. } => "rate limited".to_owned(),
        ProviderError::Other { code } => match code {
            ProviderErrorCode::Spawn => "could not start the CLI".to_owned(),
            ProviderErrorCode::Io => "I/O error while running the CLI".to_owned(),
            ProviderErrorCode::InvalidConfiguration => "invalid provider configuration".to_owned(),
            ProviderErrorCode::AuthenticationRejected => {
                "the CLI rejected its credentials".to_owned()
            }
            ProviderErrorCode::UnsupportedShim => "the CLI is an unsupported wrapper".to_owned(),
            ProviderErrorCode::InvalidOutput => "the CLI returned unparseable output".to_owned(),
            ProviderErrorCode::OutputTooLarge => "the CLI returned too much output".to_owned(),
            ProviderErrorCode::InputTooLarge => "input too large".to_owned(),
            ProviderErrorCode::UnexpectedToolActivity => "the CLI tried to use a tool".to_owned(),
            ProviderErrorCode::NonzeroExit => "the CLI reported a failure".to_owned(),
            ProviderErrorCode::EndpointUnavailable => "endpoint unavailable".to_owned(),
            ProviderErrorCode::HttpStatus(status) => format!("HTTP status {status}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// The default configuration (empty file, built-in registry).
    fn default_config() -> Config {
        crate::config::validate_text_with_descriptors(
            "",
            Path::new("pumice.yaml"),
            providers::PROVIDERS,
        )
        .expect("empty config validates")
    }

    fn status(id: &'static str, enabled: bool, found: Found, version: Version) -> ProviderStatus {
        ProviderStatus {
            id,
            enabled,
            found,
            version,
        }
    }

    fn found() -> Found {
        Found::Found(PathBuf::from("/nonexistent/example"))
    }

    /// Every registered provider, like a full [`discovery::detect`] run.
    fn full_scan() -> Vec<ProviderStatus> {
        vec![
            status(
                "claude",
                true,
                found(),
                Version::Parsed("2.1.288".to_owned()),
            ),
            status(
                "codex",
                true,
                found(),
                Version::Parsed("0.160.0".to_owned()),
            ),
            status("opencode", false, Found::Missing, Version::Unavailable),
            status("antigravity", false, found(), Version::Skipped),
            status("generic", false, Found::NotApplicable, Version::Skipped),
            status("kimi", false, Found::Missing, Version::Unavailable),
            status("kiro", false, Found::Missing, Version::Unavailable),
        ]
    }

    #[test]
    fn render_lists_every_provider_and_the_summary() {
        let report = render(
            &default_config(),
            &ConfigSource::BuiltInDefaults,
            &full_scan(),
        );

        assert!(report.contains("config: built-in defaults\n"), "{report}");
        assert!(
            report.contains("claude: enabled, found 2.1.288\n"),
            "{report}"
        );
        assert!(
            report.contains("codex: enabled, found 0.160.0\n"),
            "{report}"
        );
        assert!(
            report.contains("opencode: disabled, missing (install: npm install -g opencode-ai)\n"),
            "{report}"
        );
        assert!(
            report.contains("antigravity: disabled, found, cannot be enabled yet (see docs)\n"),
            "{report}"
        );
        assert!(
            report.contains("generic: disabled, local endpoint (checked at call time)\n"),
            "{report}"
        );
        assert!(
            report.contains(
                "kimi: disabled, missing (install: curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash)\n"
            ),
            "{report}"
        );
        assert!(
            report.contains("kiro: disabled, missing (install: curl -fsSL https://cli.kiro.dev/install | bash)\n"),
            "{report}"
        );
        assert!(
            report.contains("2 of 2 enabled providers ready\n"),
            "{report}"
        );
        // Probe output and versions of other providers never mix into lines.
        assert!(!report.contains("provider claude:"), "{report}");
    }

    #[test]
    fn render_marks_missing_enabled_providers_and_counts_them_unready() {
        let mut statuses = full_scan();
        statuses[1] = status("codex", true, Found::Missing, Version::Unavailable);
        let report = render(
            &default_config(),
            &ConfigSource::File(PathBuf::from("pumice.yaml")),
            &statuses,
        );

        assert!(
            report.contains("codex: enabled, missing (install: npm install -g @openai/codex)\n"),
            "{report}"
        );
        assert!(
            report.contains("1 of 2 enabled providers ready\n"),
            "{report}"
        );
        assert!(report.contains("config: pumice.yaml\n"), "{report}");
    }

    #[test]
    fn render_reports_unavailable_versions_and_unsupported_launchers() {
        let mut statuses = full_scan();
        statuses[0] = status("claude", true, found(), Version::Unavailable);
        statuses[1] = status("codex", true, Found::UnsupportedShim, Version::Unavailable);
        let report = render(&default_config(), &ConfigSource::BuiltInDefaults, &statuses);

        assert!(
            report.contains("claude: enabled, found (version unavailable)\n"),
            "{report}"
        );
        assert!(
            report.contains(
                "codex: enabled, unsupported launcher (install: npm install -g @openai/codex)\n"
            ),
            "{report}"
        );
        // Claude is found (version unknown); Codex's launcher is unsupported,
        // so only one enabled provider is ready.
        assert!(
            report.contains("1 of 2 enabled providers ready\n"),
            "{report}"
        );
    }

    #[test]
    fn render_warns_below_enabled_providers_with_a_risk_warning() {
        let config = crate::config::validate_text_with_descriptors(
            "providers:\n  opencode:\n    enabled: true\n    model: anthropic/claude-haiku\n",
            Path::new("pumice.yaml"),
            providers::PROVIDERS,
        )
        .expect("config validates");
        let mut statuses = full_scan();
        statuses[2] = status("opencode", true, found(), Version::Unavailable);
        let report = render(&config, &ConfigSource::BuiltInDefaults, &statuses);

        assert!(
            report.contains("opencode: enabled, found (version unavailable)\n"),
            "{report}"
        );
        assert!(
            report.contains(&format!(
                "  warning: opencode: {}\n",
                providers::opencode::RISK_WARNING
            )),
            "{report}"
        );
        assert_eq!(enabled_ready(&statuses), (3, 3));
    }

    #[test]
    fn render_reports_an_enabled_generic_as_a_local_endpoint() {
        let config = crate::config::validate_text_with_descriptors(
            "providers:\n  generic:\n    enabled: true\n    model: qwen2.5-7b\n    options:\n      base_url: \"http://127.0.0.1:11434/v1\"\n",
            Path::new("pumice.yaml"),
            providers::PROVIDERS,
        )
        .expect("config validates");
        let mut statuses = full_scan();
        statuses[4] = status("generic", true, Found::NotApplicable, Version::Skipped);
        let report = render(&config, &ConfigSource::BuiltInDefaults, &statuses);

        assert!(
            report.contains("generic: enabled, local endpoint (checked at call time)\n"),
            "{report}"
        );
        assert!(
            report.contains(&format!(
                "  warning: generic: {}\n",
                providers::generic::RISK_WARNING
            )),
            "{report}"
        );
        // An enabled generic has no executable to miss, so it is ready.
        assert_eq!(enabled_ready(&statuses), (3, 3));
    }

    #[test]
    fn enabled_ready_counts_enabled_providers_only() {
        assert_eq!(enabled_ready(&full_scan()), (2, 2));
        assert_eq!(enabled_ready(&[]), (0, 0));
    }

    #[test]
    fn error_categories_are_text_free() {
        assert_eq!(error_category(ProviderError::NotLoggedIn), "not logged in");
        assert_eq!(
            error_category(ProviderError::RateLimited { retry_after: None }),
            "rate limited"
        );
        assert_eq!(error_category(ProviderError::Timeout), "timeout");
        assert_eq!(
            error_category(ProviderError::other(ProviderErrorCode::NonzeroExit)),
            "the CLI reported a failure"
        );
        assert_eq!(
            error_category(ProviderError::other(ProviderErrorCode::InputTooLarge)),
            "input too large"
        );
        assert_eq!(
            error_category(ProviderError::other(ProviderErrorCode::EndpointUnavailable)),
            "endpoint unavailable"
        );
        assert_eq!(
            error_category(ProviderError::other(ProviderErrorCode::HttpStatus(500))),
            "HTTP status 500"
        );
        assert_eq!(
            error_category(ProviderError::other(ProviderErrorCode::HttpStatus(404))),
            "HTTP status 404"
        );
    }

    #[test]
    fn precondition_failures_map_to_fixed_lines_and_codes() {
        let (line, code) =
            LoginCheckError::Refused(ANTIGRAVITY_REFUSAL).line_and_code("antigravity");
        assert_eq!(line, ANTIGRAVITY_REFUSAL);
        assert_eq!(code, 2);

        let (line, code) = LoginCheckError::UnknownProvider.line_and_code("nope");
        assert_eq!(line, "error: provider \"nope\" is not registered");
        assert_eq!(code, 2);

        let (line, code) = LoginCheckError::Disabled.line_and_code("claude");
        assert!(line.contains("disabled"));
        assert_eq!(code, 1);

        let (line, code) = LoginCheckError::NotInstalled.line_and_code("claude");
        assert!(line.contains("not installed"));
        assert_eq!(code, 1);
    }
}
