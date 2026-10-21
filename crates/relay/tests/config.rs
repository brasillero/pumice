//! Tests for YAML configuration loading, validation and error positions.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use pumice_relay::config::{self, Config};
use pumice_relay::process::ProcessRunner;
use pumice_relay::providers::{self, ProviderDescriptor, ProviderSettings};
use tempfile::TempDir;

mod support;

use support::relay_config;

const CONFIG_NAME: &str = "relay.yml";

/// Writes `text`, completed by [`relay_config`] with the top-level settings
/// it does not write, to a temporary `relay.yml` and loads it.
fn load_text(text: &str) -> Result<Config, String> {
    load_exact(&relay_config(text))
}

/// Writes exactly `text` to a temporary `relay.yml` and loads it.
fn load_exact(text: &str) -> Result<Config, String> {
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let path = dir.path().join(CONFIG_NAME);
    fs::write(&path, text).map_err(|e| e.to_string())?;
    config::load_with_env(Some(&path), |_| None)
        .map(|loaded| loaded.config)
        .map_err(|error| error.to_string())
}

/// A complete file: every top-level setting and one enabled Claude entry,
/// the way `relay.example.yml` writes them.
const COMPLETE: &str = "\
port: 7567
total_timeout_secs: 30
max_parallel: 4
providers:
  - id: claude
    enabled: true
    model: haiku
    binary: claude
    timeout_secs: 30
debug_log:
  enabled: false
  path: pumice-debug.jsonl
";

/// [`COMPLETE`] without the line(s) of one setting: `key` is the exact
/// text that starts them, e.g. `"    binary:"`.
fn without(key: &str) -> String {
    let mut lines: Vec<&str> = COMPLETE.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with(key))
        .unwrap_or_else(|| panic!("{key} is in COMPLETE"));
    let indent = key.len() - key.trim_start().len();
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.len() - line.trim_start().len() <= indent)
        .map_or(lines.len(), |offset| start + 1 + offset);
    lines.drain(start..end);
    lines.join("\n") + "\n"
}

/// Loads `text` expecting failure, returning the rendered error.
fn load_error(text: &str) -> String {
    match load_text(text) {
        Ok(_) => panic!("expected a configuration error for:\n{text}"),
        Err(error) => error,
    }
}

/// Loads `text` expecting failure, asserting the error display ends with
/// `":line:column: message"` and names the configuration file.
fn assert_error(text: &str, line: u64, column: u64, message: &str) {
    let error = load_error(text);
    assert!(
        error.contains(&format!(":{line}:{column}: {message}")),
        "expected ':{line}:{column}: {message}' in:\n{error}"
    );
    assert!(
        error.contains(CONFIG_NAME),
        "error names the file:\n{error}"
    );
}

fn claude(config: &Config) -> &ProviderSettings {
    config.provider("claude").expect("claude is configured")
}

/// Every active provider ID with a model it accepts. The common entry rules
/// below run over this table; provider-specific validation stays in each
/// provider's own suite.
const PROVIDERS: [(&str, &str); 4] = [
    ("claude", "haiku"),
    ("codex", "gpt-6.1-sol"),
    ("kimi", "kimi-k2.7-code-highspeed"),
    ("kiro", "claude-haiku-4.5"),
];

fn no_env(_: &str) -> Option<OsString> {
    None
}

/// Synthetic provider descriptor exercising registry capabilities (explicit
/// enablement, risk warnings) against `config::validate_text_with_descriptors`,
/// leaving the built-in registry untouched.
mod capability_probe {
    use std::sync::Arc;
    use std::time::Duration;

    use pumice_relay::config::ConfigError;
    use pumice_relay::process::ProcessRunner;
    use pumice_relay::providers::{
        ProbeSpec, Provider, ProviderDescriptor, ProviderSettings, validate_settings_noop,
    };

    /// A minimal provider: off until the file says `enabled: true`.
    pub const PROBE: ProviderDescriptor = ProviderDescriptor {
        id: "probe",
        defaults: probe_defaults,
        build: probe_build,
        validate_settings: validate_settings_noop,
        probe: ProbeSpec::PathOnly,
        default_binary: "probe-cli",
        npm_entrypoint: None,
        install_hint: "the probe is not installable",
    };

    fn probe_defaults() -> ProviderSettings {
        ProviderSettings {
            enabled: false, // the loader replaces this with the entry's explicit value
            binary: None,
            model: String::new(),
            timeout: Duration::from_secs(30),
        }
    }

    fn probe_build(
        _settings: &ProviderSettings,
        _runner: Arc<ProcessRunner>,
    ) -> Result<Arc<dyn Provider>, ConfigError> {
        panic!("the probe descriptor is never built")
    }
}

#[test]
fn complete_file_loads_the_written_values() {
    let config = load_exact(COMPLETE).expect("complete file loads");
    assert_eq!(config.port, 7567);
    assert_eq!(config.total_timeout, Duration::from_secs(30));
    assert_eq!(config.max_parallel, 4);
    assert!(!config.debug_log.enabled);
    assert!(config.debug_log.path.ends_with("pumice-debug.jsonl"));
    let claude = claude(&config);
    assert!(claude.enabled);
    assert_eq!(claude.model, "haiku");
    assert_eq!(claude.binary, Some(PathBuf::from("claude")));
    assert_eq!(claude.timeout, Duration::from_secs(30));
}

