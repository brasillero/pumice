//! Antigravity adapter (`agy`), dormant (S2.5).
//!
//! Antigravity stays disabled: no supported per-launch tool policy exists
//! (docs/research/phase2-architecture.md §3.2) and third-party integration
//! may risk account access (docs/research/S0.3-terms.md). `validate_settings`
//! therefore refuses `enabled: true` at the `enabled:` line, and
//! [`DESCRIPTOR`]'s `build` fails as defense in depth, so the configuration
//! path can never produce a runnable Antigravity provider. `agy` is never
//! spawned — not even for `--version`.
//!
//! Everything protocol-shaped below is **documentation-derived**
//! (docs/research/S0.2-cli-matrix.md, "public documentation only"): the argv,
//! the stdin event and the NDJSON stream were never recorded from a real
//! run, and `agy` was never executed. The fake CLI exercises the protocol in
//! tests without implying verified safe execution.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use serde_saphyr::Location;

use super::cli::CliAdapter;
use super::{
    FormatInput, ProbeSpec, Provider, ProviderDescriptor, ProviderError, ProviderErrorCode,
    ProviderLocations, ProviderSettings, RawOption,
};
use crate::config::ConfigError;
use crate::process::{Argument, CliInvocation, ProcessOutput, ProcessRunner, ProgramSpec};

pub const ID: &str = "antigravity";
pub const DEFAULT_BINARY: &str = "agy";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Antigravity accepts no environment overrides: there is no documented
/// per-launch mechanism a safe routing variable could ride on.
const ALLOWED_ENV: &[&str] = &[];

/// Fixed notice `check-config` and startup print while the provider is
/// enabled (it cannot be, but the warning stays with any future enablement).
pub const RISK_WARNING: &str = "Experimental and disabled by default. Third-party integration may risk account access. Tool/startup isolation and transcript suppression are unverified.";

/// Fixed refusal for attempted enablement, pointing at the design note.
const ENABLEMENT_REFUSAL: &str = "providers.antigravity cannot be enabled yet: no supported per-launch tool policy exists (see docs/research/phase2-architecture.md §3.2)";

pub const DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ID,
    defaults,
    allowed_env: ALLOWED_ENV,
    validate_options,
    build,
    disabled_by_default: true,
    risk_warning: Some(RISK_WARNING),
    validate_settings,
    // PATH-only: `agy` is never spawned, not even for `--version`
    // (orchestrator decision 4).
    probe: ProbeSpec::PathOnly,
    default_binary: DEFAULT_BINARY,
    npm_entrypoint: None,
    install_hint: "bundled with the Antigravity IDE (https://antigravity.google)",
};

fn defaults() -> ProviderSettings {
    ProviderSettings {
        enabled: true, // the loader replaces this per `disabled_by_default`
        binary: None,
        // No default model: enablement is refused, so `model` stays optional.
        model: String::new(),
        timeout: DEFAULT_TIMEOUT,
        env: BTreeMap::new(),
        options: BTreeMap::new(),
    }
}

