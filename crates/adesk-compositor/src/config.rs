//! Compositor startup configuration.
//!
//! [`CompositorConfig`] is the complete, serializable-free description of how the
//! compositor thread should come up: virtual output size, renderer selection, xkb
//! keymap settings, Wayland socket name and event-channel capacity. The server maps
//! CLI flags onto it; tests use [`CompositorConfig::default`] or the builder methods.

use adesk_core::Size;

/// Which renderer the compositor should try to create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RendererKind {
    /// Try GL (surfaceless EGL) first, fall back to pixman with a warning.
    #[default]
    Auto,
    /// Require the GL renderer; fail startup if EGL is unavailable.
    Gl,
    /// Use the pixman software renderer (always available headless).
    Pixman,
}

/// The renderer that was actually created.
///
/// Reported by [`crate::ReadyInfo`] and used for the AGP `ping` result
/// (`"gl"` / `"pixman"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererName {
    /// EGL/GLES renderer (Mesa `llvmpipe` in a VM).
    Gl,
    /// pixman software renderer.
    Pixman,
}

impl RendererName {
    /// The wire name used by the AGP `ping` result.
    pub fn as_str(self) -> &'static str {
        match self {
            RendererName::Gl => "gl",
            RendererName::Pixman => "pixman",
        }
    }
}

impl std::fmt::Display for RendererName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// xkb keymap settings for the virtual keyboard.
///
/// Defaults produce a plain US layout (`rules=evdev`, `model=pc105`,
/// `layout=us`), matching `docs/architecture.md` §8. The server exposes these as
/// `--xkb-layout`, `--xkb-variant`, `--xkb-model`, `--xkb-rules`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XkbSettings {
    /// xkb rules file (default `"evdev"`).
    pub rules: String,
    /// xkb model (default `"pc105"`).
    pub model: String,
    /// Comma-separated layout list (default `"us"`).
    pub layout: String,
    /// Comma-separated variant list (default `""`).
    pub variant: String,
    /// Comma-separated option list, e.g. `"grp:alt_shift_toggle"` (default `None`).
    pub options: Option<String>,
}

impl Default for XkbSettings {
    fn default() -> Self {
        XkbSettings {
            rules: "evdev".to_owned(),
            model: "pc105".to_owned(),
            layout: "us".to_owned(),
            variant: String::new(),
            options: None,
        }
    }
}

impl XkbSettings {
    /// US layout with default rules/model.
    pub fn us() -> Self {
        Self::default()
    }

    /// Build the Smithay/xkb config describing these settings.
    pub fn to_xkb_config(&self) -> smithay::input::keyboard::XkbConfig<'_> {
        smithay::input::keyboard::XkbConfig {
            rules: &self.rules,
            model: &self.model,
            layout: &self.layout,
            variant: &self.variant,
            options: self.options.clone(),
        }
    }
}

/// Everything the compositor needs to start.
///
/// `Default` is the documented runtime default: `1280x800` virtual output,
/// renderer `Auto`, US keyboard, automatically named Wayland socket, event
/// broadcast capacity 4096 (`docs/architecture.md` §1).
#[derive(Debug, Clone)]
pub struct CompositorConfig {
    /// Size of the single virtual output; windows are tiled to fill it.
    pub output_size: Size,
    /// Renderer selection policy.
    pub renderer: RendererKind,
    /// xkb keymap settings for the virtual keyboard.
    pub xkb: XkbSettings,
    /// Explicit Wayland socket name (e.g. `"wayland-7"`).
    ///
    /// `None` binds the next free `wayland-N` name.
    pub socket_name: Option<String>,
    /// Capacity of the `RuntimeEvent` broadcast channel.
    ///
    /// Must be ≥ 4096 per the architecture contract; values below 1 are clamped
    /// to 1 when the channel is created.
    pub event_channel_capacity: usize,
}

impl Default for CompositorConfig {
    fn default() -> Self {
        CompositorConfig {
            output_size: Size { w: 1280, h: 800 },
            renderer: RendererKind::default(),
            xkb: XkbSettings::default(),
            socket_name: None,
            event_channel_capacity: 4096,
        }
    }
}

