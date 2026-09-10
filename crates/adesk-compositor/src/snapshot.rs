//! Reply payloads of the command channel: [`StateSnapshot`] and [`RenderedFrame`].
//!
//! These types are the compositor's answer to `QueryState`, `RenderWindow` and
//! `RenderOutput` (`docs/architecture.md` §3). They stay free of Smithay types so
//! the server can use them without linking a compositor.

use adesk_core::{ImageBuffer, Rect, Size, WindowId, WindowInfo};

/// One rendered frame plus the causal information that produced it.
///
/// `commit_seq` is the per-window (or per-output) commit counter at render time;
/// `damage` lists the regions the frame accounts for, window-relative (or
/// output-relative for [`crate::RuntimeCommand::RenderOutput`]). The image is always
/// `Rgba8`; encoding is the server's job.
#[derive(Debug, Clone)]
pub struct RenderedFrame {
    /// Rendered pixels (row-major, `Rgba8`).
    pub image: ImageBuffer,
    /// Commit counter this frame reflects (0 for the output composition).
    pub commit_seq: u64,
    /// Damage regions used to produce the frame, already simplified.
    pub damage: Vec<Rect>,
}

impl RenderedFrame {
    /// Build a frame from an image, its commit counter and damage regions.
    pub fn new(image: ImageBuffer, commit_seq: u64, damage: Vec<Rect>) -> Self {
        RenderedFrame {
            image,
            commit_seq,
            damage,
        }
    }

    /// Pixel size of the frame.
    pub fn size(&self) -> Size {
        self.image.size()
    }
}

/// A point-in-time view of the compositor's window state.
///
/// Answered by [`crate::RuntimeCommand::QueryState`]. `seq` is the global event
/// sequence watermark at snapshot time, so a subscriber can tell whether the events
/// it has seen cover this snapshot or whether it must resync.
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    /// All windows known to the window manager, in creation order.
    pub windows: Vec<WindowInfo>,
    /// The active (visible, tiled) window, if any.
    pub active_window_id: Option<WindowId>,
    /// The window that currently holds keyboard focus, if any.
    pub keyboard_focus: Option<WindowId>,
    /// Global event sequence watermark (`0` = no events yet).
    pub seq: u64,
    /// Monotonic milliseconds since compositor start.
    pub ts_ms: u64,
}

impl StateSnapshot {
    /// The window with the given id, if known.
    pub fn window(&self, id: WindowId) -> Option<&WindowInfo> {
        self.windows.iter().find(|window| window.id == id)
    }

    /// Number of windows in the snapshot.
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    /// Whether the compositor knows no windows at all.
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::WindowState;

    fn window(id: u64) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            app_id: None,
            title: None,
            geometry: Rect {
                x: 0,
                y: 0,
                w: 1280,
                h: 800,
            },
            state: WindowState::Active,
            mapped: true,
            pid: None,
            created_seq: 1,
            last_commit_seq: 0,
            popup_count: 0,
        }
    }

    #[test]
    fn snapshot_lookup_and_helpers() {
        let snapshot = StateSnapshot {
            windows: vec![window(1), window(2)],
            active_window_id: Some(WindowId(2)),
            keyboard_focus: Some(WindowId(2)),
            seq: 42,
            ts_ms: 1000,
        };
        assert_eq!(snapshot.len(), 2);
        assert!(!snapshot.is_empty());
        assert_eq!(
            snapshot.window(WindowId(2)).map(|w| w.id),
            Some(WindowId(2))
        );
        assert!(snapshot.window(WindowId(9)).is_none());
    }

    #[test]
    fn rendered_frame_reports_image_size() {
        let image = ImageBuffer::new_rgba(4, 2);
        let frame = RenderedFrame::new(
            image,
            7,
            vec![Rect {
                x: 0,
                y: 0,
                w: 4,
                h: 2,
            }],
        );
        assert_eq!(frame.size(), Size { w: 4, h: 2 });
        assert_eq!(frame.commit_seq, 7);
        assert_eq!(frame.damage.len(), 1);
    }
}
