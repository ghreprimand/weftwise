//! Boundary-aligned in-process clock adapter.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::supervisor::Cancellation;

const MINUTE: Duration = Duration::from_secs(60);

/// Typed wall-clock update sent to the root model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockTick {
    /// Whole Unix seconds, formatted in the local timezone on the main thread.
    pub unix_seconds: i64,
}

/// Calculate the delay to the next wall-clock minute boundary.
#[must_use]
pub fn delay_until_next_minute(since_epoch: Duration) -> Duration {
    let remainder = Duration::from_nanos(
        (since_epoch.as_nanos() % MINUTE.as_nanos())
            .try_into()
            .unwrap_or(0),
    );
    if remainder.is_zero() {
        MINUTE
    } else {
        MINUTE - remainder
    }
}

/// Publish an immediate clock value, then update at exact minute boundaries.
pub async fn run<Emit>(emit: Emit, mut cancellation: Cancellation)
where
    Emit: Fn(ClockTick) + Send + Sync + 'static,
{
    loop {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        emit(ClockTick {
            unix_seconds: i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        });
        let delay = delay_until_next_minute(since_epoch);
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = cancellation.cancelled() => return,
        }
    }
}
