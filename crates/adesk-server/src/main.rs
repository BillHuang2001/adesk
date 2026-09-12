//! `adesk-server` binary: CLI flags → [`ServerConfig`] → [`Server::start`].
//!
//! Every flag has an `ADESK_*` environment fallback (`clap`'s `env`), so the
//! binary is usable both from a shell and from a service unit.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

use adesk_compositor::XkbSettings;
use adesk_server::config::{parse_renderer, parse_size, ServerConfig};
use adesk_server::Server;

/// ADesk — AI-native headless Wayland runtime (AGP server).
#[derive(Debug, Parser)]
#[command(name = "adesk-server", version, about, long_about = None)]
struct Cli {
    /// AGP Unix socket path.
    #[arg(long, env = "ADESK_SOCKET", value_name = "PATH")]
    socket: Option<PathBuf>,
    /// Virtual output size, e.g. `1280x800`.
    #[arg(long, env = "ADESK_OUTPUT", value_name = "WxH")]
    output: Option<String>,
    /// Renderer: `auto`, `gl` or `pixman`.
    #[arg(long, env = "ADESK_RENDERER", value_name = "KIND")]
    renderer: Option<String>,
    /// xkb layout list (e.g. `us`, `de,us`).
    #[arg(long, env = "ADESK_XKB_LAYOUT", value_name = "NAME")]
    xkb_layout: Option<String>,
    /// xkb variant list.
    #[arg(long, env = "ADESK_XKB_VARIANT", value_name = "NAME")]
    xkb_variant: Option<String>,
    /// xkb model.
    #[arg(long, env = "ADESK_XKB_MODEL", value_name = "NAME")]
    xkb_model: Option<String>,
    /// xkb rules file.
    #[arg(long, env = "ADESK_XKB_RULES", value_name = "NAME")]
    xkb_rules: Option<String>,
    /// Extra `.desktop` search directories (repeatable, `:`-separated in the env).
    #[arg(
        long = "apps-dir",
        env = "ADESK_APPS_DIR",
        value_name = "DIR",
        value_delimiter = ':'
    )]
    apps_dir: Vec<PathBuf>,
    /// Viewer (VAP v1) Unix socket path.
    #[arg(
        long = "viewer-socket",
        env = "ADESK_VIEWER_SOCKET",
        value_name = "PATH"
    )]
    viewer_socket: Option<PathBuf>,
    /// Viewer (VAP v1) TCP listen address (opt-in), e.g. `127.0.0.1:7100`.
    #[arg(
        long = "viewer-tcp",
        env = "ADESK_VIEWER_TCP",
        value_name = "HOST:PORT",
        value_parser = parse_socket_addr
    )]
    viewer_tcp: Option<SocketAddr>,
    /// Directory recordings started without an explicit path are written to.
    #[arg(
        long = "recordings-dir",
        env = "ADESK_RECORDINGS_DIR",
        value_name = "DIR"
    )]
    recordings_dir: Option<PathBuf>,
    /// Disable the viewer (VAP v1) endpoint entirely.
    #[arg(long = "no-viewer")]
    no_viewer: bool,
    /// `tracing-subscriber` env-filter directive.
    #[arg(long, env = "ADESK_LOG", value_name = "FILTER", default_value = "info")]
    log: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(&cli.log);
    let config = build_config(&cli)?;
    tracing::info!(socket = %config.socket_path().display(), "starting adesk-server");
    let server = Server::start(config).await?;
    tracing::info!(socket = %server.socket_path().display(), "adesk-server ready");
    server.wait().await?;
    tracing::info!("adesk-server stopped");
    Ok(())
}

