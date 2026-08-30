//! Ownership and limits for asynchronous adapter work.

use std::future::Future;

use thiserror::Error;
use tokio::task::JoinHandle;

/// Number of Tokio worker threads owned by Relm4.
pub const ASYNC_WORKER_LIMIT: usize = 1;

/// Maximum number of Relm4 blocking worker threads.
pub const BLOCKING_WORKER_LIMIT: usize = 4;

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
#[derive(Debug, Default)]
pub struct Supervisor {
    tasks: Vec<JoinHandle<()>>,
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

    /// Abort every owned task and release its handle.
    pub fn shutdown(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    /// Return the number of currently owned handles.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}