#[test]
fn empty_or_comment_only_file_reports_the_first_missing_setting_at_line_1() {
    for text in ["", "# nothing here\n# at all\n"] {
        let error = load_exact(text).expect_err("nothing is written");
        assert!(
            error.contains(
                ":1:1: \"port\" is missing; every setting must be written (see relay.example.yml)"
            ),
            "{error}"
        );
    }
}

#[test]
fn every_top_level_setting_is_required() {
    for key in [
        "port",
        "total_timeout_secs",
        "max_parallel",
        "providers",
        "debug_log",
    ] {
        let error = load_exact(&without(&format!("{key}:"))).expect_err(key);
        assert!(
            error.contains(&format!(":1:1: \"{key}\" is missing")),
            "{key}: {error}"
        );
        assert!(error.contains(CONFIG_NAME), "{key}: {error}");
    }
}

#[test]
fn missing_nested_settings_are_reported_at_their_mapping() {
    // `debug_log` starts on line 10; its first key is at column 3.
    for key in ["enabled", "path"] {
        let error = load_exact(&without(&format!("  {key}:"))).expect_err(key);
        assert!(
            error.contains(&format!(":11:3: \"debug_log.{key}\" is missing")),
            "{key}: {error}"
        );
    }
    // A provider entry is reported at its first key (`id`, line 5, column 5).
    for key in ["model", "binary", "timeout_secs"] {
        let error = load_exact(&without(&format!("    {key}:"))).expect_err(key);
        assert!(
            error.contains(&format!(
                ":5:5: providers entry \"claude\".{key} is missing; every setting must be written"
            )),
            "{key}: {error}"
        );
    }
    // Flow mappings are located at their opening brace.
    let error = load_text("debug_log: {enabled: false}\n").expect_err("path is missing");
    assert!(
        error.contains(":1:12: \"debug_log.path\" is missing"),
        "{error}"
    );
}

#[test]
fn explicit_null_is_rejected_where_a_value_is_needed() {
    for (from, to, at, name) in [
        ("port: 7567", "port: null", ":1:7:", "\"port\""),
        (
            "total_timeout_secs: 30",
            "total_timeout_secs: ~",
            ":2:21:",
            "\"total_timeout_secs\"",
        ),
        (
            "max_parallel: 4",
            "max_parallel:",
            ":3:13:",
            "\"max_parallel\"",
        ),
        (
            "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: claude\n    timeout_secs: 30\n",
            "providers: null\n",
            ":4:12:",
            "\"providers\"",
        ),
        (
            "    enabled: true",
            "    enabled: null",
            ":6:14:",
            "providers entry \"claude\".enabled",
        ),
        (
            "    timeout_secs: 30",
            "    timeout_secs: null",
            ":9:19:",
            "providers entry \"claude\".timeout_secs",
        ),
        (
            "  enabled: false",
            "  enabled: null",
            ":11:12:",
            "\"debug_log.enabled\"",
        ),
        (
            "  path: pumice-debug.jsonl",
            "  path: null",
            ":12:9:",
            "\"debug_log.path\"",
        ),
    ] {
        let text = COMPLETE.replace(from, to);
        assert_ne!(text, COMPLETE, "{from}");
        let error = load_exact(&text).expect_err(to);
        assert!(
            error.contains(&format!("{at} {name} must not be null")),
            "{to}: {error}"
        );
    }
    let error = load_exact(&COMPLETE.replace(
        "debug_log:\n  enabled: false\n  path: pumice-debug.jsonl\n",
        "debug_log: null\n",
    ))
    .expect_err("null debug_log");
    assert!(
        error.contains(":10:12: \"debug_log\" must not be null"),
        "{error}"
    );
}

#[test]
fn explicit_values_are_kept_apart_from_omitted_ones() {
    // `false`, an empty list and a disabled entry's `model: null` are
    // explicit values, not omissions.
    let config = load_text(
        "providers:\n  - id: claude\n    enabled: false\n    model: null\n    binary: claude\n    timeout_secs: 30\n",
    )
    .expect("explicit values load");
    let claude = claude(&config);
    assert!(!claude.enabled);
    assert!(claude.model.is_empty());

    let config = load_text("providers: []\n").expect("an empty list loads");
    assert!(config.providers.is_empty());

    // The same disabled entry without the `model` key is incomplete.
    let error = load_text(
        "providers:\n  - id: claude\n    enabled: false\n    binary: claude\n    timeout_secs: 30\n",
    )
    .expect_err("model is omitted");
    assert!(
        error.contains(":2:5: providers entry \"claude\".model is missing"),
        "{error}"
    );
}

