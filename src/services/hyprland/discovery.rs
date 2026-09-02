//! Hyprland instance discovery.
//!
//! The compositor advertises one instance per `HYPRLAND_INSTANCE_SIGNATURE`
//! (HIS) under `$XDG_RUNTIME_DIR/hypr/<HIS>/` with a request socket
//! `.socket.sock` and an event socket `.socket2.sock` (primary source: the
//! Hyprland IPC documentation and `src/Compositor.cpp`, which fixes the layout
//! and the `{hash}_{timestamp}_{random}` signature format).
//!
//! The environment signature is trusted only for the first connection. Every
//! reconnect rescans the runtime directory so a compositor restart that mints a
//! fresh signature does not strand the adapter on a dead socket path. Discovery
//! never logs paths, signatures, PIDs, or the Wayland display; all `Debug`
//! output is redacted and selection results are opaque `SocketPaths`.
//!
//! Ranking is a best-effort ordering only. The authoritative liveness proof is
//! a successful event-socket connect plus a complete request snapshot, applied
//! by the caller; a stale directory that ranks first is simply skipped when it
//! fails to answer. PID and Wayland-display association are parsed defensively
//! from version-dependent files and degrade to "unknown" rather than excluding
//! an otherwise valid instance.

use std::cmp::Ordering;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use thiserror::Error;

/// Maximum instance directories examined in one scan.
pub const MAX_SCANNED_INSTANCES: usize = 64;
/// Maximum bytes read from an instance metadata file while parsing a PID or
/// Wayland-display association.
pub const MAX_METADATA_BYTES: usize = 4096;

/// Environment used to resolve one Hyprland instance without logging paths.
#[derive(Clone, Eq, PartialEq)]
pub struct DiscoveryEnvironment {
    /// Absolute XDG runtime base.
    pub runtime_dir: Option<PathBuf>,
    /// Hyprland instance directory leaf.
    pub instance_signature: Option<OsString>,
}

impl fmt::Debug for DiscoveryEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiscoveryEnvironment")
            .field(
                "runtime_dir",
                &self.runtime_dir.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "instance_signature",
                &self.instance_signature.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl DiscoveryEnvironment {
    /// Read the current process environment without exposing values.
    #[must_use]
    pub fn discover() -> Self {
        Self {
            runtime_dir: env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            instance_signature: env::var_os("HYPRLAND_INSTANCE_SIGNATURE"),
        }
    }
}

/// Resolved request and event sockets. Debug formatting is always redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct SocketPaths {
    request: PathBuf,
    events: PathBuf,
}

impl fmt::Debug for SocketPaths {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SocketPaths")
            .field("request", &"<redacted>")
            .field("events", &"<redacted>")
            .finish()
    }
}

impl SocketPaths {
    /// Re-resolve both sockets from the current process environment.
    pub fn discover() -> Result<Self, DiscoveryError> {
        Self::from_environment(&DiscoveryEnvironment::discover())
    }

    /// Resolve both sockets from explicit, testable values.
    pub fn from_environment(environment: &DiscoveryEnvironment) -> Result<Self, DiscoveryError> {
        let runtime = environment
            .runtime_dir
            .as_deref()
            .ok_or(DiscoveryError::MissingRuntimeDirectory)?;
        if !runtime.is_absolute() {
            return Err(DiscoveryError::RelativeRuntimeDirectory);
        }
        let signature = environment
            .instance_signature
            .as_deref()
            .ok_or(DiscoveryError::MissingInstanceSignature)?;
        let signature = signature
            .to_str()
            .filter(|signature| valid_signature(signature))
            .ok_or(DiscoveryError::InvalidInstanceSignature)?;
        Ok(Self::for_instance(&runtime.join("hypr"), signature))
    }

    /// Build socket paths for a validated signature under an instance root.
    fn for_instance(instance_root: &Path, signature: &str) -> Self {
        let instance = instance_root.join(signature);
        Self {
            request: instance.join(".socket.sock"),
            events: instance.join(".socket2.sock"),
        }
    }