impl CompositorConfig {
    /// Default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the virtual output size.
    pub fn with_output_size(mut self, size: Size) -> Self {
        self.output_size = size;
        self
    }

    /// Set the renderer selection policy.
    pub fn with_renderer(mut self, renderer: RendererKind) -> Self {
        self.renderer = renderer;
        self
    }

    /// Set the xkb keymap settings.
    pub fn with_xkb(mut self, xkb: XkbSettings) -> Self {
        self.xkb = xkb;
        self
    }

    /// Bind an explicit Wayland socket name.
    pub fn with_socket_name(mut self, name: impl Into<String>) -> Self {
        self.socket_name = Some(name.into());
        self
    }

    /// Set the event broadcast capacity (clamped to at least 1 on use).
    pub fn with_event_channel_capacity(mut self, capacity: usize) -> Self {
        self.event_channel_capacity = capacity;
        self
    }

    /// The output size in pixels as a Smithay physical size (`wl_output` mode).
    pub(crate) fn physical_size(&self) -> smithay::utils::Size<i32, smithay::utils::Physical> {
        smithay::utils::Size::from((self.output_size.w as i32, self.output_size.h as i32))
    }

    /// The virtual monitor size in millimeters for `wl_output` geometry.
    ///
    /// The headless output has no real panel, so we report the size its pixels
    /// would occupy at 96 DPI (rounded up to at least 1mm per axis). This keeps
    /// DPI-derived client heuristics sane; the pixel size lives in `Mode`.
    pub(crate) fn monitor_size_mm(&self) -> smithay::utils::Size<i32, smithay::utils::Raw> {
        let to_mm = |px: u32| ((px as f64) * 25.4 / 96.0).round().max(1.0) as i32;
        smithay::utils::Size::from((to_mm(self.output_size.w), to_mm(self.output_size.h)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_architecture_contract() {
        let config = CompositorConfig::default();
        assert_eq!(config.output_size, Size { w: 1280, h: 800 });
        assert_eq!(config.renderer, RendererKind::Auto);
        assert_eq!(config.xkb.layout, "us");
        assert_eq!(config.xkb.rules, "evdev");
        assert!(config.socket_name.is_none());
        assert_eq!(config.event_channel_capacity, 4096);
    }

    #[test]
    fn xkb_config_borrows_settings() {
        let settings = XkbSettings::us();
        let xkb = settings.to_xkb_config();
        assert_eq!(xkb.layout, "us");
        assert_eq!(xkb.rules, "evdev");
        assert_eq!(xkb.model, "pc105");
        assert_eq!(xkb.variant, "");
        assert!(xkb.options.is_none());
    }

    #[test]
    fn renderer_names_are_wire_names() {
        assert_eq!(RendererName::Gl.as_str(), "gl");
        assert_eq!(RendererName::Pixman.as_str(), "pixman");
        assert_eq!(RendererName::Pixman.to_string(), "pixman");
    }

    #[test]
    fn monitor_size_is_millimeters_at_96_dpi() {
        // 1280x800 px at 96 DPI -> 339x212 mm (wl_output geometry, not pixels).
        let config = CompositorConfig::default();
        assert_eq!(
            config.monitor_size_mm(),
            smithay::utils::Size::from((339, 212))
        );
        // Degenerate sizes still report at least 1mm per axis.
        let tiny = CompositorConfig::new().with_output_size(Size { w: 0, h: 1 });
        assert_eq!(tiny.monitor_size_mm(), smithay::utils::Size::from((1, 1)));
    }

    #[test]
    fn builder_overrides_defaults() {
        let config = CompositorConfig::new()
            .with_output_size(Size { w: 800, h: 600 })
            .with_renderer(RendererKind::Pixman)
            .with_socket_name("wayland-9")
            .with_event_channel_capacity(8192);
        assert_eq!(config.output_size, Size { w: 800, h: 600 });
        assert_eq!(config.renderer, RendererKind::Pixman);
        assert_eq!(config.socket_name.as_deref(), Some("wayland-9"));
        assert_eq!(config.event_channel_capacity, 8192);
        assert_eq!(
            config.physical_size(),
            smithay::utils::Size::from((800, 600))
        );
    }
}
