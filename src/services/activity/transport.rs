//! Authenticated and bounded Unix transport for local activity events.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::timeout;

use super::{
    ActivityEndpointError, ActivityEvent, ActivityObservation, ActivityProtocolError,
    MAX_ACTIVITY_FRAME_BYTES, decode_frame, endpoint_path,
};
use crate::config::{ConfigPaths, PRIVATE_DIRECTORY_MODE, PRIVATE_FILE_MODE};
use crate::supervisor::Cancellation;

/// Maximum clients whose frames may be processed concurrently.
pub const MAX_ACTIVITY_CLIENTS: usize = 8;
/// Maximum frames accepted from one client during a rate window.
pub const MAX_ACTIVITY_FRAMES_PER_WINDOW: u16 = 64;
/// Fixed rate-limit window for one connected client.
pub const ACTIVITY_RATE_WINDOW: Duration = Duration::from_secs(1);
/// Maximum time a client may retain a slot without sending its next frame.
pub const ACTIVITY_CLIENT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum time allowed when determining whether an existing socket is live.
const EXISTING_ENDPOINT_PROBE_TIMEOUT: Duration = Duration::from_millis(100);
/// Maximum time the CLI waits for a validated endpoint acknowledgement.
pub const ACTIVITY_CLIENT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const ACTIVITY_ACKNOWLEDGEMENT: &[u8; 3] = b"ok\n";

