//! [`MachineSpec`] → `podman create` argument translation.
//!
//! `build_create_argv` is pure (it spawns no process), so these tests pin the
//! exact argv the backend hands to the `podman` CLI for a spec, covering labels,
//! environment, mounts, limits, every [`NetworkMode`] and every
//! [`ViewerExposure`].

use adesk_machine::runtime::podman::build_create_argv;
use adesk_machine::{MachineSpec, Mount, NetworkMode, ViewerExposure};

/// The `podman create` argv for `spec`.
fn argv(spec: &MachineSpec) -> Vec<String> {
    build_create_argv(spec)
}

/// Owned-string form of a literal slice, to keep the assertions readable.
fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// Whether `argv` contains `flag` immediately followed by `value`.
fn has_pair(argv: &[String], flag: &str, value: &str) -> bool {
    argv.windows(2)
        .any(|pair| pair[0] == flag && pair[1] == value)
}

#[test]
fn default_spec_matches_the_documented_argv() {
    assert_eq!(
        argv(&MachineSpec::default()),
        strings(&[
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
        ])
    );
}

#[test]
fn populated_spec_translates_every_field_in_order() {
    // The default label (`adesk.io/role=machine`) is inherited and sorts before
    // the added `team` label; env keys and labels are emitted in key order.
    let spec = MachineSpec::new("work", "debian:trixie")
        .with_command(vec!["bash".to_owned(), "-l".to_owned()])
        .with_label("team", "agent")
        .with_env("LANG", "C.UTF-8")
        .with_env("TZ", "UTC")
        .with_mount(Mount::ro("/etc/localtime", "/etc/localtime"))
        .with_mount(Mount::rw("/srv/share", "/mnt/share"))
        .with_memory_mb(2048)
        .with_cpus(1.5)
        .with_network(NetworkMode::None)
        .with_viewer(ViewerExposure::None);

    assert_eq!(
        argv(&spec),
        strings(&[
            "create",
            "--name",
            "work",
            "--label",
            "adesk.io/role=machine",
            "--label",
            "team=agent",
            "--env",
            "LANG=C.UTF-8",
            "--env",
            "TZ=UTC",
            "--volume",
            "/etc/localtime:/etc/localtime:ro",
            "--volume",
            "/srv/share:/mnt/share",
            "--memory",
            "2048m",
            "--cpus",
            "1.5",
            "--network",
            "none",
            "debian:trixie",
            "bash",
            "-l",
        ])
    );
}

#[test]
fn populated_spec_with_a_published_port_orders_every_option() {
    // The same fully-populated spec, but with host networking and a TCP viewer,
    // so the exact position of `--network`/`--publish` (before the image and
    // command) is pinned alongside every other option.
    let spec = MachineSpec::new("work", "debian:trixie")
        .with_command(vec!["bash".to_owned(), "-l".to_owned()])
        .with_label("team", "agent")
        .with_env("LANG", "C.UTF-8")
        .with_mount(Mount::ro("/etc/localtime", "/etc/localtime"))
        .with_mount(Mount::rw("/srv/share", "/mnt/share"))
        .with_memory_mb(2048)
        .with_cpus(1.5)
        .with_network(NetworkMode::Host)
        .with_viewer(ViewerExposure::TcpPort {
            host_port: 7000,
            container_port: 7100,
        });

    assert_eq!(
        argv(&spec),
        strings(&[
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
        ])
    );
}

#[test]
fn network_modes_map_to_flags() {
    let none = MachineSpec::new("m", "img").with_network(NetworkMode::None);
    assert!(has_pair(&argv(&none), "--network", "none"));

    let host = MachineSpec::new("m", "img").with_network(NetworkMode::Host);
    assert!(has_pair(&argv(&host), "--network", "host"));

    // Private is Podman's default isolated network: no flag is emitted at all.
    let private = MachineSpec::new("m", "img").with_network(NetworkMode::Private);
    assert!(!argv(&private).iter().any(|arg| arg == "--network"));
}

#[test]
fn viewer_exposure_maps_to_container_options() {
    // A Unix-socket viewer becomes a bind mount.
    let unix = MachineSpec::new("m", "img").with_viewer(ViewerExposure::default_unix(
        "/host/viewer.sock",
        "/run/viewer.sock",
    ));
    let args = argv(&unix);
    assert!(has_pair(
        &args,
        "--volume",
        "/host/viewer.sock:/run/viewer.sock"
    ));
    assert!(!args.iter().any(|arg| arg == "--publish"));

    // A TCP viewer becomes a published port.
    let tcp = MachineSpec::new("m", "img").with_viewer(ViewerExposure::TcpPort {
        host_port: 7000,
        container_port: 7001,
    });
    let args = argv(&tcp);
    assert!(has_pair(&args, "--publish", "7000:7001"));
    assert!(!args.iter().any(|arg| arg == "--volume"));

    // No viewer transport -> neither option is present.
    let without = MachineSpec::new("m", "img").with_viewer(ViewerExposure::None);
    let args = argv(&without);
    assert!(!args
        .iter()
        .any(|arg| arg == "--volume" || arg == "--publish"));
}

#[test]
fn image_precedes_the_container_command() {
    let spec = MachineSpec::new("m", "img:tag")
        .with_command(vec![
            "serve".to_owned(),
            "--port".to_owned(),
            "80".to_owned(),
        ])
        .with_viewer(ViewerExposure::None);

    let argv = argv(&spec);
    let image = argv
        .iter()
        .position(|arg| arg == "img:tag")
        .expect("image is present");
    let tail: Vec<&str> = argv[image + 1..].iter().map(String::as_str).collect();
    assert_eq!(tail, ["serve", "--port", "80"]);
}

#[test]
fn unset_limits_and_empty_labels_emit_no_flags() {
    let mut spec = MachineSpec::new("m", "img")
        .with_network(NetworkMode::Private)
        .with_viewer(ViewerExposure::None)
        .with_command(Vec::new());
    spec.memory_mb = None;
    spec.cpus = None;
    spec.labels.clear();
    spec.env.clear();

    assert_eq!(argv(&spec), strings(&["create", "--name", "m", "img"]));
}