    /// Request socket, exposed only for direct transport tests.
    #[must_use]
    pub fn request(&self) -> &Path {
        &self.request
    }

    /// Event socket, exposed only for direct transport tests.
    #[must_use]
    pub fn events(&self) -> &Path {
        &self.events
    }
}

/// Accept only a safe single-segment instance signature.
#[must_use]
pub fn valid_signature(signature: &str) -> bool {
    !signature.is_empty()
        && signature.len() <= 128
        && signature != "."
        && signature != ".."
        && signature
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// Public-safe discovery failure.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum DiscoveryError {
    /// XDG_RUNTIME_DIR was absent.
    #[error("the user runtime directory is unavailable")]
    MissingRuntimeDirectory,
    /// The runtime directory was not absolute.
    #[error("the user runtime directory is invalid")]
    RelativeRuntimeDirectory,
    /// No active Hyprland instance was advertised.
    #[error("no Hyprland instance is available")]
    MissingInstanceSignature,
    /// The instance signature was not a safe directory leaf.
    #[error("the Hyprland instance identifier is invalid")]
    InvalidInstanceSignature,
    /// The scan found no candidate instance directory that passed validation.
    #[error("no live Hyprland instance was found")]
    NoLiveInstance,
    /// The current process owner could not be determined for validation.
    #[error("the process identity is unavailable")]
    OwnerUnavailable,
}

/// Liveness of a candidate compositor process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessLiveness {
    /// The process exists.
    Alive,
    /// The process is gone (`ESRCH`).
    Dead,
    /// Liveness could not be determined; do not exclude on this alone.
    Unknown,
}

/// Abstract process-liveness probe so scanning is testable without real PIDs.
pub trait ProcessProbe {
    /// Report whether `pid` currently names a live process.
    fn liveness(&self, pid: i32) -> ProcessLiveness;
}

/// Real probe backed by `/proc/<pid>` existence, consistent with the rest of
/// the codebase's `/proc` identity checks and avoiding a new dependency.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcProbe;

impl ProcessProbe for ProcProbe {
    fn liveness(&self, pid: i32) -> ProcessLiveness {
        if pid <= 0 {
            return ProcessLiveness::Unknown;
        }
        match fs::symlink_metadata(format!("/proc/{pid}")) {
            Ok(_) => ProcessLiveness::Alive,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => ProcessLiveness::Dead,
            Err(_) => ProcessLiveness::Unknown,
        }
    }
}

/// Injectable inputs for scanning the Hyprland runtime directory. Tests build a
/// synthetic tree and point `runtime_dir` at it.
#[derive(Clone, Eq, PartialEq)]
pub struct InstanceScan {
    /// Absolute XDG runtime base whose `hypr` child holds instance directories.
    pub runtime_dir: PathBuf,
    /// Current Wayland display leaf, used only for affinity ranking.
    pub wayland_display: Option<OsString>,
    /// Environment signature; promoted first only for the initial connection.
    pub environment_signature: Option<OsString>,
}

