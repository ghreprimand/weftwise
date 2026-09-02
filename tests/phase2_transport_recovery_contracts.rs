use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot};
use weftwise::services::hyprland::{
    InstanceScan, MAX_EVENT_LINE_BYTES, ProcessLiveness, ProcessProbe, current_uid, request_json,
    run_with_discovery, scan_instances,
};
use weftwise::state::{HyprlandEvent, HyprlandUpdate};
use weftwise::supervisor::Supervisor;

static SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct SyntheticRuntime {
    root: PathBuf,
}

impl SyntheticRuntime {
    fn new() -> Self {
        let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "weftwise-hyprland-contract-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("hypr")).expect("synthetic runtime root");
        Self { root }
    }

    fn instance(&self, signature: &str) -> SyntheticInstance {
        let directory = self.root.join("hypr").join(signature);
        fs::create_dir(&directory).expect("synthetic instance directory");
        let request = StdUnixListener::bind(directory.join(".socket.sock"))
            .expect("synthetic request socket");
        let events =
            StdUnixListener::bind(directory.join(".socket2.sock")).expect("synthetic event socket");
        SyntheticInstance {
            directory,
            request,
            events,
        }
    }

    fn scan(&self) -> InstanceScan {
        InstanceScan {
            runtime_dir: self.root.clone(),
            wayland_display: Some(OsString::from("synthetic-wayland")),
            environment_signature: None,
        }
    }
}

impl Drop for SyntheticRuntime {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.root);
    }
}

struct SyntheticInstance {
    directory: PathBuf,
    request: StdUnixListener,
    events: StdUnixListener,
}

impl SyntheticInstance {
    fn socket_paths_are_live(&self) {
        assert!(self.request.local_addr().is_ok());
        assert!(self.events.local_addr().is_ok());
    }
}

struct Probe {
    dead: BTreeSet<i32>,
}

impl ProcessProbe for Probe {
    fn liveness(&self, pid: i32) -> ProcessLiveness {
        if self.dead.contains(&pid) {
            ProcessLiveness::Dead
        } else {
            ProcessLiveness::Unknown
        }
    }
}

fn trusted_uid() -> u32 {
    current_uid().expect("Linux owner identity")
}

fn snapshot_response(command: &str) -> &'static [u8] {
    match command {
        "j/monitors" => {
            b"[{\"id\":1,\"name\":\"SYNTH\",\"focused\":true,\"scale\":1.0,\"activeWorkspace\":{\"id\":1}}]"
        }
        "j/workspaces" => {
            b"[{\"id\":1,\"name\":\"one\",\"monitor\":\"SYNTH\",\"windows\":0,\"hasfullscreen\":false}]"
        }
        "j/clients" => b"[]",
        "j/activeworkspace" => b"{\"id\":1}",
        "j/activewindow" => b"{}",
        unexpected => panic!("unexpected synthetic snapshot command: {unexpected}"),
    }
}

const SNAPSHOT_COMMANDS: [&str; 5] = [
    "j/monitors",
    "j/workspaces",
    "j/clients",
    "j/activeworkspace",
    "j/activewindow",
];

fn labelled_snapshot_response(command: &str, output: &str) -> Vec<u8> {
    match command {
        "j/monitors" => format!(
            "[{{\"id\":1,\"name\":\"{output}\",\"focused\":true,\"scale\":1.0,\"activeWorkspace\":{{\"id\":1}}}}]"
        )
        .into_bytes(),
        "j/workspaces" => format!(
            "[{{\"id\":1,\"name\":\"one\",\"monitor\":\"{output}\",\"windows\":0,\"hasfullscreen\":false}}]"
        )
        .into_bytes(),
        "j/clients" => b"[]".to_vec(),
        "j/activeworkspace" => b"{\"id\":1}".to_vec(),
        "j/activewindow" => b"{}".to_vec(),
        unexpected => panic!("unexpected synthetic snapshot command: {unexpected}"),
    }
}

fn tokio_listener(listener: &StdUnixListener) -> UnixListener {
    let listener = listener.try_clone().expect("synthetic listener clone");
    listener
        .set_nonblocking(true)
        .expect("nonblocking synthetic listener");
    UnixListener::from_std(listener).expect("Tokio synthetic listener")
}

