use std::collections::BTreeMap;
use std::time::Duration;

use weftwise::services::clock::delay_until_next_minute;
use weftwise::state::{
    ActiveContext, AdapterAvailability, AppState, ClientAddress, ClientState, CompositorOutput,
    DisplayText, HyprlandEvent, HyprlandSnapshot, HyprlandUpdate, OpenedClient, OutputId,
    OutputName, WorkspaceId, WorkspaceState,
};
use weftwise::supervisor::{ReconnectBackoff, Supervisor};

fn output_name(value: &str) -> OutputName {
    OutputName::new(value).expect("synthetic output name")
}

fn address(value: &str) -> ClientAddress {
    ClientAddress::new(value).expect("synthetic address")
}

fn workspace(id: i64, name: &str, output: Option<OutputName>, clients: u32) -> WorkspaceState {
    WorkspaceState {
        id: WorkspaceId::new(id),
        name: DisplayText::new(name, 64),
        output,
        clients,
        fullscreen: false,
    }
}

fn snapshot() -> HyprlandSnapshot {
    let first = output_name("SYNTH-1");
    let second = output_name("SYNTH-2");
    let one = WorkspaceId::new(1);
    let two = WorkspaceId::new(2);
    let client_address = address("0xabc123");
    let client = ClientState {
        address: client_address.clone(),
        class: DisplayText::new("SyntheticApp", 64),
        title: DisplayText::new("Synthetic document", 64),
        workspace: one,
        output: Some(first.clone()),
        fullscreen: false,
    };

    HyprlandSnapshot {
        outputs: BTreeMap::from([
            (
                first.clone(),
                CompositorOutput {
                    id: 1,
                    name: first.clone(),
                    focused: true,
                    scale_milli: 1000,
                    active_workspace: Some(one),
                    fullscreen: false,
                },
            ),
            (
                second.clone(),
                CompositorOutput {
                    id: 2,
                    name: second.clone(),
                    focused: false,
                    scale_milli: 1000,
                    active_workspace: Some(two),
                    fullscreen: false,
                },
            ),
        ]),
        workspaces: BTreeMap::from([
            (one, workspace(1, "one", Some(first.clone()), 1)),
            (two, workspace(2, "two", Some(second), 0)),
        ]),
        clients: BTreeMap::from([(client_address.clone(), client)]),
        active: ActiveContext {
            output: Some(first),
            workspace: Some(one),
            client: Some(client_address),
            class: DisplayText::new("SyntheticApp", 64),
            title: DisplayText::new("Synthetic document", 64),
        },
    }
}

#[test]
fn snapshot_is_atomic_and_projections_stay_local_to_bound_outputs() {
    let first = OutputId::new(11);
    let second = OutputId::new(12);
    let mut state = AppState::default();
    state.reconcile_outputs([first, second], [], false);
    state.bind_outputs([
        (first, Some("SYNTH-1".to_owned())),
        (second, Some("SYNTH-2".to_owned())),
    ]);

    assert_eq!(
        state.apply_hyprland_update(HyprlandUpdate::Snapshot(snapshot())),
        vec![first, second]
    );
    assert_eq!(state.desktop.availability, AdapterAvailability::Ready);

    let first_view = state.output_view(first).expect("first view");
    assert!(first_view.focused);
    assert_eq!(first_view.ribbon_label, "one · Synthetic document");
    assert_eq!(first_view.workspaces.len(), 1);
    assert!(first_view.workspaces[0].active);
    assert!(first_view.workspaces[0].occupied);

    let second_view = state.output_view(second).expect("second view");
    assert!(!second_view.focused);
    assert_eq!(second_view.ribbon_label, "two");
    assert_eq!(second_view.workspaces.len(), 1);
    assert!(!second_view.workspaces[0].active);
}

#[test]
fn stale_and_unavailable_updates_preserve_or_clear_truthfully() {
    let mut state = AppState::default();
    state.apply_hyprland_update(HyprlandUpdate::Unavailable);
    assert_eq!(state.desktop.availability, AdapterAvailability::Unavailable);

    state.apply_hyprland_update(HyprlandUpdate::Snapshot(snapshot()));
    state.apply_hyprland_update(HyprlandUpdate::Gap);
    assert_eq!(state.desktop.availability, AdapterAvailability::Stale);
    assert!(!state.desktop.outputs.is_empty());

    state.apply_hyprland_update(HyprlandUpdate::Connecting);
    assert_eq!(state.desktop.availability, AdapterAvailability::Stale);
    state.apply_hyprland_update(HyprlandUpdate::Unavailable);
    assert_eq!(state.desktop.availability, AdapterAvailability::Stale);
}

