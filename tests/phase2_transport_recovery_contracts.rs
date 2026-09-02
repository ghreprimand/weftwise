use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use weftwise::services::hyprland::{
    InstanceScan, ProcessLiveness, ProcessProbe, current_uid, request_json, scan_instances,
};

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
