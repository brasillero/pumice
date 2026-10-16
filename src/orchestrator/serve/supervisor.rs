//! The owner of `pumice serve`'s one warm transcription child
//! (p13.1-flow-seam/D4, D5, D9).
//!
//! One task owns the current [`Generation`] and moves it through
//! `starting → ready → restarting → … → failed | stopping`, publishing a
//! [`ChildSnapshot`] that status routes read without waiting:
//!
//! - Startup: spawn, await `ready` within the startup timeout, check it and
//!   the child's own `GET /ready`. The initial startup is never retried.
//! - Liveness: the child's exit and its lifecycle stream are watched
//!   continuously; `GET /ready` is probed every second with a 500 ms
//!   deadline, at most one probe outstanding (a busy inference still
//!   answers). Three consecutive failed probes recycle the child.
//! - Recovery: after a failure, restarts after 0.5, 1 and 2 s, at most three
//!   per episode; an episode ends after 60 s continuously ready. Exhaustion
//!   leaves the state `failed` with no further timer. Protocol, version and
//!   configuration errors go to `failed` at once.
//! - The previous generation is always killed and reaped before another
//!   starts; one that cannot be reaped stays owned and the state becomes
//!   `failed` instead of growing a list of children.
//! - [`SupervisorHandle::recycle`] kills and restarts a ready generation on
//!   request (D5: a transcription abandoned after dispatch), without
//!   counting against the episode. Nothing is ever replayed.
//! - Shutdown: stdin closed, a grace for the child to exit on its own, then
//!   the tree is killed and reaped.
//! - A panic in the task (a core failure, D9) drops what it owned: each
//!   unreaped tree is killed and handed to the service's cleanup scope,
//!   and [`Supervisor::failed`] tells the service, which stops.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pumice_relay::process::CleanupScope;
use serde::Serialize;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Instant, MissedTickBehavior};

use crate::orchestrator::cancel::{self, CancelGuard, CancelHandle, CancelReason};
use crate::orchestrator::serve::child::{
    self, ChildSpec, Failure, Generation, ProbeError, Watched, code,
};
use crate::orchestrator::serve::output::Reporter;

/// The supervisor's clocks. [`Timings::default`] is the documented policy;
/// in-process tests may shorten it.
#[derive(Clone, Copy, Debug)]
pub struct Timings {
    pub probe_interval: Duration,
    pub probe_timeout: Duration,
    /// Consecutive failed probes that recycle the child.
    pub probe_failures: u32,
    /// The delay before each restart of an episode; its length is the
    /// episode's restart budget.
    pub backoff: [Duration; 3],
    /// Continuously ready this long ends a failure episode.
    pub stable_after: Duration,
    /// From closing the child's stdin to killing its tree.
    pub shutdown_grace: Duration,
    /// Reaping a killed tree never waits longer than this.
    pub reap_limit: Duration,
}

impl Default for Timings {
    fn default() -> Timings {
        Timings {
            probe_interval: Duration::from_secs(1),
            probe_timeout: Duration::from_millis(500),
            probe_failures: 3,
            backoff: [
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
            ],
            stable_after: Duration::from_secs(60),
            shutdown_grace: Duration::from_millis(2_500),
            reap_limit: Duration::from_secs(1),
        }
    }
}

/// After an abort, the task's future is dropped within this.
const ABORT_LIMIT: Duration = Duration::from_millis(100);

/// What the supervisor runs.
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    pub spec: ChildSpec,
    pub startup_timeout: Duration,
    pub timings: Timings,
    /// The service's cleanup scope: a generation dropped unreaped (the task
    /// panicked or was aborted) is killed and left there for its drain.
    pub scope: Arc<CleanupScope>,
    #[doc(hidden)]
    pub checkpoints: Checkpoints,
}

/// Fixed points in the supervisor's task, as [`Checkpoints`] sees them.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checkpoint {
    /// This ready generation is about to be watched.
    Watching(u64),
    /// This generation is no longer offered as ready and is about to be
    /// killed and reaped.
    Retiring(u64),
}

/// Test seam, never set by `pumice serve` itself: a callback the
/// supervisor's own task runs at each [`Checkpoint`], so in-process tests
/// can read the published snapshot mid-transition or inject a panic.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct Checkpoints(Option<Arc<dyn Fn(Checkpoint) + Send + Sync>>);

impl Checkpoints {
    pub fn new(hook: impl Fn(Checkpoint) + Send + Sync + 'static) -> Checkpoints {
        Checkpoints(Some(Arc::new(hook)))
    }

