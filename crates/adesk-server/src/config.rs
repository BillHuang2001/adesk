//! Server configuration: socket path, compositor settings and app directories.
//!
//! [`ServerConfig`] is the single input of [`crate::Server::start`]; `adesk-testkit`
//! constructs it directly, the `adesk-server` binary builds it from CLI flags.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use adesk_compositor::{CompositorConfig, RendererKind, XkbSettings};
use adesk_core::Size;

/// Everything [`crate::Server::start`] needs.
///
/// `Default` resolves the socket path from the environment
/// ([`default_socket_path`]) and uses the compositor defaults (1280x800,
/// `RendererKind::Auto`, `us` keymap).
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Unix socket the AGP server listens on.
    pub socket_path: PathBuf,
    /// Compositor thread configuration (output size, renderer, xkb, socket name).
    pub compositor: CompositorConfig,
    /// Extra `.desktop` search directories; `None` means the XDG default set.
    pub app_dirs: Option<Vec<PathBuf>>,
    /// Viewer (VAP v1) endpoint configuration.
    pub viewer: ViewerConfig,
}

/// Viewer (VAP v1) endpoint configuration.
///
/// The endpoint is enabled by default and binds the sibling of the AGP socket
/// ([`viewer_socket_sibling`]); an explicit `socket_path` overrides that and
/// `tcp` adds an optional second transport.
#[derive(Debug, Clone)]
pub struct ViewerConfig {
    /// Whether the viewer endpoint is served at all.
    pub enabled: bool,
    /// Explicit Unix socket path; `None` derives the sibling of the AGP socket.
    pub socket_path: Option<PathBuf>,
    /// Optional TCP listener address (`None` = Unix socket only).
    pub tcp: Option<SocketAddr>,
    /// Directory a recording without an explicit path is written to; `None`
    /// derives [`ServerConfig::recordings_dir`].
    pub recordings_dir: Option<PathBuf>,
}

impl Default for ViewerConfig {
    fn default() -> ViewerConfig {
        ViewerConfig {
            enabled: true,
            socket_path: None,
            tcp: None,
            recordings_dir: None,
        }
    }
}

impl Default for ServerConfig {
    fn default() -> ServerConfig {
        ServerConfig {
            socket_path: default_socket_path(),
            compositor: CompositorConfig::default(),
            app_dirs: None,
            viewer: ViewerConfig::default(),
        }
    }
}

impl ServerConfig {
    /// A configuration for `socket_path` and `compositor` (the constructor
    /// `adesk-testkit`'s harness uses); `app_dirs` starts as `None`.
    ///
    /// Use [`ServerConfig::default`] for environment-resolved defaults.
    pub fn new(socket_path: impl Into<PathBuf>, compositor: CompositorConfig) -> ServerConfig {
        ServerConfig {
            socket_path: socket_path.into(),
            compositor,
            app_dirs: None,
            viewer: ViewerConfig::default(),
        }
    }

    /// Overrides the socket path.
    pub fn with_socket_path(mut self, path: impl Into<PathBuf>) -> ServerConfig {
        self.socket_path = path.into();
        self
    }

    /// Sets the virtual output size (windows tile to fill it).
    pub fn with_output_size(mut self, size: Size) -> ServerConfig {
        self.compositor = self.compositor.with_output_size(size);
        self
    }

    /// Selects the renderer (`Auto` / `Gl` / `Pixman`).
    pub fn with_renderer(mut self, renderer: RendererKind) -> ServerConfig {
        self.compositor = self.compositor.with_renderer(renderer);
        self
    }

    /// Sets the xkb keymap settings of the virtual keyboard.
    pub fn with_xkb(mut self, xkb: XkbSettings) -> ServerConfig {
        self.compositor = self.compositor.with_xkb(xkb);
        self
    }

    /// Replaces the application search directories (`None` = XDG default set).
    pub fn with_app_dirs(mut self, dirs: Vec<PathBuf>) -> ServerConfig {
        self.app_dirs = Some(dirs);
        self
    }

    /// Replaces the viewer (VAP v1) endpoint configuration wholesale.
    pub fn with_viewer(mut self, viewer: ViewerConfig) -> ServerConfig {
        self.viewer = viewer;
        self
    }

    /// Enables the viewer endpoint on an explicit Unix socket path.
    pub fn with_viewer_socket(mut self, path: impl Into<PathBuf>) -> ServerConfig {
        self.viewer.socket_path = Some(path.into());
        self.viewer.enabled = true;
        self
    }