async fn serve_snapshot_batches(
    listener: UnixListener,
    output: &'static str,
    batches: usize,
    completed: mpsc::UnboundedSender<usize>,
) {
    for batch in 0..batches {
        for expected in SNAPSHOT_COMMANDS {
            let (mut stream, _) = listener.accept().await.expect("snapshot request client");
            let mut command = Vec::new();
            stream
                .read_to_end(&mut command)
                .await
                .expect("bounded request command");
            assert_eq!(command, expected.as_bytes());
            stream
                .write_all(&labelled_snapshot_response(expected, output))
                .await
                .expect("synthetic snapshot response");
        }
        completed.send(batch).expect("snapshot completion receiver");
    }
}

struct RunningAdapter {
    supervisor: Supervisor,
    updates: mpsc::UnboundedReceiver<HyprlandUpdate>,
    completed: Option<oneshot::Receiver<()>>,
}

impl RunningAdapter {
    fn start(scan: InstanceScan) -> Self {
        let (updates, receiver) = mpsc::unbounded_channel();
        let (completed, completed_receiver) = oneshot::channel();
        let probe: Arc<dyn ProcessProbe + Send + Sync> = Arc::new(Probe {
            dead: BTreeSet::new(),
        });
        let mut supervisor = Supervisor::default();
        supervisor.spawn_cancellable_adapter(move |cancellation| async move {
            run_with_discovery(
                scan,
                probe,
                move |update| {
                    let _ignored_send_failure = updates.send(update);
                },
                cancellation,
            )
            .await;
            let _ignored_send_failure = completed.send(());
        });
        Self {
            supervisor,
            updates: receiver,
            completed: Some(completed_receiver),
        }
    }

    async fn next(&mut self, expectation: &'static str) -> HyprlandUpdate {
        tokio::time::timeout(Duration::from_secs(2), self.updates.recv())
            .await
            .unwrap_or_else(|_| panic!("{expectation} update deadline"))
            .unwrap_or_else(|| panic!("{expectation} update channel closed"))
    }

    async fn stop(&mut self) {
        self.supervisor.shutdown();
        tokio::time::timeout(
            Duration::from_secs(1),
            self.completed.take().expect("adapter completion receiver"),
        )
        .await
        .expect("cancellation must stop the injected adapter")
        .expect("adapter completion sender");
        assert_eq!(self.supervisor.active_task_count(), 0);
    }
}

async fn assert_initial_snapshot(adapter: &mut RunningAdapter) {
    assert!(matches!(
        adapter.next("connecting").await,
        HyprlandUpdate::Connecting
    ));
    assert!(matches!(
        adapter.next("initial snapshot").await,
        HyprlandUpdate::Snapshot(_)
    ));
}

#[test]
fn rescan_rotates_from_a_dead_instance_directory_to_a_new_socket_pair() {
    let runtime = SyntheticRuntime::new();
    let stale = runtime.instance("synthetic_100");
    stale.socket_paths_are_live();
    fs::write(stale.directory.join("hyprland.lock"), "4242\n").expect("synthetic dead pid");

    let current = runtime.instance("synthetic_200");
    current.socket_paths_are_live();
    fs::write(
        current.directory.join("hyprland.log"),
        "PID: 4343\nsynthetic-wayland\n",
    )
    .expect("synthetic display affinity");

    let candidates = scan_instances(
        &runtime.scan(),
        trusted_uid(),
        false,
        &Probe {
            dead: BTreeSet::from([4242]),
        },
    )
    .expect("fresh synthetic instance");

    assert_eq!(candidates.len(), 1);
    assert!(
        candidates[0]
            .request()
            .ends_with("synthetic_200/.socket.sock")
    );
    assert!(
        candidates[0]
            .events()
            .ends_with("synthetic_200/.socket2.sock")
    );
    assert!(!format!("{:?}", candidates[0]).contains("synthetic_200"));
}

#[tokio::test]
async fn fresh_request_socket_answers_a_complete_bounded_snapshot_request() {
    let runtime = SyntheticRuntime::new();
    let instance = runtime.instance("synthetic_300");
    let request_path = instance.directory.join(".socket.sock");
    let request_listener = instance.request.try_clone().expect("request clone");
    request_listener
        .set_nonblocking(true)
        .expect("nonblocking request listener");
    let listener = UnixListener::from_std(request_listener).expect("Tokio request listener");

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("request client");
        let mut received = [0_u8; 32];
        let read = stream.read(&mut received).await.expect("request bytes");
        assert_eq!(&received[..read], b"j/monitors");
        stream
            .write_all(b"[{\"id\":1,\"name\":\"SYNTH\"}]")
            .await
            .expect("synthetic response");
    });

    let response = request_json(Path::new(&request_path), "j/monitors")
        .await
        .expect("fresh socket response");
    server.await.expect("server task");
    assert_eq!(response, "[{\"id\":1,\"name\":\"SYNTH\"}]");
}

