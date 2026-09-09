//! Damage accumulation and coalescing.
//!
//! Damage is *evidence*, not a guarantee of visual difference
//! (`docs/architecture.md` §5). `adesk-compositor` feeds every `SurfaceCommit`
//! into a [`DamageAccumulator`] (one per window); the coalesced result becomes
//! `changed_regions` in observations and the per-element damage of a scene.
//! This module is pure arithmetic over [`adesk_core::Region`] — it is
//! independent of the renderer and needs no GPU.

use adesk_core::{Rect, Region};

/// Clips `region` to `bounds`, merges overlapping/adjacent rects and drops
/// rects whose area is below `min_area` pixels.
///
/// `min_area == 0` keeps everything non-empty. The result is coalesced,
/// empty-free and deterministic (ordered by `(y, x, h, w)`).
pub fn coalesce_damage(region: &Region, bounds: &Rect, min_area: u32) -> Region {
    let mut out = Region::empty();
    for rect in region.clip(bounds).simplified() {
        if rect.area() >= min_area as u64 {
            out.push(rect);
        }
    }
    out
}

/// Per-window damage accumulator fed by `SurfaceCommit` events.
///
/// The accumulator stores the raw union of damage rects, clipped to the
/// window's current geometry. [`DamageAccumulator::take`] returns the
/// coalesced damage and resets the pending set (used after rendering an image
/// so the next observation reports only new damage), while
/// [`DamageAccumulator::peek`] inspects without resetting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DamageAccumulator {
    bounds: Rect,
    pending: Region,
    commits: u64,
    last_commit_seq: u64,
}

impl DamageAccumulator {
    /// Creates an accumulator clipping to `bounds` (the window geometry).
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            pending: Region::empty(),
            commits: 0,
            last_commit_seq: 0,
        }
    }

    /// The clip rectangle (window geometry).
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Changes the clip rectangle, re-clipping any pending damage.
    pub fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
        self.pending = self.pending.clip(&bounds);
    }

    /// Records one commit: `damage` is the surface-tree damage of that commit,
    /// in window coordinates.
    pub fn record_commit(&mut self, commit_seq: u64, damage: &Region) {
        self.commits += 1;
        self.last_commit_seq = commit_seq;
        self.pending.extend(&damage.clip(&self.bounds));
    }

    /// Records a single damaged rectangle (clipped to the bounds).
    pub fn record_rect(&mut self, rect: Rect) {
        if let Some(clipped) = rect.intersect(&self.bounds) {
            self.pending.push(clipped);
        }
    }

    /// Coalesced damage accumulated so far, without resetting.
    pub fn peek(&self) -> Region {
        coalesce_damage(&self.pending, &self.bounds, 0)
    }

    /// Coalesced damage accumulated so far; resets the pending damage.
    pub fn take(&mut self) -> Region {
        let damage = self.peek();
        self.pending = Region::empty();
        damage
    }

    /// Drops pending damage without reporting it.
    pub fn clear(&mut self) {
        self.pending = Region::empty();
    }

    /// Whether there is pending (not yet taken) damage.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Number of commits recorded over the accumulator's lifetime.
    pub fn commits(&self) -> u64 {
        self.commits
    }

    /// Commit sequence of the most recent recorded commit (`0` if none).
    pub fn last_commit_seq(&self) -> u64 {
        self.last_commit_seq
    }
}
