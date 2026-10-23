//! Immutable public model plans. Requests use exact lookup, never split ids.
use super::http::FlowInfo;
use crate::orchestrator::config::{self, Flow, LoadedConfig, ReferenceSpans, ServeSettings};
use crate::orchestrator::error::ConfigError;
use crate::orchestrator::flow::Runtime;

#[derive(Clone, Debug, Default)]
pub struct Models {
    plans: Vec<Flow>,
    infos: Vec<FlowInfo>,
}

impl Models {
    pub fn build(
        loaded: &LoadedConfig,
        settings: &ServeSettings,
        runtime: &Runtime,
    ) -> Result<Self, ConfigError> {
        let fail = |at: Option<(u64, u64)>, message: String| {
            let at = at.or(settings.relay.at).or(settings.transcription_model_at);
            ConfigError::at_file(&loaded.path, at.map(|v| v.0), at.map(|v| v.1), message)
        };
        let mut plans = vec![Flow {
            id: settings.transcription_model.clone(),
            enabled: true,
            transcription_model: settings.transcription_model.clone(),
            formatter: None,
            relay_route: "passthrough".to_owned(),
            budgets: settings.budgets,
            references: ReferenceSpans::default(),
        }];
        if settings.relay.enabled {
            if loaded.config.modules.formatter.is_none() {
                return Err(fail(settings.relay.preset_at, "serve.relay selects a formatter but modules.formatter is null; provide its complete formatter.yml".to_owned()));
            }
            let available = runtime.providers();
            if !available.contains(&settings.relay.default_provider.as_str()) {
                return Err(fail(
                    settings.relay.provider_at,
                    "serve.relay.default_provider must name an enabled provider in relay.yml"
                        .to_owned(),
                ));
            }
            for (index, preset) in settings.relay.presets.iter().enumerate() {
                if !runtime.has_formatter(preset) {
                    return Err(fail(
                        settings.relay.presets_at[index],
                        format!("serve.relay.presets names unregistered formatter \"{preset}\""),
                    ));
                }
            }
            let providers = if settings.relay.expand_providers {
                available
            } else {
                vec![settings.relay.default_provider.as_str()]
            };
            let presets = if settings.relay.expand_presets {
                &settings.relay.presets[..]
            } else {
                std::slice::from_ref(&settings.relay.default_preset)
            };
            for provider in providers {
                for preset in presets {
                    let id = format!("{}:{provider}:{preset}", settings.transcription_model);
                    if !config::is_public_model_id(&id) {
                        return Err(fail(settings.relay.at, "generated model ids must be nonempty, at most 512 bytes, without control characters; shorten the model, provider or preset id".to_owned()));
                    }
                    if plans.iter().any(|plan| plan.id == id) {
                        return Err(fail(
                            settings.relay.at,
                            format!(
                                "generated model id collision: \"{id}\"; rename the provider or preset"
                            ),
                        ));
                    }
                    plans.push(Flow {
                        id,
                        enabled: true,
                        transcription_model: settings.transcription_model.clone(),
                        formatter: Some(preset.clone()),
                        relay_route: provider.to_owned(),
                        budgets: settings.budgets,
                        references: ReferenceSpans::default(),
                    });
                }
            }
        }
        Ok(Self::from_plans(plans))
    }

    fn from_plans(plans: Vec<Flow>) -> Self {
        let infos = plans
            .iter()
            .map(|plan| FlowInfo {
                id: plan.id.clone(),
                transcription_model: plan.transcription_model.clone(),
                formatter: plan.formatter.clone(),
                relay_route: plan.relay_route.clone(),
            })
            .collect();
        Self { plans, infos }
    }

    pub fn get(&self, id: &str) -> Option<&Flow> {
        self.plans.iter().find(|plan| plan.id == id)
    }
    pub fn plans(&self) -> &[Flow] {
        &self.plans
    }
    pub fn infos(&self) -> &[FlowInfo] {
        &self.infos
    }
}