#[tokio::test(flavor = "current_thread")]
async fn event_first_snapshot_replays_after_snapshot_and_stops_on_supervisor_cancellation() {
    let runtime = SyntheticRuntime::new();
    let instance = runtime.instance("synthetic_400");
    fs::write(
        instance.directory.join("hyprland.lock"),
        "4545\nsynthetic-wayland\n",
    )
    .expect("synthetic live instance metadata");

    let request_listener = instance.request.try_clone().expect("request clone");
    request_listener
        .set_nonblocking(true)
        .expect("nonblocking request listener");
    let request_listener =
        UnixListener::from_std(request_listener).expect("Tokio request listener");
    let events_listener = instance.events.try_clone().expect("event clone");
    events_listener
        .set_nonblocking(true)
        .expect("nonblocking event listener");
    let events_listener = UnixListener::from_std(events_listener).expect("Tokio event listener");

    let (event_written, event_written_receiver) = oneshot::channel();
    let (release_events, release_events_receiver) = oneshot::channel();
    let event_server = tokio::spawn(async move {
        let (mut stream, _) = events_listener.accept().await.expect("event client");
        stream
            .write_all(b"workspacev2>>1,one\n")
            .await
            .expect("buffered event");
        event_written.send(()).expect("event readiness receiver");
        let _release = release_events_receiver.await;
    });
    let request_server = tokio::spawn(async move {
        event_written_receiver
            .await
            .expect("event must be written before snapshot requests");
        for expected in [
            "j/monitors",
            "j/workspaces",
            "j/clients",
            "j/activeworkspace",
            "j/activewindow",
        ] {
            let (mut stream, _) = request_listener
                .accept()
                .await
                .expect("snapshot request client");
            let mut command = Vec::new();
            stream
                .read_to_end(&mut command)
                .await
                .expect("bounded request command");
            assert_eq!(command, expected.as_bytes());
            // The event connection was established first. Leave it time to be
            // observed while this fresh request socket is in flight.
            tokio::time::sleep(Duration::from_millis(20)).await;
            stream
                .write_all(snapshot_response(expected))
                .await
                .expect("synthetic snapshot response");
        }
    });

    let (updates, mut received) = mpsc::unbounded_channel();
    let (completed, completed_receiver) = oneshot::channel();
    let scan = runtime.scan();
    let probe: Arc<dyn ProcessProbe + Send + Sync> = Arc::new(Probe {
        dead: BTreeSet::new(),
    });
    let mut supervisor = Supervisor::default();
    supervisor.spawn_cancellable_adapter(move |cancellation| async move {
        run_with_discovery(
            scan,
            probe,
            move |update| {
                let _ignored_send_failure = updates.send(update);
            },
            cancellation,
        )
        .await;
        let _ignored_send_failure = completed.send(());
    });

    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .expect("connecting update deadline"),
        Some(HyprlandUpdate::Connecting)
    ));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .expect("snapshot update deadline"),
        Some(HyprlandUpdate::Snapshot(_))
    ));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .expect("buffered event replay deadline"),
        Some(HyprlandUpdate::Event(HyprlandEvent::WorkspaceChanged { id, .. }))
            if id.get() == 1
    ));

    supervisor.shutdown();
    tokio::time::timeout(Duration::from_secs(1), completed_receiver)
        .await
        .expect("cancellation must stop the injected adapter")
        .expect("adapter completion sender");
    assert_eq!(supervisor.active_task_count(), 0);
    release_events
        .send(())
        .expect("release synthetic event socket");
    request_server.await.expect("snapshot request server");
    event_server.await.expect("synthetic event server");
}

