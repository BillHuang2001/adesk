//! `adesk-machine` binary — host-side AI Machine lifecycle CLI.
//!
//! The CLI is a thin driver over the crate's host control plane
//! (`HostControlPlane` over a `MachineManager`): it parses a `MachineSpec` from
//! flags, selects the container backend (`--runtime podman|mock`) and prints a
//! human-readable summary of every lifecycle operation. All lifecycle work goes
//! through the control plane, so the host boundary from `docs/machine.md` §4
//! holds for the command line too.
//!
//! Exit codes: `0` success, `1` on a `MachineError` (printed to stderr), `2` on a
//! CLI/config error (a malformed `--mount`/`--viewer-*`/`--network` value, or a
//! `clap` usage error, which already exits `2`).

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use adesk_machine::{
    ContainerRuntime, HostCapabilities, HostControlPlane, MachineError, MachineManager,
    MachineName, MachineSpec, MachineStatus, MockRuntime, Mount, NetworkMode, PodmanRuntime,
    ViewerExposure,
};

/// Process exit code for a backend/lifecycle failure.
const EXIT_FAILURE: u8 = 1;

/// Process exit code for a CLI/config error.
const EXIT_USAGE: u8 = 2;

/// `adesk-machine` — host-side AI Machine lifecycle CLI.
#[derive(Debug, Parser)]
#[command(name = "adesk-machine", version, about, long_about = None)]
struct Cli {
    /// Container backend to drive.
    #[arg(long, value_enum, default_value = "podman", value_name = "KIND")]
    runtime: RuntimeArg,
    /// `tracing-subscriber` env-filter directive.
    #[arg(long, env = "ADESK_LOG", value_name = "FILTER", default_value = "info")]
    log: String,
    /// Lifecycle command to run.
    #[command(subcommand)]
    command: Command,
}

/// Which container backend `--runtime` selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RuntimeArg {
    /// Rootless Podman driven through the `podman` command line.
    Podman,
    /// In-memory deterministic backend (no container engine required).
    Mock,
}

/// The machine lifecycle commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Create a machine from a specification.
    Create(CreateArgs),
    /// Start a created or stopped machine.
    Start(NameArgs),
    /// Stop a running machine.
    Stop(StopArgs),
    /// Restart a machine (stop then start).
    Restart(NameArgs),
    /// Remove a machine.
    Remove(RemoveArgs),
    /// List every machine the backend knows about.
    List,
    /// Show one machine's current status.
    Status(NameArgs),
    /// Print the host's capabilities.
    Capabilities,
}

/// A subcommand that acts on a single machine by name.
#[derive(Debug, Args)]
struct NameArgs {
    /// Machine name.
    #[arg(value_name = "NAME")]
    name: String,
}

/// Arguments for `stop`.
#[derive(Debug, Args)]
struct StopArgs {
    /// Machine name.
    #[arg(value_name = "NAME")]
    name: String,
    /// Graceful-shutdown timeout in milliseconds.
    #[arg(long, value_name = "MS", default_value_t = 10_000)]
    timeout_ms: u64,
}

/// Arguments for `remove`.
#[derive(Debug, Args)]
struct RemoveArgs {
    /// Machine name.
    #[arg(value_name = "NAME")]
    name: String,
    /// Force-remove a running machine.
    #[arg(long)]
    force: bool,
}

/// Arguments for `create`.
#[derive(Debug, Args)]
struct CreateArgs {
    /// Unique machine name.
    #[arg(long, value_name = "NAME")]
    name: String,
    /// Container image to run.
    #[arg(long, value_name = "IMAGE")]
    image: String,
    /// Container command (`argv`); one flag or several values.
    #[arg(long, value_name = "ARG", num_args = 1..)]
    command: Vec<String>,
    /// Expose the viewer over a Unix socket, `HOST:CONTAINER`.
    #[arg(long, value_name = "HOST:CONTAINER")]
    viewer_unix: Option<String>,
    /// Expose the viewer over a published TCP port, `HOST:PORT`.
    #[arg(long, value_name = "HOST:PORT")]
    viewer_tcp: Option<String>,
    /// Bind mount, `HOST:CONTAINER[:ro]` (repeatable).
    #[arg(long, value_name = "HOST:CONTAINER[:ro]", num_args = 1..)]
    mount: Vec<String>,
    /// Memory limit in MiB.
    #[arg(long, value_name = "N")]
    memory_mb: Option<u64>,
    /// CPU limit in cores.
    #[arg(long, value_name = "N")]
    cpus: Option<f64>,
    /// Network mode: `none`, `host` or `private`.
    #[arg(long, value_name = "MODE")]
    network: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(&cli.log);
    let code = match cli.runtime {
        RuntimeArg::Mock => run(MockRuntime::new(), &cli.command).await,
        RuntimeArg::Podman => run(PodmanRuntime::new(), &cli.command).await,
    };
    ExitCode::from(code)
}

