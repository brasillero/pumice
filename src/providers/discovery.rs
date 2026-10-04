//! Startup detection (S2.8 part 1): which provider CLIs are installed and
//! which versions they report.
//!
//! [`detect`] resolves every registered provider through the same
//! [`resolve_program`] path a formatting call uses, then — except for
//! [`ProbeSpec::PathOnly`] providers such as Antigravity, which is never
//! spawned — runs a bounded `--version`-style probe through the shared
//! [`ProcessRunner`], so a probe gets a fresh empty workspace, no shell,
//! closed stdin and process-tree kill on timeout. The service runs detection
//! once at startup and caches the result in the pipeline; `/v1/models` then
//! lists only enabled providers detection confirmed installed. Detection
//! never spends quota, never enables a provider and never prints probe
//! stderr. A failed or timed-out probe means "version unavailable", never
//! "missing".

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio::time::Instant;

use super::{ProbeSpec, ProviderDescriptor, ProviderError, ProviderErrorCode, ProviderSettings};
use crate::config::Config;
use crate::process::{
    Argument, CliInvocation, ProcessOutput, ProcessRunner, ProgramSpec, resolve_program,
};

/// Version-probe budget per provider.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// A probe printing more stdout than this fails instead of being parsed.
const PROBE_STDOUT_CAP: usize = 4 * 1024;
/// A parsed version token never exceeds this many characters.
const VERSION_TOKEN_MAX_CHARS: usize = 32;

/// Detection result for one provider; [`detect`] returns descriptor order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderStatus {
    pub id: &'static str,
    pub enabled: bool,
    pub found: Found,
    pub version: Version,
}

/// Where resolving a provider's program landed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// Resolved to this executable; formatting calls can spawn it.
    Found(PathBuf),
    /// Not found on PATH, or the configured path does not exist.
    Missing,
    /// Found, but it is a script wrapper Pumice refuses to translate
    /// (on Windows, anything but a supported npm `.cmd` shim).
    UnsupportedShim,
}

/// What a version probe established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Version {
    /// The first version-looking token of the probe output.
    Parsed(String),
    /// The probe failed, timed out or printed nothing version-shaped. This
    /// never changes [`Found`]: the provider is still installed.
    Unavailable,
    /// Deliberately not probed ([`ProbeSpec::PathOnly`]).
    Skipped,
}

/// Detects every provider in `descriptors` — enabled or not, so a later
/// `pumice doctor` can show the whole registry — and returns the statuses in
/// descriptor order. Probes run concurrently; each gets its own deadline of
/// [`PROBE_TIMEOUT`]. Never spends quota and never spawns a
/// [`ProbeSpec::PathOnly`] provider.
pub async fn detect(
    config: &Config,
    descriptors: &[ProviderDescriptor],
    runner: &ProcessRunner,
) -> Vec<ProviderStatus> {
    detect_with_timeout(config, descriptors, runner, PROBE_TIMEOUT).await
}

/// [`detect`] with an injectable probe deadline for tests.
pub async fn detect_with_timeout(
    config: &Config,
    descriptors: &[ProviderDescriptor],
    runner: &ProcessRunner,
    probe_timeout: Duration,
) -> Vec<ProviderStatus> {
    // Resolution is a cheap filesystem lookup; do all of it up front and run
    // the (possibly slow) version probes concurrently afterwards.
    let mut found: Vec<Found> = Vec::with_capacity(descriptors.len());
    let mut probes: Vec<(usize, CliInvocation)> = Vec::new();
    for (index, descriptor) in descriptors.iter().enumerate() {
        let program = probe_program(descriptor, config.providers.get(descriptor.id));
        let resolved = match resolve_program(&program) {
            Ok(resolved) => Found::Found(resolved.path),
            Err(ProviderError::Other {
                code: ProviderErrorCode::UnsupportedShim,
            }) => Found::UnsupportedShim,
            Err(_) => Found::Missing,
        };
        if let (Found::Found(_), ProbeSpec::Version(args)) = (&resolved, descriptor.probe) {
            probes.push((index, probe_invocation(&program, args)));
        }
        found.push(resolved);
    }

    let mut set: JoinSet<(usize, Version)> = JoinSet::new();
    for (index, invocation) in probes {
        // `ProcessRunner` is stateless, so a copy runs the probe with exactly
        // the same lifecycle (fresh workspace, no shell, process-tree kill).
        let runner = *runner;
        set.spawn(async move {
            (
                index,
                probe_version(runner, invocation, probe_timeout).await,
            )
        });
    }
    let mut probed: BTreeMap<usize, Version> = BTreeMap::new();
    while let Some(joined) = set.join_next().await {
        // A probe task has no panicking path; if one ever appeared, that
        // provider would simply report `Unavailable`.
        if let Ok((index, version)) = joined {
            probed.insert(index, version);
        }
    }

    descriptors
        .iter()
        .enumerate()
        .map(|(index, descriptor)| {
            let settings = config.providers.get(descriptor.id);
            let version = match (&found[index], descriptor.probe) {
                (Found::Found(_), ProbeSpec::Version(_)) => {
                    probed.get(&index).cloned().unwrap_or(Version::Unavailable)
                }
                (Found::Found(_), ProbeSpec::PathOnly) => Version::Skipped,
                (Found::Missing, _) | (Found::UnsupportedShim, _) => Version::Unavailable,
            };
            ProviderStatus {
                id: descriptor.id,
                enabled: settings.is_some_and(|settings| settings.enabled),
                found: found[index].clone(),
                version,
            }
        })
        .collect()
}