#[tokio::test(flavor = "current_thread")]
async fn clean_event_eof_reconnects_and_takes_a_fresh_snapshot() {
    let runtime = SyntheticRuntime::new();
    let instance = runtime.instance("synthetic_500");
    let (snapshots, _snapshot_receiver) = mpsc::unbounded_channel();
    let request_server = tokio::spawn(serve_snapshot_batches(
        tokio_listener(&instance.request),
        "SYNTH-EOF",
        2,
        snapshots,
    ));
    let (close_first, close_first_receiver) = oneshot::channel();
    let (release_second, release_second_receiver) = oneshot::channel();
    let event_server = tokio::spawn(async move {
        let (first, _) = tokio_listener(&instance.events)
            .accept()
            .await
            .expect("first event client");
        close_first_receiver
            .await
            .expect("close first event stream");
        drop(first);
        let (_second, _) = tokio_listener(&instance.events)
            .accept()
            .await
            .expect("reconnected event client");
        let _release = release_second_receiver.await;
    });

    let mut adapter = RunningAdapter::start(runtime.scan());
    assert_initial_snapshot(&mut adapter).await;
    close_first.send(()).expect("close first event stream");
    assert!(matches!(
        adapter.next("clean EOF gap").await,
        HyprlandUpdate::Gap
    ));
    assert!(matches!(
        adapter.next("clean EOF reconnect").await,
        HyprlandUpdate::Connecting
    ));
    assert!(matches!(
        adapter.next("fresh snapshot after clean EOF").await,
        HyprlandUpdate::Snapshot(_)
    ));

    adapter.stop().await;
    release_second
        .send(())
        .expect("release second event stream");
    request_server.await.expect("clean EOF snapshot server");
    event_server.await.expect("clean EOF event server");
}

#[tokio::test(flavor = "current_thread")]
async fn truncated_event_record_reconnects_without_crashing() {
    let runtime = SyntheticRuntime::new();
    let instance = runtime.instance("synthetic_510");
    let (snapshots, _snapshot_receiver) = mpsc::unbounded_channel();
    let request_server = tokio::spawn(serve_snapshot_batches(
        tokio_listener(&instance.request),
        "SYNTH-TRUNCATED",
        2,
        snapshots,
    ));
    let (truncate_first, truncate_first_receiver) = oneshot::channel();
    let (release_second, release_second_receiver) = oneshot::channel();
    let event_server = tokio::spawn(async move {
        let (mut first, _) = tokio_listener(&instance.events)
            .accept()
            .await
            .expect("first event client");
        truncate_first_receiver
            .await
            .expect("truncate first event stream");
        first
            .write_all(b"workspacev2>>1,one")
            .await
            .expect("partial event record");
        drop(first);
        let (_second, _) = tokio_listener(&instance.events)
            .accept()
            .await
            .expect("reconnected event client");
        let _release = release_second_receiver.await;
    });

    let mut adapter = RunningAdapter::start(runtime.scan());
    assert_initial_snapshot(&mut adapter).await;
    truncate_first.send(()).expect("truncate event stream");
    assert!(matches!(
        adapter.next("truncated record gap").await,
        HyprlandUpdate::Gap
    ));
    assert!(matches!(
        adapter.next("truncated record reconnect").await,
        HyprlandUpdate::Connecting
    ));
    assert!(matches!(
        adapter.next("fresh snapshot after truncated record").await,
        HyprlandUpdate::Snapshot(_)
    ));

    adapter.stop().await;
    release_second
        .send(())
        .expect("release second event stream");
    request_server
        .await
        .expect("truncated record snapshot server");
    event_server.await.expect("truncated record event server");
}

#[tokio::test(flavor = "current_thread")]
async fn socket_rotation_rescans_and_snapshots_the_new_instance() {
    let runtime = SyntheticRuntime::new();
    let first = runtime.instance("synthetic_520");
    let (first_snapshots, _first_snapshot_receiver) = mpsc::unbounded_channel();
    let first_request_server = tokio::spawn(serve_snapshot_batches(
        tokio_listener(&first.request),
        "SYNTH-A",
        1,
        first_snapshots,
    ));
    let (close_first, close_first_receiver) = oneshot::channel();
    let first_event_server = tokio::spawn(async move {
        let (_stream, _) = tokio_listener(&first.events)
            .accept()
            .await
            .expect("first event client");
        close_first_receiver
            .await
            .expect("close first event stream");
    });

    let mut adapter = RunningAdapter::start(runtime.scan());
    assert_initial_snapshot(&mut adapter).await;

    fs::remove_file(runtime.root.join("hypr/synthetic_520/.socket.sock"))
        .expect("remove first request socket path");
    fs::remove_file(runtime.root.join("hypr/synthetic_520/.socket2.sock"))
        .expect("remove first event socket path");
    fs::remove_dir(runtime.root.join("hypr/synthetic_520")).expect("remove first instance");
    let second = runtime.instance("synthetic_530");
    let (second_snapshots, _second_snapshot_receiver) = mpsc::unbounded_channel();
    let second_request_server = tokio::spawn(serve_snapshot_batches(
        tokio_listener(&second.request),
        "SYNTH-B",
        1,
        second_snapshots,
    ));
    let (release_second, release_second_receiver) = oneshot::channel();
    let second_event_server = tokio::spawn(async move {
        let (_stream, _) = tokio_listener(&second.events)
            .accept()
            .await
            .expect("rotated event client");
        let _release = release_second_receiver.await;
    });
    close_first
        .send(())
        .expect("close rotated first event stream");

    assert!(matches!(
        adapter.next("rotation gap").await,
        HyprlandUpdate::Gap
    ));
    assert!(matches!(
        adapter.next("rotation reconnect").await,
        HyprlandUpdate::Connecting
    ));
    let rotated_snapshot = adapter.next("rotated snapshot").await;
    assert!(matches!(
        rotated_snapshot,
        HyprlandUpdate::Snapshot(snapshot)
            if snapshot.outputs.keys().any(|output| output.as_str() == "SYNTH-B")
    ));

    adapter.stop().await;
    release_second
        .send(())
        .expect("release rotated event stream");
    first_request_server
        .await
        .expect("first rotation snapshot server");
    first_event_server
        .await
        .expect("first rotation event server");
    second_request_server
        .await
        .expect("second rotation snapshot server");
    second_event_server
        .await
        .expect("second rotation event server");
}

