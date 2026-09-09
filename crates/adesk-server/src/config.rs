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
    /// A default configuration (equivalent to [`ServerConfig::default`]).
    pub fn new() -> ServerConfig {
        ServerConfig::default()
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
    todo!()
}

/// Parses a `--renderer` value: `auto`, `gl` or `pixman` (case-insensitive).
///
/// # Errors
///
/// Returns a human-readable message for any other value.
pub fn parse_renderer(value: &str) -> std::result::Result<RendererKind, String> {
    todo!()
}
