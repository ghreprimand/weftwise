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
    InstanceScan, ProcessLiveness, ProcessProbe, current_uid, request_json, run_with_discovery,
    scan_instances,
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