#[test]
fn mapping_form_of_providers_fails_with_a_removal_hint() {
    // The pre-0.2 mapping form (empty or not) fails with the list form that
    // replaced it, positioned at the mapping value.
    let error = load_error("providers: {}\n");
    assert!(
        error.contains(
            "the mapping form of \"providers\" was removed in 0.2; use an ordered list instead"
        ),
        "{error}"
    );
    assert!(
        error.contains(":1:12:"),
        "positioned at the value:\n{error}"
    );

    let error = load_error("providers:\n  claude:\n    enabled: true\n");
    assert!(
        error.contains(
            "the mapping form of \"providers\" was removed in 0.2; use an ordered list instead"
        ),
        "{error}"
    );
    assert!(
        error.contains(":2:3:"),
        "positioned at the first entry key:\n{error}"
    );
    assert!(
        error.contains(CONFIG_NAME),
        "error names the file:\n{error}"
    );
}

#[test]
fn crlf_line_endings_parse_and_positions_count_lines() {
    let config = load_text(
        "port: 9001\r\nproviders:\r\n  - id: claude\r\n    enabled: true\r\n    model: olá\r\n    binary: claude\r\n    timeout_secs: 30\r\n",
    )
    .expect("CRLF loads");
    assert_eq!(config.port, 9001);
    assert_eq!(claude(&config).model, "olá");

    // Line numbers count CRLF files exactly like LF files.
    assert_error(
        "port: 9001\r\nproviders:\r\n  - id: claude\r\n    enabled: true\r\n    model: haiku\r\n    timeout_secs: 0\r\n    binary: claude\r\n",
        6,
        19,
        "providers entry \"claude\".timeout_secs must be greater than zero",
    );
}

#[test]
fn unicode_content_round_trips() {
    let config = load_text(
        "# configuração\nproviders:\n  - id: claude\n    enabled: true\n    model: ünïcodé-ãõ\n    binary: claude\n    timeout_secs: 30\n",
    )
    .expect("unicode loads");
    assert_eq!(claude(&config).model, "ünïcodé-ãõ");
    assert_eq!(config.port, 7567);
}

#[test]
fn zero_port_is_rejected_at_the_value() {
    assert_error("port: 0\n", 1, 7, "port must be greater than zero");
}

#[test]
fn zero_timeouts_are_rejected_at_the_value() {
    assert_error(
        "total_timeout_secs: 0\n",
        1,
        21,
        "total_timeout_secs must be greater than zero",
    );
    // Line 12 and column 19, matching the documented error shape.
    assert_error(
        "# seven comment lines place the value on line 12\n# 2\n# 3\n# 4\n# 5\n# 6\n# 7\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    timeout_secs: 0\n    binary: claude\n",
        12,
        19,
        "providers entry \"claude\".timeout_secs must be greater than zero",
    );
}

#[test]
fn max_parallel_is_read_from_config() {
    let config = load_text(
        "max_parallel: 2\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: claude\n    timeout_secs: 30\n",
    )
    .expect("loads");
    assert_eq!(config.max_parallel, 2);
}

#[test]
fn zero_max_parallel_is_rejected_at_the_value() {
    assert_error(
        "max_parallel: 0\n",
        1,
        15,
        "max_parallel must be at least 1",
    );
}

#[test]
fn max_parallel_above_limit_is_rejected_at_the_value() {
    assert_error(
        "max_parallel: 33\n",
        1,
        15,
        "max_parallel must be at most 32",
    );
}

#[test]
fn port_outside_u16_range_is_a_type_error_at_the_value() {
    assert_error("port: 70000\n", 1, 7, "invalid u16");
    assert_error("port: not-a-port\n", 1, 7, "invalid u16");
}

#[test]
fn syntax_error_reports_its_position() {
    // Unterminated quoted scalar.
    let error = load_error("port: 'abc\n");
    assert!(
        error.contains(":2:1:") && error.contains("invalid indentation"),
        "unexpected error:\n{error}"
    );
    // Unterminated flow sequence.
    assert_error("port: [7567,\n", 1, 7, "expected string scalar");
}

#[test]
fn unknown_top_level_key_is_rejected_at_the_key() {
    assert_error("porrt: 7567\n", 1, 1, "unknown setting `porrt`");
}

#[test]
fn unknown_nested_key_is_rejected_at_the_key() {
    assert_error(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    unknown_key: 1\n",
        5,
        5,
        "unknown setting `unknown_key`",
    );
    assert_error(
        "debug_log:\n  enabled: true\n  bogus: x\n",
        3,
        3,
        "unknown setting `bogus`",
    );
}

#[test]
fn duplicate_keys_are_rejected_at_the_second_key() {
    assert_error("port: 1\nport: 2\n", 2, 1, "duplicate key `port`");
    assert_error(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    timeout_secs: 1\n    timeout_secs: 2\n",
        6,
        5,
        "duplicate key `timeout_secs`",
    );
}

#[test]
fn multiple_documents_are_rejected() {
    assert_error(
        "port: 7567\n---\nport: 2\n",
        3,
        1,
        "multiple YAML documents are not allowed",
    );
}

#[test]
fn unknown_provider_is_rejected_at_its_id() {
    assert_error(
        "providers:\n  - id: bogus\n    enabled: true\n    model: haiku\n",
        2,
        9,
        "\"bogus\" is not a known provider",
    );
}