    fn reach(&self, point: Checkpoint) {
        if let Some(hook) = &self.0 {
            hook(point);
        }
    }
}

impl std::fmt::Debug for Checkpoints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Checkpoints(set)"
        } else {
            "Checkpoints(none)"
        })
    }
}

/// The child's state as status reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildState {
    Starting,
    Ready,
    Restarting,
    Failed,
    Stopping,
}

/// A cheap copy of the supervisor's state. `port` is the child's private
/// address and is never published by the status routes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildSnapshot {
    pub state: ChildState,
    /// The current (or last) generation; 1 is the first child.
    pub generation: u64,
    /// Restarts used in the current failure episode.
    pub restart_attempts: u32,
    /// While waiting to restart, the delay chosen for it.
    pub next_retry_delay_ms: Option<u64>,
    /// The ready generation's model load time.
    pub model_load_ms: Option<u64>,
    /// The last failure's fixed code.
    pub last_failure: Option<&'static str>,
    pub port: Option<u16>,
    /// The newest generation confirmed killed and reaped (0: none yet).
    pub retired: u64,
}

impl ChildSnapshot {
    fn initial() -> ChildSnapshot {
        ChildSnapshot {
            state: ChildState::Starting,
            generation: 0,
            restart_attempts: 0,
            next_retry_delay_ms: None,
            model_load_ms: None,
            last_failure: None,
            port: None,
            retired: 0,
        }
    }
}

/// The ready generation a request may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadyChild {
    pub generation: u64,
    pub port: u16,
}

enum Command {
    Recycle(u64),
}

/// A cheap, cloneable view of the supervisor for the frontend and, from
/// P13.4, the dictation route.
#[derive(Clone)]
pub struct SupervisorHandle {
    shared: Arc<Mutex<ChildSnapshot>>,
    /// Woken on every snapshot change.
    changed: Arc<Notify>,
    commands: mpsc::Sender<Command>,
}

impl SupervisorHandle {
    /// The current state; never waits for the supervisor.
    pub fn snapshot(&self) -> ChildSnapshot {
        self.shared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// The ready generation, if any.
    pub fn ready_child(&self) -> Option<ReadyChild> {
        let snapshot = self.snapshot();
        match (snapshot.state, snapshot.port) {
            (ChildState::Ready, Some(port)) => Some(ReadyChild {
                generation: snapshot.generation,
                port,
            }),
            _ => None,
        }
    }

    /// Asks for `generation` to be killed, reaped and replaced
    /// (p13.1-flow-seam/D5: a transcription abandoned after dispatch).
    /// Ignored unless `generation` is still the ready one, so a stale
    /// request never recycles its successor. Returns whether the request
    /// was queued.
    pub fn recycle(&self, generation: u64) -> bool {
        if self.ready_child().map(|child| child.generation) != Some(generation) {
            return false;
        }
        self.commands.try_send(Command::Recycle(generation)).is_ok()
    }

    /// Waits until `generation` is gone: confirmed killed and reaped, or
    /// owned by a supervisor that gave up on it (`failed`, `stopping`: its
    /// own stop reaps it, and the service reports a cleanup failure).
    /// False when `deadline` passed first.
    pub async fn wait_retired(&self, generation: u64, deadline: Instant) -> bool {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let snapshot = self.snapshot();
            if snapshot.retired >= generation
                || matches!(snapshot.state, ChildState::Failed | ChildState::Stopping)
            {
                return true;
            }
            tokio::select! {
                () = &mut changed => {}
                () = tokio::time::sleep_until(deadline) => return false,
            }
        }
    }
}

/// Why the initial startup failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupError {
    pub failure: Failure,
    /// The failed child could not be reaped.
    pub cleanup_failed: bool,
}

/// The running supervisor task.
pub struct Supervisor {
    handle: SupervisorHandle,
    initial: Option<oneshot::Receiver<Result<(), StartupError>>>,
    stop: CancelHandle,
    task: tokio::task::JoinHandle<bool>,
    /// The task ended on its own (see [`Supervisor::failed`]).
    ended: bool,
}

