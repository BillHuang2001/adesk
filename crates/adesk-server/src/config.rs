//! Server configuration: socket path, compositor settings and app directories.
//!
//! [`ServerConfig`] is the single input of [`crate::Server::start`]; `adesk-testkit`
//! constructs it directly, the `adesk-server` binary builds it from CLI flags.

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
}

impl Default for ServerConfig {
    fn default() -> ServerConfig {
        ServerConfig {
            socket_path: default_socket_path(),
            compositor: CompositorConfig::default(),
            app_dirs: None,
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
        }
    }

    /// Overrides the socket path.
    pub fn with_socket_path(mut self, path: impl Into<PathBuf>) -> ServerConfig {
        self.socket_path = path.into();
        self
    }

    /// Replaces the compositor configuration wholesale.
    pub fn with_compositor(mut self, compositor: CompositorConfig) -> ServerConfig {
        self.compositor = compositor;
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

    /// The socket path this configuration will bind.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
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
}