/// Builds a [`ServerConfig`] from parsed CLI flags (pure, unit-testable).
fn build_config(cli: &Cli) -> anyhow::Result<ServerConfig> {
    let mut config = ServerConfig::default();
    if let Some(socket) = &cli.socket {
        config = config.with_socket_path(socket);
    }
    if let Some(output) = &cli.output {
        let size =
            parse_size(output).map_err(|message| anyhow::anyhow!("invalid --output: {message}"))?;
        config = config.with_output_size(size);
    }
    if let Some(renderer) = &cli.renderer {
        let kind = parse_renderer(renderer)
            .map_err(|message| anyhow::anyhow!("invalid --renderer: {message}"))?;
        config = config.with_renderer(kind);
    }
    if cli.xkb_layout.is_some()
        || cli.xkb_variant.is_some()
        || cli.xkb_model.is_some()
        || cli.xkb_rules.is_some()
    {
        let mut xkb = XkbSettings::us();
        if let Some(layout) = &cli.xkb_layout {
            xkb.layout = layout.clone();
        }
        if let Some(variant) = &cli.xkb_variant {
            xkb.variant = variant.clone();
        }
        if let Some(model) = &cli.xkb_model {
            xkb.model = model.clone();
        }
        if let Some(rules) = &cli.xkb_rules {
            xkb.rules = rules.clone();
        }
        config = config.with_xkb(xkb);
    }
    if !cli.apps_dir.is_empty() {
        config = config.with_app_dirs(cli.apps_dir.clone());
    }
    // The viewer endpoint is enabled by default; an explicit socket or TCP
    // address opts into it, and `--no-viewer` wins over both.
    if let Some(path) = &cli.viewer_socket {
        config = config.with_viewer_socket(path);
    }
    if let Some(addr) = cli.viewer_tcp {
        config = config.with_viewer_tcp(addr);
    }
    if let Some(dir) = &cli.recordings_dir {
        config = config.with_recordings_dir(dir);
    }
    if cli.no_viewer {
        config = config.without_viewer();
    }
    Ok(config)
}

/// Parses `HOST:PORT` with a friendlier message than the standard `SocketAddr` error.
fn parse_socket_addr(value: &str) -> std::result::Result<SocketAddr, String> {
    value.trim().parse::<SocketAddr>().map_err(|_| {
        format!("invalid socket address `{value}`: expected HOST:PORT (e.g. 127.0.0.1:7100)")
    })
}

