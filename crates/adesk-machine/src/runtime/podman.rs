//! Rootless Podman CLI container backend.
//!
//! `PodmanRuntime` is the first real [`ContainerRuntime`] backend
//! (`docs/machine.md` §2): it drives rootless Podman through the `podman`
//! command line. The program path is configurable
//! ([`PodmanRuntime::with_program`]) so the backend can be exercised against a
//! stub; it is never required for the crate's own tests.
//!
//! # Commands
//!
//! - `create` — `podman create <argv>` where `<argv>` is [`build_create_argv`].
//!   The new container id is read from stdout.
//! - `start` — `podman start <id>`.
//! - `stop` — `podman stop --time <seconds> <id>`; the graceful timeout is
//!   rounded up to whole seconds (`--time 0` disables it).
//! - `remove` — `podman rm [--force] <id>`.
//! - `status` — `podman inspect <id>`, parsed (see below).
//! - `list` — `podman ps -a --quiet` for the container ids, then `podman inspect
//!   <id>` for each, so `list` reports the same shape as `status`.
//!
//! Every non-zero exit becomes [`MachineError::Backend`] carrying the program,
//! the exit status and the captured stderr; the backend never panics on an
//! ordinary request path.
//!
//! # `podman inspect` payload
//!
//! `podman inspect <id>` prints a JSON array with one object per container.
//! Only the fields the backend reads are significant:
//!
//! ```json
//! [
//!   {
//!     "Id": "3f1c9f…",
//!     "Name": "adesk",
//!     "Image": "ghcr.io/adesk/machine:latest",
//!     "Created": "2024-06-01T12:00:00.500000000Z",
//!     "State": {
//!       "Status": "running",
//!       "Pid": 4242,
//!       "ExitCode": 0
//!     }
//!   }
//! ]
//! ```
//!
//! - `Id` → [`MachineId`], `Name` → [`MachineName`], `Image` → the machine image.
//! - `State.Status` is mapped onto [`MachineState`]: `created`/`configured` →
//!   `Created`, `running`/`paused` → `Running`, `stopped` → `Stopped`, `exited` →
//!   `Exited { code }` using `State.ExitCode`, and `dead` or any unknown status →
//!   `Failed { message }`. `State.Pid` (0 means "not running") becomes the
//!   status pid only while the container is running.
//! - `Created` is an RFC 3339 timestamp converted to milliseconds since the Unix
//!   epoch. It is a wall-clock instant, not a monotonic one — the best a
//!   stateless CLI backend can report.
//! - The viewer exposure is not part of the inspect payload, so `status` and
//!   `list` report [`ViewerExposure::None`]; the manager's cached spec is the
//!   source of truth for the viewer endpoint.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;

use crate::error::{MachineError, Result};
use crate::runtime::{ContainerRuntime, RuntimeKind};
use crate::spec::{MachineSpec, NetworkMode, ViewerExposure};
use crate::state::{MachineId, MachineName, MachineState, MachineStatus};

/// A [`ContainerRuntime`] backed by the rootless `podman` command line.
#[derive(Debug, Clone)]
pub struct PodmanRuntime {
    program: PathBuf,
    env: Vec<(String, String)>,
}

impl PodmanRuntime {
    /// A backend that runs `podman` from `PATH`.
    pub fn new() -> PodmanRuntime {
        PodmanRuntime {
            program: PathBuf::from("podman"),
            env: Vec::new(),
        }
    }

    /// A backend that runs the program at `program` instead of `podman`.
    ///
    /// Used to exercise the backend against a stub program in tests.
    pub fn with_program(program: impl Into<PathBuf>) -> PodmanRuntime {
        PodmanRuntime {
            program: program.into(),
            env: Vec::new(),
        }
    }

