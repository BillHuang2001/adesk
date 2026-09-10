//! Small protocol vocabulary types shared by several methods (§2, §3, §5.1, §5.4).

use serde::{Deserialize, Serialize};

/// Encoding of an [`ImagePayload`](crate::ImagePayload) (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    /// PNG-encoded bytes in `data`.
    #[default]
    Png,
    /// Raw tightly packed RGBA8 pixels in `data` (`stride == width * 4`).
    Rgba8,
}

/// Renderer selected by the runtime, reported by `ping` (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RendererKind {
    /// EGL/GLES renderer (`gl` feature).
    Gl,
    /// Software pixman renderer.
    Pixman,
}

/// Resumption condition of `observe` (§5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    /// Resolve once the window has seen no counted surface commit for `quiet_ms`.
    Quiet {
        /// Milliseconds of surface quiet required.
        quiet_ms: u64,
    },
    /// Resolve on the first counted surface commit or window lifecycle event.
    Change,
    /// Wait the full `timeout_ms` and report what accumulated.
    Timeout,
}

/// Keys accepted by `keypress` (§3): a chord array or a single key string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeySpec {
    /// A single key name, e.g. `"a"` or `"RETURN"`.
    Single(String),
    /// A chord, e.g. `["CTRL", "L"]`: pressed in order, released in reverse.
    Chord(Vec<String>),
}

impl KeySpec {
    /// The keys in press order (a single key yields a one-element slice).
    pub fn keys(&self) -> &[String] {
        match self {
            KeySpec::Single(key) => std::slice::from_ref(key),
            KeySpec::Chord(keys) => keys,
        }
    }
}

impl From<&str> for KeySpec {
    fn from(key: &str) -> KeySpec {
        KeySpec::Single(key.to_owned())
    }
}