#[test]
fn enabled_entry_with_a_model_loads_for_every_provider() {
    for (id, model) in PROVIDERS {
        let config = load_text(&format!(
            "providers:\n  - id: {id}\n    enabled: true\n    model: {model}\n    binary: {id}-cli\n    timeout_secs: 7\n"
        ))
        .unwrap_or_else(|error| panic!("{id}: {error}"));
        let settings = config.provider(id).expect("provider configured");
        assert!(settings.enabled, "{id}");
        assert_eq!(settings.model, model, "{id}");
        assert_eq!(
            settings.binary,
            Some(PathBuf::from(format!("{id}-cli"))),
            "{id}"
        );
        assert_eq!(settings.timeout, Duration::from_secs(7), "{id}");
        assert_eq!(config.provider("bogus"), None);
    }
}

#[test]
fn disabled_entry_is_kept_but_off_for_every_provider() {
    for (id, _) in PROVIDERS {
        let config = load_text(&format!(
            "providers:\n  - id: {id}\n    enabled: false\n    model: null\n    binary: {id}\n    timeout_secs: 30\n"
        ))
            .unwrap_or_else(|error| panic!("{id}: {error}"));
        let settings = config.provider(id).expect("provider configured");
        assert!(!settings.enabled, "{id}");
        assert_eq!(settings.model, "", "{id}");
    }
}

#[test]
fn duplicate_provider_entries_are_rejected_at_the_second_id() {
    assert_error(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: claude\n    timeout_secs: 30\n  - id: claude\n    enabled: false\n",
        7,
        9,
        "duplicate provider \"claude\" in providers",
    );
    for (id, _) in PROVIDERS {
        assert_error(
            &format!(
                "providers:\n  - id: {id}\n    enabled: false\n    model: null\n    binary: {id}\n    timeout_secs: 30\n  - id: {id}\n    enabled: false\n"
            ),
            7,
            9,
            &format!("duplicate provider \"{id}\" in providers"),
        );
    }
}

#[test]
fn missing_enabled_is_rejected_at_the_id() {
    assert_error(
        "providers:\n  - id: claude\n    model: haiku\n",
        2,
        9,
        "providers entry \"claude\" has no \"enabled\"; add \"enabled: true\" or \"enabled: false\"",
    );
}

#[test]
fn default_provider_key_fails_with_a_removal_hint() {
    let error = load_error("default_provider: gemini\n");
    assert!(
        error.contains("\"default_provider\" was removed in 0.2:"),
        "{error}"
    );
    assert!(
        error.contains("providers:\n  - id: claude\n    enabled: true\n    model: haiku"),
        "{error}"
    );
    assert!(
        error.contains(CONFIG_NAME),
        "error names the file:\n{error}"
    );
}

#[test]
fn default_key_fails_with_a_removal_hint() {
    let error = load_error("default: claude\n");
    assert!(error.contains("\"default\" was removed in 0.2:"), "{error}");
    assert!(
        error.contains("the client's model field picks the provider"),
        "{error}"
    );
    assert!(
        error.contains("a request without a model gets an error and the app keeps its own text"),
        "{error}"
    );
    assert!(
        error.contains("providers:\n  - id: claude\n    enabled: true\n    model: haiku"),
        "{error}"
    );
    assert!(
        error.contains(&format!("{CONFIG_NAME}:1:10")),
        "the error points at the key's line:column:\n{error}"
    );
    assert!(
        error.contains(CONFIG_NAME),
        "error names the file:\n{error}"
    );
}

#[test]
fn fallback_order_key_fails_with_a_removal_hint() {
    let error = load_error("fallback_order:\n  - claude\n  - claude\n");
    assert!(
        error.contains("\"fallback_order\" was removed in 0.2:"),
        "{error}"
    );
    assert!(
        error.contains("providers:\n  - id: claude\n    enabled: true\n    model: haiku"),
        "{error}"
    );
    assert!(
        error.contains(CONFIG_NAME),
        "error names the file:\n{error}"
    );
}

#[test]
fn empty_model_is_rejected_at_the_value() {
    assert_error(
        "providers:\n  - id: claude\n    enabled: true\n    model: \"\"\n    binary: claude\n",
        4,
        12,
        "providers entry \"claude\".model must not be empty",
    );
    assert_error(
        "providers:\n  - id: claude\n    enabled: true\n    model: \"   \"\n    binary: claude\n",
        4,
        12,
        "providers entry \"claude\".model must not be empty",
    );
}

#[test]
fn empty_model_on_a_disabled_entry_is_allowed() {
    // `model` is only required when the entry is enabled; a disabled entry
    // may keep an empty one (this is the shape `pumice setup` will write
    // for a provider the owner has not picked a model for yet).
    let config = load_text(
        "providers:\n  - id: claude\n    enabled: false\n    model: \"\"\n    binary: claude\n    timeout_secs: 30\n",
    )
        .expect("a disabled entry with an empty model loads");
    let claude = config.provider("claude").expect("claude is configured");
    assert!(!claude.enabled);
    assert!(claude.model.is_empty());
}