/// Public-safe endpoint setup or client failure.
#[derive(Debug, Error)]
pub enum ActivityTransportError {
    /// The configured endpoint path is unavailable or invalid.
    #[error("the activity endpoint path is unavailable")]
    Endpoint(#[from] ActivityEndpointError),
    /// The trusted XDG runtime directory is absent or not a private directory.
    #[error("the activity runtime directory is not private")]
    UntrustedRuntime,
    /// The application runtime entry is not a safe owned directory.
    #[error("the activity application runtime directory is unsafe")]
    UnsafeApplicationDirectory,
    /// The runtime directory is not owned by the effective process user.
    #[error("the activity runtime directory owner does not match this process")]
    RuntimeOwnerMismatch,
    /// Another live process already owns the versioned endpoint.
    #[error("the activity endpoint is already active")]
    EndpointActive,
    /// An existing endpoint is not an owned Unix socket and cannot be replaced.
    #[error("the existing activity endpoint is unsafe")]
    UnsafeExistingEndpoint,
    /// Filesystem or socket setup failed without exposing its path.
    #[error("the activity endpoint could not be prepared")]
    Setup(#[source] io::Error),
    /// The connecting process does not match the trusted runtime owner.
    #[error("the activity peer is unauthorized")]
    UnauthorizedPeer,
    /// Peer credentials could not be established.
    #[error("the activity peer credentials are unavailable")]
    PeerCredentials(#[source] io::Error),
    /// The client did not send another frame within the fixed idle bound.
    #[error("the activity peer exceeded the idle limit")]
    IdleLimit,
    /// The client exceeded its fixed message-rate bound.
    #[error("the activity peer exceeded the message-rate limit")]
    RateLimit,
    /// Reading a client frame failed.
    #[error("the activity peer transport failed")]
    ClientIo(#[source] io::Error),
    /// A client supplied an invalid bounded protocol frame.
    #[error("the activity peer supplied an invalid frame")]
    Protocol(#[source] ActivityProtocolError),
    /// The CLI could not verify a private owned endpoint before connecting.
    #[error("the local activity endpoint is unavailable or unsafe")]
    UnsafeClientEndpoint,
    /// The CLI could not connect to the verified endpoint.
    #[error("the local activity endpoint is not accepting connections")]
    ClientConnect(#[source] io::Error),
    /// The CLI could not write its bounded request.
    #[error("the local activity request could not be sent")]
    ClientWrite(#[source] io::Error),
    /// The endpoint did not acknowledge the validated request in time.
    #[error("the local activity request was not acknowledged")]
    ClientAcknowledgement(#[source] io::Error),
    /// The endpoint returned an unsupported acknowledgement.
    #[error("the local activity endpoint returned an invalid acknowledgement")]
    InvalidAcknowledgement,
}

/// Bound endpoint with ownership-aware cleanup.
pub struct ActivityEndpoint {
    listener: UnixListener,
    _cleanup: SocketCleanup,
    owner_uid: u32,
}

impl fmt::Debug for ActivityEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActivityEndpoint")
            .field("path", &"<redacted>")
            .field("owner", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl ActivityEndpoint {
    /// Create the private runtime directory and bind the versioned socket.
    pub async fn bind(paths: &ConfigPaths) -> Result<Self, ActivityTransportError> {
        let socket_path = endpoint_path(paths)?;
        let application_dir = socket_path
            .parent()
            .ok_or(ActivityTransportError::UnsafeApplicationDirectory)?;
        let process_uid = effective_process_uid()?;
        let owner_uid = prepare_application_directory(application_dir, process_uid)?;
        remove_owned_stale_socket(&socket_path, owner_uid).await?;

        let listener = UnixListener::bind(&socket_path).map_err(ActivityTransportError::Setup)?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))
            .map_err(ActivityTransportError::Setup)?;
        let metadata = fs::symlink_metadata(&socket_path).map_err(ActivityTransportError::Setup)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != owner_uid
            || metadata.mode() & 0o777 != PRIVATE_FILE_MODE
        {
            return Err(ActivityTransportError::UnsafeExistingEndpoint);
        }

        Ok(Self {
            listener,
            _cleanup: SocketCleanup {
                path: socket_path,
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            owner_uid,
        })
    }

    /// Accept authenticated peers until supervisor cancellation.
    pub async fn run<Emit>(self, emit: Emit, mut cancellation: Cancellation)
    where
        Emit: Fn(ActivityObservation) + Send + Sync + 'static,
    {
        let emit = Arc::new(emit);
        let permits = Arc::new(Semaphore::new(MAX_ACTIVITY_CLIENTS));
        let mut clients = JoinSet::new();
        let started = Instant::now();

        loop {
            while clients.try_join_next().is_some() {}
            tokio::select! {
                result = self.listener.accept() => {
                    let Ok((stream, _address)) = result else {
                        tracing::warn!("activity endpoint accept failed");
                        continue;
                    };
                    let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                        continue;
                    };
                    let emit = Arc::clone(&emit);
                    let owner_uid = self.owner_uid;
                    clients.spawn(async move {
                        let _permit = permit;
                        let publish = |event| {
                            emit(ActivityObservation {
                                event,
                                observed_millis: elapsed_millis(started),
                            });
                        };
                        if let Err(error) = serve_client(stream, owner_uid, &publish).await {
                            tracing::warn!(reason = %error, "activity peer disconnected");
                        }
                    });
                }
                () = cancellation.cancelled() => return,
            }
        }
    }
}

/// Send one validated event and wait for the endpoint to acknowledge receipt.
pub fn send_event(
    paths: &ConfigPaths,
    event: &ActivityEvent,
) -> Result<(), ActivityTransportError> {
    let socket_path = endpoint_path(paths)?;
    validate_client_endpoint(&socket_path)?;
    let mut stream =
        StdUnixStream::connect(&socket_path).map_err(ActivityTransportError::ClientConnect)?;
    stream
        .set_write_timeout(Some(ACTIVITY_CLIENT_RESPONSE_TIMEOUT))
        .map_err(ActivityTransportError::ClientWrite)?;
    stream
        .set_read_timeout(Some(ACTIVITY_CLIENT_RESPONSE_TIMEOUT))
        .map_err(ActivityTransportError::ClientAcknowledgement)?;
    let frame = super::encode_frame(event).map_err(ActivityTransportError::Protocol)?;
    stream
        .write_all(&frame)
        .map_err(ActivityTransportError::ClientWrite)?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(ActivityTransportError::ClientWrite)?;
    let mut acknowledgement = [0_u8; ACTIVITY_ACKNOWLEDGEMENT.len()];
    stream
        .read_exact(&mut acknowledgement)
        .map_err(ActivityTransportError::ClientAcknowledgement)?;
    if acknowledgement != *ACTIVITY_ACKNOWLEDGEMENT {
        return Err(ActivityTransportError::InvalidAcknowledgement);
    }
    Ok(())
}

fn validate_client_endpoint(path: &Path) -> Result<(), ActivityTransportError> {
    let application_dir = path
        .parent()
        .ok_or(ActivityTransportError::UnsafeClientEndpoint)?;
    let runtime_base = application_dir
        .parent()
        .ok_or(ActivityTransportError::UnsafeClientEndpoint)?;
    let process_uid = fs::metadata("/proc/self")
        .map_err(|_| ActivityTransportError::UnsafeClientEndpoint)?
        .uid();
    let base = fs::symlink_metadata(runtime_base)
        .map_err(|_| ActivityTransportError::UnsafeClientEndpoint)?;
    let application = fs::symlink_metadata(application_dir)
        .map_err(|_| ActivityTransportError::UnsafeClientEndpoint)?;
    let socket =
        fs::symlink_metadata(path).map_err(|_| ActivityTransportError::UnsafeClientEndpoint)?;
    if base.file_type().is_symlink()
        || !base.is_dir()
        || base.uid() != process_uid
        || base.mode() & 0o077 != 0
        || application.file_type().is_symlink()
        || !application.is_dir()
        || application.uid() != process_uid
        || application.mode() & 0o777 != PRIVATE_DIRECTORY_MODE
        || !socket.file_type().is_socket()
        || socket.uid() != process_uid
        || socket.mode() & 0o777 != PRIVATE_FILE_MODE
    {
        return Err(ActivityTransportError::UnsafeClientEndpoint);
    }
    Ok(())
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn effective_process_uid() -> Result<u32, ActivityTransportError> {
    let (probe, _peer) = UnixStream::pair().map_err(ActivityTransportError::PeerCredentials)?;
    probe
        .peer_cred()
        .map(|credentials| credentials.uid())
        .map_err(ActivityTransportError::PeerCredentials)
}

fn prepare_application_directory(
    path: &Path,
    process_uid: u32,
) -> Result<u32, ActivityTransportError> {
    let runtime_base = path
        .parent()
        .ok_or(ActivityTransportError::UntrustedRuntime)?;
    let base =
        fs::symlink_metadata(runtime_base).map_err(|_| ActivityTransportError::UntrustedRuntime)?;
    if base.file_type().is_symlink() || !base.is_dir() || base.mode() & 0o077 != 0 {
        return Err(ActivityTransportError::UntrustedRuntime);
    }
    let owner_uid = base.uid();
    if owner_uid != process_uid {
        return Err(ActivityTransportError::RuntimeOwnerMismatch);
    }

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.uid() != owner_uid
            {
                return Err(ActivityTransportError::UnsafeApplicationDirectory);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(ActivityTransportError::Setup)?;
        }
        Err(error) => return Err(ActivityTransportError::Setup(error)),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE))
        .map_err(ActivityTransportError::Setup)?;
    let metadata = fs::symlink_metadata(path).map_err(ActivityTransportError::Setup)?;
    if metadata.uid() != owner_uid || metadata.mode() & 0o777 != PRIVATE_DIRECTORY_MODE {
        return Err(ActivityTransportError::UnsafeApplicationDirectory);
    }
    Ok(owner_uid)
}

async fn remove_owned_stale_socket(
    path: &Path,
    owner_uid: u32,
) -> Result<(), ActivityTransportError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() || metadata.uid() != owner_uid {
                return Err(ActivityTransportError::UnsafeExistingEndpoint);
            }
            match timeout(EXISTING_ENDPOINT_PROBE_TIMEOUT, UnixStream::connect(path)).await {
                Ok(Ok(_stream)) => Err(ActivityTransportError::EndpointActive),
                Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    let current =
                        fs::symlink_metadata(path).map_err(ActivityTransportError::Setup)?;
                    if current.dev() != metadata.dev()
                        || current.ino() != metadata.ino()
                        || !current.file_type().is_socket()
                    {
                        return Err(ActivityTransportError::UnsafeExistingEndpoint);
                    }
                    fs::remove_file(path).map_err(ActivityTransportError::Setup)
                }
                Ok(Err(_)) | Err(_) => Err(ActivityTransportError::UnsafeExistingEndpoint),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ActivityTransportError::Setup(error)),
    }
}

async fn serve_client<Emit>(
    stream: UnixStream,
    owner_uid: u32,
    emit: &Emit,
) -> Result<(), ActivityTransportError>
where
    Emit: Fn(ActivityEvent) + Send + Sync,
{
    let peer = stream
        .peer_cred()
        .map_err(ActivityTransportError::PeerCredentials)?;
    authorize_uid(owner_uid, peer.uid())?;

    let mut reader = BufReader::new(stream);
    let mut rate = RateWindow::new(Instant::now());
    loop {
        let mut frame = Vec::with_capacity(512);
        let read = timeout(
            ACTIVITY_CLIENT_IDLE_TIMEOUT,
            (&mut reader)
                .take((MAX_ACTIVITY_FRAME_BYTES + 1) as u64)
                .read_until(b'\n', &mut frame),
        )
        .await
        .map_err(|_| ActivityTransportError::IdleLimit)?
        .map_err(ActivityTransportError::ClientIo)?;
        if read == 0 {
            return Ok(());
        }
        if frame.len() > MAX_ACTIVITY_FRAME_BYTES {
            return Err(ActivityTransportError::Protocol(
                ActivityProtocolError::FrameTooLarge,
            ));
        }
        rate.record(Instant::now())?;
        emit(decode_frame(&frame).map_err(ActivityTransportError::Protocol)?);
        reader
            .get_mut()
            .write_all(ACTIVITY_ACKNOWLEDGEMENT)
            .await
            .map_err(ActivityTransportError::ClientIo)?;
    }
}

fn authorize_uid(expected: u32, observed: u32) -> Result<(), ActivityTransportError> {
    (expected == observed)
        .then_some(())
        .ok_or(ActivityTransportError::UnauthorizedPeer)
}

struct RateWindow {
    started: Instant,
    frames: u16,
}

impl RateWindow {
    fn new(started: Instant) -> Self {
        Self { started, frames: 0 }
    }

    fn record(&mut self, now: Instant) -> Result<(), ActivityTransportError> {
        if now.duration_since(self.started) >= ACTIVITY_RATE_WINDOW {
            self.started = now;
            self.frames = 0;
        }
        if self.frames >= MAX_ACTIVITY_FRAMES_PER_WINDOW {
            return Err(ActivityTransportError::RateLimit);
        }
        self.frames += 1;
        Ok(())
    }
}

struct SocketCleanup {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let Ok(metadata) = fs::symlink_metadata(&self.path) else {
            return;
        };
        if metadata.file_type().is_socket()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
        {
            let _ignored = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct SyntheticRuntime {
        base: PathBuf,
        paths: ConfigPaths,
    }

    impl SyntheticRuntime {
        fn new() -> Self {
            let suffix = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let base = std::env::temp_dir().join(format!(
                "weftwise-activity-test-{}-{suffix}",
                std::process::id()
            ));
            fs::create_dir(&base).expect("synthetic runtime base");
            fs::set_permissions(&base, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE))
                .expect("private synthetic base");
            let paths = ConfigPaths {
                config_file: base.join("config.toml"),
                cache_dir: base.join("cache"),
                state_dir: base.join("state"),
                runtime_dir: Some(base.join("weftwise")),
            };
            Self { base, paths }
        }
    }

    impl Drop for SyntheticRuntime {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.base);
        }
    }

    #[tokio::test]
    async fn endpoint_uses_private_modes_and_removes_only_its_socket() {
        let runtime = SyntheticRuntime::new();
        let socket = endpoint_path(&runtime.paths).expect("socket path");
        let endpoint = ActivityEndpoint::bind(&runtime.paths)
            .await
            .expect("bound endpoint");
        let directory = fs::metadata(socket.parent().expect("application directory"))
            .expect("directory metadata");
        let socket_metadata = fs::symlink_metadata(&socket).expect("socket metadata");
        assert_eq!(directory.mode() & 0o777, PRIVATE_DIRECTORY_MODE);
        assert_eq!(socket_metadata.mode() & 0o777, PRIVATE_FILE_MODE);
        assert!(socket_metadata.file_type().is_socket());
        drop(endpoint);
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn endpoint_rejects_a_permissive_runtime_base() {
        let runtime = SyntheticRuntime::new();
        fs::set_permissions(&runtime.base, fs::Permissions::from_mode(0o755))
            .expect("permissive synthetic base");
        assert!(matches!(
            ActivityEndpoint::bind(&runtime.paths).await,
            Err(ActivityTransportError::UntrustedRuntime)
        ));
    }

    #[tokio::test]
    async fn endpoint_never_unlinks_an_active_socket() {
        let runtime = SyntheticRuntime::new();
        let first = ActivityEndpoint::bind(&runtime.paths)
            .await
            .expect("first endpoint");
        assert!(matches!(
            ActivityEndpoint::bind(&runtime.paths).await,
            Err(ActivityTransportError::EndpointActive)
        ));
        drop(first);
    }

    #[test]
    fn runtime_owner_must_match_the_effective_process_identity() {
        let runtime = SyntheticRuntime::new();
        let application_dir = runtime
            .paths
            .runtime_dir
            .as_ref()
            .expect("runtime directory");
        let owner = fs::metadata(&runtime.base).expect("runtime owner").uid();
        assert!(matches!(
            prepare_application_directory(application_dir, owner.saturating_add(1)),
            Err(ActivityTransportError::RuntimeOwnerMismatch)
        ));
    }

    #[tokio::test]
    async fn endpoint_never_replaces_a_regular_file() {
        let runtime = SyntheticRuntime::new();
        let application_dir = runtime
            .paths
            .runtime_dir
            .as_ref()
            .expect("runtime directory");
        fs::create_dir(application_dir).expect("application directory");
        let socket = endpoint_path(&runtime.paths).expect("socket path");
        let mut file = File::create(&socket).expect("synthetic collision");
        file.write_all(b"synthetic").expect("synthetic content");
        assert!(matches!(
            ActivityEndpoint::bind(&runtime.paths).await,
            Err(ActivityTransportError::UnsafeExistingEndpoint)
        ));
        assert_eq!(fs::read(socket).expect("collision retained"), b"synthetic");
    }

    #[test]
    fn peer_identity_requires_the_trusted_runtime_owner() {
        assert!(authorize_uid(1000, 1000).is_ok());
        assert!(matches!(
            authorize_uid(1000, 1001),
            Err(ActivityTransportError::UnauthorizedPeer)
        ));
    }

    #[test]
    fn rate_window_has_a_fixed_per_client_bound() {
        let start = Instant::now();
        let mut window = RateWindow::new(start);
        for _ in 0..MAX_ACTIVITY_FRAMES_PER_WINDOW {
            window.record(start).expect("within rate bound");
        }
        assert!(matches!(
            window.record(start),
            Err(ActivityTransportError::RateLimit)
        ));
        assert!(window.record(start + ACTIVITY_RATE_WINDOW).is_ok());
    }

    #[tokio::test]
    async fn same_owner_peer_emits_only_valid_bounded_events() {
        let runtime = SyntheticRuntime::new();
        let owner_uid = fs::metadata(&runtime.base).expect("runtime owner").uid();
        let (server, mut client) = UnixStream::pair().expect("socket pair");
        let emitted = Mutex::new(Vec::new());
        let capture = |event| {
            emitted.lock().expect("emitted lock").push(event);
        };
        let serve = serve_client(server, owner_uid, &capture);
        let write = async {
            client
                .write_all(
                    b"{\"operation\":\"cancel\",\"schema_version\":1,\"id\":\"timer.synthetic\"}\n",
                )
                .await
                .expect("valid frame");
            let mut acknowledgement = [0_u8; ACTIVITY_ACKNOWLEDGEMENT.len()];
            client
                .read_exact(&mut acknowledgement)
                .await
                .expect("acknowledgement");
            assert_eq!(acknowledgement, *ACTIVITY_ACKNOWLEDGEMENT);
            client.shutdown().await.expect("client shutdown");
        };
        let (result, ()) = tokio::join!(serve, write);
        result.expect("authenticated client");
        assert_eq!(emitted.lock().expect("emitted lock").len(), 1);
    }

    #[tokio::test]
    async fn synchronous_client_validates_sends_and_waits_for_acknowledgement() {
        let runtime = SyntheticRuntime::new();
        let endpoint = ActivityEndpoint::bind(&runtime.paths)
            .await
            .expect("bound endpoint");
        let owner_uid = endpoint.owner_uid;
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&emitted);
        let serve = async move {
            let (stream, _address) = endpoint.listener.accept().await.expect("accepted client");
            serve_client(stream, owner_uid, &|event| {
                captured.lock().expect("emitted lock").push(event);
            })
            .await
            .expect("served client");
        };
        let paths = runtime.paths.clone();
        let client = tokio::task::spawn_blocking(move || {
            let event = ActivityEvent::cancel("timer.synthetic").expect("cancel event");
            send_event(&paths, &event)
        });
        let ((), client_result) = tokio::join!(serve, client);
        client_result
            .expect("client task")
            .expect("acknowledged event");
        assert_eq!(emitted.lock().expect("emitted lock").len(), 1);
    }

    #[tokio::test]
    async fn unknown_protocol_version_disconnects_without_emission() {
        let runtime = SyntheticRuntime::new();
        let owner_uid = fs::metadata(&runtime.base).expect("runtime owner").uid();
        let (server, mut client) = UnixStream::pair().expect("socket pair");
        let emitted = Mutex::new(Vec::new());
        let capture = |event| {
            emitted.lock().expect("emitted lock").push(event);
        };
        let serve = serve_client(server, owner_uid, &capture);
        let write = async {
            client
                .write_all(
                    b"{\"operation\":\"cancel\",\"schema_version\":2,\"id\":\"timer.synthetic\"}\n",
                )
                .await
                .expect("unsupported frame");
        };
        let (result, ()) = tokio::join!(serve, write);
        assert!(matches!(
            result,
            Err(ActivityTransportError::Protocol(
                ActivityProtocolError::UnsupportedVersion
            ))
        ));
        assert!(emitted.lock().expect("emitted lock").is_empty());
    }
}