impl fmt::Debug for InstanceScan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstanceScan")
            .field("runtime_dir", &"<redacted>")
            .field(
                "wayland_display",
                &self.wayland_display.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "environment_signature",
                &self.environment_signature.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl InstanceScan {
    /// Read the scan inputs from the current process environment.
    pub fn from_environment() -> Result<Self, DiscoveryError> {
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or(DiscoveryError::MissingRuntimeDirectory)?;
        if !runtime_dir.is_absolute() {
            return Err(DiscoveryError::RelativeRuntimeDirectory);
        }
        Ok(Self {
            runtime_dir,
            wayland_display: env::var_os("WAYLAND_DISPLAY"),
            environment_signature: env::var_os("HYPRLAND_INSTANCE_SIGNATURE"),
        })
    }
}

/// Determine the current process owner via `/proc/self`, matching the identity
/// checks used by the local activity endpoint.
#[must_use]
pub fn current_uid() -> Option<u32> {
    fs::metadata("/proc/self")
        .ok()
        .map(|metadata| metadata.uid())
}

/// Ordering keys for one validated instance. Higher tuples rank first.
#[derive(Clone, Eq, PartialEq)]
struct InstanceCandidate {
    signature: String,
    sockets: SocketPaths,
    display_affinity: bool,
    signature_timestamp: Option<u64>,
    recency: Option<SystemTime>,
}

impl InstanceCandidate {
    fn rank_key(&self) -> (bool, u64, u128) {
        (
            self.display_affinity,
            self.signature_timestamp.unwrap_or(0),
            self.recency
                .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_nanos()),
        )
    }

    fn cmp_rank(&self, other: &Self) -> Ordering {
        // Best (largest) key first; break ties lexically by signature so the
        // order is deterministic across runs.
        other
            .rank_key()
            .cmp(&self.rank_key())
            .then_with(|| self.signature.cmp(&other.signature))
    }
}

/// Scan the runtime directory for validated, live-or-unknown Hyprland instances
/// and return their sockets in deterministic rank order (best first).
///
/// Validation rejects symlinked or non-owned directories and requires both
/// sockets to be owned, non-symlink Unix sockets. Instances whose parsed PID is
/// definitively dead are excluded; an unparseable or unknown PID is retained so
/// the connect-plus-snapshot gate makes the final decision.
pub fn scan_instances(
    scan: &InstanceScan,
    trusted_uid: u32,
    prefer_environment: bool,
    probe: &dyn ProcessProbe,
) -> Result<Vec<SocketPaths>, DiscoveryError> {
    if !scan.runtime_dir.is_absolute() {
        return Err(DiscoveryError::RelativeRuntimeDirectory);
    }
    let instance_root = scan.runtime_dir.join("hypr");
    let entries = match fs::read_dir(&instance_root) {
        Ok(entries) => entries,
        Err(_) => return Err(DiscoveryError::NoLiveInstance),
    };

    let display = scan
        .wayland_display
        .as_ref()
        .and_then(|display| display.to_str())
        .map(str::to_owned);

    let mut candidates: Vec<InstanceCandidate> = Vec::new();
    for entry in entries.take(MAX_SCANNED_INSTANCES).flatten() {
        let Some(signature) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !valid_signature(&signature) {
            continue;
        }
        let directory = instance_root.join(&signature);
        let Some(sockets) = validate_instance(&directory, &signature, &instance_root, trusted_uid)
        else {
            continue;
        };
        let pid = read_instance_pid(&directory);
        if matches!(
            pid.map(|pid| probe.liveness(pid)),
            Some(ProcessLiveness::Dead)
        ) {
            continue;
        }
        candidates.push(InstanceCandidate {
            display_affinity: display
                .as_deref()
                .is_some_and(|display| instance_matches_display(&directory, display)),
            signature_timestamp: signature_timestamp(&signature),
            recency: recency(&directory),
            signature,
            sockets,
        });
    }

    candidates.sort_by(InstanceCandidate::cmp_rank);

    let mut ordered: Vec<SocketPaths> = Vec::with_capacity(candidates.len());
    if prefer_environment && let Some(index) = environment_index(scan, &candidates) {
        ordered.push(candidates.remove(index).sockets);
    }
    ordered.extend(candidates.into_iter().map(|candidate| candidate.sockets));

    if ordered.is_empty() {
        Err(DiscoveryError::NoLiveInstance)
    } else {
        Ok(ordered)
    }
}

/// Index of the environment-signature candidate, if it is present and valid.
fn environment_index(scan: &InstanceScan, candidates: &[InstanceCandidate]) -> Option<usize> {
    let signature = scan
        .environment_signature
        .as_ref()
        .and_then(|signature| signature.to_str())
        .filter(|signature| valid_signature(signature))?;
    candidates
        .iter()
        .position(|candidate| candidate.signature == signature)
}