    /// Adds a TCP listener to the viewer endpoint.
    pub fn with_viewer_tcp(mut self, addr: SocketAddr) -> ServerConfig {
        self.viewer.tcp = Some(addr);
        self
    }

    /// Sets the directory a recording without an explicit path is written to.
    pub fn with_recordings_dir(mut self, dir: impl Into<PathBuf>) -> ServerConfig {
        self.viewer.recordings_dir = Some(dir.into());
        self
    }

    /// Disables the viewer endpoint entirely.
    pub fn without_viewer(mut self) -> ServerConfig {
        self.viewer.enabled = false;
        self
    }

    /// The socket path this configuration will bind.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The viewer Unix socket path this configuration will bind, or `None` when
    /// the viewer endpoint is disabled: the explicit override if set, else the
    /// sibling of `self.socket_path`.
    pub fn viewer_socket_path(&self) -> Option<PathBuf> {
        if !self.viewer.enabled {
            return None;
        }
        Some(match &self.viewer.socket_path {
            Some(path) => path.clone(),
            None => viewer_socket_sibling(&self.socket_path),
        })
    }

    /// The directory a recording started without an explicit path is written to:
    /// the viewer's explicit [`ViewerConfig::recordings_dir`] if set, else
    /// `adesk-recordings` inside the AGP socket's directory.
    ///
    /// The directory is created on demand by the viewer recording backend, not
    /// here (a plain accessor never has a filesystem side effect).
    pub fn recordings_dir(&self) -> PathBuf {
        if let Some(dir) = &self.viewer.recordings_dir {
            return dir.clone();
        }
        match self.socket_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join("adesk-recordings"),
            _ => PathBuf::from("adesk-recordings"),
        }
    }
}

/// Resolves the default AGP socket path, in order:
///
/// 1. `$ADESK_SOCKET`
/// 2. `$XDG_RUNTIME_DIR/adesk.sock`
/// 3. `<system temp dir>/adesk.sock`
///
/// This matches `adesk_client::default_socket_path` so a default-configured
/// server and a default-configured client always meet.
pub fn default_socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ADESK_SOCKET") {
        return PathBuf::from(path);
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("adesk.sock");
    }
    std::env::temp_dir().join("adesk.sock")
}

/// Derives the viewer socket path beside the AGP socket `agp_socket_path`.
///
/// `…/adesk.sock` → `…/adesk-viewer.sock`: the file stem gains a `-viewer`
/// suffix; directory and extension are preserved. A path with no file name (or
/// an empty stem) yields `adesk-viewer.sock` in the same directory.
pub fn viewer_socket_sibling(agp_socket_path: &Path) -> PathBuf {
    let name = match agp_socket_path.file_stem().and_then(|stem| stem.to_str()) {
        Some(stem) if !stem.is_empty() => {
            let mut name = format!("{stem}-viewer");
            if let Some(extension) = agp_socket_path.extension() {
                name.push('.');
                name.push_str(&extension.to_string_lossy());
            }
            name
        }
        _ => "adesk-viewer.sock".to_owned(),
    };
    agp_socket_path.with_file_name(name)
}

/// Parses an `WxH` output size for `--output` (e.g. `1280x800`).
///
/// # Errors
///
/// Returns a human-readable message when the value is not `WxH` with
/// non-zero dimensions.
pub fn parse_size(value: &str) -> std::result::Result<Size, String> {
    let trimmed = value.trim();
    let (width, height) = trimmed
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("invalid size `{value}`: expected WxH (e.g. 1280x800)"))?;
    let width = width
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("invalid size `{value}`: `{}` is not a width", width.trim()))?;
    let height = height.trim().parse::<u32>().map_err(|_| {
        format!(
            "invalid size `{value}`: `{}` is not a height",
            height.trim()
        )
    })?;
    if width == 0 || height == 0 {
        return Err(format!(
            "invalid size `{value}`: dimensions must be non-zero"
        ));
    }
    Ok(Size::new(width, height))
}

