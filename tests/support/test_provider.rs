//! A scripted [`Provider`] double for fallback-chain tests.
//!
//! Every `format` call pops the next [`Step`] and counts as one call. Real
//! fake-CLI providers cannot build two chain entries (every adapter reports
//! one fixed id), so chain logic runs against this double, with one real
//! adapter mixed in where the protocol matters.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pumice::providers::{FormatInput, Provider, ProviderError, ProviderFuture};
use tokio::time::Instant;

/// One scripted reaction of a [`TestProvider`].
pub enum Step {
    /// Succeed with this final text.
    Ready(String),
    /// Fail immediately with this error.
    Fail(ProviderError),
    /// Sleep this long before succeeding; the test expects the pipeline's
    /// deadline to fire first.
    Sleep(Duration),
}

/// A scripted [`Provider`] for fallback-chain tests.
pub struct TestProvider {
    id: &'static str,
    calls: AtomicUsize,
    steps: Mutex<VecDeque<Step>>,
}

impl TestProvider {
    pub fn new(id: &'static str, steps: Vec<Step>) -> Arc<TestProvider> {
        Arc::new(TestProvider {
            id,
            calls: AtomicUsize::new(0),
            steps: Mutex::new(steps.into()),
        })
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Provider for TestProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn format<'a>(&'a self, _input: FormatInput<'a>, _deadline: Instant) -> ProviderFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .expect("test provider steps")
            .pop_front()
            .expect("test provider has a scripted step");
        Box::pin(async move {
            match step {
                Step::Ready(text) => Ok(text),
                Step::Fail(error) => Err(error),
                Step::Sleep(duration) => {
                    tokio::time::sleep(duration).await;
                    Ok("woke up after the deadline".to_owned())
                }
            }
        })
    }
}