/// No options are supported: with no per-launch policy mechanism there is
/// nothing safe to configure.
fn validate_options(options: &[RawOption<'_>]) -> Result<(), ConfigError> {
    if let Some(option) = options.first() {
        return Err(ConfigError::at(
            option.key_at,
            format!("providers.{ID}.options.{} is not supported", option.key),
        ));
    }
    Ok(())
}

/// The safety preflight: turning the provider on is refused with a fixed
/// explanation, at the `enabled:` line when there is one.
fn validate_settings(
    settings: &ProviderSettings,
    locations: &ProviderLocations,
) -> Result<(), ConfigError> {
    if settings.enabled {
        return Err(ConfigError::at(
            locations.enabled.unwrap_or(Location::UNKNOWN),
            ENABLEMENT_REFUSAL,
        ));
    }
    Ok(())
}

/// Defense in depth: `validate_settings` already keeps `build` unreachable,
/// and a direct call still fails rather than producing a runnable provider.
fn build(
    _settings: &ProviderSettings,
    _runner: Arc<ProcessRunner>,
) -> Result<Arc<dyn Provider>, ConfigError> {
    Err(ConfigError::general(ENABLEMENT_REFUSAL))
}

/// Builds the documentation-derived stream-JSON `agy` call.
///
/// Tests construct this directly; the configuration path cannot reach it,
/// because `validate_settings` refuses `enabled: true` and [`DESCRIPTOR]'s
/// `build` fails (defense in depth).
#[derive(Clone, Debug)]
pub struct AntigravityAdapter {
    binary: PathBuf,
    model: String,
    /// Provider timeout, forwarded as `--print-timeout` (whole seconds).
    timeout: Duration,
}

impl AntigravityAdapter {
    /// `binary` is a command name or a path; `model` must be nonempty and
    /// not look like a flag.
    pub fn new(binary: PathBuf, model: String, timeout: Duration) -> AntigravityAdapter {
        AntigravityAdapter {
            binary,
            model,
            timeout,
        }
    }
}

impl CliAdapter for AntigravityAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn invocation(&self, input: FormatInput<'_>) -> Result<CliInvocation, ProviderError> {
        // A model that looks like a flag would change the command's meaning.
        if self.model.is_empty() || self.model.starts_with('-') {
            return Err(ProviderError::other(ProviderErrorCode::InvalidConfiguration));
        }

        // Documentation-derived argv (headless docs, S0.2): one fresh process
        // per dictation; `--print-timeout` takes the provider timeout.
        let args: Vec<Argument> = [
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--print-timeout",
            &format!("{}s", self.timeout.as_secs()),
            "--sandbox",
            "--model",
            &self.model,
            "--effort",
            "low",
        ]
        .into_iter()
        .map(Argument::literal)
        .collect();

        // The documented transport has no system-prompt flag, so the composed
        // system instructions merge with the user message (E3.3's fallback),
        // separated by a blank line, in one newline-terminated `user` event.
        // Dictated text travels as stdin data, never as argv.
        let user = input.user_prompt;
        let content = format!(
            "{}\n\n{}",
            input.system_prompt,
            [user.before_text, input.text, user.after_text].concat()
        );
        let event = serde_json::json!({
            "event": "user",
            "message": { "content": content },
        });
        let mut stdin = serde_json::to_vec(&event)
            .map_err(|_| ProviderError::other(ProviderErrorCode::InvalidConfiguration))?;
        stdin.push(b'\n');

        Ok(CliInvocation {
            program: ProgramSpec {
                binary: self.binary.clone(),
                npm_entrypoint: None,
            },
            args,
            stdin,
            env: BTreeMap::new(),
            remove_env: Vec::new(),
            control_files: Vec::new(),
            parser: parse_output,
        })
    }
}

/// Parses the documentation-derived stream-JSON NDJSON output.
///
/// Success requires exit code 0, exactly one initialization record and
/// exactly one terminal `result` record with `status == "SUCCESS"` and a
/// string `result.response`; only that response is returned. Text deltas,
/// reasoning, checkpoints and usage are ignored. Any tool record fails with
/// [`ProviderErrorCode::UnexpectedToolActivity`]; malformed, incomplete or
/// contradictory streams fail with [`ProviderErrorCode::InvalidOutput`];
/// documented "authentication required" evidence fails with
/// [`ProviderError::NotLoggedIn`]. Diagnostics are never copied into errors.
pub fn parse_output(output: &ProcessOutput) -> Result<String, ProviderError> {
    parse_stream(
        &String::from_utf8_lossy(&output.stdout),
        output.status.success(),
        &String::from_utf8_lossy(&output.stderr_tail),
    )
}

/// One terminal `result` record's documentation-derived fields.
struct TerminalResult {
    status: Option<String>,
    response: Option<String>,
    /// The serialized `result` object of a failed turn: CLI diagnostics used
    /// only to classify the failure, never copied anywhere.
    failure: String,
}