/// Starts the supervisor: it spawns generation 1 at once.
pub fn spawn(config: SupervisorConfig, reporter: Reporter) -> Supervisor {
    let shared = Arc::new(Mutex::new(ChildSnapshot::initial()));
    let changed = Arc::new(Notify::new());
    let (commands, receiver) = mpsc::channel(4);
    let (initial_sender, initial) = oneshot::channel();
    let (stop, stop_guard) = cancel::context();
    let owner = Owner {
        config,
        shared: Arc::clone(&shared),
        changed: Arc::clone(&changed),
        reporter,
        generation: 0,
        attempts: 0,
        leftover: None,
        unclean: false,
    };
    let task = tokio::spawn(owner.run(receiver, stop_guard, initial_sender));
    Supervisor {
        handle: SupervisorHandle {
            shared,
            changed,
            commands,
        },
        initial: Some(initial),
        stop,
        task,
        ended: false,
    }
}

impl Supervisor {
    pub fn handle(&self) -> SupervisorHandle {
        self.handle.clone()
    }

    /// Resolves once the first generation is ready, or with why it failed.
    /// Cancellation-safe; after it resolved once it stays pending, and so
    /// does it when the task ended without reporting (it panicked:
    /// [`Supervisor::failed`] resolves instead).
    pub async fn initial(&mut self) -> Result<(), StartupError> {
        initial_result(&mut self.initial).await
    }

    /// Resolves when the task ended before [`Supervisor::shutdown`] asked
    /// it to: it panicked (a core failure, D9). Whatever it owned was killed
    /// and left in the cleanup scope. Cancellation-safe.
    pub async fn failed(&mut self) {
        task_ended(&mut self.task, &mut self.ended).await;
    }

    /// [`Supervisor::initial`] and [`Supervisor::failed`] together, for a
    /// service that waits on both.
    pub async fn next(&mut self) -> Event {
        let Supervisor {
            initial,
            task,
            ended,
            ..
        } = self;
        tokio::select! {
            biased;
            () = task_ended(task, ended) => Event::Failed,
            result = initial_result(initial) => match result {
                Ok(()) => Event::Ready,
                Err(error) => Event::StartupFailed(error),
            },
        }
    }

    /// Stops the child cooperatively (stdin closed, grace, then kill and
    /// reap) and ends the task within `limit`. Returns false when a child
    /// could not be confirmed reaped; the task is then aborted, which kills
    /// whatever tree it still owned and leaves it in the cleanup scope.
    /// After [`Supervisor::failed`] there is nothing left to stop here: the
    /// scope holds what the task owned.
    pub async fn shutdown(mut self, limit: Duration) -> bool {
        self.stop.cancel(CancelReason::Shutdown);
        if self.ended {
            return true;
        }
        match tokio::time::timeout(limit, &mut self.task).await {
            Ok(Ok(cleaned)) => cleaned,
            Ok(Err(_)) => false,
            Err(_) => {
                self.task.abort();
                // The abort drops the task's future, handing its trees to
                // the scope; that takes one more poll.
                let _ = tokio::time::timeout(ABORT_LIMIT, &mut self.task).await;
                false
            }
        }
    }
}

/// What [`Supervisor::next`] observed.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// The first generation is ready.
    Ready,
    /// The first generation did not start.
    StartupFailed(StartupError),
    /// The task panicked.
    Failed,
}

async fn initial_result(
    initial: &mut Option<oneshot::Receiver<Result<(), StartupError>>>,
) -> Result<(), StartupError> {
    let Some(receiver) = initial.as_mut() else {
        return std::future::pending().await;
    };
    let result = receiver.await;
    *initial = None;
    match result {
        Ok(result) => result,
        Err(_) => std::future::pending().await,
    }
}

async fn task_ended(task: &mut tokio::task::JoinHandle<bool>, ended: &mut bool) {
    if !*ended {
        let _ = task.await;
        *ended = true;
    }
}

/// How a generation start ended.
enum Started {
    Ready(Generation),
    Failed {
        failure: Failure,
        cleaned: bool,
    },
    /// The stop fired during the start; the child was stopped.
    Stopped {
        cleaned: bool,
    },
}

/// Why a ready generation's watch ended.
enum End {
    Stop,
    Recycle,
    Failure(Failure),
}

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), ProbeError>> + Send>>;

struct Owner {
    config: SupervisorConfig,
    shared: Arc<Mutex<ChildSnapshot>>,
    changed: Arc<Notify>,
    reporter: Reporter,
    generation: u64,
    /// Restarts used in the current episode.
    attempts: u32,
    /// A killed generation that could not be reaped; while set, no other
    /// generation starts.
    leftover: Option<Box<Generation>>,
    /// A stop could not confirm its generation reaped.
    unclean: bool,
}

