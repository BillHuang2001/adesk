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
    if region.is_empty() || bounds.is_empty() {
        return Region::empty();
    }

    // Clip into one scratch buffer: `bounds` is non-empty here and `Region`
    // never stores empty rects, so every `intersect` result is non-empty.
    let mut rects = Vec::with_capacity(region.len());
    for rect in region.rects() {
        if let Some(part) = rect.intersect(bounds) {
            rects.push(part);
        }
    }

    // Sort *before* coalescing. Merging keeps the earlier rect's `(y, x)` and
    // only grows its extent, so a `(y, x, h, w)`-sorted input coalesces to a
    // `(y, x, h, w)`-sorted result. That makes this exactly
    // `clip(bounds).simplified()` while skipping the throwaway clipped
    // `Region` and the clone inside `Region::simplified`.
    rects.sort_unstable_by_key(|rect| (rect.y, rect.x, rect.h, rect.w));

    let mut out = Region::empty();
    for rect in rects {
        out.push(rect);
    }
    out.coalesce();

    // `min_area == 0` keeps every coalesced rect, so the common path returns
    // the coalesced region as is. Only rebuild (one further allocation) when a
    // rect is actually below the threshold.
    if min_area == 0 || !out.rects().iter().any(|rect| rect.area() < min_area as u64) {
        return out;
    }

    let mut filtered = Region::empty();
    for rect in out.rects() {
        if rect.area() >= min_area as u64 {
            filtered.push(*rect);
        }
    }
    filtered
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
        if self.bounds.is_empty() {
            return;
        }
        // Clip straight into `pending`. This is exactly what
        // `damage.clip(&self.bounds)` does per rect, but without allocating (and
        // then copying) a throwaway `Region`.
        for rect in damage.rects() {
            if let Some(part) = rect.intersect(&self.bounds) {
                self.pending.push(part);
            }
        }
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
