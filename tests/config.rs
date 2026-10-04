//! Tests for YAML configuration loading, validation and error positions.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::sync::atomic::Ordering;
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
    config
        .providers
        .get("claude")
        .expect("claude is configured")
}

fn codex(config: &Config) -> &ProviderSettings {
    config.providers.get("codex").expect("codex is configured")
}

fn no_env(_: &str) -> Option<OsString> {
    None
}

/// Synthetic provider descriptors exercising registry capabilities
/// (default-off behavior, risk warnings, full-settings validation) against
/// `config::validate_text_with_descriptors`, leaving the built-in registry
/// untouched.
mod capability_probe {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use pumice::config::ConfigError;
    use pumice::process::ProcessRunner;
    use pumice::providers::{
        Provider, ProviderDescriptor, ProviderLocations, ProviderSettings, RawOption,
    };
    use serde_saphyr::Location;

    /// How often `COUNTING.validate_settings` ran. Dedicated to one test so
    /// parallel tests never observe each other's counts.
    pub static COUNT_VALIDATIONS: AtomicUsize = AtomicUsize::new(0);

    /// The OpenCode shape: off by default, and `enabled` without an explicit
    /// `model` is rejected at the `enabled` line.
    pub const PROBE: ProviderDescriptor = ProviderDescriptor {
        id: "probe",
        defaults: probe_defaults,
        allowed_env: &[],
        validate_options: probe_validate_options,
        build: probe_build,
        disabled_by_default: true,
        risk_warning: Some("the probe formats nothing and is not real"),
        validate_settings: probe_validate_settings,
    };

    /// The same model requirement without the opt-in: even an absent entry
    /// is validated, and with no `enabled` line the error has no position.
    pub const BARE: ProviderDescriptor = ProviderDescriptor {
        id: "bare",
        defaults: probe_defaults,
        allowed_env: &[],
        validate_options: probe_validate_options,
        build: probe_build,
        disabled_by_default: false,
        risk_warning: None,
        validate_settings: bare_validate_settings,
    };

    /// Validates quietly when left disabled; exists only to be counted.
    pub const COUNTING: ProviderDescriptor = ProviderDescriptor {
        id: "counting",
        defaults: probe_defaults,
        allowed_env: &[],
        validate_options: probe_validate_options,
        build: probe_build,
        disabled_by_default: true,
        risk_warning: None,
        validate_settings: counting_validate_settings,
    };

    fn probe_defaults() -> ProviderSettings {
        ProviderSettings {
            enabled: true, // the loader replaces this per `disabled_by_default`
            binary: None,
            model: String::new(),
            timeout: Duration::from_secs(30),
            env: BTreeMap::new(),
            options: BTreeMap::new(),
        }
    }