#[tokio::test(flavor = "current_thread")]
async fn oversize_event_record_is_discarded_before_a_later_valid_event() {
    let runtime = SyntheticRuntime::new();
    let instance = runtime.instance("synthetic_540");
    let (snapshots, _snapshot_receiver) = mpsc::unbounded_channel();
    let request_server = tokio::spawn(serve_snapshot_batches(
        tokio_listener(&instance.request),
        "SYNTH-OVERSIZE",
        1,
        snapshots,
    ));
    let (send_records, send_records_receiver) = oneshot::channel();
    let (release, release_receiver) = oneshot::channel();
    let event_server = tokio::spawn(async move {
        let (mut stream, _) = tokio_listener(&instance.events)
            .accept()
            .await
            .expect("oversize event client");
        send_records_receiver
            .await
            .expect("send oversize event record");
        stream
            .write_all(&vec![b'x'; MAX_EVENT_LINE_BYTES + 1])
            .await
            .expect("oversize event bytes");
        stream
            .write_all(b"\nworkspacev2>>1,one\n")
            .await
            .expect("valid event");
        let _release = release_receiver.await;
    });

    let mut adapter = RunningAdapter::start(runtime.scan());
    assert_initial_snapshot(&mut adapter).await;
    send_records.send(()).expect("release oversize event bytes");
    assert!(matches!(
        adapter.next("valid event after oversize record").await,
        HyprlandUpdate::Event(HyprlandEvent::WorkspaceChanged { id, .. }) if id.get() == 1
    ));

    adapter.stop().await;
    release.send(()).expect("release oversize event stream");
    request_server.await.expect("oversize snapshot server");
    event_server.await.expect("oversize event server");
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_known_event_repairs_with_one_gap_and_same_socket_snapshot() {
    let runtime = SyntheticRuntime::new();
    let instance = runtime.instance("synthetic_550");
    let (snapshots, _snapshot_receiver) = mpsc::unbounded_channel();
    let request_server = tokio::spawn(serve_snapshot_batches(
        tokio_listener(&instance.request),
        "SYNTH-REPAIR",
        2,
        snapshots,
    ));
    let (send_malformed, send_malformed_receiver) = oneshot::channel();
    let (release, release_receiver) = oneshot::channel();
    let event_server = tokio::spawn(async move {
        let (mut stream, _) = tokio_listener(&instance.events)
            .accept()
            .await
            .expect("repair event client");
        send_malformed_receiver
            .await
            .expect("send malformed known event");
        stream
            .write_all(b"workspacev2>>notanid,x\n")
            .await
            .expect("malformed known event");
        let _release = release_receiver.await;
    });

    let mut adapter = RunningAdapter::start(runtime.scan());
    assert_initial_snapshot(&mut adapter).await;
    send_malformed.send(()).expect("release malformed event");
    assert!(matches!(
        adapter.next("malformed event gap").await,
        HyprlandUpdate::Gap
    ));
    assert!(matches!(
        adapter.next("same socket repair snapshot").await,
        HyprlandUpdate::Snapshot(_)
    ));

    adapter.stop().await;
    release.send(()).expect("release repair event stream");
    request_server.await.expect("repair snapshot server");
    event_server.await.expect("repair event server");
}
