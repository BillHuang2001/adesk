//! Server-side [`adesk_inspector::InspectionSource`]: cached inspection state
//! plus the async refresh that renders the full output and queries window state.
//!
//! `adesk-inspector` is pure and synchronous: its trait must not block the
//! compositor, so the server keeps an [`crate::inspection::InspectionSnapshot`]
//! in [`crate::inspection::InspectionCache`] and refreshes it asynchronously
//! ([`crate::inspection::refresh`]) before each `inspect_capture` /
//! `inspect_subscribe` frame.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use adesk_compositor::RuntimeCommand;
use adesk_core::{ImageBuffer, OverlayKind, Point, Rect, Region, WindowId, WindowInfo};
use adesk_inspector::{ActionMarker, CommitInfo, InspectionInput, InspectionSource};
use tokio::sync::oneshot;

use crate::context::ServerContext;
use crate::error::{Result, ServerError};

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
///
/// The snapshot is stored behind an `Arc` so [`InspectionCache::snapshot`] can
/// hand out a copy while holding the read guard only for the cheap pointer
/// clone, never for the deep copy of the (large) output frame.
#[derive(Clone, Default)]
pub struct InspectionCache {
    inner: Arc<RwLock<Option<Arc<InspectionSnapshot>>>>,
}

impl InspectionCache {
    /// An empty cache.
    pub fn new() -> InspectionCache {
        InspectionCache::default()
    }

    /// Stores a freshly refreshed snapshot.
    pub fn store(&self, snapshot: InspectionSnapshot) {
        *self
            .inner
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(Arc::new(snapshot));
    }

    /// The current snapshot, if the cache has been primed.
    ///
    /// Only the cheap `Arc` handle is cloned while the read guard is held; the
    /// deep copy of the (large) surface frame happens after the guard is
    /// dropped, so a concurrent `InspectionCache::update` never waits for it.
    pub fn snapshot(&self) -> Option<InspectionSnapshot> {
        let cached = self
            .inner
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        cached.map(|snapshot| (*snapshot).clone())
    }

    /// Mutates the cached snapshot in place; a no-op while the cache is empty.
    ///
    /// The event pump folds single events in this way: it must not clone the
    /// (large) base frame on every `surface_commit`. The snapshot is
    /// copy-on-write (`Arc::make_mut`), so the frame is duplicated only in the
    /// rare case where a concurrent [`InspectionCache::snapshot`] still holds a
    /// handle to it.
    pub(crate) fn update(&self, update: impl FnOnce(&mut InspectionSnapshot)) {
        if let Some(snapshot) = self
            .inner
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
        {
            update(Arc::make_mut(snapshot));
        }
    }
}

/// Translates window-relative damage rects into output coordinates.
///
/// `geometry` is the window's output-coordinate rect; `None` (geometry not yet
/// known) leaves the rects untouched.
pub(crate) fn damage_to_output(damage: &Region, geometry: Option<Rect>) -> Vec<Rect> {
    let (offset_x, offset_y) = geometry.map_or((0, 0), |rect| (rect.x, rect.y));
    damage
        .simplified()
        .into_iter()
        .map(|mut rect| {
            rect.x += offset_x;
            rect.y += offset_y;
            rect
        })
        .collect()
}

/// Maps a compositor command reply failure into a server error.
///
/// Command replies carry the umbrella [`adesk_core::Error`]; the codes the
/// render and state paths produce are mapped back onto their
/// [`adesk_compositor::CompositorError`] counterparts so the AGP error code
/// survives.
fn compositor_reply(error: adesk_core::Error) -> ServerError {
    use adesk_core::ErrorCode;
    match error.code {
        ErrorCode::InvalidRequest => ServerError::Compositor(
            adesk_compositor::CompositorError::InvalidRequest(error.message),
        ),
        ErrorCode::RenderFailed | ErrorCode::CaptureFailed => {
            ServerError::Compositor(adesk_compositor::CompositorError::Render(error.message))
        }
        ErrorCode::ShuttingDown => ServerError::ShuttingDown,
        _ => ServerError::Internal(error.message),
    }
}

