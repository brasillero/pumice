//! `pumice serve` (p13.3-serve-supervisor): the local stack's service.
//!
//! One loopback frontend (`http`), one owned warm transcription child
//! (`supervisor`, `child`) and, from P13.4, the shared flow execution
//! (`flow.rs`) in-process. Startup follows design §4:
//!
//! 1. signal handlers, before anything is loaded or spawned;
//! 2. the orchestrator file and its `serve:` section, the relay pipeline,
//!    the formatter registry and every enabled flow's references;
//! 3. the sibling `pumice-transcribe` and its `models list` (the served
//!    model must be configured and installed);
//! 4. the frontend port, claimed before the child starts (an occupied port
//!    fails, with no fallback);
//! 5. the child, until its handshake succeeds (never retried).
//!
//! Shutdown (D9): Ctrl-C or SIGTERM stops admission, closes the child's
//! stdin, waits a 2.5 s grace and kills and reaps what is left; exit 130 or
//! 143, or 1 when cleanup could not be confirmed. A core failure — the
//! supervisor's task or a request handler panicked — takes the same
//! bounded path and exits 1; the panic itself is reported as metadata only
//! (`output::PanicHook`).

pub mod child;
pub mod cli;
pub mod http;
pub mod output;
pub mod supervisor;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::time::Instant;

use pumice_relay::process::CleanupScope;

use crate::dispatch;
use crate::orchestrator::cancel::{self, CancelReason};
use crate::orchestrator::config;
use crate::orchestrator::error::StageError;
use crate::orchestrator::flow::{self, Runtime, set_reference_location};
use crate::orchestrator::result::stage;
use crate::orchestrator::serve::child::{ChildSpec, Failure, code};
use crate::orchestrator::serve::http::{FlowInfo, Frontend, RequestHook, ServiceState};
use crate::orchestrator::serve::output::Reporter;
use crate::orchestrator::serve::supervisor::{
    Checkpoints, Event, StartupError, SupervisorConfig, Timings,
};
use crate::orchestrator::transcription::Transcribe;

/// The `models list` preflight never takes longer than this.
const MODELS_LIST_CAP: Duration = Duration::from_secs(5);
/// The supervisor's whole stop: the 2.5 s grace plus the reap.
const SUPERVISOR_STOP_LIMIT: Duration = Duration::from_millis(3_750);
/// The frontend task's own end, after it was told to stop.
const FRONTEND_STOP_LIMIT: Duration = Duration::from_millis(100);
/// The service's cleanup scope's drain at the end.
const SCOPE_DRAIN_LIMIT: Duration = Duration::from_millis(250);

/// Already parsed and validated arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServeParams {
    pub config: Option<PathBuf>,
    /// `--port`, already checked; overrides only `serve.port`.
    pub port: Option<u16>,
}

/// Runs the service to its end and returns the process exit code.
pub async fn execute(params: &ServeParams, reporter: &Reporter) -> u8 {
    let (signals, signal_task) = match cancel::install() {
        Ok(installed) => installed,
        Err(_) => {
            reporter.error(
                "internal",
                None,
                "the cancellation signal handlers could not be registered",
            );
            return 1;
        }
    };
    let exit = serve(params, reporter, signals).await;
    signal_task.abort();
    exit
}

/// How the service ended.
enum Ending {
    Signal(CancelReason),
    StartupFailed(StartupError),
    /// The supervisor's task or a request handler panicked.
    CoreFailure,
}

/// What [`run`] serves once the startup checks passed and the frontend
/// port is claimed.
pub struct Service {
    pub listener: TcpListener,
    pub supervisor: SupervisorConfig,
    /// The served transcription model.
    pub model: String,
    /// The enabled flows.
    pub flows: Vec<FlowInfo>,
    /// The service's cleanup scope, drained at the end; the supervisor's
    /// configuration carries the same one.
    pub scope: Arc<CleanupScope>,
    #[doc(hidden)]
    pub request_hook: Option<RequestHook>,
}