fn parse_stream(
    stdout: &str,
    exit_success: bool,
    stderr_tail: &str,
) -> Result<String, ProviderError> {
    let mut init_records = 0usize;
    let mut results: Vec<TerminalResult> = Vec::new();
    let mut tool_activity = false;

    for line in stdout.lines() {
        if line.trim().is_empty() {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        };
        // Every documented record is keyed by "event" (like the input event
        // in S0.2); a record without it is not this protocol.
        let Some(event) = record.get("event").and_then(Value::as_str) else {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        };
        match event {
            "init" => init_records += 1,
            // Documentation-derived tool event names (permissions docs); any
            // tool step makes the run ineligible.
            "tool_use" | "tool_call" | "tool_result" => tool_activity = true,
            "result" => results.push(TerminalResult {
                status: record.get("status").and_then(Value::as_str).map(str::to_owned),
                response: record
                    .pointer("/result/response")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                failure: serde_json::to_string(&record["result"]).unwrap_or_default(),
            }),
            // Text deltas, reasoning, checkpoints, usage and future
            // informational records carry no result.
            _ => {}
        }
    }

    if tool_activity {
        return Err(ProviderError::other(
            ProviderErrorCode::UnexpectedToolActivity,
        ));
    }
    let succeeded = exit_success
        && init_records == 1
        && results.len() == 1
        && results[0].status.as_deref() == Some("SUCCESS")
        && results[0].response.is_some();
    if succeeded {
        // Unwrap: `response.is_some()` is part of `succeeded`.
        return Ok(results[0].response.clone().unwrap());
    }
    if auth_required(&results, stderr_tail) {
        return Err(ProviderError::NotLoggedIn);
    }
    if !exit_success {
        return Err(ProviderError::other(ProviderErrorCode::NonzeroExit));
    }
    Err(ProviderError::other(ProviderErrorCode::InvalidOutput))
}