/// Installs the global `tracing-subscriber` with the given filter.
///
/// An invalid directive falls back to `info`; a subscriber that is already
/// installed is left alone (never panics).
fn init_tracing(filter: &str) {
    let filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_compositor::RendererKind;
    use adesk_core::Size;

    fn cli() -> Cli {
        Cli {
            socket: None,
            output: None,
            renderer: None,
            xkb_layout: None,
            xkb_variant: None,
            xkb_model: None,
            xkb_rules: None,
            apps_dir: Vec::new(),
            viewer_socket: None,
            viewer_tcp: None,
            recordings_dir: None,
            no_viewer: false,
            log: "info".to_owned(),
        }
    }

    #[test]
    fn no_flags_keeps_the_environment_defaults() {
        let config = build_config(&cli()).unwrap();
        assert_eq!(config.socket_path(), adesk_server::default_socket_path());
        assert_eq!(config.compositor.output_size, Size::new(1280, 800));
        assert_eq!(config.compositor.renderer, RendererKind::Auto);
        assert!(config.app_dirs.is_none());
    }

    #[test]
    fn flags_override_the_compositor_settings() {
        let mut cli = cli();
        cli.socket = Some(PathBuf::from("/tmp/cli.sock"));
        cli.output = Some("800x600".to_owned());
        cli.renderer = Some("pixman".to_owned());
        cli.xkb_layout = Some("de,us".to_owned());
        cli.xkb_variant = Some("nodeadkeys".to_owned());
        cli.xkb_model = Some("pc104".to_owned());
        cli.xkb_rules = Some("evdev".to_owned());

        let config = build_config(&cli).unwrap();
        assert_eq!(config.socket_path(), std::path::Path::new("/tmp/cli.sock"));
        assert_eq!(config.compositor.output_size, Size::new(800, 600));
        assert_eq!(config.compositor.renderer, RendererKind::Pixman);
        assert_eq!(config.compositor.xkb.layout, "de,us");
        assert_eq!(config.compositor.xkb.variant, "nodeadkeys");
        assert_eq!(config.compositor.xkb.model, "pc104");
        assert_eq!(config.compositor.xkb.rules, "evdev");
    }

    #[test]
    fn invalid_flag_values_are_reported_with_the_flag_name() {
        let mut bad_output = cli();
        bad_output.output = Some("wide".to_owned());
        let error = build_config(&bad_output).unwrap_err().to_string();
        assert!(error.contains("--output"), "{error}");

        let mut bad_renderer = cli();
        bad_renderer.renderer = Some("vulkan".to_owned());
        let error = build_config(&bad_renderer).unwrap_err().to_string();
        assert!(error.contains("--renderer"), "{error}");
    }

    #[test]
    fn app_directories_are_passed_through() {
        let mut cli = cli();
        cli.apps_dir = vec![PathBuf::from("/opt/apps"), PathBuf::from("/srv/apps")];
        let config = build_config(&cli).unwrap();
        assert_eq!(
            config.app_dirs,
            Some(vec![PathBuf::from("/opt/apps"), PathBuf::from("/srv/apps")])
        );
    }

    #[test]
    fn viewer_is_enabled_by_default_on_the_agp_sibling() {
        let config = build_config(&cli()).unwrap();
        assert!(config.viewer.enabled);
        assert_eq!(
            config.viewer_socket_path(),
            Some(adesk_server::config::viewer_socket_sibling(
                &adesk_server::default_socket_path()
            ))
        );
    }

    #[test]
    fn viewer_socket_flag_overrides_the_derived_path() {
        let mut cli = cli();
        cli.viewer_socket = Some(PathBuf::from("/tmp/explicit-viewer.sock"));
        let config = build_config(&cli).unwrap();
        assert!(config.viewer.enabled);
        assert_eq!(
            config.viewer_socket_path(),
            Some(PathBuf::from("/tmp/explicit-viewer.sock"))
        );
    }

    #[test]
    fn no_viewer_disables_the_endpoint_even_with_an_explicit_socket() {
        let mut cli = cli();
        cli.viewer_socket = Some(PathBuf::from("/tmp/explicit-viewer.sock"));
        cli.viewer_tcp = Some("127.0.0.1:7100".parse().unwrap());
        cli.no_viewer = true;
        let config = build_config(&cli).unwrap();
        assert!(!config.viewer.enabled);
        assert_eq!(config.viewer_socket_path(), None);
    }

    #[test]
    fn viewer_tcp_flag_is_recorded() {
        let mut cli = cli();
        cli.viewer_tcp = Some("127.0.0.1:7100".parse().unwrap());
        let config = build_config(&cli).unwrap();
        assert_eq!(config.viewer.tcp, Some("127.0.0.1:7100".parse().unwrap()));
    }

    #[test]
    fn recordings_dir_flag_overrides_the_derived_directory() {
        let baseline = build_config(&cli()).unwrap();
        assert_eq!(
            baseline.viewer.recordings_dir, None,
            "an unset flag keeps the derived recordings directory"
        );

        let mut cli = cli();
        cli.recordings_dir = Some(PathBuf::from("/var/lib/adesk/rec"));
        let config = build_config(&cli).unwrap();
        assert_eq!(
            config.viewer.recordings_dir,
            Some(PathBuf::from("/var/lib/adesk/rec"))
        );
        assert_eq!(config.recordings_dir(), PathBuf::from("/var/lib/adesk/rec"));
    }

    #[test]
    fn socket_addr_parser_reports_the_expected_shape() {
        assert_eq!(
            parse_socket_addr("127.0.0.1:7100").unwrap(),
            "127.0.0.1:7100".parse::<SocketAddr>().unwrap()
        );
        let error = parse_socket_addr("7100").unwrap_err();
        assert!(error.contains("HOST:PORT"), "{error}");
    }
}
