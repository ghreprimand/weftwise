//! Ownership and limits for asynchronous adapter work.

use std::future::Future;
use std::thread;
use std::time::Duration;

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

    /// Begin shutdown, join owned tasks under a deadline, and abort survivors.
    ///
    /// Cancellation is signalled first so cooperative adapters can stop. The
    /// bounded join then runs off the calling (GTK) thread in a dedicated
    /// runtime rather than busy-waiting the caller, and every join result is
    /// reaped so a panicked adapter is observed as a redacted category count
    /// instead of being silently dropped.
    pub fn shutdown(&mut self) {
        let _already_cancelled = self.cancellation.send(true);
        let tasks = std::mem::take(&mut self.tasks);
        if tasks.is_empty() {
            return;
        }
        let outcome = drain_off_thread(tasks, SHUTDOWN_GRACE);
        if outcome.panicked > 0 {
            tracing::error!(
                count = outcome.panicked,
                "supervised adapter task panicked before shutdown"
            );
        }
        if outcome.aborted > 0 {
            tracing::warn!(
                count = outcome.aborted,
                "aborted unfinished adapter tasks at shutdown deadline"
            );
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

/// Categorized result of a bounded shutdown drain.
///
/// Counts are redacted categories only: no task identity, payload, or panic
/// message is retained, which keeps shutdown diagnostics public-safe.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ShutdownOutcome {
    /// Tasks that finished on their own before the deadline.
    completed: usize,
    /// Tasks whose join surfaced a panic.
    panicked: usize,
    /// Tasks still running at the deadline that were aborted.
    aborted: usize,
}

/// Await every owned task under a single deadline, aborting survivors at the
/// deadline and reaping each join result.
///
/// This is GTK-free and runtime-agnostic: it can be driven by the process
/// runtime in tests or by the dedicated shutdown runtime in
/// [`drain_off_thread`]. A panicked task is reported through
/// [`ShutdownOutcome::panicked`] rather than dropped; a task still running at
/// the deadline is aborted and then awaited so its cancellation is observed.
async fn drain_tasks(tasks: Vec<JoinHandle<()>>, grace: Duration) -> ShutdownOutcome {
    let mut outcome = ShutdownOutcome::default();
    let deadline = tokio::time::sleep(grace);
    tokio::pin!(deadline);
    let mut deadline_hit = false;
    for mut task in tasks {
        if deadline_hit {
            reap_past_deadline(&mut task, &mut outcome).await;
            continue;
        }
        tokio::select! {
            biased;
            () = &mut deadline => {
                deadline_hit = true;
                reap_past_deadline(&mut task, &mut outcome).await;
            }
            result = &mut task => record_join(&result, &mut outcome),
        }
    }
    outcome
}

/// Abort a still-running task, then await it so the join result is reaped.
async fn reap_past_deadline(task: &mut JoinHandle<()>, outcome: &mut ShutdownOutcome) {
    if !task.is_finished() {
        task.abort();
        outcome.aborted += 1;
    }
    let result = task.await;
    // An aborted task reports cancellation, already counted above; only an
    // independent completion or panic changes the remaining tallies.
    if !result
        .as_ref()
        .err()
        .is_some_and(tokio::task::JoinError::is_cancelled)
    {
        record_join(&result, outcome);
    }
}

/// Fold one join result into the outcome tallies.
fn record_join(result: &Result<(), tokio::task::JoinError>, outcome: &mut ShutdownOutcome) {
    match result {
        Ok(()) => outcome.completed += 1,
        Err(join_error) if join_error.is_panic() => outcome.panicked += 1,
        Err(_) => {}
    }
}

/// Run [`drain_tasks`] to completion in a dedicated current-thread runtime on a
/// separate OS thread, so the caller's (GTK) thread is never busy-waited.
fn drain_off_thread(tasks: Vec<JoinHandle<()>>, grace: Duration) -> ShutdownOutcome {
    let worker = thread::Builder::new()
        .name("weftwise-shutdown".to_owned())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            {
                Ok(runtime) => runtime.block_on(drain_tasks(tasks, grace)),
                Err(_) => {
                    // No runtime is available to await the handles; abort every
                    // unfinished task so none is left running past shutdown.
                    let mut outcome = ShutdownOutcome::default();
                    for task in &tasks {
                        if !task.is_finished() {
                            task.abort();
                            outcome.aborted += 1;
                        }
                    }
                    outcome
                }
            }
        });
    match worker {
        Ok(handle) => handle.join().unwrap_or_default(),
        Err(_) => ShutdownOutcome::default(),
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

#[cfg(test)]
mod shutdown_tests {
    use super::{drain_off_thread, drain_tasks};
    use std::time::Duration;

    // These tests exercise the shutdown drain without any GTK dependency: they
    // spawn plain Tokio tasks and drive them through the same drain the
    // supervisor uses at shutdown.

    #[tokio::test]
    async fn cancellation_ignoring_task_is_aborted_at_the_deadline() {
        // A task that never observes cooperative cancellation must still be
        // aborted once the grace deadline elapses.
        let task = tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        let outcome = drain_tasks(vec![task], Duration::from_millis(25)).await;
        assert_eq!(outcome.aborted, 1);
        assert_eq!(outcome.completed, 0);
        assert_eq!(outcome.panicked, 0);
    }

    #[tokio::test]
    async fn panicking_task_is_reaped_as_a_category_count() {
        // A panicking adapter is observed through the reaped join result rather
        // than being silently dropped.
        let task = tokio::spawn(async {
            panic!("synthetic adapter panic");
        });
        let outcome = drain_tasks(vec![task], Duration::from_millis(50)).await;
        assert_eq!(outcome.panicked, 1);
        assert_eq!(outcome.completed, 0);
        assert_eq!(outcome.aborted, 0);
    }

    #[tokio::test]
    async fn cooperative_tasks_complete_within_the_deadline() {
        // Well-behaved tasks finish before the deadline and the drain returns
        // promptly without aborting anything.
        let quick = tokio::spawn(async {});
        let brief = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
        });
        let start = std::time::Instant::now();
        let outcome = drain_tasks(vec![quick, brief], Duration::from_secs(5)).await;
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(outcome.completed, 2);
        assert_eq!(outcome.aborted, 0);
        assert_eq!(outcome.panicked, 0);
    }

    #[tokio::test]
    async fn off_thread_drain_reaps_finished_tasks() {
        // The production entry point builds its own runtime on a separate OS
        // thread. Drive the tasks to completion first so the blocking join in
        // the wrapper resolves the already-finished handles.
        let first = tokio::spawn(async {});
        let second = tokio::spawn(async {});
        while !first.is_finished() || !second.is_finished() {
            tokio::task::yield_now().await;
        }
        let outcome = drain_off_thread(vec![first, second], Duration::from_millis(50));
        assert_eq!(outcome.completed, 2);
        assert_eq!(outcome.aborted, 0);
        assert_eq!(outcome.panicked, 0);
    }
}
