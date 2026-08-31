//! Bounded systemd-logind idle-inhibitor evidence adapter.
//!
//! The adapter subscribes to manager property changes before taking its initial
//! inhibitor snapshot. It discards the descriptive and process fields returned
//! by logind and publishes only typed idle-inhibitor evidence. Positive logind
//! evidence is active; its absence remains unknown because compositor protocol
//! inhibitors are outside logind. A failed or oversized snapshot is
//! unavailable, never inactive.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use zbus::fdo::DBusProxy;
use zbus::message::Type as MessageType;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, MatchRule, MessageStream, Proxy};

use crate::context::privacy::{PrivacyEvidence, PrivacyState, PrivacyUpdate};
use crate::supervisor::{Cancellation, ReconnectBackoff};

const LOGIN1_DESTINATION: &str = "org.freedesktop.login1";
const LOGIN1_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_MANAGER: &str = "org.freedesktop.login1.Manager";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const CURRENT_INHIBITORS_PROPERTY: &str = "NCurrentInhibitors";
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_INHIBITORS: usize = 256;
const SIGNAL_CAPACITY: usize = 32;

type InhibitorRecord = (String, String, String, String, u32, u32);

/// One adapter-to-root observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrivacyObservation {
    /// Typed evidence update.
    pub update: PrivacyUpdate,
    /// Adapter-relative observation time.
    pub observed_millis: u64,
}