#[test]
fn relative_binary_path_resolves_against_the_config_directory() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(CONFIG_NAME);
    fs::write(
        &path,
        relay_config(
            "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: tools/claude\n    timeout_secs: 30\n",
        ),
    )
    .expect("write config");

    let loaded = config::load_with_env(Some(&path), no_env).expect("config loads");
    assert_eq!(
        claude(&loaded.config).binary,
        Some(dir.path().join("tools").join("claude"))
    );
}

#[test]
fn bare_and_absolute_binary_paths_are_not_resolved() {
    let config = load_text(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: myclaude\n    timeout_secs: 30\n",
    )
    .expect("bare name loads");
    assert_eq!(claude(&config).binary, Some(PathBuf::from("myclaude")));

    // An absolute path on every OS (`/opt/claude` is relative on Windows).
    let absolute = std::env::temp_dir().join("claude");
    let yaml = format!(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{}'\n    timeout_secs: 30\n",
        absolute.display()
    );
    let config = load_text(&yaml).expect("absolute loads");
    assert_eq!(claude(&config).binary, Some(absolute));

    // `null` used to mean the command on PATH: the command is now written.
    assert_error(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: null\n    timeout_secs: 30\n",
        5,
        13,
        "providers entry \"claude\".binary must not be null; write the command, e.g. \"binary: claude\" to look it up on PATH",
    );
}

#[test]
fn relative_debug_log_path_resolves_against_the_config_directory() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(CONFIG_NAME);
    fs::write(
        &path,
        relay_config("debug_log:\n  enabled: true\n  path: logs/debug.jsonl\n"),
    )
    .expect("write");

    let loaded = config::load_with_env(Some(&path), no_env).expect("config loads");
    assert!(loaded.config.debug_log.enabled);
    assert_eq!(
        loaded.config.debug_log.path,
        dir.path().join("logs").join("debug.jsonl")
    );

    // The example's file name also lands beside the configuration file.
    fs::write(&path, COMPLETE).expect("write config");
    let loaded = config::load_with_env(Some(&path), no_env).expect("config loads");
    assert_eq!(
        loaded.config.debug_log.path,
        dir.path().join("pumice-debug.jsonl")
    );

    let error = load_text("debug_log:\n  enabled: false\n  path: ''\n").expect_err("empty path");
    assert!(
        error.contains(":3:9: debug_log.path must not be empty"),
        "{error}"
    );
}

#[test]
fn explicit_missing_file_is_an_error_naming_the_path() {
    let dir = TempDir::new().expect("temp dir");
    let missing = dir.path().join("nope.yaml");
    let error = config::load_with_env(Some(&missing), no_env).expect_err("missing file fails");
    assert_eq!(
        error.to_string(),
        format!(
            "{}: the configuration file does not exist",
            missing.display()
        )
    );
}

#[test]
fn explicit_unreadable_file_reports_the_os_error() {
    let dir = TempDir::new().expect("temp dir");
    let error = config::load_with_env(Some(dir.path()), no_env).expect_err("directory fails");
    let text = error.to_string();
    assert!(text.contains(&dir.path().display().to_string()), "{text}");
    assert!(text.contains("cannot read configuration file"), "{text}");
}

#[cfg(not(windows))]
#[test]
fn per_user_config_is_used_when_present() {
    let dir = TempDir::new().expect("temp dir");
    let xdg = dir.path().join("xdg");
    let pumice_dir = xdg.join("pumice");
    fs::create_dir_all(&pumice_dir).expect("create config dir");
    let file = pumice_dir.join(CONFIG_NAME);
    fs::write(&file, relay_config("port: 9123\n")).expect("write config");

    let env = |key: &str| (key == "XDG_CONFIG_HOME").then(|| xdg.clone().into_os_string());
    let loaded = config::load_with_env(None, env).expect("per-user config loads");
    assert_eq!(loaded.path, file);
    assert_eq!(loaded.config.port, 9123);
    let optional = config::load_optional_with_env(None, env).expect("per-user config loads");
    assert_eq!(optional.map(|loaded| loaded.path), Some(file.clone()));

    // A missing per-user file is an error naming it, never built-in
    // defaults; only the optional loader (doctor) runs without it.
    fs::remove_file(&file).expect("remove config");
    let error = config::load_with_env(None, env)
        .expect_err("no file")
        .to_string();
    assert!(error.contains(&file.display().to_string()), "{error}");
    assert!(
        error.contains("the relay configuration file does not exist; pass --config <path>"),
        "{error}"
    );
    assert_eq!(
        config::load_optional_with_env(None, env).expect("no file"),
        None
    );
}

#[cfg(not(windows))]
#[test]
fn per_user_config_falls_back_to_home_config() {
    let dir = TempDir::new().expect("temp dir");
    let home = dir.path().join("home");
    let pumice_dir = home.join(".config").join("pumice");
    fs::create_dir_all(&pumice_dir).expect("create config dir");
    fs::write(pumice_dir.join(CONFIG_NAME), relay_config("port: 9124\n")).expect("write config");

    let env = |key: &str| (key == "HOME").then(|| home.clone().into_os_string());
    let loaded = config::load_with_env(None, env).expect("home config loads");
    assert_eq!(loaded.path, pumice_dir.join(CONFIG_NAME));
    assert_eq!(loaded.config.port, 9124);
}

