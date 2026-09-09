//! Server-side [`InspectionSource`]: cached inspection state plus the async
//! refresh that renders the full output and queries window state.
//!
//! `adesk-inspector` is pure and synchronous: its trait must not block the
//! compositor, so the server keeps an [`InspectionSnapshot`] in
//! [`InspectionCache`] and refreshes it asynchronously ([`refresh`]) before each
//! `inspect_capture` / `inspect_subscribe` frame.

use std::sync::{Arc, RwLock};

use adesk_core::{ImageBuffer, OverlayKind, Point, Rect, WindowId, WindowInfo};
use adesk_inspector::{ActionMarker, CommitInfo, InspectionInput, InspectionSource};

use crate::context::ServerContext;
use crate::error::Result;

/// Everything the inspector needs to paint one full-output frame.
#[derive(Debug, Clone)]
pub struct InspectionSnapshot {
    /// Full virtual output, rendered without overlays and without crop.
    pub frame: ImageBuffer,
    /// Windows in draw order (tiled to fill the output).
    pub windows: Vec<WindowInfo>,
    /// Active (focused) window.
    pub active: Option<WindowId>,
    /// Last pointer position the runtime commanded.
    pub cursor: Option<Point>,
    /// Recent action markers; `age_ms` is relative to `ts_ms`.
    pub actions: Vec<ActionMarker>,
    /// Damage of the most recent counted commit.
    pub damage: Vec<Rect>,
    /// Most recent counted commit.
    pub commit: Option<CommitInfo>,
    /// Compositor state sequence this snapshot was taken at.
    pub seq: u64,
    /// Monotonic timestamp this snapshot was taken at.
    pub ts_ms: u64,
}

/// Cached snapshot exposed through the synchronous [`InspectionSource`] trait.
///
/// Cheap to clone; all clones share one cache. `None` means the cache has not
/// been primed yet — [`InspectionSource::inspection_input`] then returns
/// `InvalidRequest`.
#[derive(Clone, Default)]
pub struct InspectionCache {
    inner: Arc<RwLock<Option<InspectionSnapshot>>>,
}

impl InspectionCache {
    /// An empty cache.
    pub fn new() -> InspectionCache {
        InspectionCache::default()
    }

    /// Stores a freshly refreshed snapshot.
    pub fn store(&self, snapshot: InspectionSnapshot) {
        *self.inner.write().unwrap_or_else(|error| error.into_inner()) = Some(snapshot);
    }

    /// The current snapshot, if the cache has been primed.
    pub fn snapshot(&self) -> Option<InspectionSnapshot> {
        self.inner
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl InspectionSource for InspectionCache {
    /// Converts the cached snapshot into an `InspectionInput`, collecting only
    /// the state the requested overlays need.
    fn inspection_input(&self, overlays: &[OverlayKind]) -> adesk_inspector::Result<InspectionInput> {
        todo!()
    }
}

/// Renders the full output (`RenderOutput { overlays: [], region: None,
/// max_dimension: None }`) and snapshots windows/active window (`QueryState`),
/// cursor, damage, commit and action markers, producing a new
/// [`InspectionSnapshot`] for the cache.
///
/// Overlays are composited at full output resolution and cropped/downscaled
/// afterwards, so overlay coordinates need no translation.
///
/// # Errors
///
/// Returns [`crate::ServerError::Compositor`] when the render or state query
/// fails.
pub async fn refresh(context: &ServerContext) -> Result<InspectionSnapshot> {
    todo!()
}
