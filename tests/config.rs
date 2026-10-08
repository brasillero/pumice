//! Tests for YAML configuration loading, validation and error positions.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use pumice::config::{self, Config, ConfigSource, DEFAULT_PORT};
use pumice::process::ProcessRunner;
use pumice::providers::{self, ProviderDescriptor, ProviderSettings};
use tempfile::TempDir;

const CONFIG_NAME: &str = "pumice.yaml";

/// Writes `text` to a temporary `pumice.yaml` and loads it.
fn load_text(text: &str) -> Result<Config, String> {
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let path = dir.path().join(CONFIG_NAME);
    fs::write(&path, text).map_err(|e| e.to_string())?;
    config::load_with_env(Some(&path), |_| None)
        .map(|loaded| loaded.config)
        .map_err(|error| error.to_string())
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

fn no_env(_: &str) -> Option<OsString> {
    None
}

/// Synthetic provider descriptor exercising registry capabilities (explicit
/// enablement, risk warnings) against `config::validate_text_with_descriptors`,
/// leaving the built-in registry untouched.
mod capability_probe {
    use std::sync::Arc;
    use std::time::Duration;

    use pumice::config::ConfigError;
    use pumice::process::ProcessRunner;
    use pumice::providers::{
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
fn empty_file_gives_all_defaults() {
    let config = load_text("").expect("empty file loads");
    assert_eq!(config.port, DEFAULT_PORT);
    assert_eq!(config.port, 7567);
    assert_eq!(config.total_timeout, Duration::from_secs(30));
    assert!(!config.debug_log.enabled);

    // Since 0.2 an empty file configures nothing: no providers — every
    // request returns the original text.
    assert!(config.providers.is_empty());
}

#[test]
fn missing_default_config_gives_all_defaults() {
    let loaded = config::load_with_env(None, no_env).expect("defaults load");
    assert_eq!(loaded.source, ConfigSource::BuiltInDefaults);
    assert_eq!(loaded.config.port, 7567);
    // With no configuration file there is no directory to anchor relative
    // paths, so the default debug log path stays relative.
    assert_eq!(
        loaded.config.debug_log.path,
        PathBuf::from("pumice-debug.jsonl")
    );

    // An empty file yields the same settings everywhere else.
    let from_empty = load_text("").expect("empty file loads");
    assert_eq!(loaded.config.port, from_empty.port);
    assert_eq!(loaded.config.providers, from_empty.providers);
    assert_eq!(
        loaded.config.debug_log.enabled,
        from_empty.debug_log.enabled
    );
}

#[test]
fn comment_only_file_gives_all_defaults() {
    let config = load_text("# nothing here\n# at all\n").expect("comment file loads");
    assert_eq!(config.port, 7567);
}

#[test]
fn partial_nested_override_keeps_sibling_defaults() {
    let config = load_text(
        "debug_log:\n  enabled: true\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    timeout_secs: 5\n",
    )
    .expect("partial override loads");
    assert_eq!(config.port, 7567);
    assert!(config.debug_log.enabled);
    // The sibling `path` keeps its default.
    assert!(config.debug_log.path.ends_with("pumice-debug.jsonl"));

    let claude = claude(&config);
    assert_eq!(claude.timeout, Duration::from_secs(5));
    assert_eq!(claude.model, "haiku");
    assert!(claude.enabled);
}

#[test]
fn null_sections_behave_like_absent_ones() {
    let config = load_text("providers: null\ndebug_log: null\n").expect("null sections load");
    assert_eq!(config.port, 7567);
    assert!(config.providers.is_empty());
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
        "port: 9001\r\nproviders:\r\n  - id: claude\r\n    enabled: true\r\n    model: olá\r\n",
    )
    .expect("CRLF loads");
    assert_eq!(config.port, 9001);
    assert_eq!(claude(&config).model, "olá");

    // Line numbers count CRLF files exactly like LF files.
    assert_error(
        "port: 9001\r\nproviders:\r\n  - id: claude\r\n    enabled: true\r\n    model: haiku\r\n    timeout_secs: 0\r\n",
        6,
        19,
        "providers entry \"claude\".timeout_secs must be greater than zero",
    );
}

#[test]
fn unicode_content_round_trips() {
    let config = load_text(
        "# configuração\nproviders:\n  - id: claude\n    enabled: true\n    model: ünïcodé-ãõ\n",
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
        "# seven comment lines place the value on line 12\n# 2\n# 3\n# 4\n# 5\n# 6\n# 7\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    timeout_secs: 0\n",
        12,
        19,
        "providers entry \"claude\".timeout_secs must be greater than zero",
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
fn duplicate_provider_entries_are_rejected_at_the_second_id() {
    assert_error(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n  - id: claude\n    enabled: false\n",
        5,
        9,
        "duplicate provider \"claude\" in providers",
    );
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
        "providers:\n  - id: claude\n    enabled: true\n    model: \"\"\n",
        4,
        12,
        "providers entry \"claude\".model must not be empty",
    );
    assert_error(
        "providers:\n  - id: claude\n    enabled: true\n    model: \"   \"\n",
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
    let config = load_text("providers:\n  - id: claude\n    enabled: false\n    model: \"\"\n")
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
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: tools/claude\n",
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
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: myclaude\n",
    )
    .expect("bare name loads");
    assert_eq!(claude(&config).binary, Some(PathBuf::from("myclaude")));

    // An absolute path on every OS (`/opt/claude` is relative on Windows).
    let absolute = std::env::temp_dir().join("claude");
    let yaml = format!(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{}'\n",
        absolute.display()
    );
    let config = load_text(&yaml).expect("absolute loads");
    assert_eq!(claude(&config).binary, Some(absolute));

    let config = load_text(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: null\n",
    )
    .expect("null loads");
    assert_eq!(claude(&config).binary, None);
}

#[test]
fn relative_debug_log_path_resolves_against_the_config_directory() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(CONFIG_NAME);
    fs::write(
        &path,
        "debug_log:\n  enabled: true\n  path: logs/debug.jsonl\n",
    )
    .expect("write");

    let loaded = config::load_with_env(Some(&path), no_env).expect("config loads");
    assert!(loaded.config.debug_log.enabled);
    assert_eq!(
        loaded.config.debug_log.path,
        dir.path().join("logs").join("debug.jsonl")
    );

    // The default file name also lands beside the configuration file.
    fs::write(&path, "debug_log:\n  enabled: true\n").expect("write config");
    let loaded = config::load_with_env(Some(&path), no_env).expect("config loads");
    assert_eq!(
        loaded.config.debug_log.path,
        dir.path().join("pumice-debug.jsonl")
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
    fs::write(&file, "port: 9123\n").expect("write config");

    let env = |key: &str| (key == "XDG_CONFIG_HOME").then(|| xdg.clone().into_os_string());
    let loaded = config::load_with_env(None, env).expect("per-user config loads");
    assert_eq!(loaded.source, ConfigSource::File(file.clone()));
    assert_eq!(loaded.config.port, 9123);

    // A missing per-user file falls back to built-in defaults.
    fs::remove_file(&file).expect("remove config");
    let loaded = config::load_with_env(None, env).expect("defaults load");
    assert_eq!(loaded.source, ConfigSource::BuiltInDefaults);
}

#[cfg(not(windows))]
#[test]
fn per_user_config_falls_back_to_home_config() {
    let dir = TempDir::new().expect("temp dir");
    let home = dir.path().join("home");
    let pumice_dir = home.join(".config").join("pumice");
    fs::create_dir_all(&pumice_dir).expect("create config dir");
    fs::write(pumice_dir.join(CONFIG_NAME), "port: 9124\n").expect("write config");

    let env = |key: &str| (key == "HOME").then(|| home.clone().into_os_string());
    let loaded = config::load_with_env(None, env).expect("home config loads");
    assert_eq!(
        loaded.source,
        ConfigSource::File(pumice_dir.join(CONFIG_NAME))
    );
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
    fs::write(&file, "port: 9125\n").expect("write config");

    let env = |key: &str| (key == "APPDATA").then(|| appdata.clone().into_os_string());
    let loaded = config::load_with_env(None, env).expect("per-user config loads");
    assert_eq!(loaded.source, ConfigSource::File(file));
    assert_eq!(loaded.config.port, 9125);
}

#[test]
fn registry_builds_enabled_providers_from_config() {
    let config = load_text(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n  - id: codex\n    enabled: true\n    model: gpt-6.1-sol\n  - id: kimi\n    enabled: false\n",
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
    let config = load_with_descriptors("", &descriptors).expect("defaults load");
    assert!(
        config.providers.is_empty(),
        "nothing is configured without entries"
    );

    let config = load_with_descriptors(
        "providers:\n  - id: probe\n    enabled: false\n",
        &descriptors,
    )
    .expect("disabled entry loads");
    let probe = config.provider("probe").expect("probe is configured");
    assert!(!probe.enabled);
    assert!(probe.model.is_empty());

    let config = load_with_descriptors(
        "providers:\n  - id: probe\n    enabled: true\n    model: test/model\n",
        &descriptors,
    )
    .expect("explicit enablement loads");
    let probe = config.provider("probe").expect("probe is configured");
    assert!(probe.enabled);
    assert_eq!(probe.model, "test/model");
}

#[test]
fn enabled_without_a_model_is_rejected_at_the_id() {
    let descriptors = probe_registry();
    let error = load_with_descriptors(
        "providers:\n  - id: probe\n    enabled: true\n",
        &descriptors,
    )
    .expect_err("enabled without a model fails");
    assert!(
        error.contains(
            ":2:9: providers entry \"probe\" is enabled but has no model; add \"model: <name>\""
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
    std::process::Command::new(env!("CARGO_BIN_EXE_pumice"))
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
        "port: 8000\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n  - id: kimi\n    enabled: false\n",
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
    assert!(!stdout.contains("default"), "{stdout}");
    assert!(stdout.contains("providers (in order):"), "{stdout}");
    assert!(
        stdout.contains("  claude: enabled, model haiku, timeout 30s"),
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
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    timeout_secs: 0\n",
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
        stderr(&output).contains("usage: pumice serve"),
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
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    timeout_secs: 0\n",
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

/// Path to the shipped `pumice.example.yaml` at the repository root.
fn example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("pumice.example.yaml")
}

#[test]
fn example_config_loads() {
    let example = example_path();
    let loaded = config::load_with_env(Some(&example), no_env).expect("example loads");
    assert_eq!(loaded.source, ConfigSource::File(example.clone()));

    let config = &loaded.config;
    assert_eq!(config.port, 7567);
    assert_eq!(config.total_timeout, Duration::from_secs(30));
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
    assert_eq!(ids, ["claude", "codex", "kimi"]);

    let claude = config.provider("claude").expect("claude is listed");
    assert!(claude.enabled);
    assert_eq!(claude.model, "haiku");
    assert_eq!(claude.timeout, Duration::from_secs(30));
    assert!(claude.binary.is_none());

    // Every other entry ships disabled, with no model.
    for id in ["codex", "kimi"] {
        let settings = config
            .provider(id)
            .unwrap_or_else(|| panic!("{id} is listed"));
        assert!(!settings.enabled, "{id} stays off in the example");
        assert!(settings.model.is_empty(), "{id} ships without a model");
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
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: tools/claude\n",
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
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: ''\n",
    );
    assert!(error.contains(":5:"), "{error}");
    assert!(
        error.contains("providers entry \"claude\".binary must not be empty"),
        "{error}"
    );
}

#[test]
fn model_starting_with_a_dash_is_rejected_at_the_value() {
    for provider in ["claude", "codex", "kimi"] {
        let error = load_error(&format!(
            "providers:\n  - id: {provider}\n    enabled: true\n    model: -haiku\n"
        ));
        assert!(error.contains(":4:"), "{error}");
        assert!(
            error.contains(&format!(
                "providers entry \"{provider}\".model must not start with '-'"
            )),
            "{error}"
        );
    }
}

#[test]
fn env_and_options_are_rejected_as_removed() {
    for key in ["env", "options"] {
        for value in ["\n      A: b\n", " null\n", " {}\n"] {
            let error = load_error(&format!(
                "providers:\n  - id: codex\n    enabled: true\n    model: gpt-6.1-sol\n    {key}:{value}"
            ));
            assert!(
                error.contains(&format!("providers entry \"codex\".{key} was removed")),
                "{key}:{value:?}\n{error}"
            );
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