/// Validate one instance directory and both sockets against the trusted owner.
fn validate_instance(
    directory: &Path,
    signature: &str,
    instance_root: &Path,
    trusted_uid: u32,
) -> Option<SocketPaths> {
    let directory_metadata = fs::symlink_metadata(directory).ok()?;
    if directory_metadata.file_type().is_symlink()
        || !directory_metadata.is_dir()
        || directory_metadata.uid() != trusted_uid
    {
        return None;
    }
    let sockets = SocketPaths::for_instance(instance_root, signature);
    if !socket_is_trusted(sockets.request(), trusted_uid)
        || !socket_is_trusted(sockets.events(), trusted_uid)
    {
        return None;
    }
    Some(sockets)
}

/// A trusted socket is an owned, non-symlink Unix socket.
fn socket_is_trusted(path: &Path, trusted_uid: u32) -> bool {
    use std::os::unix::fs::FileTypeExt;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            !metadata.file_type().is_symlink()
                && metadata.file_type().is_socket()
                && metadata.uid() == trusted_uid
        }
        Err(_) => false,
    }
}

/// Parse the trailing timestamp field of a `{hash}_{timestamp}_{random}` or
/// `{hash}_{timestamp}` signature. Larger means a more recent compositor start.
fn signature_timestamp(signature: &str) -> Option<u64> {
    let mut fields = signature.split('_');
    let _hash = fields.next()?;
    fields.next()?.parse::<u64>().ok()
}

/// Modification time used as a recency tiebreak, preferring an instance lock and
/// falling back to the directory. Never fails the candidate on absence.
fn recency(directory: &Path) -> Option<SystemTime> {
    fs::symlink_metadata(directory.join("hyprland.lock"))
        .or_else(|_| fs::symlink_metadata(directory))
        .ok()
        .and_then(|metadata| metadata.modified().ok())
}

/// Best-effort PID extraction from version-dependent instance files. Returns
/// `None` (treated as unknown liveness) when no PID can be parsed.
fn read_instance_pid(directory: &Path) -> Option<i32> {
    if let Some(pid) = read_bounded(&directory.join("hyprland.lock"))
        .and_then(|contents| contents.lines().next().and_then(parse_pid_line))
    {
        return Some(pid);
    }
    let log = read_bounded(&directory.join("hyprland.log"))?;
    log.lines().find_map(|line| {
        line.split_once("PID:")
            .and_then(|(_, rest)| parse_pid_line(rest))
    })
}

/// Whether the instance log positively references the current Wayland display.
fn instance_matches_display(directory: &Path, display: &str) -> bool {
    read_bounded(&directory.join("hyprland.log")).is_some_and(|contents| {
        contents
            .split(|c: char| !is_display_char(c))
            .any(|token| token == display)
    })
}

fn is_display_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
}

fn parse_pid_line(line: &str) -> Option<i32> {
    let pid = line.trim().parse::<i32>().ok()?;
    (pid > 0).then_some(pid)
}