    fn probe_validate_options(options: &[RawOption<'_>]) -> Result<(), ConfigError> {
        if let Some(option) = options.first() {
            return Err(ConfigError::at(
                option.key_at,
                format!(
                    "option {} is not supported by the test provider",
                    option.key
                ),
            ));
        }
        Ok(())
    }

    fn probe_build(
        _settings: &ProviderSettings,
        _runner: Arc<ProcessRunner>,
    ) -> Result<Arc<dyn Provider>, ConfigError> {
        panic!("the probe descriptors are never built")
    }

    fn probe_validate_settings(
        settings: &ProviderSettings,
        locations: &ProviderLocations,
    ) -> Result<(), ConfigError> {
        model_required_when_enabled("probe", settings, locations)
    }

    fn bare_validate_settings(
        settings: &ProviderSettings,
        locations: &ProviderLocations,
    ) -> Result<(), ConfigError> {
        model_required_when_enabled("bare", settings, locations)
    }

    fn counting_validate_settings(
        settings: &ProviderSettings,
        locations: &ProviderLocations,
    ) -> Result<(), ConfigError> {
        COUNT_VALIDATIONS.fetch_add(1, Ordering::SeqCst);
        model_required_when_enabled("counting", settings, locations)
    }

    fn model_required_when_enabled(
        id: &str,
        settings: &ProviderSettings,
        locations: &ProviderLocations,
    ) -> Result<(), ConfigError> {
        if settings.enabled && settings.model.is_empty() {
            return Err(ConfigError::at(
                locations.enabled.unwrap_or(Location::UNKNOWN),
                format!("providers.{id}.model is required when the provider is enabled"),
            ));
        }
        Ok(())
    }
}

#[test]
fn empty_file_gives_all_defaults() {
    let config = load_text("").expect("empty file loads");
    assert_eq!(config.port, DEFAULT_PORT);
    assert_eq!(config.port, 7567);
    assert_eq!(config.default_provider, "claude");
    assert_eq!(config.total_timeout, Duration::from_secs(30));
    assert!(config.fallback_order.is_empty());
    assert_eq!(config.prompts.system, None);
    assert_eq!(config.prompts.user, None);
    assert!(!config.debug_log.enabled);
    assert_eq!(config.providers.len(), 4);

    let claude = claude(&config);
    assert!(claude.enabled);
    assert_eq!(claude.binary, None);
    assert_eq!(claude.model, "haiku");
    assert_eq!(claude.timeout, Duration::from_secs(30));
    assert!(claude.env.is_empty());
    assert!(claude.options.is_empty());

    let codex = codex(&config);
    assert!(codex.enabled);
    assert_eq!(codex.binary, None);
    assert_eq!(codex.model, "gpt-6.1-sol");
    assert_eq!(codex.timeout, Duration::from_secs(30));
    assert!(codex.env.is_empty());
    assert!(codex.options.is_empty());

    // Antigravity is registered but dormant: off by default, with no model.
    let antigravity = config
        .providers
        .get("antigravity")
        .expect("antigravity is configured");
    assert!(!antigravity.enabled);
    assert!(antigravity.model.is_empty());
    assert_eq!(antigravity.timeout, Duration::from_secs(30));
    assert!(antigravity.env.is_empty());
    assert!(antigravity.options.is_empty());

    // OpenCode is registered but dormant: off by default, with no model.
    let opencode = config
        .providers
        .get("opencode")
        .expect("opencode is configured");
    assert!(!opencode.enabled);
    assert!(opencode.model.is_empty());
    assert_eq!(opencode.timeout, Duration::from_secs(30));
    assert!(opencode.env.is_empty());
    assert!(opencode.options.is_empty());
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
    assert_eq!(loaded.config.prompts, from_empty.prompts);
    assert_eq!(loaded.config.fallback_order, from_empty.fallback_order);
}

#[test]
fn comment_only_file_gives_all_defaults() {
    let config = load_text("# nothing here\n# at all\n").expect("comment file loads");
    assert_eq!(config.port, 7567);
}

#[test]
fn partial_nested_override_keeps_sibling_defaults() {
    let config = load_text(
        "prompts:\n  system: Keep technical terms.\nproviders:\n  claude:\n    timeout_secs: 5\n",
    )
    .expect("partial override loads");
    assert_eq!(config.port, 7567);
    assert_eq!(
        config.prompts.system.as_deref(),
        Some("Keep technical terms.")
    );
    assert_eq!(config.prompts.user, None);

    let claude = claude(&config);
    assert_eq!(claude.timeout, Duration::from_secs(5));
    assert_eq!(claude.model, "haiku");
    assert!(claude.enabled);
}

#[test]
fn null_sections_behave_like_absent_ones() {
    let config =
        load_text("providers: null\nprompts: null\ndebug_log: null\n").expect("null sections load");
    assert_eq!(config.port, 7567);
    assert!(claude(&config).enabled);

    let config = load_text("providers:\n  claude:\n    env: null\n    options: null\n")
        .expect("null provider sections load");
    let claude = claude(&config);
    assert!(claude.env.is_empty());
    assert!(claude.options.is_empty());
}

#[test]
fn empty_mappings_behave_like_absent_ones() {
    let config = load_text("providers: {}\nfallback_order: []\n").expect("empty maps load");
    assert_eq!(config.port, 7567);
    assert!(claude(&config).enabled);
    assert!(config.fallback_order.is_empty());
}

#[test]
fn block_scalar_prompts_are_read_exactly() {
    let config = load_text(
        "prompts:\n  system: |\n    First line.\n    Second line.\n  user: >\n    folded\n    text\n",
    )
    .expect("block prompts load");
    assert_eq!(
        config.prompts.system.as_deref(),
        Some("First line.\nSecond line.\n")
    );
    assert_eq!(config.prompts.user.as_deref(), Some("folded text\n"));
}

#[test]
fn crlf_line_endings_parse_and_positions_count_lines() {
    let config =
        load_text("port: 9001\r\nprompts:\r\n  system: |\r\n    olá\r\n").expect("CRLF loads");
    assert_eq!(config.port, 9001);
    assert_eq!(config.prompts.system.as_deref(), Some("olá\n"));

    // Line numbers count CRLF files exactly like LF files.
    assert_error(
        "port: 9001\r\nproviders:\r\n  claude:\r\n    timeout_secs: 0\r\n",
        4,
        19,
        "providers.claude.timeout_secs must be greater than zero",
    );
}

#[test]
fn unicode_content_round_trips() {
    let config = load_text("# configuração\nprompts:\n  system: |\n    ünïcodé e acentos: ãõ\n")
        .expect("unicode loads");
    assert_eq!(
        config.prompts.system.as_deref(),
        Some("ünïcodé e acentos: ãõ\n")
    );
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
        "# nine comment lines place the value on line 12\n# 2\n# 3\n# 4\n# 5\n# 6\n# 7\n# 8\n# 9\nproviders:\n  claude:\n    timeout_secs: 0\n",
        12,
        19,
        "providers.claude.timeout_secs must be greater than zero",
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
        "providers:\n  claude:\n    unknown_key: 1\n",
        3,
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
        "providers:\n  claude:\n    timeout_secs: 1\n    timeout_secs: 2\n",
        4,
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
fn unknown_provider_is_rejected_at_its_key() {
    assert_error(
        "providers:\n  bogus:\n    enabled: true\n",
        2,
        3,
        "providers.bogus is not a known provider",
    );
}

#[test]
fn unknown_default_provider_is_rejected_at_the_value() {
    assert_error(
        "default_provider: gemini\n",
        1,
        19,
        "default_provider \"gemini\" is not a known provider",
    );
}

#[test]
fn disabled_default_provider_is_rejected() {
    // Explicit default provider: the error points at its value.
    assert_error(
        "default_provider: claude\nproviders:\n  claude:\n    enabled: false\n",
        1,
        19,
        "default_provider \"claude\" is disabled",
    );
    // Implicit default provider: the error points at the `enabled` flag.
    assert_error(
        "providers:\n  claude:\n    enabled: false\n",
        3,
        14,
        "the default provider \"claude\" is disabled",
    );
}

#[test]
fn empty_model_is_rejected_at_the_value() {
    assert_error(
        "providers:\n  claude:\n    model: \"\"\n",
        3,
        12,
        "providers.claude.model must not be empty",
    );
    assert_error(
        "providers:\n  claude:\n    model: \"   \"\n",
        3,
        12,
        "providers.claude.model must not be empty",
    );
}

#[test]
fn unknown_fallback_entry_is_rejected_at_the_entry() {
    assert_error(
        "fallback_order:\n  - gemini\n",
        2,
        5,
        "unknown provider \"gemini\" in fallback_order",
    );
}

#[test]
fn duplicate_fallback_entries_are_rejected_at_the_second_entry() {
    assert_error(
        "fallback_order:\n  - claude\n  - claude\n",
        3,
        5,
        "duplicate provider \"claude\" in fallback_order",
    );
}

#[test]
fn disallowed_env_override_is_rejected_at_the_key() {
    assert_error(
        "providers:\n  claude:\n    env:\n      FOO: bar\n",
        4,
        7,
        "providers.claude.env.FOO is not an allowed environment variable (allowed: ANTHROPIC_BASE_URL)",
    );
}

#[test]
fn credential_looking_env_names_are_rejected_at_the_key() {
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        let text = format!("providers:\n  claude:\n    env:\n      {name}: value\n");
        assert_error(
            &text,
            4,
            7,
            &format!(
                "providers.claude.env.{name} is not allowed: credential-like variable names are rejected"
            ),
        );
    }
}

#[test]
fn allowed_env_override_is_kept() {
    let config = load_text(
        "providers:\n  claude:\n    env:\n      ANTHROPIC_BASE_URL: \"http://127.0.0.1:9999\"\n",
    )
    .expect("allowed env override loads");
    assert_eq!(
        claude(&config)
            .env
            .get("ANTHROPIC_BASE_URL")
            .map(String::as_str),
        Some("http://127.0.0.1:9999")
    );
}

#[test]
fn claude_options_are_rejected_at_the_key() {
    assert_error(
        "providers:\n  claude:\n    options:\n      foo: bar\n",
        4,
        7,
        "providers.claude.options.foo is not supported",
    );
}

#[test]
fn codex_openai_base_url_option_is_kept() {
    let config = load_text(
        "providers:\n  codex:\n    options:\n      openai_base_url: \"https://gw.example/v1\"\n",
    )
    .expect("allowed option loads");
    assert_eq!(
        codex(&config)
            .options
            .get("openai_base_url")
            .map(String::as_str),
        Some("https://gw.example/v1")
    );

    let config = load_text(
        "providers:\n  codex:\n    options:\n      openai_base_url: \"http://127.0.0.1:9999\"\n",
    )
    .expect("http option loads");
    assert_eq!(
        codex(&config)
            .options
            .get("openai_base_url")
            .map(String::as_str),
        Some("http://127.0.0.1:9999")
    );
}

#[test]
fn codex_rejects_invalid_openai_base_url_at_the_value() {
    assert_error(
        "providers:\n  codex:\n    options:\n      openai_base_url: \"ftp://gw.example/v1\"\n",
        4,
        24,
        "providers.codex.options.openai_base_url must be an http:// or https:// URL with a valid host",
    );
    assert_error(
        "providers:\n  codex:\n    options:\n      openai_base_url: \"http://exa mple\"\n",
        4,
        24,
        "providers.codex.options.openai_base_url must be an http:// or https:// URL with a valid host",
    );
}

#[test]
fn codex_rejects_hostless_or_malformed_base_urls() {
    for bad in [
        "https://?",
        "http:///",
        "https://[invalid",
        "http://host:99999",
        "https://user@host",
        "https://a..b",
    ] {
        assert_error(
            &format!("providers:\n  codex:\n    options:\n      openai_base_url: \"{bad}\"\n"),
            4,
            24,
            "providers.codex.options.openai_base_url must be an http:// or https:// URL with a valid host",
        );
    }
}

#[test]
fn codex_accepts_realistic_base_urls() {
    for good in [
        "http://localhost:8317/v1",
        "https://gw.example.ts.net/v1",
        "http://[::1]:8080",
        "https://10.0.0.2",
    ] {
        let config = load_text(&format!(
            "providers:\n  codex:\n    options:\n      openai_base_url: \"{good}\"\n"
        ))
        .unwrap_or_else(|e| panic!("{good} should load: {e}"));
        assert_eq!(
            codex(&config)
                .options
                .get("openai_base_url")
                .map(String::as_str),
            Some(good)
        );
    }
}

#[test]
fn codex_unknown_options_are_rejected_at_the_key() {
    assert_error(
        "providers:\n  codex:\n    options:\n      web_search: disabled\n",
        4,
        7,
        "providers.codex.options.web_search is not supported",
    );
}

#[test]
fn codex_env_overrides_are_rejected_at_the_key() {
    assert_error(
        "providers:\n  codex:\n    env:\n      OPENAI_BASE_URL: http://gw.example\n",
        4,
        7,
        "providers.codex.env.OPENAI_BASE_URL is not an allowed environment variable (no environment overrides are allowed for this provider)",
    );
}

#[test]
fn relative_binary_path_resolves_against_the_config_directory() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(CONFIG_NAME);
    fs::write(&path, "providers:\n  claude:\n    binary: tools/claude\n").expect("write config");