#[cfg(windows)]
#[test]
fn per_user_config_uses_appdata_on_windows() {
    let dir = TempDir::new().expect("temp dir");
    let appdata = dir.path().join("appdata");
    let pumice_dir = appdata.join("pumice");
    fs::create_dir_all(&pumice_dir).expect("create config dir");
    let file = pumice_dir.join(CONFIG_NAME);
    fs::write(&file, relay_config("port: 9125\n")).expect("write config");

    let env = |key: &str| (key == "APPDATA").then(|| appdata.clone().into_os_string());
    let loaded = config::load_with_env(None, env).expect("per-user config loads");
    assert_eq!(loaded.path, file);
    assert_eq!(loaded.config.port, 9125);
}

#[test]
fn no_configuration_directory_is_an_error_but_optional_for_doctor() {
    let error = config::load_with_env(None, no_env)
        .expect_err("nowhere to look")
        .to_string();
    assert!(
        error.contains("no per-user configuration directory found"),
        "{error}"
    );
    assert!(error.contains("--config <path>"), "{error}");
    assert_eq!(
        config::load_optional_with_env(None, no_env).expect("optional"),
        None
    );

    // An explicit file must exist even for the optional loader.
    let dir = TempDir::new().expect("temp dir");
    let missing = dir.path().join("nope.yml");
    let error = config::load_optional_with_env(Some(&missing), no_env)
        .expect_err("explicit missing file")
        .to_string();
    assert!(error.contains("does not exist"), "{error}");
}

#[test]
fn registry_builds_enabled_providers_from_config() {
    let config = load_text(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: claude\n    timeout_secs: 30\n  - id: codex\n    enabled: true\n    model: gpt-6.1-sol\n    binary: codex\n    timeout_secs: 30\n  - id: kimi\n    enabled: false\n    model: null\n    binary: kimi\n    timeout_secs: 30\n",
    )
    .expect("config loads");
    let built = providers::build_from_config(&config, Arc::new(ProcessRunner::new()))
        .expect("providers build");
    let ids: Vec<&str> = built.iter().map(|provider| provider.id()).collect();
    assert_eq!(ids, ["claude", "codex"]);
}

/// Registry slice for the capability tests: a synthetic probe plus the real
/// Claude descriptor.
fn probe_registry() -> [ProviderDescriptor; 2] {
    [
        capability_probe::PROBE,
        *providers::descriptor("claude").expect("claude is registered"),
    ]
}

fn load_with_descriptors(text: &str, descriptors: &[ProviderDescriptor]) -> Result<Config, String> {
    config::validate_text_with_descriptors(text, Path::new(CONFIG_NAME), descriptors)
        .map_err(|error| error.to_string())
}

#[test]
fn unlisted_provider_is_absent_until_listed() {
    let descriptors = probe_registry();
    let config = load_with_descriptors(&relay_config(""), &descriptors).expect("empty list loads");
    assert!(
        config.providers.is_empty(),
        "nothing is configured without entries"
    );

    let config = load_with_descriptors(
        &relay_config(
            "providers:\n  - id: probe\n    enabled: false\n    model: null\n    binary: probe-cli\n    timeout_secs: 30\n",
        ),
        &descriptors,
    )
    .expect("disabled entry loads");
    let probe = config.provider("probe").expect("probe is configured");
    assert!(!probe.enabled);
    assert!(probe.model.is_empty());

    let config = load_with_descriptors(
        &relay_config(
            "providers:\n  - id: probe\n    enabled: true\n    model: test/model\n    binary: probe-cli\n    timeout_secs: 30\n",
        ),
        &descriptors,
    )
    .expect("explicit enablement loads");
    let probe = config.provider("probe").expect("probe is configured");
    assert!(probe.enabled);
    assert_eq!(probe.model, "test/model");
}

#[test]
fn enabled_with_a_null_model_is_rejected_at_the_null() {
    let descriptors = probe_registry();
    let error = load_with_descriptors(
        &relay_config(
            "providers:\n  - id: probe\n    enabled: true\n    model: null\n    binary: probe-cli\n    timeout_secs: 30\n",
        ),
        &descriptors,
    )
    .expect_err("enabled without a model fails");
    assert!(
        error.contains(
            ":4:12: providers entry \"probe\" is enabled but has no model; add \"model: <name>\""
        ),
        "{error}"
    );
    assert!(
        error.contains(CONFIG_NAME),
        "error names the file:\n{error}"
    );
}

