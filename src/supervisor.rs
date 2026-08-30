//! Ownership and limits for asynchronous adapter work.

use std::future::Future;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use thiserror::Error;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Number of Tokio worker threads owned by Relm4.
pub const ASYNC_WORKER_LIMIT: usize = 1;

/// Maximum number of Relm4 blocking worker threads.
pub const BLOCKING_WORKER_LIMIT: usize = 4;

/// Maximum synchronous shutdown grace before unfinished adapters are aborted.
pub const SHUTDOWN_GRACE: Duration = Duration::from_millis(100);

/// Failure to configure Relm4 before its runtime is initialized.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum RuntimeConfigurationError {
    /// A caller initialized the asynchronous worker limit to another value.
    #[error("Relm4 async worker limit was initialized incompatibly")]
    AsyncWorkers,
    /// A caller initialized the blocking worker limit to another value.
    #[error("Relm4 blocking worker limit was initialized incompatibly")]
    BlockingWorkers,
}

/// Set bounded Relm4 runtime limits before the first future is spawned.
pub fn configure_relm_runtime() -> Result<(), RuntimeConfigurationError> {
    if relm4::RELM_THREADS.set(ASYNC_WORKER_LIMIT).is_err()
        && relm4::RELM_THREADS.get() != Some(&ASYNC_WORKER_LIMIT)
    {
        return Err(RuntimeConfigurationError::AsyncWorkers);
    }

    if relm4::RELM_BLOCKING_THREADS
        .set(BLOCKING_WORKER_LIMIT)
        .is_err()
        && relm4::RELM_BLOCKING_THREADS.get() != Some(&BLOCKING_WORKER_LIMIT)
    {
        return Err(RuntimeConfigurationError::BlockingWorkers);
    }

    Ok(())
}

/// Owner of long-lived adapter tasks.
///
/// Adapter factories are invoked inside Relm4's entered Tokio runtime. This is
/// required for Tokio-backed zbus connections. Futures return typed data to the
/// root model and must not capture GTK objects.
#[derive(Debug)]
pub struct Supervisor {
    tasks: Vec<JoinHandle<()>>,
    cancellation: watch::Sender<bool>,
}

impl Default for Supervisor {
    fn default() -> Self {
        let (cancellation, _) = watch::channel(false);
        Self {
            tasks: Vec::new(),
            cancellation,
        }
    }
}

impl Supervisor {
    /// Spawn an adapter factory on the shared bounded runtime.
    pub fn spawn_adapter<Factory, Task>(&mut self, factory: Factory)
    where
        Factory: FnOnce() -> Task + Send + 'static,
        Task: Future<Output = ()> + Send + 'static,
    {
        self.tasks.push(relm4::spawn(async move {
            factory().await;
        }));
    }

    /// Spawn an adapter with an owned cancellation receiver.
    pub fn spawn_cancellable_adapter<Factory, Task>(&mut self, factory: Factory)
    where
        Factory: FnOnce(Cancellation) -> Task + Send + 'static,
        Task: Future<Output = ()> + Send + 'static,
    {
        let cancellation = Cancellation {
            receiver: self.cancellation.subscribe(),
        };
        self.tasks.push(relm4::spawn(async move {
            factory(cancellation).await;
        }));
    }

    /// Abort every owned task and release its handle.
    pub fn shutdown(&mut self) {
        let _already_cancelled = self.cancellation.send(true);
        let tasks = std::mem::take(&mut self.tasks);
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while tasks.iter().any(|task| !task.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        for task in tasks {
            if !task.is_finished() {
                task.abort();
            }
        }
    }

    /// Return the number of currently owned handles.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Return the number of tasks that have not completed yet.
    #[must_use]
    pub fn active_task_count(&self) -> usize {
        self.tasks.iter().filter(|task| !task.is_finished()).count()
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Cancellation receiver owned by one supervised adapter.
#[derive(Debug, Clone)]
pub struct Cancellation {
    receiver: watch::Receiver<bool>,
}

impl Cancellation {
    /// Wait until the owning supervisor begins shutdown.
    pub async fn cancelled(&mut self) {
        if *self.receiver.borrow() {
            return;
        }
        while self.receiver.changed().await.is_ok() {
            if *self.receiver.borrow() {
                return;
            }
        }
    }

    /// Whether shutdown has already begun.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.receiver.borrow()
    }
}

/// Bounded exponential reconnect delay with deterministic per-attempt jitter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconnectBackoff {
    minimum: Duration,
    maximum: Duration,
    attempt: u32,
    seed: u64,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self::new(
            Duration::from_millis(250),
            Duration::from_secs(30),
            0x6f_64_79_73_73_65_79,
        )
    }
}

impl ReconnectBackoff {
    /// Create a deterministic backoff suitable for production or tests.
    #[must_use]
    pub const fn new(minimum: Duration, maximum: Duration, seed: u64) -> Self {
        Self {
            minimum,
            maximum,
            attempt: 0,
            seed,
        }
    }

    /// Return the next delay, including bounded jitter of up to 20 percent.
    pub fn next_delay(&mut self) -> Duration {
        let multiplier = 1_u32.checked_shl(self.attempt.min(30)).unwrap_or(u32::MAX);
        let base = self.minimum.saturating_mul(multiplier).min(self.maximum);
        self.attempt = self.attempt.saturating_add(1);

        let jitter_bound = base / 5;
        if jitter_bound.is_zero() {
            return base;
        }
        let mixed = self
            .seed
            .wrapping_add(u64::from(self.attempt))
            .wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let span_nanos = jitter_bound.as_nanos().saturating_mul(2).saturating_add(1);
        let offset = (u128::from(mixed) % span_nanos) as i128 - jitter_bound.as_nanos() as i128;
        let base_nanos = base.as_nanos() as i128;
        let jittered = (base_nanos + offset).max(0) as u128;
        duration_from_nanos(jittered).min(self.maximum)
    }

    /// Reset the sequence after a complete initial snapshot succeeds.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Number of delays produced since the last reset.
    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }
}

fn duration_from_nanos(nanos: u128) -> Duration {
    let seconds = (nanos / 1_000_000_000).min(u128::from(u64::MAX)) as u64;
    let subsecond = (nanos % 1_000_000_000) as u32;
    Duration::new(seconds, subsecond)
}