impl Owner {
    fn update(&self, change: impl FnOnce(&mut ChildSnapshot)) {
        change(
            &mut self
                .shared
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        self.changed.notify_waiters();
    }

    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut stop: CancelGuard,
        initial: oneshot::Sender<Result<(), StartupError>>,
    ) -> bool {
        let mut current = match self.start(&stop).await {
            Started::Ready(generation) => {
                let _ = initial.send(Ok(()));
                generation
            }
            Started::Failed { failure, cleaned } => {
                // The caller reports the startup failure itself.
                self.set_failed(failure.code);
                let _ = initial.send(Err(StartupError {
                    failure,
                    cleanup_failed: !cleaned,
                }));
                return self.wait_for_stop(&mut stop).await;
            }
            Started::Stopped { cleaned } => return cleaned,
        };
        loop {
            self.config
                .checkpoints
                .reach(Checkpoint::Watching(current.generation));
            let end = self.watch(&mut current, &mut commands, &mut stop).await;
            let failure = match end {
                End::Stop => return self.stop_generation(current).await,
                End::Recycle => {
                    self.reporter.child_recycled(current.generation);
                    if !self
                        .retire(current, code::RECYCLED, ChildState::Restarting)
                        .await
                    {
                        return self.wait_for_stop(&mut stop).await;
                    }
                    match self.start(&stop).await {
                        Started::Ready(next) => {
                            current = next;
                            continue;
                        }
                        Started::Failed { failure, cleaned } => {
                            if !cleaned || failure.fatal {
                                self.enter_failed(failure.code);
                                return self.wait_for_stop(&mut stop).await;
                            }
                            failure
                        }
                        Started::Stopped { cleaned } => return cleaned,
                    }
                }
                End::Failure(failure) => {
                    self.reporter.child_failed(
                        current.generation,
                        failure.code,
                        failure.child_code,
                    );
                    let next = if failure.fatal {
                        ChildState::Failed
                    } else {
                        ChildState::Restarting
                    };
                    if !self.retire(current, failure.code, next).await {
                        return self.wait_for_stop(&mut stop).await;
                    }
                    if failure.fatal {
                        self.enter_failed(failure.code);
                        return self.wait_for_stop(&mut stop).await;
                    }
                    failure
                }
            };
            match self.recover(failure, &mut stop).await {
                Some(next) => current = next,
                None => return self.wait_for_stop(&mut stop).await,
            }
        }
    }

    /// Restarts with backoff until a generation is ready (`Some`), or the
    /// episode is exhausted, a fatal failure or a cleanup failure happened,
    /// or the stop fired (`None`; the state says which).
    async fn recover(
        &mut self,
        mut failure: Failure,
        stop: &mut CancelGuard,
    ) -> Option<Generation> {
        loop {
            let backoff = self.config.timings.backoff;
            let Some(&delay) = backoff.get(self.attempts as usize) else {
                self.enter_failed(failure.code);
                return None;
            };
            self.attempts += 1;
            let attempts = self.attempts;
            self.update(|snapshot| {
                snapshot.state = ChildState::Restarting;
                snapshot.restart_attempts = attempts;
                snapshot.next_retry_delay_ms = Some(delay.as_millis() as u64);
                snapshot.last_failure = Some(failure.code);
                snapshot.port = None;
                snapshot.model_load_ms = None;
            });
            self.reporter
                .child_restarting(attempts, backoff.len() as u32, delay);
            tokio::select! {
                biased;
                _ = cancel::cancelled(stop) => return None,
                () = tokio::time::sleep(delay) => {}
            }
            self.update(|snapshot| snapshot.next_retry_delay_ms = None);
            match self.start(stop).await {
                Started::Ready(generation) => return Some(generation),
                Started::Failed {
                    failure: next,
                    cleaned,
                } => {
                    if !cleaned || next.fatal {
                        self.enter_failed(next.code);
                        return None;
                    }
                    failure = next;
                }
                Started::Stopped { .. } => return None,
            }
        }
    }