    let loaded = config::load_with_env(Some(&path), no_env).expect("config loads");
    assert_eq!(
        claude(&loaded.config).binary,
        Some(dir.path().join("tools").join("claude"))
    );
}

#[test]
fn bare_and_absolute_binary_paths_are_not_resolved() {
    let config =
        load_text("providers:\n  claude:\n    binary: myclaude\n").expect("bare name loads");
    assert_eq!(claude(&config).binary, Some(PathBuf::from("myclaude")));

    // An absolute path on every OS (`/opt/claude` is relative on Windows).
    let absolute = std::env::temp_dir().join("claude");
    let yaml = format!(
        "providers:\n  claude:\n    binary: '{}'\n",
        absolute.display()
    );
    let config = load_text(&yaml).expect("absolute loads");
    assert_eq!(claude(&config).binary, Some(absolute));

    let config = load_text("providers:\n  claude:\n    binary: null\n").expect("null loads");
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
            "{}: configuration file {} does not exist",
            missing.display(),
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
    let config = load_text("").expect("empty file loads");
    let built = providers::build_from_config(&config, Arc::new(ProcessRunner::new()))
        .expect("providers build");
    let ids: Vec<&str> = built.iter().map(|provider| provider.id()).collect();
    assert_eq!(ids, ["claude", "codex"]);
}