    /// Adds an environment variable for the spawned `podman` process (builder
    /// style; a later call with the same key overrides an earlier one).
    pub fn with_binary_env(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> PodmanRuntime {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Runs `podman <args>` and returns its stdout, or a [`MachineError`].
    ///
    /// A non-zero exit becomes [`MachineError::Backend`] (with the captured
    /// stderr); a process that cannot be spawned becomes [`MachineError::Spawn`].
    async fn run(&self, args: &[String]) -> Result<String> {
        let mut command = Command::new(&self.program);
        command.args(args);
        for (key, value) in &self.env {
            command.env(key, value);
        }
        tracing::debug!(
            program = %self.program.display(),
            subcommand = %args.first().map(String::as_str).unwrap_or(""),
            "running podman command"
        );

        let output = command.output().await.map_err(MachineError::Spawn)?;
        if !output.status.success() {
            return Err(MachineError::Backend {
                program: self.program.display().to_string(),
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl Default for PodmanRuntime {
    fn default() -> PodmanRuntime {
        PodmanRuntime::new()
    }
}

/// Builds the arguments passed to the `podman` binary to create a machine.
///
/// The returned vector starts with `"create"` and translates the full
/// [`MachineSpec`]: `--name`, one `--label K=V` per label, one `--env K=V` per
/// environment entry, one `--volume host:container[:ro]` per mount, `--memory
/// <mb>m` / `--cpus <cores>` when set, the network mode (`none` → `--network
/// none`, `host` → `--network host`, `private` → no flag, i.e. Podman's default
/// isolated network), the viewer exposure ([`ViewerExposure::UnixSocket`] → a
/// bind-mount `--volume`, [`ViewerExposure::TcpPort`] → `--publish
/// host:container`, [`ViewerExposure::None`] → nothing), then the image and the
/// container command.
///
/// This is a pure function: it spawns no process and performs no I/O.
pub fn build_create_argv(spec: &MachineSpec) -> Vec<String> {
    let mut args = vec!["create".to_owned()];

    args.push("--name".to_owned());
    args.push(spec.name.to_string());

    for (key, value) in &spec.labels {
        args.push("--label".to_owned());
        args.push(format!("{key}={value}"));
    }

    for (key, value) in &spec.env {
        args.push("--env".to_owned());
        args.push(format!("{key}={value}"));
    }

    for mount in &spec.mounts {
        args.push("--volume".to_owned());
        args.push(volume_argument(
            &mount.host_path,
            &mount.container_path,
            mount.read_only,
        ));
    }

    if let Some(memory_mb) = spec.memory_mb {
        args.push("--memory".to_owned());
        args.push(format!("{memory_mb}m"));
    }

    if let Some(cpus) = spec.cpus {
        args.push("--cpus".to_owned());
        args.push(format!("{cpus}"));
    }

    match spec.network {
        NetworkMode::None => {
            args.push("--network".to_owned());
            args.push("none".to_owned());
        }
        NetworkMode::Host => {
            args.push("--network".to_owned());
            args.push("host".to_owned());
        }
        // Podman's default is already an isolated per-container network.
        NetworkMode::Private => {}
    }

    match &spec.viewer {
        ViewerExposure::UnixSocket {
            host_path,
            container_path,
        } => {
            args.push("--volume".to_owned());
            args.push(volume_argument(host_path, container_path, false));
        }
        ViewerExposure::TcpPort {
            host_port,
            container_port,
        } => {
            args.push("--publish".to_owned());
            args.push(format!("{host_port}:{container_port}"));
        }
        ViewerExposure::None => {}
    }

    args.push(spec.image.clone());
    args.extend(spec.command.iter().cloned());
    args
}

/// Renders one `--volume` value, appending `:ro` for a read-only mount.
fn volume_argument(host: &Path, container: &Path, read_only: bool) -> String {
    let host = host.display();
    let container = container.display();
    if read_only {
        format!("{host}:{container}:ro")
    } else {
        format!("{host}:{container}")
    }
}

#[async_trait]
impl ContainerRuntime for PodmanRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Podman
    }

    async fn create(&self, spec: &MachineSpec) -> Result<MachineId> {
        tracing::info!(name = %spec.name, "creating machine");
        let stdout = self.run(&build_create_argv(spec)).await?;
        let id = stdout.trim();
        if id.is_empty() {
            return Err(MachineError::Runtime {
                message: "podman create produced an empty container id".to_owned(),
            });
        }
        Ok(MachineId::from(id))
    }

    async fn start(&self, id: &MachineId) -> Result<()> {
        tracing::info!(id = %id, "starting machine");
        self.run(&["start".to_owned(), id.to_string()]).await?;
        Ok(())
    }

    async fn stop(&self, id: &MachineId, timeout_ms: u64) -> Result<()> {
        tracing::info!(id = %id, timeout_ms, "stopping machine");
        let seconds = timeout_ms.div_ceil(1000);
        self.run(&[
            "stop".to_owned(),
            "--time".to_owned(),
            seconds.to_string(),
            id.to_string(),
        ])
        .await?;
        Ok(())
    }

    async fn remove(&self, id: &MachineId, force: bool) -> Result<()> {
        tracing::info!(id = %id, force, "removing machine");
        let mut args = vec!["rm".to_owned()];
        if force {
            args.push("--force".to_owned());
        }
        args.push(id.to_string());
        self.run(&args).await?;
        Ok(())
    }

    async fn status(&self, id: &MachineId) -> Result<MachineStatus> {
        let stdout = self.run(&["inspect".to_owned(), id.to_string()]).await?;
        let mut statuses = parse_inspect(&stdout)?;
        match statuses.len() {
            1 => Ok(statuses.remove(0)),
            count => Err(MachineError::Runtime {
                message: format!("podman inspect returned {count} containers for `{id}`"),
            }),
        }
    }

    async fn list(&self) -> Result<Vec<MachineStatus>> {
        let stdout = self
            .run(&["ps".to_owned(), "-a".to_owned(), "--quiet".to_owned()])
            .await?;
        let mut statuses = Vec::new();
        for id in stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let payload = self.run(&["inspect".to_owned(), id.to_owned()]).await?;
            statuses.extend(parse_inspect(&payload)?);
        }
        Ok(statuses)
    }
}

/// One container object in a `podman inspect` payload (only the fields read).
#[derive(Debug, Deserialize)]
struct InspectContainer {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Image")]
    image: String,
    #[serde(rename = "Created", default)]
    created: Option<String>,
    #[serde(rename = "State")]
    state: InspectState,
}

/// The `State` object of a `podman inspect` payload.
#[derive(Debug, Deserialize)]
struct InspectState {
    #[serde(rename = "Status")]
    status: String,
    #[serde(rename = "Pid", default)]
    pid: Option<u32>,
    #[serde(rename = "ExitCode", default)]
    exit_code: Option<i32>,
}

impl InspectContainer {
    /// Converts one parsed container into a [`MachineStatus`].
    fn into_status(self) -> Result<MachineStatus> {
        let state = map_state(&self.state);
        let created_at_ms = match self.created.as_deref() {
            Some(created) => parse_rfc3339_millis(created)?,
            None => 0,
        };
        let pid = if state.is_running() {
            self.state.pid.filter(|&pid| pid != 0)
        } else {
            None
        };

        Ok(MachineStatus {
            id: MachineId::from(self.id),
            name: MachineName::from(self.name),
            image: self.image,
            state,
            pid,
            created_at_ms,
            viewer: ViewerExposure::None,
        })
    }
}

/// Parses a `podman inspect` payload into machine statuses.
fn parse_inspect(json: &str) -> Result<Vec<MachineStatus>> {
    let containers: Vec<InspectContainer> =
        serde_json::from_str(json).map_err(|error| MachineError::Runtime {
            message: format!("invalid `podman inspect` payload: {error}"),
        })?;
    containers
        .into_iter()
        .map(InspectContainer::into_status)
        .collect()
}

/// Maps a Podman `State` object onto a [`MachineState`].
fn map_state(state: &InspectState) -> MachineState {
    match state.status.as_str() {
        "created" | "configured" => MachineState::Created,
        "running" | "paused" => MachineState::Running,
        "stopped" => MachineState::Stopped,
        "exited" => MachineState::Exited {
            code: state.exit_code.unwrap_or(0),
        },
        "dead" => MachineState::Failed {
            message: "container is dead".to_owned(),
        },
        other => MachineState::Failed {
            message: format!("unknown podman state `{other}`"),
        },
    }
}

/// Converts an RFC 3339 timestamp (as Podman emits in `Created`) into
/// milliseconds since the Unix epoch.
///
/// Accepts `YYYY-MM-DDTHH:MM:SS[.fraction][Z|±HH:MM]`; sub-millisecond digits
/// are truncated. Anything else is a [`MachineError::Runtime`].
fn parse_rfc3339_millis(input: &str) -> Result<u64> {
    let timestamp = input.trim();
    let invalid = || MachineError::Runtime {
        message: format!("invalid `Created` timestamp `{input}`"),
    };

    // Everything below indexes by byte, so require pure ASCII up front and the
    // shortest legal form (`YYYY-MM-DDTHH:MM:SS`, 19 bytes).
    if !timestamp.is_ascii() || timestamp.len() < 19 {
        return Err(invalid());
    }
    let bytes = timestamp.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || (bytes[10] != b'T' && bytes[10] != b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return Err(invalid());
    }

    let year: i64 = timestamp[0..4].parse().map_err(|_| invalid())?;
    let month: u32 = timestamp[5..7].parse().map_err(|_| invalid())?;
    let day: u32 = timestamp[8..10].parse().map_err(|_| invalid())?;
    let hour: i64 = timestamp[11..13].parse().map_err(|_| invalid())?;
    let minute: i64 = timestamp[14..16].parse().map_err(|_| invalid())?;
    let second: i64 = timestamp[17..19].parse().map_err(|_| invalid())?;

    let mut rest = &timestamp[19..];
    let mut millis: u64 = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits: String = fraction.chars().take_while(char::is_ascii_digit).collect();
        rest = &fraction[digits.len()..];
        let mut padded = digits;
        while padded.len() < 3 {
            padded.push('0');
        }
        millis = padded[..3].parse().map_err(|_| invalid())?;
    }

    let offset_seconds: i64 = match rest.as_bytes().first() {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            if rest.len() != 6 || rest.as_bytes()[3] != b':' {
                return Err(invalid());
            }
            let hours: i64 = rest[1..3].parse().map_err(|_| invalid())?;
            let minutes: i64 = rest[4..6].parse().map_err(|_| invalid())?;
            let seconds = hours * 3600 + minutes * 60;
            if *sign == b'-' {
                -seconds
            } else {
                seconds
            }
        }
        _ => return Err(invalid()),
    };

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds;
    if seconds < 0 {
        return Err(invalid());
    }
    Ok(seconds as u64 * 1000 + millis)
}

/// Days between the civil date `(year, month, day)` and 1970-01-01 (the classic
/// Howard Hinnant algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = if month > 2 { month - 3 } else { month + 9 } as i64;
    let day_of_year = (153 * month_prime + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Mount;
    use std::collections::BTreeMap;

    /// An argv with nothing but the required name/image.
    fn base_spec() -> MachineSpec {
        MachineSpec {
            name: MachineName::from("adesk"),
            image: "img:latest".to_owned(),
            command: vec!["adesk-server".to_owned()],
            env: BTreeMap::new(),
            mounts: Vec::new(),
            memory_mb: None,
            cpus: None,
            network: NetworkMode::None,
            viewer: ViewerExposure::None,
            labels: BTreeMap::new(),
        }
    }

    /// Whether `flag value` appears as an adjacent pair in `argv`.
    fn has_arg_pair(argv: &[String], flag: &str, value: &str) -> bool {
        argv.windows(2)
            .any(|pair| pair[0] == flag && pair[1] == value)
    }

    #[test]
    fn default_spec_argv() {
        assert_eq!(
            build_create_argv(&MachineSpec::default()),
            vec![
                "create",
                "--name",
                "adesk",
                "--label",
                "adesk.io/role=machine",
                "--memory",
                "4096m",
                "--cpus",
                "2",
                "--volume",
                "/run/adesk/viewer.sock:/run/adesk/viewer.sock",
                "ghcr.io/adesk/machine:latest",
                "adesk-server",
            ]
        );
    }

    #[test]
    fn full_spec_translation() {
        let spec = MachineSpec {
            name: MachineName::from("work"),
            image: "debian:trixie".to_owned(),
            command: vec!["bash".to_owned(), "-l".to_owned()],
            env: BTreeMap::from([("LANG".to_owned(), "C.UTF-8".to_owned())]),
            mounts: vec![
                Mount::ro("/etc/localtime", "/etc/localtime"),
                Mount::rw("/srv/share", "/mnt/share"),
            ],
            memory_mb: Some(2048),
            cpus: Some(1.5),
            network: NetworkMode::Host,
            viewer: ViewerExposure::TcpPort {
                host_port: 7000,
                container_port: 7100,
            },
            labels: BTreeMap::from([
                ("adesk.io/role".to_owned(), "machine".to_owned()),
                ("team".to_owned(), "agent".to_owned()),
            ]),
        };

        assert_eq!(
            build_create_argv(&spec),
            vec![
                "create",
                "--name",
                "work",
                "--label",
                "adesk.io/role=machine",
                "--label",
                "team=agent",
                "--env",
                "LANG=C.UTF-8",
                "--volume",
                "/etc/localtime:/etc/localtime:ro",
                "--volume",
                "/srv/share:/mnt/share",
                "--memory",
                "2048m",
                "--cpus",
                "1.5",
                "--network",
                "host",
                "--publish",
                "7000:7100",
                "debian:trixie",
                "bash",
                "-l",
            ]
        );
    }

    #[test]
    fn network_modes_map_to_podman() {
        let none = base_spec().with_network(NetworkMode::None);
        assert!(has_arg_pair(&build_create_argv(&none), "--network", "none"));

        let host = base_spec().with_network(NetworkMode::Host);
        assert!(has_arg_pair(&build_create_argv(&host), "--network", "host"));

        // Private uses Podman's default isolated network: no `--network` flag.
        let private = base_spec().with_network(NetworkMode::Private);
        assert!(!build_create_argv(&private)
            .iter()
            .any(|arg| arg == "--network"));
    }

    #[test]
    fn viewer_exposures_map_to_container_options() {
        let unix =
            base_spec().with_viewer(ViewerExposure::default_unix("/run/v.sock", "/run/v.sock"));
        assert!(has_arg_pair(
            &build_create_argv(&unix),
            "--volume",
            "/run/v.sock:/run/v.sock"
        ));

        let tcp = base_spec().with_viewer(ViewerExposure::TcpPort {
            host_port: 1,
            container_port: 2,
        });
        assert!(has_arg_pair(&build_create_argv(&tcp), "--publish", "1:2"));

        let argv = build_create_argv(&base_spec());
        assert!(!argv
            .iter()
            .any(|arg| arg == "--publish" || arg == "--volume"));
    }

    #[test]
    fn inspect_payload_parses_running_container() {
        let json = r#"[
            {
                "Id": "abc123",
                "Name": "adesk",
                "Image": "ghcr.io/adesk/machine:latest",
                "Created": "2024-06-01T12:00:00.500Z",
                "State": { "Status": "running", "Pid": 4242, "ExitCode": 0 }
            }
        ]"#;
        let statuses = parse_inspect(json).unwrap();
        assert_eq!(statuses.len(), 1);
        let status = &statuses[0];
        assert_eq!(status.id, MachineId::from("abc123"));
        assert_eq!(status.name, MachineName::from("adesk"));
        assert_eq!(status.image, "ghcr.io/adesk/machine:latest");
        assert_eq!(status.state, MachineState::Running);
        assert_eq!(status.pid, Some(4242));
        assert_eq!(status.created_at_ms, 1_717_243_200_500);
        assert_eq!(status.viewer, ViewerExposure::None);
    }

    #[test]
    fn exited_state_uses_available_exit_code() {
        let json = r#"[{"Id":"c1","Name":"n","Image":"img","Created":"2024-06-01T12:00:00Z","State":{"Status":"exited","Pid":0,"ExitCode":137}}]"#;
        let status = parse_inspect(json).unwrap().pop().unwrap();
        assert_eq!(status.state, MachineState::Exited { code: 137 });
        assert_eq!(status.pid, None);
    }

    #[test]
    fn state_strings_map_to_machine_state() {
        let cases = [
            ("configured", MachineState::Created),
            ("created", MachineState::Created),
            ("running", MachineState::Running),
            ("paused", MachineState::Running),
            ("stopped", MachineState::Stopped),
            ("exited", MachineState::Exited { code: 137 }),
            (
                "dead",
                MachineState::Failed {
                    message: "container is dead".to_owned(),
                },
            ),
        ];
        for (podman, expected) in cases {
            let state = InspectState {
                status: podman.to_owned(),
                pid: Some(0),
                exit_code: Some(137),
            };
            assert_eq!(map_state(&state), expected, "podman state {podman}");
        }

        let unknown = InspectState {
            status: "weird".to_owned(),
            pid: None,
            exit_code: None,
        };
        assert!(matches!(
            map_state(&unknown),
            MachineState::Failed { message } if message.contains("weird")
        ));
    }

    #[test]
    fn invalid_inspect_payload_is_a_runtime_error() {
        let err = parse_inspect("not json").unwrap_err();
        assert!(matches!(err, MachineError::Runtime { .. }));
        // An empty array is valid JSON; `status` rejects it at its call site.
        assert!(parse_inspect("[]").unwrap().is_empty());
    }

    #[test]
    fn rfc3339_timestamps_convert_to_epoch_millis() {
        assert_eq!(
            parse_rfc3339_millis("2024-06-01T00:00:00Z").unwrap(),
            1_717_200_000_000
        );
        assert_eq!(
            parse_rfc3339_millis("2024-06-01T12:00:00.250Z").unwrap(),
            1_717_243_200_250
        );
        // A numeric offset denotes the same instant as the equivalent UTC time.
        assert_eq!(
            parse_rfc3339_millis("2024-06-01T14:00:00+02:00").unwrap(),
            1_717_243_200_000
        );
        // Nanosecond precision is truncated to milliseconds.
        assert_eq!(
            parse_rfc3339_millis("2024-06-01T12:00:00.123456789Z").unwrap(),
            1_717_243_200_123
        );
    }

    #[test]
    fn invalid_timestamps_are_rejected() {
        for bad in [
            "",
            "not-a-time",
            "2024-06-01",
            "2024/06/01T12:00:00Z",
            "2024-06-01T12:00:00+2",
        ] {
            assert!(
                matches!(
                    parse_rfc3339_millis(bad).unwrap_err(),
                    MachineError::Runtime { .. }
                ),
                "accepted invalid timestamp {bad:?}"
            );
        }
    }

    /// Serializes the tests that spawn a child process.
    ///
    /// A `fork` in one test can inherit another test's still-open write
    /// descriptor to a stub script, which makes a concurrent `exec` of that
    /// script fail with `ETXTBSY` ("Text file busy"). Holding this lock across
    /// both stub creation and spawning removes the race.
    static SPAWN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[cfg(unix)]
    fn stub(script: &str) -> (tempfile::TempDir, PathBuf) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stub-podman");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(script.as_bytes()).unwrap();
        drop(file);
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        (dir, path)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stub_backend_parses_create_id_and_starts() {
        let _spawn = SPAWN_LOCK.lock().await;
        let (_dir, program) = stub(
            "#!/bin/sh\ncase \"$1\" in\n  create) echo deadbeef ;;\n  start) exit 0 ;;\n  *) exit 2 ;;\nesac\n",
        );
        let runtime = PodmanRuntime::with_program(program);
        let id = runtime.create(&MachineSpec::default()).await.unwrap();
        assert_eq!(id, MachineId::from("deadbeef"));
        runtime.start(&id).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stub_non_zero_exit_maps_to_backend_error() {
        let _spawn = SPAWN_LOCK.lock().await;
        let (_dir, program) = stub("#!/bin/sh\necho boom 1>&2\nexit 125\n");
        let runtime = PodmanRuntime::with_program(program).with_binary_env("ADESK_TEST", "1");
        let err = runtime
            .start(&MachineId::from("machine-1"))
            .await
            .unwrap_err();
        match err {
            MachineError::Backend {
                program,
                status,
                stderr,
            } => {
                assert_eq!(status, 125);
                assert_eq!(stderr, "boom");
                assert!(program.ends_with("stub-podman"));
            }
            other => panic!("expected a backend error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_program_maps_to_spawn_error() {
        let _spawn = SPAWN_LOCK.lock().await;
        let runtime = PodmanRuntime::with_program("/nonexistent/definitely-not-podman");
        let err = runtime.create(&MachineSpec::default()).await.unwrap_err();
        assert!(matches!(err, MachineError::Spawn(_)));
    }
}