/// Runs the real `pumice` binary with `args`.
fn run_pumice(args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_pumice-relay"))
        .args(args)
        .output()
        .expect("pumice runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn check_config_success_prints_summary() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(CONFIG_NAME);
    fs::write(
        &path,
        relay_config(
            "port: 8000\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: claude\n    timeout_secs: 30\n  - id: kimi\n    enabled: false\n    model: null\n    binary: kimi\n    timeout_secs: 30\n",
        ),
    )
    .expect("write config");

    let output = run_pumice(&["check-config", "--config", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains(&format!("config: {}", path.display())),
        "{stdout}"
    );
    assert!(stdout.contains("port: 8000"), "{stdout}");
    assert!(stdout.contains("total timeout: 30s"), "{stdout}");
    assert!(stdout.contains("max parallel: 4"), "{stdout}");
    assert!(!stdout.contains("default"), "{stdout}");
    assert!(stdout.contains("providers (in order):"), "{stdout}");
    assert!(
        stdout.contains("  claude: enabled, model haiku, timeout 30s, binary claude"),
        "{stdout}"
    );
    assert!(stdout.contains("  kimi: disabled"), "{stdout}");
}

#[test]
fn check_config_rejects_the_removed_default_key_at_its_position() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(CONFIG_NAME);
    fs::write(
        &path,
        "port: 8000\ndefault: kimi\nproviders:\n  - id: kimi\n    enabled: false\n",
    )
    .expect("write config");

    let output = run_pumice(&["check-config", "--config", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2), "stdout: {}", stdout(&output));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("\"default\" was removed in 0.2:"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("{}:2:10", path.display())),
        "the error points at the key's line:column:\n{stderr}"
    );
}

#[test]
fn check_config_error_exits_2_with_the_exact_error() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(CONFIG_NAME);
    fs::write(
        &path,
        relay_config(
            "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    timeout_secs: 0\n    binary: claude\n",
        ),
    )
    .expect("write config");

    let output = run_pumice(&["check-config", "--config", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty());
    let stderr = stderr(&output);
    assert!(stderr.contains(":5:19:"), "{stderr}");
    assert!(
        stderr.contains("providers entry \"claude\".timeout_secs must be greater than zero"),
        "{stderr}"
    );
}