/// Decode one URL path component exactly once, with a literal `+`.
pub fn decode_id(encoded: &str) -> Result<String, &'static str> {
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = |b: u8| (b as char).to_digit(16).map(|v| v as u8);
            let high = bytes
                .get(i + 1)
                .and_then(|v| hex(*v))
                .ok_or("Malformed model id encoding.")?;
            let low = bytes
                .get(i + 2)
                .and_then(|v| hex(*v))
                .ok_or("Malformed model id encoding.")?;
            decoded.push(high * 16 + low);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| "The model id is not valid UTF-8.")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_decoding_is_exactly_once_and_preserves_plus() {
        assert_eq!(
            decode_id("a%3Ac%3Abuiltin%3Aclean").unwrap(),
            "a:c:builtin:clean"
        );
        assert_eq!(decode_id("a+b%2520").unwrap(), "a+b%20");
        assert_eq!(decode_id("caf%C3%A9").unwrap(), "café");
        for invalid in ["%", "%a", "%gg", "%FF"] {
            assert!(decode_id(invalid).is_err());
        }
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use pumice_relay::process::CleanupScope;
    use std::sync::Arc;

    fn fixture() -> (tempfile::TempDir, LoadedConfig) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = "schema_version: 1
modules:
  transcription:
    config: ./transcribe.yml
  formatter:
    config: ./formatter.yml
  relay:
    location: in_process
    config: ./relay.yml
flows: []
serve:
  enabled: true
  port: 0
  transcription_model: voice
  startup_timeout_ms: 60000
  allowed_origins: []
  budgets_ms:
    total: 12000
    transcription: 5000
    formatter: 1000
    relay: 7000
  relay:
    enabled: true
    default_provider: claude
    default_preset: clean
    presets: [clean, builtin:clean, inspect, passthrough]
    expand_providers: false
    expand_presets: false
";
        std::fs::write(dir.path().join("orchestrator.yml"), cfg).unwrap();
        std::fs::write(
            dir.path().join("relay.yml"),
            "port: 7567
total_timeout_secs: 30
max_parallel: 4
providers:
  - id: codex
    enabled: true
    model: test
    binary: unused-codex
    timeout_secs: 30
    effort: low
  - id: kiro
    enabled: false
    model: null
    binary: unused-kiro
    timeout_secs: 30
    effort: low
  - id: claude
    enabled: true
    model: test
    binary: unused-claude
    timeout_secs: 30
    effort: low
    thinking: false
debug_log:
  enabled: false
  path: debug.jsonl
",
        )
        .unwrap();
        let mut formatter = "schema_version: 1\ndefault: null\nformatters:\n".to_owned();
        for id in ["clean", "inspect", "passthrough"] {
            std::fs::write(dir.path().join(format!("{id}.yml")), format!("schema_version: 1\nid: {id}\ndescription: Test preset\nsystem: Test instructions\n")).unwrap();
            formatter.push_str(&format!("  - path: {id}.yml\n"));
        }
        std::fs::write(dir.path().join("formatter.yml"), formatter).unwrap();
        let loaded = config::load(Some(&dir.path().join("orchestrator.yml"))).unwrap();
        (dir, loaded)
    }

    #[test]
    fn expansions_are_ordered_cartesian_products_of_enabled_providers_and_listed_presets() {
        let (_dir, loaded) = fixture();
        let runtime = Runtime::build(&loaded, Arc::new(CleanupScope::new())).unwrap();
        for (providers, presets, expected) in [
            (false, false, vec!["voice", "voice:claude:clean"]),
            (
                true,
                false,
                vec!["voice", "voice:codex:clean", "voice:claude:clean"],
            ),
            (
                false,
                true,
                vec![
                    "voice",
                    "voice:claude:clean",
                    "voice:claude:builtin:clean",
                    "voice:claude:inspect",
                    "voice:claude:passthrough",
                ],
            ),
            (
                true,
                true,
                vec![
                    "voice",
                    "voice:codex:clean",
                    "voice:codex:builtin:clean",
                    "voice:codex:inspect",
                    "voice:codex:passthrough",
                    "voice:claude:clean",
                    "voice:claude:builtin:clean",
                    "voice:claude:inspect",
                    "voice:claude:passthrough",
                ],
            ),
        ] {
            let mut settings = config::serve_settings(&loaded).unwrap();
            settings.relay.expand_providers = providers;
            settings.relay.expand_presets = presets;
            let models = Models::build(&loaded, &settings, &runtime).unwrap();
            assert_eq!(
                models
                    .plans()
                    .iter()
                    .map(|p| p.id.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            for plan in models.plans() {
                assert_eq!(plan.budgets, settings.budgets);
            }
            assert!(models.get("voice:claude:builtin:clean").is_some() == presets);
            assert!(models.get("Voice").is_none());
            assert!(models.get("voice:passthrough:clean").is_none());
            assert!(models.get("voice:inspect:clean").is_none());
            assert!(models.get("voice:kiro:clean").is_none());
        }
    }

    #[test]
    fn registry_validation_reports_reference_lines_and_generated_id_limits() {
        let (_dir, loaded) = fixture();
        let runtime = Runtime::build(&loaded, Arc::new(CleanupScope::new())).unwrap();
        let mut settings = config::serve_settings(&loaded).unwrap();
        settings.relay.default_provider = "kiro".to_owned();
        let error = Models::build(&loaded, &settings, &runtime).unwrap_err();
        assert!(error.to_string().contains("default_provider"));
        assert!(error.to_string().contains(":24:"), "{error}");
        settings.relay.default_provider = "claude".to_owned();
        settings.relay.presets[0] = "missing".to_owned();
        let error = Models::build(&loaded, &settings, &runtime).unwrap_err();
        assert!(error.to_string().contains("unregistered formatter"));
        assert!(error.to_string().contains(":26:"), "{error}");
        settings.relay.presets[0] = "clean".to_owned();
        settings.transcription_model = "m".repeat(512);
        let error = Models::build(&loaded, &settings, &runtime).unwrap_err();
        assert!(error.to_string().contains("generated model ids"));
        settings.relay.enabled = false;
        let raw_runtime = Runtime::raw(&loaded, Arc::new(CleanupScope::new()));
        let models = Models::build(&loaded, &settings, &raw_runtime).unwrap();
        assert_eq!(models.plans().len(), 1);
        assert!(!raw_runtime.relay_enabled());
    }

    #[test]
    fn serve_budgets_bound_uploads_and_connections_without_dictation_flows() {
        let (_dir, loaded) = fixture();
        let mut settings = config::serve_settings(&loaded).unwrap();
        settings.relay.enabled = false;
        settings.budgets.total = std::time::Duration::from_secs(4);
        let runtime = Arc::new(Runtime::raw(&loaded, Arc::new(CleanupScope::new())));
        let models = Models::build(&loaded, &settings, &runtime).unwrap();
        let dictations = super::super::dictation::Dictations {
            runtime,
            flows: Vec::new(),
            models,
        };
        assert_eq!(
            dictations.upload_limit(),
            std::time::Duration::from_millis(1500)
        );
        assert_eq!(
            dictations.policy().connection_limit,
            std::time::Duration::from_secs(9)
        );
    }

    #[test]
    fn generated_built_in_presets_require_the_formatter_module() {
        let (_dir, mut loaded) = fixture();
        loaded.config.modules.formatter = None;
        let runtime = Runtime::build(&loaded, Arc::new(CleanupScope::new())).unwrap();
        let mut settings = config::serve_settings(&loaded).unwrap();
        settings.relay.default_preset = "builtin:clean".to_owned();
        let error = Models::build(&loaded, &settings, &runtime).unwrap_err();
        assert!(
            error.to_string().contains("modules.formatter is null"),
            "{error}"
        );
    }

    #[test]
    fn every_new_setting_is_required_and_legacy_publication_reports_migration() {
        let (dir, _) = fixture();
        let path = dir.path().join("orchestrator.yml");
        let full = std::fs::read_to_string(&path).unwrap();
        for line in [
            "    total: 12000",
            "    transcription: 5000",
            "    formatter: 1000",
            "    relay: 7000",
            "    enabled: true",
            "    default_provider: claude",
            "    default_preset: clean",
            "    presets: [clean, builtin:clean, inspect, passthrough]",
            "    expand_providers: false",
            "    expand_presets: false",
        ] {
            std::fs::write(&path, full.replace(&format!("{line}\n"), "")).unwrap();
            let error = config::load(Some(&path)).unwrap_err().to_string();
            assert!(
                error.contains("is missing") && error.contains("orchestrator.yml:"),
                "{line}: {error}"
            );
        }
        for extra in ["[]", "null", "obsolete"] {
            std::fs::write(&path, format!("{full}  transcription_flows: {extra}\n")).unwrap();
            let error = config::load(Some(&path)).unwrap_err().to_string();
            assert!(
                error.contains("was removed") && error.contains("serve.relay"),
                "{error}"
            );
        }
        for (old, new, problem) in [
            (
                "presets: [clean, builtin:clean, inspect, passthrough]",
                "presets: [clean, clean]",
                "duplicate preset",
            ),
            (
                "default_preset: clean",
                "default_preset: missing",
                "must be listed",
            ),
            (
                "default_provider: claude",
                "default_provider: inspect",
                "never passthrough or inspect",
            ),
        ] {
            std::fs::write(&path, full.replace(old, new)).unwrap();
            let error = config::load(Some(&path)).unwrap_err().to_string();
            assert!(error.contains(problem), "{error}");
        }
    }
}