/// Registry slice for the capability tests: a synthetic probe plus the real
/// Claude descriptor, which stays enabled and remains the implicit default
/// provider.
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
fn disabled_by_default_provider_is_off_until_enabled() {
    let descriptors = probe_registry();
    let config = load_with_descriptors("", &descriptors).expect("defaults load");
    let probe = config.providers.get("probe").expect("probe is configured");
    assert!(!probe.enabled);
    assert!(
        claude(&config).enabled,
        "claude is unaffected by the probe descriptor"
    );

    let config = load_with_descriptors(
        "providers:\n  probe:\n    enabled: true\n    model: test/model\n",
        &descriptors,
    )
    .expect("explicit enablement loads");
    let probe = config.providers.get("probe").expect("probe is configured");
    assert!(probe.enabled);
    assert_eq!(probe.model, "test/model");
}

#[test]
fn full_settings_validation_runs_without_a_yaml_entry() {
    let descriptors = [
        capability_probe::COUNTING,
        *providers::descriptor("claude").expect("claude is registered"),
    ];
    capability_probe::COUNT_VALIDATIONS.store(0, Ordering::SeqCst);
    let config = load_with_descriptors("", &descriptors).expect("defaults load");
    assert!(!config.providers["counting"].enabled);
    assert_eq!(
        capability_probe::COUNT_VALIDATIONS.load(Ordering::SeqCst),
        1,
        "the validator ran for a provider with no YAML entry"
    );
}