async fn serve(params: &ServeParams, reporter: &Reporter, signals: cancel::CancelGuard) -> u8 {
    // The configuration, the served settings and every enabled flow's
    // relay route and formatter.
    let loaded = match config::load(params.config.as_deref()) {
        Ok(loaded) => loaded,
        Err(error) => {
            reporter.error("invalid_config", None, &error.to_string());
            return 2;
        }
    };
    let settings = match config::serve_settings(&loaded) {
        Ok(settings) => settings,
        Err(error) => {
            reporter.error("invalid_config", None, &error.to_string());
            return 2;
        }
    };
    let scope = Arc::new(CleanupScope::new());
    // Kept for the service's lifetime: P13.4 runs the flows on it.
    let _runtime = match Runtime::build(&loaded, Arc::clone(&scope)) {
        Ok(runtime) => runtime,
        Err(error) => return report_stage_error(reporter, &error),
    };

    // The sibling (never PATH) and the served model in its configuration.
    let Ok(binary) = dispatch::sibling_transcribe() else {
        reporter.error(
            code::NOT_INSTALLED,
            None,
            "pumice-transcribe is not installed next to pumice",
        );
        return 1;
    };
    let transcribe_config = loaded.config.modules.transcription.config.clone();
    if let Err(error) = check_model(
        &binary,
        &transcribe_config,
        &settings,
        &loaded.path,
        &signals,
    )
    .await
    {
        return report_stage_error(reporter, &error);
    }

    // The frontend port, before the child starts.
    let port = params.port.unwrap_or(settings.port);
    let listener = match pumice_relay::api::bind(port).await {
        Ok(listener) => listener,
        Err(_) => {
            reporter.error(
                "port_unavailable",
                None,
                &format!("cannot listen on 127.0.0.1:{port} (in use or not permitted)"),
            );
            return 1;
        }
    };
    let flows = loaded
        .config
        .flows
        .iter()
        .filter(|flow| flow.enabled)
        .map(|flow| FlowInfo {
            id: flow.id.clone(),
            transcription_model: flow.transcription_model.clone(),
            formatter: flow.formatter.clone(),
            relay_route: flow.relay_route.clone(),
        })
        .collect();
    let service = Service {
        listener,
        supervisor: SupervisorConfig {
            spec: ChildSpec {
                binary,
                config: transcribe_config,
                model: settings.transcription_model.clone(),
            },
            startup_timeout: settings.startup_timeout,
            timings: Timings::default(),
            scope: Arc::clone(&scope),
            checkpoints: Checkpoints::default(),
        },
        model: settings.transcription_model,
        flows,
        scope,
        request_hook: None,
    };
    run(service, reporter, signals).await
}

/// Serves until a signal or a core failure, then tears everything down
/// within the D9 budget and returns the exit code.
pub async fn run(service: Service, reporter: &Reporter, mut signals: cancel::CancelGuard) -> u8 {
    let Service {
        listener,
        supervisor,
        model,
        flows,
        scope,
        request_hook,
    } = service;
    let port = match listener.local_addr() {
        Ok(address) => address.port(),
        Err(_) => {
            reporter.error("internal", None, "the frontend address is unknown");
            return 1;
        }
    };
    reporter.listening(port);

    let mut supervisor = supervisor::spawn(supervisor, reporter.clone());
    let mut frontend = Frontend::new(port, supervisor.handle(), model, flows);
    if let Some(hook) = request_hook {
        frontend.set_request_hook(hook);
    }
    let frontend = Arc::new(frontend);
    let (frontend_stop, frontend_guard) = cancel::context();
    let mut frontend_task =
        tokio::spawn(http::serve(listener, Arc::clone(&frontend), frontend_guard));

    // Until the signal, the supervisor's and the frontend's tasks are
    // watched: either ending on its own is a core failure.
    let mut frontend_ended = false;
    let ending = loop {
        tokio::select! {
            biased;
            reason = cancel::cancelled(&mut signals) => break Ending::Signal(reason),
            _ = &mut frontend_task => {
                frontend_ended = true;
                break Ending::CoreFailure;
            }
            event = supervisor.next() => match event {
                Event::Ready => {
                    frontend.set_state(ServiceState::Running);
                    reporter.ready(port);
                }
                Event::StartupFailed(error) => break Ending::StartupFailed(error),
                Event::Failed => break Ending::CoreFailure,
            },
        }
    };

    // Teardown: admission first, then the child, then the cleanup scope
    // (the relay's trees and any the supervisor dropped when it failed).
    frontend.set_state(ServiceState::Stopping);
    if matches!(ending, Ending::Signal(_)) {
        reporter.stopping();
    }
    frontend_stop.cancel(CancelReason::Shutdown);
    let child_cleaned = supervisor.shutdown(SUPERVISOR_STOP_LIMIT).await;
    if !frontend_ended
        && tokio::time::timeout(FRONTEND_STOP_LIMIT, &mut frontend_task)
            .await
            .is_err()
    {
        frontend_task.abort();
    }
    let scope_cleaned = scope.drain(SCOPE_DRAIN_LIMIT).await == 0;
    let cleaned = child_cleaned && scope_cleaned;

    match ending {
        Ending::Signal(reason) => {
            if !cleaned {
                reporter.error(
                    code::CLEANUP_FAILED,
                    None,
                    "an owned process could not be confirmed stopped",
                );
                return 1;
            }
            let exit = reason.exit_code();
            reporter.stopped(exit);
            exit
        }
        Ending::StartupFailed(error) => {
            report_startup_failure(reporter, error.failure, error.cleanup_failed || !cleaned)
        }
        Ending::CoreFailure => {
            if cleaned {
                reporter.error(
                    "internal",
                    None,
                    "an internal task of pumice serve failed; the service stopped",
                );
            } else {
                reporter.error(
                    code::CLEANUP_FAILED,
                    Some("internal"),
                    "an internal task of pumice serve failed and an owned process could not be confirmed stopped",
                );
            }
            1
        }
    }
}