/// Run the independently supervised logind adapter until cancellation.
pub async fn run(
    publish: impl Fn(PrivacyObservation) + Send + Sync + 'static,
    mut cancellation: Cancellation,
) {
    let started = Instant::now();
    let mut backoff = ReconnectBackoff::default();
    publish_observation(
        &publish,
        PrivacyUpdate::Supported {
            evidence: PrivacyEvidence::IdleInhibitor,
            supported: true,
        },
        started,
    );

    loop {
        let result = run_connection(&publish, &mut cancellation, &mut backoff, started).await;
        if cancellation.is_cancelled() || result.is_ok() {
            return;
        }
        tracing::warn!("logind idle-inhibitor adapter unavailable");
        publish_observation(
            &publish,
            PrivacyUpdate::Unavailable(PrivacyEvidence::IdleInhibitor),
            started,
        );
        let delay = backoff.next_delay();
        tokio::select! {
            _ = cancellation.cancelled() => return,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

async fn run_connection(
    publish: &(impl Fn(PrivacyObservation) + Send + Sync + 'static),
    cancellation: &mut Cancellation,
    backoff: &mut ReconnectBackoff,
    started: Instant,
) -> zbus::Result<()> {
    let connection = tokio::time::timeout(CALL_TIMEOUT, Connection::system())
        .await
        .map_err(|_| zbus::Error::Failure("logind system-bus connection timed out".to_owned()))??;
    let bus = tokio::time::timeout(CALL_TIMEOUT, DBusProxy::new(&connection))
        .await
        .map_err(|_| zbus::Error::Failure("logind bus proxy timed out".to_owned()))??;
    let mut owner_changes = tokio::time::timeout(CALL_TIMEOUT, bus.receive_name_owner_changed())
        .await
        .map_err(|_| zbus::Error::Failure("logind owner subscription timed out".to_owned()))??;
    let proxy = tokio::time::timeout(
        CALL_TIMEOUT,
        Proxy::new(&connection, LOGIN1_DESTINATION, LOGIN1_PATH, LOGIN1_MANAGER),
    )
    .await
    .map_err(|_| zbus::Error::Failure("logind manager proxy timed out".to_owned()))??;
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(PROPERTIES_INTERFACE)?
        .member("PropertiesChanged")?
        .path(LOGIN1_PATH)?
        .build();
    let mut property_changes = tokio::time::timeout(
        CALL_TIMEOUT,
        MessageStream::for_match_rule(rule, &connection, Some(SIGNAL_CAPACITY)),
    )
    .await
    .map_err(|_| zbus::Error::Failure("logind property subscription timed out".to_owned()))??;

    publish_snapshot(publish, &proxy, started).await?;
    backoff.reset();

    loop {
        tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            signal = owner_changes.next() => {
                let Some(signal) = signal else {
                    return Err(zbus::Error::Failure(
                        "logind owner-change stream ended".to_owned(),
                    ));
                };
                let args = signal.args()?;
                if args.name().as_str() == LOGIN1_DESTINATION {
                    return Err(zbus::Error::Failure(
                        "logind service owner changed".to_owned(),
                    ));
                }
            }
            message = property_changes.next() => {
                let Some(message) = message else {
                    return Err(zbus::Error::Failure(
                        "logind property stream ended".to_owned(),
                    ));
                };
                let message = message?;
                let Ok((interface, changed, invalidated)) = message.body().deserialize::<(
                    String,
                    HashMap<String, OwnedValue>,
                    Vec<String>,
                )>() else {
                    continue;
                };
                if interface != LOGIN1_MANAGER
                    || (!changed.contains_key(CURRENT_INHIBITORS_PROPERTY)
                        && !invalidated
                            .iter()
                            .any(|name| name == CURRENT_INHIBITORS_PROPERTY))
                {
                    continue;
                }
                publish_snapshot(publish, &proxy, started).await?;
            }
        }
    }
}

async fn publish_snapshot(
    publish: &(impl Fn(PrivacyObservation) + Send + Sync + 'static),
    proxy: &Proxy<'_>,
    started: Instant,
) -> zbus::Result<()> {
    let records = tokio::time::timeout(
        CALL_TIMEOUT,
        proxy.call::<_, _, Vec<InhibitorRecord>>("ListInhibitors", &()),
    )
    .await
    .map_err(|_| zbus::Error::Failure("logind inhibitor snapshot timed out".to_owned()))??;
    let state = idle_inhibitor_state(&records).ok_or_else(|| {
        zbus::Error::Failure("logind inhibitor snapshot exceeded its bound".to_owned())
    })?;
    publish_observation(
        publish,
        PrivacyUpdate::Observed {
            evidence: PrivacyEvidence::IdleInhibitor,
            state,
        },
        started,
    );
    Ok(())
}

fn idle_inhibited(records: &[InhibitorRecord]) -> Option<bool> {
    if records.len() > MAX_INHIBITORS {
        return None;
    }
    Some(
        records
            .iter()
            .any(|record| record.0.split(':').any(|target| target.trim() == "idle")),
    )
}

fn idle_inhibitor_state(records: &[InhibitorRecord]) -> Option<PrivacyState> {
    idle_inhibited(records).map(|active| {
        if active {
            PrivacyState::Active
        } else {
            PrivacyState::Unknown
        }
    })
}

fn publish_observation(
    publish: &(impl Fn(PrivacyObservation) + Send + Sync + 'static),
    update: PrivacyUpdate,
    started: Instant,
) {
    publish(PrivacyObservation {
        update,
        observed_millis: elapsed_millis(started),
    });
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::{
        InhibitorRecord, LOGIN1_DESTINATION, LOGIN1_MANAGER, LOGIN1_PATH, MAX_INHIBITORS,
        idle_inhibited, idle_inhibitor_state,
    };
    use crate::context::privacy::PrivacyState;

    fn record(what: &str) -> InhibitorRecord {
        (
            what.to_owned(),
            "Synthetic owner".to_owned(),
            "Synthetic reason".to_owned(),
            "block".to_owned(),
            1000,
            2000,
        )
    }

    #[test]
    fn exact_idle_target_is_positive_within_a_compound_request() {
        assert_eq!(idle_inhibited(&[record("sleep:idle")]), Some(true));
        assert_eq!(
            idle_inhibitor_state(&[record("sleep:idle")]),
            Some(PrivacyState::Active)
        );
        assert_eq!(idle_inhibited(&[record("idleish")]), Some(false));
    }

    #[test]
    fn unrelated_and_empty_snapshots_are_not_false_inactive() {
        assert_eq!(idle_inhibited(&[]), Some(false));
        assert_eq!(idle_inhibited(&[record("shutdown:sleep")]), Some(false));
        assert_eq!(idle_inhibitor_state(&[]), Some(PrivacyState::Unknown));
        assert_eq!(
            idle_inhibitor_state(&[record("shutdown:sleep")]),
            Some(PrivacyState::Unknown)
        );
    }

    #[test]
    fn oversized_snapshot_is_uncertain_instead_of_false_inactive() {
        let records = (0..=MAX_INHIBITORS)
            .map(|_| record("sleep"))
            .collect::<Vec<_>>();
        assert_eq!(idle_inhibited(&records), None);
    }

    #[tokio::test]
    #[ignore = "requires a live systemd-logind system bus"]
    async fn live_logind_snapshot_is_readable_and_bounded() {
        let connection = zbus::Connection::system()
            .await
            .expect("live system bus connection");
        let proxy = zbus::Proxy::new(&connection, LOGIN1_DESTINATION, LOGIN1_PATH, LOGIN1_MANAGER)
            .await
            .expect("live logind proxy");
        let records = proxy
            .call::<_, _, Vec<InhibitorRecord>>("ListInhibitors", &())
            .await
            .expect("live bounded logind snapshot");
        assert!(idle_inhibited(&records).is_some());
    }
}