    /// Spawns the next generation and runs its handshake.
    async fn start(&mut self, stop: &CancelGuard) -> Started {
        self.generation += 1;
        let generation = self.generation;
        let restarting = generation > 1;
        self.update(|snapshot| {
            snapshot.generation = generation;
            snapshot.state = if restarting {
                ChildState::Restarting
            } else {
                ChildState::Starting
            };
            snapshot.port = None;
            snapshot.model_load_ms = None;
        });
        self.reporter.child_starting(generation);
        let deadline = Instant::now() + self.config.startup_timeout;
        let mut spawned = match Generation::spawn(&self.config.spec, generation, &self.config.scope)
        {
            Ok(spawned) => spawned,
            Err(failure) => {
                self.reporter.child_failed(generation, failure.code, None);
                return Started::Failed {
                    failure,
                    cleaned: true,
                };
            }
        };
        match spawned.handshake(&self.config.spec, deadline, stop).await {
            Ok(info) => {
                self.update(|snapshot| {
                    snapshot.state = ChildState::Ready;
                    snapshot.next_retry_delay_ms = None;
                    snapshot.model_load_ms = Some(info.model_load_ms);
                    snapshot.port = Some(info.port);
                });
                self.reporter.child_ready(generation, info.model_load_ms);
                Started::Ready(spawned)
            }
            Err(None) => Started::Stopped {
                cleaned: self.stop_generation(spawned).await,
            },
            Err(Some(failure)) => {
                self.reporter
                    .child_failed(generation, failure.code, failure.child_code);
                let state = if restarting {
                    ChildState::Restarting
                } else {
                    ChildState::Starting
                };
                let cleaned = self.retire(spawned, failure.code, state).await;
                Started::Failed { failure, cleaned }
            }
        }
    }

    /// Watches a ready generation until it fails, is recycled or the stop
    /// fires.
    async fn watch(
        &mut self,
        current: &mut Generation,
        commands: &mut mpsc::Receiver<Command>,
        stop: &mut CancelGuard,
    ) -> End {
        let timings = self.config.timings;
        let port = current.ready().map(|info| info.port).unwrap_or(0);
        let instance_id = current.instance_id.clone();
        let model = self.config.spec.model.clone();
        let stable_at = Instant::now() + timings.stable_after;
        let mut stable = false;
        let mut ticker = tokio::time::interval_at(
            Instant::now() + timings.probe_interval,
            timings.probe_interval,
        );
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut probe: Option<ProbeFuture> = None;
        let mut failed_probes = 0u32;
        let mut commands_open = true;
        loop {
            tokio::select! {
                biased;
                _ = cancel::cancelled(stop) => return End::Stop,
                watched = current.watch() => {
                    return End::Failure(match watched {
                        Watched::Exited => Failure::transient(code::EXITED),
                        Watched::Protocol => Failure::fatal(code::PROTOCOL),
                    });
                }
                command = commands.recv(), if commands_open => match command {
                    Some(Command::Recycle(generation)) if generation == current.generation => {
                        return End::Recycle;
                    }
                    Some(Command::Recycle(_)) => {}
                    None => commands_open = false,
                },
                result = async { probe.as_mut().expect("guarded").await }, if probe.is_some() => {
                    probe = None;
                    match result {
                        Ok(()) => failed_probes = 0,
                        Err(ProbeError::Mismatch) => {
                            return End::Failure(Failure::fatal(code::READY_CHECK_FAILED));
                        }
                        Err(ProbeError::Timeout | ProbeError::Unavailable) => {
                            failed_probes += 1;
                            if failed_probes >= timings.probe_failures {
                                return End::Failure(Failure::transient(code::PROBE_FAILED));
                            }
                        }
                    }
                }
                _ = ticker.tick(), if probe.is_none() => {
                    let instance_id = instance_id.clone();
                    let model = model.clone();
                    let deadline = Instant::now() + timings.probe_timeout;
                    probe = Some(Box::pin(async move {
                        child::probe_ready(port, &instance_id, &model, deadline).await
                    }));
                }
                () = tokio::time::sleep_until(stable_at), if !stable => {
                    stable = true;
                    self.attempts = 0;
                    self.update(|snapshot| snapshot.restart_attempts = 0);
                }
            }
        }
    }

    /// Kills and reaps a generation that is being replaced. First `state`
    /// (never `ready`) is published and the usable-child metadata cleared
    /// in one update, so nothing offers the generation while it dies. False
    /// when it could not be reaped: it stays owned and the state becomes
    /// `failed`.
    async fn retire(
        &mut self,
        generation: Generation,
        why: &'static str,
        state: ChildState,
    ) -> bool {
        self.update(|snapshot| {
            snapshot.state = state;
            snapshot.next_retry_delay_ms = None;
            snapshot.port = None;
            snapshot.model_load_ms = None;
            snapshot.last_failure = Some(why);
        });
        self.config
            .checkpoints
            .reach(Checkpoint::Retiring(generation.generation));
        let number = generation.generation;
        match generation
            .kill_and_reap(self.config.timings.reap_limit)
            .await
        {
            Ok(()) => {
                self.update(|snapshot| snapshot.retired = snapshot.retired.max(number));
                true
            }
            Err(unreaped) => {
                self.leftover = Some(unreaped);
                self.enter_failed(code::CLEANUP_FAILED);
                false
            }
        }
    }