/// Parses a `--renderer` value: `auto`, `gl` or `pixman` (case-insensitive).
///
/// # Errors
///
/// Returns a human-readable message for any other value.
pub fn parse_renderer(value: &str) -> std::result::Result<RendererKind, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(RendererKind::Auto),
        "gl" => Ok(RendererKind::Gl),
        "pixman" => Ok(RendererKind::Pixman),
        other => Err(format!(
            "invalid renderer `{other}`: expected `auto`, `gl` or `pixman`"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    /// Serializes the environment-mutating tests below (the process environment
    /// is global, so parallel tests would otherwise race).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Sets the given environment variables and restores them on drop.
    struct EnvGuard(Vec<(String, Option<OsString>)>);

    impl EnvGuard {
        fn set(vars: &[(&str, Option<&str>)]) -> EnvGuard {
            let saved = vars
                .iter()
                .map(|(key, _)| ((*key).to_owned(), std::env::var_os(key)))
                .collect();
            for (key, value) in vars {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            EnvGuard(saved)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn parse_size_accepts_wxh() {
        assert_eq!(parse_size("1280x800"), Ok(Size::new(1280, 800)));
        assert_eq!(parse_size("640X480"), Ok(Size::new(640, 480)));
        assert_eq!(parse_size(" 1920 x 1080 "), Ok(Size::new(1920, 1080)));
        assert_eq!(parse_size("1x1"), Ok(Size::new(1, 1)));
    }

    #[test]
    fn parse_size_rejects_malformed_values() {
        for value in [
            "",
            "1280",
            "1280x",
            "x800",
            "1280x800x600",
            "1280xabc",
            "abcx800",
        ] {
            let error = parse_size(value).expect_err(value);
            assert!(!error.is_empty(), "`{value}` should explain the failure");
        }
    }

    #[test]
    fn parse_size_rejects_zero_dimensions() {
        assert!(parse_size("0x800").is_err());
        assert!(parse_size("1280x0").is_err());
        assert!(parse_size("0x0").is_err());
    }

    #[test]
    fn parse_renderer_is_case_insensitive() {
        assert_eq!(parse_renderer("auto"), Ok(RendererKind::Auto));
        assert_eq!(parse_renderer("AUTO"), Ok(RendererKind::Auto));
        assert_eq!(parse_renderer("Gl"), Ok(RendererKind::Gl));
        assert_eq!(parse_renderer(" pixman "), Ok(RendererKind::Pixman));
    }

    #[test]
    fn parse_renderer_rejects_unknown_kinds() {
        for value in ["", "vulkan", "softwar"] {
            let error = parse_renderer(value).expect_err(value);
            assert!(
                error.contains("auto"),
                "`{error}` should list the accepted kinds"
            );
        }
    }

    #[test]
    fn default_socket_path_prefers_adesk_socket() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _env = EnvGuard::set(&[
            ("ADESK_SOCKET", Some("/tmp/custom.sock")),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
        ]);
        assert_eq!(default_socket_path(), PathBuf::from("/tmp/custom.sock"));
    }

    #[test]
    fn default_socket_path_falls_back_to_xdg_runtime_dir() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _env = EnvGuard::set(&[
            ("ADESK_SOCKET", None),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
        ]);
        assert_eq!(
            default_socket_path(),
            PathBuf::from("/run/user/1000").join("adesk.sock")
        );
    }

    #[test]
    fn default_socket_path_falls_back_to_temp_dir() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _env = EnvGuard::set(&[("ADESK_SOCKET", None), ("XDG_RUNTIME_DIR", None)]);
        assert_eq!(
            default_socket_path(),
            std::env::temp_dir().join("adesk.sock")
        );
    }

    #[test]
    fn config_builders_override_fields() {
        let config = ServerConfig::new("/tmp/test.sock", CompositorConfig::default())
            .with_output_size(Size::new(800, 600))
            .with_renderer(RendererKind::Pixman)
            .with_app_dirs(vec![PathBuf::from("/opt/apps")]);
        assert_eq!(config.socket_path(), Path::new("/tmp/test.sock"));
        assert_eq!(config.compositor.output_size, Size::new(800, 600));
        assert_eq!(config.app_dirs, Some(vec![PathBuf::from("/opt/apps")]));

        let replaced = config.with_socket_path("/tmp/other.sock");
        assert_eq!(replaced.socket_path(), Path::new("/tmp/other.sock"));
    }

    #[test]
    fn viewer_socket_sibling_suffixes_the_file_stem() {
        assert_eq!(
            viewer_socket_sibling(Path::new("/run/user/1000/adesk.sock")),
            PathBuf::from("/run/user/1000/adesk-viewer.sock")
        );
    }

    #[test]
    fn viewer_socket_sibling_preserves_the_directory() {
        assert_eq!(
            viewer_socket_sibling(Path::new("/tmp/nested/deeper/adesk.sock")),
            PathBuf::from("/tmp/nested/deeper/adesk-viewer.sock")
        );
    }

    #[test]
    fn viewer_socket_sibling_preserves_the_extension() {
        // The suffix goes on the stem, so a multi-dot name keeps its last
        // extension: `adesk.v2` + `.sock` → `adesk.v2-viewer.sock`.
        assert_eq!(
            viewer_socket_sibling(Path::new("/tmp/adesk.v2.sock")),
            PathBuf::from("/tmp/adesk.v2-viewer.sock")
        );
        assert_eq!(
            viewer_socket_sibling(Path::new("/run/desk.custom")),
            PathBuf::from("/run/desk-viewer.custom")
        );
    }

    #[test]
    fn viewer_socket_sibling_handles_a_path_without_a_file_name() {
        // No file name (or an empty stem): the canonical default name in the
        // same directory.
        assert_eq!(
            viewer_socket_sibling(Path::new("")),
            PathBuf::from("adesk-viewer.sock")
        );
        assert_eq!(
            viewer_socket_sibling(Path::new("/")),
            PathBuf::from("/adesk-viewer.sock")
        );
    }

    #[test]
    fn viewer_socket_path_is_none_when_disabled() {
        let config = ServerConfig::new("/tmp/test.sock", CompositorConfig::default())
            .with_viewer_socket("/tmp/explicit.sock")
            .without_viewer();
        assert_eq!(config.viewer_socket_path(), None);
    }

    #[test]
    fn viewer_socket_path_prefers_an_explicit_override() {
        let config = ServerConfig::new("/tmp/test.sock", CompositorConfig::default())
            .with_viewer_socket("/tmp/explicit.sock");
        assert!(config.viewer.enabled);
        assert_eq!(
            config.viewer_socket_path(),
            Some(PathBuf::from("/tmp/explicit.sock"))
        );
    }

    #[test]
    fn viewer_socket_path_defaults_to_the_agp_sibling() {
        let config = ServerConfig::new("/tmp/test.sock", CompositorConfig::default());
        assert!(config.viewer.enabled);
        assert_eq!(
            config.viewer_socket_path(),
            Some(PathBuf::from("/tmp/test-viewer.sock"))
        );
        assert_eq!(
            viewer_socket_sibling(config.socket_path()),
            PathBuf::from("/tmp/test-viewer.sock")
        );
    }

    #[test]
    fn viewer_tcp_is_recorded_independently_of_the_unix_path() {
        let config = ServerConfig::new("/tmp/test.sock", CompositorConfig::default())
            .with_viewer_tcp("127.0.0.1:7100".parse().unwrap());
        assert_eq!(config.viewer.tcp, Some("127.0.0.1:7100".parse().unwrap()));
        assert_eq!(
            config.viewer_socket_path(),
            Some(PathBuf::from("/tmp/test-viewer.sock"))
        );

        let disabled = config.without_viewer();
        assert_eq!(disabled.viewer_socket_path(), None);
    }

    #[test]
    fn recordings_dir_defaults_beside_the_agp_socket() {
        let config = ServerConfig::new("/tmp/test.sock", CompositorConfig::default());
        assert!(
            config.viewer.recordings_dir.is_none(),
            "the baseline derives the directory, it does not pin one"
        );
        assert_eq!(
            config.recordings_dir(),
            PathBuf::from("/tmp/adesk-recordings")
        );
    }

    #[test]
    fn recordings_dir_prefers_an_explicit_override() {
        let config = ServerConfig::new("/tmp/test.sock", CompositorConfig::default())
            .with_recordings_dir("/var/lib/adesk/rec");
        assert_eq!(
            config.viewer.recordings_dir,
            Some(PathBuf::from("/var/lib/adesk/rec"))
        );
        assert_eq!(config.recordings_dir(), PathBuf::from("/var/lib/adesk/rec"));
    }

    #[test]
    fn recordings_dir_falls_back_when_the_socket_has_no_directory() {
        let config = ServerConfig::new("adesk.sock", CompositorConfig::default());
        assert_eq!(config.recordings_dir(), PathBuf::from("adesk-recordings"));
    }
}