/// Read at most `MAX_METADATA_BYTES` bytes of a regular file as UTF-8, ignoring
/// non-UTF-8 tails. Symlinks and non-regular files are rejected.
fn read_bounded(path: &Path) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    let contents = fs::read(path).ok()?;
    let end = contents.len().min(MAX_METADATA_BYTES);
    Some(String::from_utf8_lossy(&contents[..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};

    /// Removes a scratch tree on drop so tests leave no runtime residue.
    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("wftw-hypr-{}-{unique}", std::process::id()));
            fs::create_dir_all(root.join("hypr")).expect("create scratch hypr root");
            Self { root }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct FakeProbe {
        alive: HashSet<i32>,
        dead: HashSet<i32>,
    }

    impl ProcessProbe for FakeProbe {
        fn liveness(&self, pid: i32) -> ProcessLiveness {
            if self.dead.contains(&pid) {
                ProcessLiveness::Dead
            } else if self.alive.contains(&pid) {
                ProcessLiveness::Alive
            } else {
                ProcessLiveness::Unknown
            }
        }
    }

    /// Holds bound sockets alive for the duration of a test. Dropping frees them.
    struct Instance {
        _request: Option<UnixListener>,
        _events: Option<UnixListener>,
    }

    #[allow(clippy::too_many_arguments)]
    fn make_instance(
        scratch: &Scratch,
        signature: &str,
        request_socket: bool,
        events_socket: bool,
        lock_pid: Option<i32>,
        log_pid: Option<i32>,
        log_display: Option<&str>,
    ) -> Instance {
        let directory = scratch.root.join("hypr").join(signature);
        fs::create_dir_all(&directory).expect("create instance dir");
        let request = request_socket
            .then(|| UnixListener::bind(directory.join(".socket.sock")).expect("bind request"));
        let events = events_socket
            .then(|| UnixListener::bind(directory.join(".socket2.sock")).expect("bind events"));
        if let Some(pid) = lock_pid {
            fs::write(directory.join("hyprland.lock"), format!("{pid}\n")).expect("write lock");
        }
        if log_pid.is_some() || log_display.is_some() {
            let mut log = String::new();
            if let Some(pid) = log_pid {
                log.push_str(&format!("[LOG] Hyprland PID: {pid}\n"));
            }
            if let Some(display) = log_display {
                log.push_str(&format!("[LOG] Wayland display: {display}\n"));
            }
            fs::write(directory.join("hyprland.log"), log).expect("write log");
        }
        Instance {
            _request: request,
            _events: events,
        }
    }

    fn scan_for(scratch: &Scratch, display: Option<&str>) -> InstanceScan {
        InstanceScan {
            runtime_dir: scratch.root.clone(),
            wayland_display: display.map(OsString::from),
            environment_signature: None,
        }
    }

    fn uid() -> u32 {
        current_uid().expect("current uid")
    }

    fn empty_probe() -> FakeProbe {
        FakeProbe {
            alive: HashSet::new(),
            dead: HashSet::new(),
        }
    }

    #[test]
    fn signature_timestamp_reads_the_second_field() {
        assert_eq!(
            signature_timestamp("hash_1728841215_1074557723"),
            Some(1728841215)
        );
        assert_eq!(signature_timestamp("hash_42"), Some(42));
        assert_eq!(signature_timestamp("hashonly"), None);
        assert_eq!(signature_timestamp("hash_notanumber"), None);
    }

    #[test]
    fn newer_signature_timestamp_ranks_first() {
        let scratch = Scratch::new();
        let _older = make_instance(&scratch, "a_100", true, true, None, None, None);
        let _newer = make_instance(&scratch, "b_200", true, true, None, None, None);
        let ordered =
            scan_instances(&scan_for(&scratch, None), uid(), false, &empty_probe()).unwrap();
        assert_eq!(ordered.len(), 2);
        assert!(ordered[0].events().ends_with("hypr/b_200/.socket2.sock"));
    }

    #[test]
    fn wayland_display_affinity_outranks_a_newer_timestamp() {
        let scratch = Scratch::new();
        let _newer = make_instance(&scratch, "a_900", true, true, None, None, None);
        let _match = make_instance(&scratch, "b_100", true, true, None, None, Some("wayland-7"));
        let ordered = scan_instances(
            &scan_for(&scratch, Some("wayland-7")),
            uid(),
            false,
            &empty_probe(),
        )
        .unwrap();
        assert!(ordered[0].events().ends_with("hypr/b_100/.socket2.sock"));
    }

    #[test]
    fn a_dead_pid_is_excluded_but_unknown_is_retained() {
        let scratch = Scratch::new();
        let _dead = make_instance(&scratch, "dead_100", true, true, Some(4242), None, None);
        let _unknown = make_instance(&scratch, "unkn_200", true, true, None, None, None);
        let probe = FakeProbe {
            alive: HashSet::new(),
            dead: HashSet::from([4242]),
        };
        let ordered = scan_instances(&scan_for(&scratch, None), uid(), false, &probe).unwrap();
        assert_eq!(ordered.len(), 1);
        assert!(ordered[0].events().ends_with("hypr/unkn_200/.socket2.sock"));
    }

    #[test]
    fn a_missing_socket_rejects_the_instance() {
        let scratch = Scratch::new();
        let _no_events = make_instance(&scratch, "a_100", true, false, None, None, None);
        let result = scan_instances(&scan_for(&scratch, None), uid(), false, &empty_probe());
        assert_eq!(result, Err(DiscoveryError::NoLiveInstance));
    }

    #[test]
    fn an_invalid_signature_directory_is_skipped() {
        let scratch = Scratch::new();
        let _valid = make_instance(&scratch, "a_100", true, true, None, None, None);
        // A space is outside the accepted signature alphabet; even with valid
        // sockets the directory must not become a candidate.
        let _invalid = make_instance(&scratch, "has space", true, true, None, None, None);
        assert!(!valid_signature("has space"));
        let ordered =
            scan_instances(&scan_for(&scratch, None), uid(), false, &empty_probe()).unwrap();
        assert_eq!(ordered.len(), 1);
        assert!(ordered[0].events().ends_with("hypr/a_100/.socket2.sock"));
    }

    #[test]
    fn a_symlinked_instance_directory_is_rejected() {
        let scratch = Scratch::new();
        let _real = make_instance(&scratch, "real_100", true, true, None, None, None);
        let target = scratch.root.join("hypr").join("real_100");
        let link = scratch.root.join("hypr").join("link_200");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let ordered =
            scan_instances(&scan_for(&scratch, None), uid(), false, &empty_probe()).unwrap();
        assert!(
            ordered
                .iter()
                .all(|paths| !paths.events().to_string_lossy().contains("link_200"))
        );
    }

    #[test]
    fn the_environment_signature_is_promoted_first_only_when_requested() {
        let scratch = Scratch::new();
        let _newer = make_instance(&scratch, "a_900", true, true, None, None, None);
        let _env = make_instance(&scratch, "b_100", true, true, None, None, None);
        let mut scan = scan_for(&scratch, None);
        scan.environment_signature = Some(OsString::from("b_100"));

        let promoted = scan_instances(&scan, uid(), true, &empty_probe()).unwrap();
        assert!(promoted[0].events().ends_with("hypr/b_100/.socket2.sock"));

        let ranked = scan_instances(&scan, uid(), false, &empty_probe()).unwrap();
        assert!(ranked[0].events().ends_with("hypr/a_900/.socket2.sock"));
    }

    #[test]
    fn a_pid_is_read_from_the_lock_then_the_log() {
        let scratch = Scratch::new();
        let _lock = make_instance(&scratch, "lock_100", true, true, Some(11), None, None);
        let _log = make_instance(&scratch, "log_100", true, true, None, Some(22), None);
        assert_eq!(
            read_instance_pid(&scratch.root.join("hypr").join("lock_100")),
            Some(11)
        );
        assert_eq!(
            read_instance_pid(&scratch.root.join("hypr").join("log_100")),
            Some(22)
        );
    }

    #[test]
    fn a_relative_runtime_directory_is_rejected() {
        let scan = InstanceScan {
            runtime_dir: PathBuf::from("relative/runtime"),
            wayland_display: None,
            environment_signature: None,
        };
        assert_eq!(
            scan_instances(&scan, 0, false, &empty_probe()),
            Err(DiscoveryError::RelativeRuntimeDirectory)
        );
    }
}