/// The served model must be in the transcription configuration and
/// installed, checked with `models list` like `pumice run` checks a flow's
/// model; a missing one is reported at `serve.transcription_model`.
async fn check_model(
    binary: &std::path::Path,
    transcribe_config: &std::path::Path,
    settings: &config::ServeSettings,
    orchestrator: &std::path::Path,
    signals: &cancel::CancelGuard,
) -> Result<(), StageError> {
    let stt = Transcribe::new(binary.to_path_buf(), transcribe_config.to_path_buf());
    let deadline = Instant::now() + MODELS_LIST_CAP;
    let entries = stt
        .models_list(deadline, signals)
        .await
        .map_err(|failure| flow::map_stt_failure(stage::PREPARE, failure, deadline))?;
    let message = match entries
        .iter()
        .find(|entry| entry.id == settings.transcription_model)
    {
        Some(entry) if entry.exists => return Ok(()),
        Some(_) => "the served transcription model file is missing; install or download it first",
        None => "serve.transcription_model is not in the transcription configuration",
    };
    let mut error = StageError::new(stage::PREPARE, "transcription_model_not_found", message, 2);
    set_reference_location(&mut error, orchestrator, settings.transcription_model_at);
    Err(error)
}

/// A preparation failure (or a signal during it).
fn report_stage_error(reporter: &Reporter, error: &StageError) -> u8 {
    if error.code == "cancelled" {
        reporter.stopped(error.exit);
    } else {
        reporter.error(error.code, error.cause_code.as_deref(), &error.message);
    }
    error.exit
}

/// The initial child did not become ready: one fixed error, exit 2 for the
/// transcription module's own configuration errors, 1 otherwise.
fn report_startup_failure(reporter: &Reporter, failure: Failure, cleanup_failed: bool) -> u8 {
    if cleanup_failed {
        reporter.error(
            code::CLEANUP_FAILED,
            Some(failure.code),
            "the transcription module did not start and could not be confirmed stopped",
        );
        return 1;
    }
    reporter.error(
        failure.code,
        failure.child_code,
        startup_message(failure.code),
    );
    match failure.child_code {
        Some("invalid_arguments" | "invalid_config" | "model_not_found") => 2,
        _ => 1,
    }
}

fn startup_message(failure: &str) -> &'static str {
    match failure {
        code::NOT_INSTALLED => "pumice-transcribe is not installed next to pumice",
        code::SPAWN_FAILED => "the transcription module could not be started",
        code::STARTUP_TIMEOUT => {
            "the transcription module was not ready within serve.startup_timeout_ms"
        }
        code::EXITED => "the transcription module exited before it was ready",
        code::MODULE_ERROR => "the transcription module reported a startup error",
        code::VERSION_MISMATCH => {
            "the transcription module's version differs from pumice's; install matching versions"
        }
        code::MODEL_MISMATCH => "the transcription module announced a different model",
        code::CAPABILITIES_MISSING => {
            "the transcription module lacks a capability pumice serve needs"
        }
        code::ADDRESS_INVALID => "the transcription module announced an invalid address",
        code::READY_CHECK_FAILED => "the transcription module's readiness check failed",
        _ => "the transcription module broke the managed protocol",
    }
}