impl InspectionSource for InspectionCache {
    /// Converts the cached snapshot into an `InspectionInput`, collecting only
    /// the state the requested overlays need.
    fn inspection_input(
        &self,
        overlays: &[OverlayKind],
    ) -> adesk_inspector::Result<InspectionInput> {
        let Some(snapshot) = self.snapshot() else {
            return Err(adesk_inspector::Error::InvalidRequest(
                "inspection cache is not primed; call `inspection::refresh` first".to_owned(),
            ));
        };
        let mut builder = InspectionInput::builder(snapshot.frame);
        let mut windows_needed = false;
        let mut active_needed = false;
        for overlay in overlays {
            match overlay {
                OverlayKind::Damage => builder = builder.damage(snapshot.damage.clone()),
                OverlayKind::SurfaceBounds | OverlayKind::WindowIds | OverlayKind::AppIds => {
                    windows_needed = true;
                }
                OverlayKind::Focus => {
                    windows_needed = true;
                    active_needed = true;
                }
                OverlayKind::Cursor => builder = builder.cursor(snapshot.cursor),
                OverlayKind::Actions => builder = builder.actions(snapshot.actions.clone()),
                OverlayKind::CommitTiming => builder = builder.commit(snapshot.commit),
            }
        }
        if windows_needed {
            builder = builder.windows(snapshot.windows.clone());
        }
        if active_needed {
            builder = builder.active(snapshot.active);
        }
        Ok(builder.build())
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
    let (reply, rendered) = oneshot::channel();
    context.compositor.send(RuntimeCommand::RenderOutput {
        overlays: Vec::new(),
        region: None,
        max_dimension: None,
        reply,
    })?;
    let frame = rendered
        .await
        .map_err(|_| ServerError::ShuttingDown)?
        .map_err(compositor_reply)?;

    let (reply, state) = oneshot::channel();
    context
        .compositor
        .send(RuntimeCommand::QueryState { reply })?;
    let state = state.await.map_err(|_| ServerError::ShuttingDown)?;

    let now_ms = context.now_ms();
    let observer = context.observer.snapshot();
    let latest_commit = observer
        .windows
        .iter()
        .filter(|window| window.last_commit_at > 0)
        .max_by_key(|window| window.last_commit_at);
    let (damage, commit) = match latest_commit {
        Some(window) => (
            damage_to_output(&window.last_damage, window.geometry),
            Some(CommitInfo {
                commit_seq: window.last_commit_seq,
                age_ms: now_ms.saturating_sub(window.last_commit_at),
            }),
        ),
        None => (Vec::new(), None),
    };

    // Resolve every action's geometry from the observer snapshot taken above
    // (`window_state` would take a fresh lock and clone the whole window state
    // once per record); the snapshot already carries each window's geometry.
    let geometry_by_window: HashMap<WindowId, Rect> = observer
        .windows
        .iter()
        .filter_map(|window| window.geometry.map(|geometry| (window.window_id, geometry)))
        .collect();

    let actions = context
        .observer
        .action_registry()
        .records()
        .iter()
        .map(|record| {
            let geometry = record
                .window_id
                .and_then(|window_id| geometry_by_window.get(&window_id).copied());
            crate::translate::action_marker(record, geometry, now_ms)
        })
        .collect();

    let snapshot = InspectionSnapshot {
        frame: frame.image,
        windows: state.windows.clone(),
        active: state.active_window_id,
        cursor: context.cursor.get(),
        actions,
        damage,
        commit,
        seq: state.seq,
        ts_ms: now_ms,
    };
    context.inspection.store(snapshot.clone());
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{ActionId, WindowState};

    fn snapshot() -> InspectionSnapshot {
        InspectionSnapshot {
            frame: ImageBuffer::new_rgba(8, 6),
            windows: vec![WindowInfo {
                id: WindowId(1),
                app_id: None,
                title: Some("window".to_owned()),
                geometry: Rect {
                    x: 0,
                    y: 0,
                    w: 8,
                    h: 6,
                },
                state: WindowState::Active,
                mapped: true,
                pid: Some(4242),
                created_seq: 3,
                last_commit_seq: 7,
                popup_count: 0,
            }],
            active: Some(WindowId(1)),
            cursor: Some(Point { x: 2, y: 3 }),
            actions: vec![ActionMarker {
                action_id: ActionId(9),
                kind: adesk_inspector::ActionKind::Click,
                position: Some(Point { x: 1, y: 1 }),
                age_ms: 5,
            }],
            damage: vec![Rect {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            }],
            commit: Some(CommitInfo {
                commit_seq: 7,
                age_ms: 12,
            }),
            seq: 42,
            ts_ms: 1000,
        }
    }

    #[test]
    fn store_and_snapshot_round_trip() {
        let cache = InspectionCache::new();
        assert!(cache.snapshot().is_none());
        cache.store(snapshot());
        let cached = cache.snapshot().expect("primed");
        assert_eq!(cached.seq, 42);
        assert_eq!(cached.frame.size(), adesk_core::Size::new(8, 6));
        assert_eq!(cached.active, Some(WindowId(1)));
    }

    #[test]
    fn unprimed_cache_rejects_inspection_input() {
        let cache = InspectionCache::new();
        let error = cache
            .inspection_input(&[OverlayKind::Focus])
            .expect_err("empty cache");
        assert!(matches!(error, adesk_inspector::Error::InvalidRequest(_)));
    }

    #[test]
    fn inspection_input_collects_only_requested_state() {
        let cache = InspectionCache::new();
        cache.store(snapshot());

        let cursor_only = cache.inspection_input(&[OverlayKind::Cursor]).unwrap();
        assert!(cursor_only.windows.is_empty());
        assert!(cursor_only.active.is_none());
        assert!(cursor_only.damage.is_empty());
        assert!(cursor_only.actions.is_empty());
        assert!(cursor_only.commit.is_none());
        assert_eq!(cursor_only.cursor, Some(Point { x: 2, y: 3 }));

        let focus = cache.inspection_input(&[OverlayKind::Focus]).unwrap();
        assert_eq!(focus.windows.len(), 1);
        assert_eq!(focus.active, Some(WindowId(1)));
        assert!(focus.cursor.is_none());

        let damage = cache
            .inspection_input(&[OverlayKind::Damage, OverlayKind::CommitTiming])
            .unwrap();
        assert_eq!(damage.damage.len(), 1);
        assert_eq!(damage.commit.map(|commit| commit.commit_seq), Some(7));
        assert!(damage.windows.is_empty());
    }

    #[test]
    fn update_folds_events_into_the_cached_snapshot() {
        let cache = InspectionCache::new();
        cache.update(|_| panic!("empty cache must not be touched"));
        cache.store(snapshot());
        cache.update(|cached| {
            cached.seq = 99;
            cached.active = None;
        });
        let cached = cache.snapshot().unwrap();
        assert_eq!(cached.seq, 99);
        assert!(cached.active.is_none());
    }

    #[test]
    fn snapshot_is_an_independent_copy_of_the_cached_state() {
        let cache = InspectionCache::new();
        cache.store(snapshot());
        // A held snapshot must not observe later in-place updates (this also
        // drives the copy-on-write branch while an `Arc` handle is live).
        let taken = cache.snapshot().expect("primed");
        cache.update(|cached| {
            cached.seq = 99;
            cached.active = None;
        });
        assert_eq!(taken.seq, 42);
        assert_eq!(taken.active, Some(WindowId(1)));
        let current = cache.snapshot().unwrap();
        assert_eq!(current.seq, 99);
        assert!(current.active.is_none());
    }

    #[test]
    fn damage_to_output_offsets_by_window_geometry() {
        let damage = Region::from_rect(Rect {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
        });
        let translated = damage_to_output(
            &damage,
            Some(Rect {
                x: 10,
                y: 20,
                w: 100,
                h: 100,
            }),
        );
        assert_eq!(
            translated,
            vec![Rect {
                x: 11,
                y: 22,
                w: 3,
                h: 4
            }]
        );
        assert_eq!(damage_to_output(&damage, None), damage.simplified());
    }
}