#[test]
fn client_lifecycle_updates_workspace_counts_without_underflow() {
    let mut state = AppState::default();
    state.apply_hyprland_update(HyprlandUpdate::Snapshot(snapshot()));
    let client = address("abc123");
    let one = WorkspaceId::new(1);
    let two = WorkspaceId::new(2);

    state.apply_hyprland_update(HyprlandUpdate::Event(HyprlandEvent::ClientMoved {
        address: client.clone(),
        workspace: two,
        workspace_name: DisplayText::new("two", 64),
    }));
    assert_eq!(state.desktop.workspaces[&one].clients, 0);
    assert_eq!(state.desktop.workspaces[&two].clients, 1);

    state.apply_hyprland_update(HyprlandUpdate::Event(HyprlandEvent::ClientClosed(client)));
    assert_eq!(state.desktop.workspaces[&two].clients, 0);
    assert!(state.desktop.clients.is_empty());

    state.apply_hyprland_update(HyprlandUpdate::Event(HyprlandEvent::ClientClosed(address(
        "abc123",
    ))));
    assert_eq!(state.desktop.workspaces[&two].clients, 0);
}

#[test]
fn client_open_resolves_the_wire_workspace_name_to_stable_identity() {
    let mut state = AppState::default();
    state.apply_hyprland_update(HyprlandUpdate::Snapshot(snapshot()));
    let opened_address = address("def456");

    state.apply_hyprland_update(HyprlandUpdate::Event(HyprlandEvent::ClientOpened(
        OpenedClient {
            address: opened_address.clone(),
            class: DisplayText::new("SyntheticApp", 64),
            title: DisplayText::new("Synthetic document", 64),
            workspace_name: DisplayText::new("two", 64),
        },
    )));

    assert_eq!(
        state.desktop.clients[&opened_address].workspace,
        WorkspaceId::new(2)
    );
    assert_eq!(state.desktop.workspaces[&WorkspaceId::new(2)].clients, 1);
    assert_eq!(state.desktop.availability, AdapterAvailability::Ready);
}

#[test]
fn hostile_text_and_addresses_are_bounded_normalized_and_redacted() {
    let text = DisplayText::new("  alpha\n\u{0007}beta\t  ", 12);
    assert_eq!(text.as_str(), "alpha beta");
    assert_eq!(format!("{text:?}"), "<redacted-text>");

    let output = output_name(&"x".repeat(128));
    assert_eq!(format!("{output:?}"), "<redacted-output>");
    assert!(OutputName::new(&"x".repeat(129)).is_none());

    assert_eq!(address("0xA1b2").as_str(), "a1b2");
    assert_eq!(format!("{:?}", address("a1b2")), "<redacted-client>");
    assert!(ClientAddress::new("not-an-address").is_none());
    assert!(ClientAddress::new(&"a".repeat(33)).is_none());
}

#[test]
fn reconnect_backoff_is_bounded_deterministic_and_resettable() {
    let minimum = Duration::from_millis(100);
    let maximum = Duration::from_millis(700);
    let mut first = ReconnectBackoff::new(minimum, maximum, 42);
    let mut second = ReconnectBackoff::new(minimum, maximum, 42);

    for expected_base in [100_u64, 200, 400, 700, 700, 700] {
        let first_delay = first.next_delay();
        assert_eq!(first_delay, second.next_delay());
        assert!(first_delay <= maximum);
        assert!(first_delay >= Duration::from_millis(expected_base.saturating_mul(4) / 5));
    }
    assert_eq!(first.attempt(), 6);
    first.reset();
    assert_eq!(first.attempt(), 0);
    assert_eq!(
        first.next_delay(),
        ReconnectBackoff::new(minimum, maximum, 42).next_delay()
    );
}

#[test]
fn clock_waits_until_the_next_boundary_without_a_refresh_subprocess() {
    assert_eq!(
        delay_until_next_minute(Duration::ZERO),
        Duration::from_secs(60)
    );
    assert_eq!(
        delay_until_next_minute(Duration::from_secs(60) + Duration::from_millis(250)),
        Duration::from_millis(59_750)
    );
    assert_eq!(
        delay_until_next_minute(Duration::from_secs(119) + Duration::from_millis(999)),
        Duration::from_millis(1)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn supervisor_cancellation_reaches_owned_adapter_without_unrelated_state() {
    let (completed, receiver) = tokio::sync::oneshot::channel();
    let mut supervisor = Supervisor::default();
    supervisor.spawn_cancellable_adapter(move |mut cancellation| async move {
        cancellation.cancelled().await;
        let _ignored_send_failure = completed.send(());
    });

    tokio::task::yield_now().await;
    assert_eq!(supervisor.task_count(), 1);
    assert_eq!(supervisor.active_task_count(), 1);
    supervisor.shutdown();

    tokio::time::timeout(Duration::from_secs(1), receiver)
        .await
        .expect("owned adapter must observe cancellation promptly")
        .expect("adapter completion sender");
    assert_eq!(supervisor.task_count(), 0);
}