#[test]
fn check_config_missing_file_exits_2() {
    let dir = TempDir::new().expect("temp dir");
    let missing = dir.path().join("missing.yaml");
    let output = run_pumice(&["check-config", "--config", missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr(&output);
    assert!(stderr.contains("does not exist"), "{stderr}");
    assert!(stderr.contains(&missing.display().to_string()), "{stderr}");
}

#[test]
fn check_config_without_any_file_exits_2_naming_the_per_user_path() {
    let dir = TempDir::new().expect("temp dir");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pumice-relay"))
        .arg("check-config")
        .env("XDG_CONFIG_HOME", dir.path())
        .env("HOME", dir.path())
        .env("APPDATA", dir.path())
        .output()
        .expect("pumice runs");
    assert_eq!(output.status.code(), Some(2), "stdout: {}", stdout(&output));
    assert!(stdout(&output).is_empty());
    let stderr = stderr(&output);
    let expected = dir.path().join("pumice").join(CONFIG_NAME);
    assert!(stderr.contains(&expected.display().to_string()), "{stderr}");
    assert!(
        stderr.contains("the relay configuration file does not exist"),
        "{stderr}"
    );
}

#[test]
fn check_config_rejects_bad_arguments() {
    let output = run_pumice(&["check-config", "--bogus"]);
    assert_eq!(output.status.code(), Some(2));

    let output = run_pumice(&["check-config", "extra"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn serve_rejects_bad_arguments() {
    let output = run_pumice(&["serve", "--bogus"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("usage: pumice-relay serve"),
        "{}",
        stderr(&output)
    );

    let output = run_pumice(&["serve", "extra"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn serve_reports_config_errors_like_check_config() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(CONFIG_NAME);
    fs::write(
        &path,
        relay_config(
            "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    timeout_secs: 0\n    binary: claude\n",
        ),
    )
    .expect("write config");

    let output = run_pumice(&["serve", "--config", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("providers entry \"claude\".timeout_secs must be greater than zero"),
        "{stderr}"
    );
}

/// Path to the shipped `relay.example.yml` at the repository root.
fn example_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("manifest dir has a parent")
        .parent()
        .expect("manifest dir has a grandparent")
        .join("relay.example.yml")
}

#[test]
fn example_config_loads() {
    let example = example_path();
    let loaded = config::load_with_env(Some(&example), no_env).expect("example loads");
    assert_eq!(loaded.path, example);

    // The values the relay used as built-in defaults before every setting
    // had to be written.
    let config = &loaded.config;
    assert_eq!(config.port, 7567);
    assert_eq!(config.total_timeout, Duration::from_secs(30));
    assert_eq!(config.max_parallel, 4);
    // The example enables Claude.
    assert!(!config.debug_log.enabled);
    // Relative paths anchor at the configuration file's directory.
    assert_eq!(
        config.debug_log.path,
        example
            .parent()
            .expect("example lives in a directory")
            .join("pumice-debug.jsonl")
    );

    let ids: Vec<&str> = config
        .providers
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(ids, ["claude", "codex", "kimi", "kiro"]);

    let claude = config.provider("claude").expect("claude is listed");
    assert!(claude.enabled);
    assert_eq!(claude.model, "haiku");
    assert_eq!(claude.timeout, Duration::from_secs(30));
    assert_eq!(claude.binary, Some(PathBuf::from("claude")));

    // Every other entry ships disabled, with no model, its standard command
    // and the same timeout.
    for (id, binary) in [("codex", "codex"), ("kimi", "kimi"), ("kiro", "kiro-cli")] {
        let settings = config
            .provider(id)
            .unwrap_or_else(|| panic!("{id} is listed"));
        assert!(!settings.enabled, "{id} stays off in the example");
        assert!(settings.model.is_empty(), "{id} ships without a model");
        assert_eq!(settings.binary, Some(PathBuf::from(binary)), "{id}");
        assert_eq!(settings.timeout, Duration::from_secs(30), "{id}");
    }
}

#[test]
fn relative_config_path_still_anchors_relative_settings() {
    // A relative --config path still anchors relative settings: the config
    // directory is absolutized before paths are joined. The temp dir sits
    // in the current directory so the path to it can stay relative, and no
    // test mutates the process working directory.
    let dir = tempfile::Builder::new()
        .prefix("pumice-configtest-")
        .tempdir_in(".")
        .expect("temp dir in working directory");
    let relative_config = dir.path().join(CONFIG_NAME);
    fs::write(
        &relative_config,
        relay_config(
            "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: tools/claude\n    timeout_secs: 30\n",
        ),
    )
    .expect("write config");

    let loaded = config::load_with_env(Some(&relative_config), no_env).expect("config loads");
    let resolved = claude(&loaded.config)
        .binary
        .as_ref()
        .expect("binary is set");
    assert!(resolved.is_absolute(), "{resolved:?}");
    assert_eq!(
        resolved,
        std::env::current_dir()
            .expect("current dir")
            .join(dir.path())
            .join("tools")
            .join("claude")
            .as_path()
    );
}

#[test]
fn removed_keys_set_to_null_are_still_rejected() {
    for (key, message) in [
        ("default", "\"default\" was removed in 0.2"),
        (
            "default_provider",
            "\"default_provider\" was removed in 0.2",
        ),
        ("fallback_order", "\"fallback_order\" was removed in 0.2"),
    ] {
        for value in ["null", "~"] {
            let error = load_error(&format!("port: 7567\n{key}: {value}\n"));
            assert!(error.contains(":2:"), "points at line 2:\n{error}");
            assert!(error.contains(message), "{key}: {value}:\n{error}");
        }
    }
}

#[test]
fn empty_binary_is_rejected_at_the_value() {
    let error = load_error(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: ''\n    timeout_secs: 30\n",
    );
    assert!(error.contains(":5:"), "{error}");
    assert!(
        error.contains(
            "providers entry \"claude\".binary must not be empty; write the command, e.g. \"binary: claude\""
        ),
        "{error}"
    );
}

#[test]
fn enabled_with_a_null_model_is_rejected_at_the_null_for_every_provider() {
    // Kimi checks this itself, with its own message (tests/kimi.rs).
    for (id, _) in PROVIDERS.into_iter().filter(|(id, _)| *id != "kimi") {
        assert_error(
            &format!(
                "providers:\n  - id: {id}\n    enabled: true\n    model: null\n    binary: {id}\n    timeout_secs: 30\n"
            ),
            4,
            12,
            &format!("providers entry \"{id}\" is enabled but has no model; add \"model: <name>\""),
        );
    }
}

#[test]
fn model_starting_with_a_dash_is_rejected_at_the_value() {
    for (id, _) in PROVIDERS {
        for model in ["-haiku", "-x"] {
            assert_error(
                &format!(
                    "providers:\n  - id: {id}\n    enabled: true\n    model: {model}\n    binary: {id}\n"
                ),
                4,
                12,
                &format!("providers entry \"{id}\".model must not start with '-'"),
            );
        }
    }
}

#[test]
fn env_and_options_are_rejected_as_removed() {
    for (id, model) in PROVIDERS {
        for key in ["env", "options"] {
            // A block mapping is reported at its first key on the next line.
            assert_error(
                &format!(
                    "providers:\n  - id: {id}\n    enabled: true\n    model: {model}\n    {key}:\n      A: b\n"
                ),
                6,
                7,
                &format!("providers entry \"{id}\".{key} was removed"),
            );
            for value in [" null\n", " {}\n"] {
                let error = load_error(&format!(
                    "providers:\n  - id: {id}\n    enabled: true\n    model: {model}\n    {key}:{value}"
                ));
                assert!(
                    error.contains(&format!("providers entry \"{id}\".{key} was removed")),
                    "{id} {key}:{value:?}\n{error}"
                );
            }
        }
    }
}

#[test]
fn removed_opencode_entry_says_so() {
    let error = load_error("providers:\n  - id: opencode\n    enabled: false\n");
    assert!(error.contains(":2:"), "{error}");
    assert!(
        error.contains("\"opencode\" support was removed; delete this entry"),
        "{error}"
    );
}

#[test]
fn prompts_section_is_rejected_as_removed() {
    // The error points at the value: the next line for a block mapping.
    for (value, line) in [
        ("\n  system: Keep technical terms.\n", 3),
        (" null\n", 2),
        (" Keep technical terms.\n", 2),
    ] {
        let error = load_error(&format!("port: 7567\nprompts:{value}"));
        assert!(error.contains(&format!(":{line}:")), "{error}");
        assert!(error.contains("\"prompts\" was removed"), "{error}");
    }
}