#[test]
fn enabled_without_a_model_is_rejected_at_the_enabled_line() {
    let descriptors = probe_registry();
    let error = load_with_descriptors("providers:\n  probe:\n    enabled: true\n", &descriptors)
        .expect_err("enabled without a model fails");
    assert!(
        error.contains(":3:14: providers.probe.model is required when the provider is enabled"),
        "{error}"
    );
    assert!(
        error.contains(CONFIG_NAME),
        "error names the file:\n{error}"
    );
}

#[test]
fn missing_field_without_an_entry_reports_file_and_path_without_a_line() {
    let descriptors = [
        capability_probe::BARE,
        *providers::descriptor("claude").expect("claude is registered"),
    ];
    let error = load_with_descriptors("", &descriptors).expect_err("no model fails");
    assert_eq!(
        error,
        format!("{CONFIG_NAME}: providers.bare.model is required when the provider is enabled")
    );
}

#[test]
fn risk_warnings_render_only_for_enabled_providers() {
    let descriptors = probe_registry();
    let config = load_with_descriptors(
        "providers:\n  probe:\n    enabled: true\n    model: test/model\n",
        &descriptors,
    )
    .expect("config loads");
    assert_eq!(
        providers::risk_warnings(&config, &descriptors),
        ["warning: probe: the probe formats nothing and is not real"]
    );

    let config = load_with_descriptors("", &descriptors).expect("defaults load");
    assert!(
        providers::risk_warnings(&config, &descriptors).is_empty(),
        "a disabled provider prints no warning"
    );
}