/// Installs the global `tracing-subscriber` with `filter`.
///
/// An invalid directive falls back to `info`; a subscriber that is already
/// installed is left alone (never panics).
fn init_tracing(filter: &str) {
    let filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Why a command could not complete, split by the exit code it maps to.
enum Failure {
    /// A backend/lifecycle failure (`MachineError`): exit code `1`.
    Machine(MachineError),
    /// A CLI/config error: exit code `2`.
    Usage(String),
}

/// Drives `command` through a control plane over `runtime`, returning an exit
/// code.
///
/// Every failure is reported on stderr and mapped to a process exit code so
/// `main` never panics.
async fn run<R: ContainerRuntime>(runtime: R, command: &Command) -> u8 {
    let plane = HostControlPlane::new(MachineManager::new(runtime), HostCapabilities::default());
    match dispatch(&plane, command).await {
        Ok(()) => 0,
        Err(Failure::Machine(error)) => {
            eprintln!("adesk-machine: {error}");
            EXIT_FAILURE
        }
        Err(Failure::Usage(message)) => {
            eprintln!("adesk-machine: {message}");
            EXIT_USAGE
        }
    }
}

/// Runs one subcommand against the host control plane `plane`.
async fn dispatch<R: ContainerRuntime>(
    plane: &HostControlPlane<R>,
    command: &Command,
) -> Result<(), Failure> {
    match command {
        Command::Create(args) => {
            let spec = build_spec(args).map_err(Failure::Usage)?;
            let status = plane.create(&spec).await.map_err(Failure::Machine)?;
            tracing::info!(machine = %status.name, id = %status.id, "machine created");
            println!("created machine");
            print_status(&status);
        }
        Command::Start(args) => {
            let name = MachineName::from(args.name.as_str());
            let status = plane.start(&name).await.map_err(Failure::Machine)?;
            tracing::info!(machine = %status.name, id = %status.id, "machine started");
            println!("started machine");
            print_status(&status);
        }
        Command::Stop(args) => {
            let name = MachineName::from(args.name.as_str());
            let status = plane
                .stop(&name, args.timeout_ms)
                .await
                .map_err(Failure::Machine)?;
            tracing::info!(machine = %status.name, id = %status.id, "machine stopped");
            println!("stopped machine");
            print_status(&status);
        }
        Command::Restart(args) => {
            let name = MachineName::from(args.name.as_str());
            let status = plane
                .manager()
                .restart(&name)
                .await
                .map_err(Failure::Machine)?;
            tracing::info!(machine = %status.name, id = %status.id, "machine restarted");
            println!("restarted machine");
            print_status(&status);
        }
        Command::Remove(args) => {
            let name = MachineName::from(args.name.as_str());
            let status = plane
                .remove(&name, args.force)
                .await
                .map_err(Failure::Machine)?;
            tracing::info!(machine = %status.name, id = %status.id, "machine removed");
            println!("removed machine");
            print_status(&status);
        }
        Command::List => {
            let machines = plane.machines().await.map_err(Failure::Machine)?;
            if machines.is_empty() {
                println!("no machines");
            } else {
                for status in &machines {
                    println!(
                        "{}\t{}\t{}\t{}",
                        status.name, status.id, status.state, status.image
                    );
                }
            }
        }
        Command::Status(args) => {
            let name = MachineName::from(args.name.as_str());
            let status = plane.status(&name).await.map_err(Failure::Machine)?;
            print_status(&status);
        }
        Command::Capabilities => {
            print_capabilities(plane.capabilities());
        }
    }
    Ok(())
}

/// Prints a machine's identity and lifecycle summary.
fn print_status(status: &MachineStatus) {
    println!("name:  {}", status.name);
    println!("id:    {}", status.id);
    println!("state: {}", status.state);
    println!("image: {}", status.image);
    if let Some(pid) = status.pid {
        println!("pid:   {pid}");
    }
}

/// Prints the host's capabilities.
fn print_capabilities(capabilities: &HostCapabilities) {
    println!("gpu: {}", capabilities.gpu);
    println!("kvm: {}", capabilities.kvm);
    println!("publish_ports: {}", capabilities.publish_ports);
    if capabilities.allowed_mounts.is_empty() {
        println!("allowed_mounts: -");
    } else {
        let mounts = capabilities
            .allowed_mounts
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("allowed_mounts: {mounts}");
    }
}

/// Builds a [`MachineSpec`] from the `create` flags.
///
/// Pure and unit-testable: it never touches a backend. The spec starts from
/// [`MachineSpec::new`] (the ADesk defaults) and applies the builders for every
/// flag that was supplied.
fn build_spec(args: &CreateArgs) -> Result<MachineSpec, String> {
    if args.viewer_unix.is_some() && args.viewer_tcp.is_some() {
        return Err("--viewer-unix and --viewer-tcp are mutually exclusive".to_owned());
    }

    let mut spec = MachineSpec::new(args.name.as_str(), args.image.as_str());
    if !args.command.is_empty() {
        spec = spec.with_command(args.command.clone());
    }
    for mount in &args.mount {
        spec = spec.with_mount(parse_mount(mount)?);
    }
    if let Some(memory_mb) = args.memory_mb {
        spec = spec.with_memory_mb(memory_mb);
    }
    if let Some(cpus) = args.cpus {
        spec = spec.with_cpus(cpus);
    }
    if let Some(network) = &args.network {
        spec = spec.with_network(parse_network(network)?);
    }
    if let Some(viewer) = &args.viewer_unix {
        spec = spec.with_viewer(parse_viewer_unix(viewer)?);
    } else if let Some(viewer) = &args.viewer_tcp {
        spec = spec.with_viewer(parse_viewer_tcp(viewer)?);
    }
    Ok(spec)
}

/// Parses a `--mount HOST:CONTAINER[:ro]` value.
///
/// The optional trailing `ro`/`rw` on the last `:` selects read-only or
/// read-write; read-write is the default, so a plain `HOST:CONTAINER` never
/// needs a mode.
fn parse_mount(value: &str) -> Result<Mount, String> {
    let invalid = || format!("invalid --mount `{value}`: expected HOST:CONTAINER[:ro]");

    // Split the optional trailing mode off the last `:`; anything that is not a
    // known mode word means there was no mode and `value` is all paths.
    let (paths, read_only) = match value.rsplit_once(':') {
        Some((paths, "ro")) => (paths, true),
        Some((paths, "rw")) => (paths, false),
        _ => (value, false),
    };

    let (host, container) = paths.split_once(':').ok_or_else(invalid)?;
    if host.is_empty() || container.is_empty() {
        return Err(invalid());
    }
    Ok(if read_only {
        Mount::ro(host, container)
    } else {
        Mount::rw(host, container)
    })
}

/// Splits a `FIRST:SECOND` value (a `HOST:CONTAINER` path pair or a `HOST:PORT`
/// port pair) on its first `:`, requiring both sides to be non-empty.
///
/// Returns `None` on a missing separator or an empty side, so each caller can
/// report its own flag-specific error.
fn split_host_and_container(value: &str) -> Option<(&str, &str)> {
    let (host, container) = value.split_once(':')?;
    if host.is_empty() || container.is_empty() {
        return None;
    }
    Some((host, container))
}

/// Parses a `--viewer-unix HOST:CONTAINER` value into a Unix-socket exposure.
fn parse_viewer_unix(value: &str) -> Result<ViewerExposure, String> {
    let invalid = || format!("invalid --viewer-unix `{value}`: expected HOST:CONTAINER");
    let (host, container) = split_host_and_container(value).ok_or_else(invalid)?;
    Ok(ViewerExposure::default_unix(host, container))
}

/// Parses a `--viewer-tcp HOST:PORT` value into a published-port exposure.
fn parse_viewer_tcp(value: &str) -> Result<ViewerExposure, String> {
    let invalid = || format!("invalid --viewer-tcp `{value}`: expected HOST:PORT");
    let (host, container) = split_host_and_container(value).ok_or_else(&invalid)?;
    let host_port = host.parse::<u16>().map_err(|_| invalid())?;
    let container_port = container.parse::<u16>().map_err(|_| invalid())?;
    Ok(ViewerExposure::TcpPort {
        host_port,
        container_port,
    })
}

/// Parses a `--network` value into a [`NetworkMode`].
fn parse_network(value: &str) -> Result<NetworkMode, String> {
    match value {
        "none" => Ok(NetworkMode::None),
        "host" => Ok(NetworkMode::Host),
        "private" => Ok(NetworkMode::Private),
        other => Err(format!(
            "invalid --network `{other}`: expected none, host or private"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `create` argument set with every optional flag absent.
    fn create_args() -> CreateArgs {
        CreateArgs {
            name: "demo".to_owned(),
            image: "ghcr.io/adesk/machine:latest".to_owned(),
            command: Vec::new(),
            viewer_unix: None,
            viewer_tcp: None,
            mount: Vec::new(),
            memory_mb: None,
            cpus: None,
            network: None,
        }
    }

    #[test]
    fn mount_parses_paths_and_modes_and_rejects_malformed_values() {
        // Input -> the expected `Mount`, or `Err(())` for a rejected value.
        let cases: [(&str, Result<Mount, ()>); 6] = [
            (
                "/host/data:/mnt/data",
                Ok(Mount::rw("/host/data", "/mnt/data")),
            ),
            (
                "/host/data:/mnt/data:ro",
                Ok(Mount::ro("/host/data", "/mnt/data")),
            ),
            ("/a:/b:rw", Ok(Mount::rw("/a", "/b"))),
            ("/only-a-path", Err(())),
            (":/b", Err(())),
            ("/a:", Err(())),
        ];
        for (input, expected) in cases {
            match expected {
                Ok(mount) => assert_eq!(parse_mount(input).unwrap(), mount, "parsed {input:?}"),
                Err(()) => assert!(parse_mount(input).is_err(), "accepted {input:?}"),
            }
        }
    }

    #[test]
    fn viewer_unix_parses_host_and_container() {
        let viewer = parse_viewer_unix("/run/adesk/viewer.sock:/run/adesk/viewer.sock").unwrap();
        assert_eq!(
            viewer,
            ViewerExposure::default_unix("/run/adesk/viewer.sock", "/run/adesk/viewer.sock")
        );
    }

    #[test]
    fn viewer_unix_rejects_malformed_values() {
        for bad in ["/run/adesk/viewer.sock", ":/inside", "/outside:"] {
            assert!(parse_viewer_unix(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn viewer_tcp_parses_host_and_container_ports() {
        let viewer = parse_viewer_tcp("7100:8100").unwrap();
        assert_eq!(
            viewer,
            ViewerExposure::TcpPort {
                host_port: 7100,
                container_port: 8100,
            }
        );
    }

    #[test]
    fn viewer_tcp_rejects_malformed_values() {
        for bad in ["7100", "7100:", "abc:80", "0:99999", "70000:80"] {
            assert!(parse_viewer_tcp(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn network_parses_every_mode() {
        assert_eq!(parse_network("none").unwrap(), NetworkMode::None);
        assert_eq!(parse_network("host").unwrap(), NetworkMode::Host);
        assert_eq!(parse_network("private").unwrap(), NetworkMode::Private);
    }

    #[test]
    fn network_rejects_unknown_modes() {
        assert!(parse_network("bridge").is_err());
        assert!(parse_network("").is_err());
    }

    #[test]
    fn build_spec_keeps_defaults_when_flags_are_absent() {
        let spec = build_spec(&create_args()).unwrap();
        assert_eq!(spec.name, MachineName::from("demo"));
        assert_eq!(spec.image, "ghcr.io/adesk/machine:latest");
        assert_eq!(spec.command, vec!["adesk-server".to_owned()]);
        assert_eq!(spec.memory_mb, Some(4096));
        assert_eq!(spec.cpus, Some(2.0));
        assert_eq!(spec.network, NetworkMode::Private);
    }

    #[test]
    fn build_spec_applies_every_flag() {
        let mut args = create_args();
        args.command = vec![
            "adesk-server".to_owned(),
            "--renderer".to_owned(),
            "pixman".to_owned(),
        ];
        args.mount = vec!["/host/data:/mnt/data:ro".to_owned()];
        args.memory_mb = Some(2048);
        args.cpus = Some(1.5);
        args.network = Some("host".to_owned());
        args.viewer_tcp = Some("7100:8100".to_owned());

        let spec = build_spec(&args).unwrap();
        assert_eq!(
            spec.command,
            vec![
                "adesk-server".to_owned(),
                "--renderer".to_owned(),
                "pixman".to_owned(),
            ]
        );
        assert_eq!(spec.mounts, vec![Mount::ro("/host/data", "/mnt/data")]);
        assert_eq!(spec.memory_mb, Some(2048));
        assert_eq!(spec.cpus, Some(1.5));
        assert_eq!(spec.network, NetworkMode::Host);
        assert_eq!(
            spec.viewer,
            ViewerExposure::TcpPort {
                host_port: 7100,
                container_port: 8100,
            }
        );
    }

    #[test]
    fn build_spec_rejects_two_viewer_transports() {
        let mut args = create_args();
        args.viewer_unix = Some("/run/adesk/viewer.sock:/run/adesk/viewer.sock".to_owned());
        args.viewer_tcp = Some("7100:8100".to_owned());

        let error = build_spec(&args).unwrap_err();
        assert!(error.contains("mutually exclusive"), "{error}");
    }

    #[test]
    fn build_spec_propagates_parser_errors() {
        let mut args = create_args();
        args.mount = vec!["nonsense".to_owned()];
        assert!(build_spec(&args).is_err());
    }
}