/// The documented missing-login evidence (S0.2): "authentication required"
/// without a cached login. Searched in failed terminal results and stderr;
/// the matched text never leaves this function, and successful text is never
/// heuristically classified.
fn auth_required(results: &[TerminalResult], stderr_tail: &str) -> bool {
    if stderr_tail.to_lowercase().contains("authentication required") {
        return true;
    }
    results.iter().any(|result| {
        result.status.as_deref() != Some("SUCCESS")
            && result
                .failure
                .to_lowercase()
                .contains("authentication required")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::UserPrompt;
    use serde_saphyr::Location;

    const OK: &str = concat!(
        r#"{"event":"init","model":"gemini-test"}"#,
        "\n",
        r#"{"event":"message","type":"model","content":"partial"}"#,
        "\n",
        r#"{"event":"result","status":"SUCCESS","result":{"response":"Olá mundo."}}"#,
        "\n",
    );

    fn option(key: &'static str, value: &'static str) -> RawOption<'static> {
        RawOption {
            key,
            value,
            key_at: Location::UNKNOWN,
            value_at: Location::UNKNOWN,
        }
    }

    #[test]
    fn accepts_no_options() {
        assert!(validate_options(&[]).is_ok());
        let error = validate_options(&[option("effort", "high")]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "providers.antigravity.options.effort is not supported"
        );
    }

    #[test]
    fn validate_settings_refuses_only_enablement() {
        let mut settings = defaults();
        settings.enabled = false;
        assert!(validate_settings(&settings, &ProviderLocations::default()).is_ok());

        settings.enabled = true;
        let error = validate_settings(&settings, &ProviderLocations::default()).unwrap_err();
        assert_eq!(error.to_string(), ENABLEMENT_REFUSAL);
    }

    #[test]
    fn build_fails_as_defense_in_depth() {
        let error = build(&defaults(), Arc::new(ProcessRunner::new()))
            .err()
            .expect("build never produces a runnable provider")
            .to_string();
        assert!(
            error.contains("cannot be enabled yet"),
            "{error}"
        );
    }

    #[test]
    fn parser_returns_only_the_terminal_response() {
        assert_eq!(parse_stream(OK, true, "").unwrap(), "Olá mundo.");
    }

    #[test]
    fn parser_rejects_tool_records_even_when_the_turn_completes() {
        let stream = concat!(
            r#"{"event":"init"}"#,
            "\n",
            r#"{"event":"tool_use","name":"read_file","args":{"path":"/etc/hostname"}}"#,
            "\n",
            r#"{"event":"result","status":"SUCCESS","result":{"response":"ignored"}}"#,
            "\n",
        );
        assert_eq!(
            parse_stream(stream, true, "").unwrap_err(),
            ProviderError::other(ProviderErrorCode::UnexpectedToolActivity)
        );
    }

    #[test]
    fn parser_classifies_documented_authentication_evidence() {
        let failed = concat!(
            r#"{"event":"init"}"#,
            "\n",
            r#"{"event":"result","status":"FAILURE","result":{"error":"authentication required: no cached login"}}"#,
            "\n",
        );
        assert_eq!(parse_stream(failed, false, "").unwrap_err(), ProviderError::NotLoggedIn);
        // The same evidence is documented on stderr.
        assert_eq!(
            parse_stream("{\"event\":\"init\"}\n", false, "authentication required"),
            Err(ProviderError::NotLoggedIn)
        );
        // A successful turn is never reclassified on stderr text.
        assert_eq!(parse_stream(OK, true, "authentication required").unwrap(), "Olá mundo.");
    }

    #[test]
    fn parser_rejects_malformed_and_contradictory_streams() {
        let failure = concat!(
            r#"{"event":"init"}"#,
            "\n",
            r#"{"event":"result","status":"FAILURE","result":{"error":"boom"}}"#,
            "\n",
        );
        for (stream, exit_success, expected) in [
            ("", true, ProviderErrorCode::InvalidOutput),
            ("not json\n", true, ProviderErrorCode::InvalidOutput),
            ("\n", true, ProviderErrorCode::InvalidOutput),
            ("[1,2]\n", true, ProviderErrorCode::InvalidOutput),
            // No "event" key.
            (
                "{\"status\":\"SUCCESS\",\"result\":{\"response\":\"x\"}}\n",
                true,
                ProviderErrorCode::InvalidOutput,
            ),
            // Missing initialization record.
            (
                "{\"event\":\"result\",\"status\":\"SUCCESS\",\"result\":{\"response\":\"x\"}}\n",
                true,
                ProviderErrorCode::InvalidOutput,
            ),
            // Two initialization records.
            (
                "{\"event\":\"init\"}\n{\"event\":\"init\"}\n{\"event\":\"result\",\"status\":\"SUCCESS\",\"result\":{\"response\":\"x\"}}\n",
                true,
                ProviderErrorCode::InvalidOutput,
            ),
            // Two contradictory terminal results.
            (
                "{\"event\":\"init\"}\n{\"event\":\"result\",\"status\":\"SUCCESS\",\"result\":{\"response\":\"a\"}}\n{\"event\":\"result\",\"status\":\"SUCCESS\",\"result\":{\"response\":\"b\"}}\n",
                true,
                ProviderErrorCode::InvalidOutput,
            ),
            // Missing and non-string responses.
            (
                "{\"event\":\"init\"}\n{\"event\":\"result\",\"status\":\"SUCCESS\"}\n",
                true,
                ProviderErrorCode::InvalidOutput,
            ),
            (
                "{\"event\":\"init\"}\n{\"event\":\"result\",\"status\":\"SUCCESS\",\"result\":{\"response\":42}}\n",
                true,
                ProviderErrorCode::InvalidOutput,
            ),
            // FAILURE with a clean exit is contradictory.
            (failure, true, ProviderErrorCode::InvalidOutput),
        ] {
            assert_eq!(
                parse_stream(stream, exit_success, "").unwrap_err(),
                ProviderError::other(expected),
                "case: {stream}"
            );
        }
        // A successful-looking stream with a nonzero exit is a failure.
        assert_eq!(
            parse_stream(OK, false, "").unwrap_err(),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
        assert_eq!(
            parse_stream(failure, false, "").unwrap_err(),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
    }

    #[test]
    fn stdin_event_merges_system_prompt_and_user_message() {
        let adapter = AntigravityAdapter::new(
            PathBuf::from(DEFAULT_BINARY),
            "gemini-test".to_owned(),
            DEFAULT_TIMEOUT,
        );
        let invocation = adapter
            .invocation(FormatInput {
                system_prompt: "System. Coração 🎤",
                user_prompt: UserPrompt {
                    before_text: "Before\n",
                    after_text: "\nAfter",
                },
                text: "Ditado $(rm -rf ~) &",
            })
            .expect("invocation builds");

        let stdin = String::from_utf8(invocation.stdin).expect("UTF-8 stdin");
        assert_eq!(stdin.matches('\n').count(), 1, "one newline-terminated line");
        let event: Value =
            serde_json::from_str(stdin.trim_end_matches('\n')).expect("valid JSON event");
        assert_eq!(event["event"], serde_json::json!("user"));
        assert_eq!(
            event["message"]["content"].as_str().unwrap(),
            "System. Coração 🎤\n\nBefore\nDitado $(rm -rf ~) &\nAfter"
        );
        assert!(invocation.control_files.is_empty());
    }
}