#[test]
fn debug_output_masks_private_content() {
    let config = load_text(
        "prompts:\n  system: PROMPT-MARKER-SECRET\nproviders:\n  claude:\n    env:\n      ANTHROPIC_BASE_URL: ENV-MARKER-SECRET\n",
    )
    .expect("config loads");
    let debug = format!("{config:?}");
    assert!(!debug.contains("PROMPT-MARKER-SECRET"), "{debug}");
    assert!(!debug.contains("ENV-MARKER-SECRET"), "{debug}");
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
    fs::write(&path, "port: 8000\n").expect("write config");

    let output = run_pumice(&["check-config", "--config", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains(&format!("config: {}", path.display())),
        "{stdout}"
    );
    assert!(stdout.contains("port: 8000"), "{stdout}");
    assert!(stdout.contains("default provider: claude"), "{stdout}");
    assert!(stdout.contains("enabled providers: claude"), "{stdout}");
    assert!(stdout.contains("fallback order: (none)"), "{stdout}");
    assert!(stdout.contains("total timeout: 30s"), "{stdout}");
}

#[test]
fn check_config_success_never_prints_private_values() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(CONFIG_NAME);
    fs::write(
        &path,
        "prompts:\n  system: PROMPT-MARKER\nproviders:\n  claude:\n    env:\n      ANTHROPIC_BASE_URL: ENV-MARKER\n",
    )
    .expect("write config");

    let output = run_pumice(&["check-config", "--config", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(!combined.contains("PROMPT-MARKER"), "{combined}");
    assert!(!combined.contains("ENV-MARKER"), "{combined}");
}

#[test]
fn check_config_error_exits_2_with_the_exact_error() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(CONFIG_NAME);
    fs::write(&path, "providers:\n  claude:\n    timeout_secs: 0\n").expect("write config");

    let output = run_pumice(&["check-config", "--config", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty());
    let stderr = stderr(&output);
    assert!(stderr.contains(":3:19:"), "{stderr}");
    assert!(
        stderr.contains("providers.claude.timeout_secs must be greater than zero"),
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
    fs::write(&path, "providers:\n  claude:\n    timeout_secs: 0\n").expect("write config");

    let output = run_pumice(&["serve", "--config", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("providers.claude.timeout_secs must be greater than zero"),
        "{stderr}"
    );
}

#[test]
fn configured_env_reaches_the_cli_invocation() {
    use pumice::providers::claude::ClaudeAdapter;
    use pumice::providers::cli::CliAdapter;
    use pumice::providers::{FormatInput, UserPrompt};

    let config = load_text(
        "providers:\n  claude:\n    env:\n      ANTHROPIC_BASE_URL: \"http://127.0.0.1:9999\"\n",
    )
    .expect("config loads");
    let built = providers::build_from_config(&config, Arc::new(ProcessRunner::new()))
        .expect("provider builds");
    assert_eq!(built.len(), 2);

    // The adapter built from config forwards the override; its own
    // variables are still present.
    let adapter = ClaudeAdapter::new(
        PathBuf::from("claude"),
        "haiku".to_owned(),
        claude(&config).env.clone(),
    );
    let invocation = adapter
        .invocation(FormatInput {
            system_prompt: "system",
            user_prompt: UserPrompt::default(),
            text: "text",
        })
        .expect("invocation builds");
    let env: BTreeMap<_, _> = invocation
        .env
        .iter()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    assert_eq!(
        env.get("ANTHROPIC_BASE_URL").map(String::as_str),
        Some("http://127.0.0.1:9999")
    );
    assert_eq!(
        env.get("MAX_THINKING_TOKENS").map(String::as_str),
        Some("0")
    );
}

#[test]
fn configured_openai_base_url_reaches_the_codex_invocation() {
    use pumice::providers::cli::CliAdapter;
    use pumice::providers::codex::CodexAdapter;
    use pumice::providers::{FormatInput, UserPrompt};

    let config = load_text(
        "providers:\n  codex:\n    options:\n      openai_base_url: \"https://gw.example/v1\"\n",
    )
    .expect("config loads");
    let codex_settings = codex(&config);
    assert_eq!(
        codex_settings
            .options
            .get("openai_base_url")
            .map(String::as_str),
        Some("https://gw.example/v1")
    );

    let adapter = CodexAdapter::new(
        PathBuf::from("codex"),
        codex_settings.model.clone(),
        codex_settings.options.get("openai_base_url").cloned(),
    );
    let invocation = adapter
        .invocation(FormatInput {
            system_prompt: "system",
            user_prompt: UserPrompt::default(),
            text: "text",
        })
        .expect("invocation builds");
    let args: Vec<String> = invocation
        .args
        .iter()
        .map(|a| match a {
            pumice::process::Argument::Literal(value) => value.to_string_lossy().into_owned(),
            // Path-bearing variants are materialized by the runner; the key
            // is enough to recognize them here.
            pumice::process::Argument::ControlPath { .. } => String::new(),
            pumice::process::Argument::ConfigControlPath { key, .. } => (*key).to_owned(),
        })
        .collect();
    let position = args
        .iter()
        .position(|a| a == "openai_base_url=\"https://gw.example/v1\"")
        .expect("encoded base URL argument is present");
    assert_eq!(args[position - 1], "-c");
}

/// Path to the shipped `pumice.example.yaml` at the repository root.
fn example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("pumice.example.yaml")
}

#[test]
fn example_config_loads_with_all_defaults() {
    let example = example_path();
    let loaded = config::load_with_env(Some(&example), no_env).expect("example loads");

    let defaults = config::load_with_env(None, no_env)
        .expect("built-in defaults load")
        .config;
    let mut expected = defaults.clone();
    // Relative paths anchor at the configuration file's directory, so the
    // example's default debug log path resolves beside the example file
    // instead of staying relative.
    expected.debug_log.path = example
        .parent()
        .expect("example lives in a directory")
        .join("pumice-debug.jsonl");

    assert_eq!(loaded.source, ConfigSource::File(example));
    assert_eq!(loaded.config, expected);
}

/// Replaces the last occurrence of `from` with `to`.
fn replace_last(text: &str, from: &str, to: &str) -> String {
    let start = text
        .rfind(from)
        .unwrap_or_else(|| panic!("{from:?} is present"));
    format!("{}{}{}", &text[..start], to, &text[start + from.len()..])
}

#[test]
fn example_documented_overrides_load() {
    let text = fs::read_to_string(example_path()).expect("read example");

    // Apply the documented examples: try Codex after Claude, route Codex
    // through a gateway, and set both formatting prompts.
    let uncommented = text.replace("fallback_order: []", "fallback_order: [codex]");
    // Codex's `options: {}` is the last one in the file.
    let uncommented = replace_last(
        &uncommented,
        "    options: {}",
        "    options:\n      openai_base_url: \"https://your-existing-gateway.example/v1\"",
    );
    let uncommented = uncommented
        .replace(
            "  system: null",
            "  system: |\n    Preserve technical terms and product names.",
        )
        .replace(
            "  user: null",
            "  user: |\n    Format spoken enumerations as Markdown lists.",
        );

    let config = load_text(&uncommented).expect("documented overrides load");
    assert_eq!(config.fallback_order, vec!["codex".to_owned()]);
    assert_eq!(
        codex(&config)
            .options
            .get("openai_base_url")
            .map(String::as_str),
        Some("https://your-existing-gateway.example/v1")
    );
    assert_eq!(
        config.prompts.system.as_deref(),
        Some("Preserve technical terms and product names.\n")
    );
    assert_eq!(
        config.prompts.user.as_deref(),
        Some("Format spoken enumerations as Markdown lists.\n")
    );
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
        "providers:\n  claude:\n    binary: tools/claude\n",
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