    fn enter_failed(&self, why: &'static str) {
        self.set_failed(why);
        self.reporter.transcription_unavailable(why);
    }

    fn set_failed(&self, why: &'static str) {
        self.update(|snapshot| {
            snapshot.state = ChildState::Failed;
            snapshot.next_retry_delay_ms = None;
            snapshot.port = None;
            snapshot.model_load_ms = None;
            snapshot.last_failure = Some(why);
        });
    }

    /// The cooperative stop of a live generation, plus any leftover.
    async fn stop_generation(&mut self, generation: Generation) -> bool {
        self.update(|snapshot| {
            snapshot.state = ChildState::Stopping;
            snapshot.port = None;
        });
        let timings = self.config.timings;
        let number = generation.generation;
        match generation
            .stop(timings.shutdown_grace, timings.reap_limit)
            .await
        {
            Ok(()) => self.update(|snapshot| snapshot.retired = snapshot.retired.max(number)),
            Err(unreaped) => {
                // Dropped here: its kill-on-drop is the last defense.
                drop(unreaped);
                self.unclean = true;
            }
        }
        self.reap_leftover().await && !self.unclean
    }

    /// In `failed` (or after a cleanup failure): no timers, no children;
    /// waits for the stop, then retries any unreaped leftover once.
    async fn wait_for_stop(&mut self, stop: &mut CancelGuard) -> bool {
        cancel::cancelled(stop).await;
        self.update(|snapshot| snapshot.state = ChildState::Stopping);
        self.reap_leftover().await && !self.unclean
    }

    async fn reap_leftover(&mut self) -> bool {
        match self.leftover.take() {
            None => true,
            Some(leftover) => match leftover.kill_and_reap(self.config.timings.reap_limit).await {
                Ok(()) => true,
                Err(still) => {
                    self.leftover = Some(still);
                    false
                }
            },
        }
    }
}

/// Handles with a fixed snapshot, for the frontend's unit tests.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub fn handle(state: ChildState, generation: u64) -> SupervisorHandle {
        let (commands, _) = mpsc::channel(1);
        let mut snapshot = ChildSnapshot::initial();
        snapshot.state = state;
        snapshot.generation = generation;
        if state == ChildState::Ready {
            snapshot.model_load_ms = Some(12);
            snapshot.port = Some(5555);
        }
        SupervisorHandle {
            shared: Arc::new(Mutex::new(snapshot)),
            changed: Arc::new(Notify::new()),
            commands,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_policy_is_the_documented_one() {
        let timings = Timings::default();
        assert_eq!(timings.probe_interval, Duration::from_secs(1));
        assert_eq!(timings.probe_timeout, Duration::from_millis(500));
        assert_eq!(timings.probe_failures, 3);
        assert_eq!(
            timings.backoff,
            [
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2)
            ]
        );
        assert_eq!(timings.stable_after, Duration::from_secs(60));
        // D9: 2.5 s grace plus the reap stay inside the 5 s shutdown.
        assert!(timings.shutdown_grace + timings.reap_limit < Duration::from_millis(4_500));
    }

    #[test]
    fn recycle_requests_need_the_current_ready_generation() {
        let shared = Arc::new(Mutex::new(ChildSnapshot::initial()));
        let (commands, mut receiver) = mpsc::channel(4);
        let handle = SupervisorHandle {
            shared: Arc::clone(&shared),
            changed: Arc::new(Notify::new()),
            commands,
        };
        assert!(!handle.recycle(1), "nothing is ready yet");
        {
            let mut snapshot = shared.lock().unwrap();
            snapshot.state = ChildState::Ready;
            snapshot.generation = 2;
            snapshot.port = Some(4000);
        }
        assert_eq!(
            handle.ready_child(),
            Some(ReadyChild {
                generation: 2,
                port: 4000
            })
        );
        assert!(!handle.recycle(1), "a stale generation is ignored");
        assert!(handle.recycle(2));
        assert!(matches!(receiver.try_recv(), Ok(Command::Recycle(2))));
    }
}