/// The program detection resolves for `descriptor`: the configured `binary`
/// or the provider's default command name, with the same npm entrypoint the
/// adapter declares, so detection resolves exactly like a formatting call.
fn probe_program(
    descriptor: &ProviderDescriptor,
    settings: Option<&ProviderSettings>,
) -> ProgramSpec {
    let binary = settings
        .and_then(|settings| settings.binary.clone())
        .unwrap_or_else(|| PathBuf::from(descriptor.default_binary));
    ProgramSpec {
        binary,
        npm_entrypoint: descriptor.npm_entrypoint,
    }
}

/// A version probe: just the version arguments, an empty stdin (the runner
/// closes the pipe), no control files and no environment changes. The parser
/// is never called — detection reads the output itself — but the invocation
/// type requires one.
fn probe_invocation(program: &ProgramSpec, version_args: &'static [&'static str]) -> CliInvocation {
    CliInvocation {
        program: program.clone(),
        args: version_args
            .iter()
            .map(|arg| Argument::literal(OsString::from(*arg)))
            .collect(),
        stdin: Vec::new(),
        env: BTreeMap::new(),
        remove_env: Vec::new(),
        control_files: Vec::new(),
        parser: ignore_output,
    }
}

fn ignore_output(_: &ProcessOutput) -> Result<String, ProviderError> {
    Ok(String::new())
}

/// Runs one version probe through the shared runner. Every failure —
/// deadline, spawn error, nonzero exit, oversized or unparseable output — is
/// [`Version::Unavailable`]; a failed probe never means "missing".
async fn probe_version(
    runner: ProcessRunner,
    invocation: CliInvocation,
    timeout: Duration,
) -> Version {
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        return Version::Unavailable;
    };
    let output = match runner.run(invocation, deadline).await {
        Ok(output) => output,
        Err(_) => return Version::Unavailable,
    };
    if !output.status.success() || output.stdout.len() > PROBE_STDOUT_CAP {
        return Version::Unavailable;
    }
    parse_version(&String::from_utf8_lossy(&output.stdout))
        .map_or(Version::Unavailable, Version::Parsed)
}

/// Extracts the first version-looking token of the probe output (for example
/// `2.1.288`), keeping at most [`VERSION_TOKEN_MAX_CHARS`] characters.
/// Returns `None` when no token looks like a version. Probe stderr is never
/// examined.
fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace().find_map(version_token)
}

/// Accepts `token` when its core is digits and dots with at least two
/// nonempty numeric parts (`2`, `1`, `288`), optionally suffixed with
/// `-pre`/`+build` characters; returns the token bounded to
/// [`VERSION_TOKEN_MAX_CHARS`] characters.
fn version_token(token: &str) -> Option<String> {
    let core = token.split(['-', '+']).next().unwrap_or(token);
    let mut parts = core.split('.');
    let (Some(first), Some(second)) = (parts.next(), parts.next()) else {
        return None;
    };
    let core_ok = !first.is_empty()
        && !second.is_empty()
        && core.ends_with(|c: char| c.is_ascii_digit())
        && core.chars().all(|c| c.is_ascii_digit() || c == '.');
    if !core_ok {
        return None;
    }
    Some(token.chars().take(VERSION_TOKEN_MAX_CHARS).collect())
}

/// One startup line for an enabled provider, for example
/// `provider claude: found 2.1.288` or
/// `provider codex: missing (install: npm install -g @openai/codex)`.
pub fn status_line(status: &ProviderStatus, install_hint: &str) -> String {
    let state = match &status.found {
        Found::Found(_) => match &status.version {
            Version::Parsed(version) => format!("found {version}"),
            Version::Unavailable => "found (version unavailable)".to_owned(),
            Version::Skipped => "found".to_owned(),
        },
        Found::Missing => format!("missing (install: {install_hint})"),
        Found::UnsupportedShim => format!("unsupported wrapper (install: {install_hint})"),
    };
    format!("provider {}: {state}", status.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_token_accepts_numeric_versions() {
        assert_eq!(version_token("2.1.288"), Some("2.1.288".to_owned()));
        assert_eq!(version_token("0.160.0"), Some("0.160.0".to_owned()));
        assert_eq!(version_token("1.18.34"), Some("1.18.34".to_owned()));
        assert_eq!(version_token("1.2-beta.3"), Some("1.2-beta.3".to_owned()));
    }

    #[test]
    fn version_token_rejects_non_versions() {
        for token in [
            "", "claude", "1", "1.", ".1", ".1.2", "1..2", "1.2.", "v2.1.288", "1.2/3",
        ] {
            assert_eq!(version_token(token), None, "token: {token}");
        }
    }

    #[test]
    fn version_token_caps_its_length() {
        let long = format!("1.2.3-{}", "x".repeat(64));
        assert_eq!(
            version_token(&long).map(|token| token.len()),
            Some(VERSION_TOKEN_MAX_CHARS)
        );
    }

    #[test]
    fn parse_version_finds_the_first_version_token() {
        assert_eq!(
            parse_version("2.1.288 (Claude Code)\n"),
            Some("2.1.288".to_owned())
        );
        assert_eq!(
            parse_version("codex-cli 0.160.0\n"),
            Some("0.160.0".to_owned())
        );
        assert_eq!(
            parse_version("noise first\n1.18.34\n"),
            Some("1.18.34".to_owned())
        );
        assert_eq!(parse_version("nothing to see here\n"), None);
    }
}
