//! Geometry primitives: [`Point`], [`Size`], [`Rect`] and [`Region`].
//!
//! Rectangles use half-open intervals: a rect covers `x..x+w` and `y..y+h`, so
//! [`Rect::right`] and [`Rect::bottom`] are exclusive edges. Coordinates are
//! window-relative; conversion to output coordinates happens in the window
//! model (`adesk-wm`), never with hard-coded constants.

use serde::{Deserialize, Serialize};

/// Saturating cast from `i64` to `i32` (never wraps, never panics).
pub(crate) const fn clamp_i32(value: i64) -> i32 {
    if value > i32::MAX as i64 {
        i32::MAX
    } else if value < i32::MIN as i64 {
        i32::MIN
    } else {
        value as i32
    }
}

/// A point in window-relative pixel coordinates.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Point {
    /// Horizontal offset in pixels.
    pub x: i32,
    /// Vertical offset in pixels.
    pub y: i32,
}

impl Point {
    /// The origin `(0, 0)`.
    pub const ORIGIN: Point = Point { x: 0, y: 0 };

    /// Creates a point.
    pub const fn new(x: i32, y: i32) -> Point {
        Point { x, y }
    }
}

/// A width/height pair in pixels.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Size {
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

impl Size {
    /// The zero size `0x0`.
    pub const ZERO: Size = Size { w: 0, h: 0 };

    /// Creates a size.
    pub const fn new(w: u32, h: u32) -> Size {
        Size { w, h }
    }

    /// Returns `true` when either dimension is zero.
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// A window-relative pixel rectangle: covers `x..x+w` and `y..y+h` (half-open).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Rect {
    /// Left edge in pixels.
    pub x: i32,
    /// Top edge in pixels.
    pub y: i32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

impl Rect {
    /// The empty rectangle at the origin.
    pub const EMPTY: Rect = Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };

    /// Creates a rectangle.
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    /// Creates a rectangle at the origin `(0, 0)` with the given size.
    pub const fn from_size(size: Size) -> Rect {
        Rect {
            x: 0,
            y: 0,
            w: size.w,
            h: size.h,
        }
    }

    /// Exclusive right edge (`x + w`), saturating at `i32::MAX`.
    pub const fn right(&self) -> i32 {
        clamp_i32(self.x as i64 + self.w as i64)
    }

    /// Exclusive bottom edge (`y + h`), saturating at `i32::MAX`.
    pub const fn bottom(&self) -> i32 {
        clamp_i32(self.y as i64 + self.h as i64)
    }

    /// Returns `true` when width or height is zero.
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Returns the size of the rectangle.
    pub const fn size(&self) -> Size {
        Size {
            w: self.w,
            h: self.h,
        }
    }

    /// Returns the area in pixels.
    pub const fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }

    /// Returns `true` when `p` lies inside the half-open rect.
    ///
    /// An empty rect contains nothing, so `(x, y)` is inside a `1x1` rect but
    /// `right()`/`bottom()` themselves are outside.
    pub fn contains(&self, p: Point) -> bool {
        if self.is_empty() {
            return false;
        }
        let (px, py) = (p.x as i64, p.y as i64);
        px >= self.x as i64
            && px < self.x as i64 + self.w as i64
            && py >= self.y as i64
            && py < self.y as i64 + self.h as i64
    }

    /// Intersection with `other`, or `None` when they are disjoint or either is
    /// empty. Touching edges (zero-area overlap) yield `None`.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        if self.is_empty() || other.is_empty() {
            return None;
        }
        let x0 = self.x.max(other.x) as i64;
        let y0 = self.y.max(other.y) as i64;
        let x1 = (self.x as i64 + self.w as i64).min(other.x as i64 + other.w as i64);
        let y1 = (self.y as i64 + self.h as i64).min(other.y as i64 + other.h as i64);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Rect {
            x: clamp_i32(x0),
            y: clamp_i32(y0),
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
        })
    }

    /// Smallest rect covering both operands.
    ///
    /// Empty operands are ignored: `union` with [`Rect::EMPTY`] returns the
    /// other rect, and two empty rects yield [`Rect::EMPTY`].
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x0 = self.x.min(other.x) as i64;
        let y0 = self.y.min(other.y) as i64;
        let x1 = (self.x as i64 + self.w as i64).max(other.x as i64 + other.w as i64);
        let y1 = (self.y as i64 + self.h as i64).max(other.y as i64 + other.h as i64);
        Rect {
            x: clamp_i32(x0),
            y: clamp_i32(y0),
            w: (x1 - x0).min(u32::MAX as i64) as u32,
            h: (y1 - y0).min(u32::MAX as i64) as u32,
        }
    }
}

/// An ordered list of rectangles that may overlap.
///
/// Empty rectangles are never stored: [`Region::push`] and [`Region::extend`]
/// drop them, [`Region::clip`] filters them, and [`Region::coalesce`] removes
/// any that appear. The region is not a canonical form; call
/// [`Region::simplified`] when a deterministic, minimal list is needed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Region {
    rects: Vec<Rect>,
}

impl Region {
    /// An empty region.
    pub const fn empty() -> Region {
        Region { rects: Vec::new() }
    }

    /// A region containing exactly `rect`; an empty rect yields an empty region.
    pub fn from_rect(rect: Rect) -> Region {
        let mut region = Region::empty();
        region.push(rect);
        region
    }

    /// Appends `rect`, ignoring empty rectangles.
    pub fn push(&mut self, rect: Rect) {
        if !rect.is_empty() {
            self.rects.push(rect);
        }
    }

    /// Appends all rectangles of `other`, ignoring empty ones.
    pub fn extend(&mut self, other: &Region) {
        self.rects
            .extend(other.rects.iter().filter(|rect| !rect.is_empty()).copied());
    }

    /// Returns the part of this region inside `clip`.
    ///
    /// The result is not coalesced; the caller decides when to simplify.
    pub fn clip(&self, clip: &Rect) -> Region {
        let mut out = Region::empty();
        if clip.is_empty() {
            return out;
        }
        for rect in &self.rects {
            if let Some(part) = rect.intersect(clip) {
                out.push(part);
            }
        }
        out
    }

    /// Merges overlapping or edge-adjacent rectangles in place.
    ///
    /// Two rectangles are merged into their bounding box when they overlap or
    /// touch along a shared edge segment; corner-touching rectangles stay
    /// separate. Merging an L-shaped overlap into a bounding box adds area,
    /// which is acceptable because a region is evidence of damage, not an
    /// exact set. Runs until no pair can be merged, so the result is stable and
    /// exact duplicates collapse into one rect.
    pub fn coalesce(&mut self) {
        self.rects.retain(|rect| !rect.is_empty());
        let mut merged = true;
        while merged {
            merged = false;
            'search: for i in 0..self.rects.len() {
                for j in (i + 1)..self.rects.len() {
                    if let Some(union) = merge_pair(self.rects[i], self.rects[j]) {
                        self.rects[i] = union;
                        self.rects.remove(j);
                        merged = true;
                        break 'search;
                    }
                }
            }
        }
    }

    /// Coalesced, empty-free, deduplicated rectangles sorted by `(y, x, h, w)`.
    ///
    /// The deterministic order makes observations reproducible across replays.
    pub fn simplified(&self) -> Vec<Rect> {
        let mut region = self.clone();
        region.coalesce();
        region
            .rects
            .sort_by_key(|rect| (rect.y, rect.x, rect.h, rect.w));
        region.rects
    }

    /// Bounding box of all rectangles, or `None` when the region is empty.
    pub fn bounds(&self) -> Option<Rect> {
        let mut iter = self.rects.iter().filter(|rect| !rect.is_empty());
        let first = *iter.next()?;
        Some(iter.fold(first, |acc, rect| acc.union(rect)))
    }

    /// Returns `true` when the region holds no rectangles.
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// Number of stored rectangles.
    pub fn len(&self) -> usize {
        self.rects.len()
    }

    /// The stored rectangles in insertion order (all non-empty, may overlap).
    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }
}

/// Returns the bounding box of `a` and `b` when they overlap or touch along an
/// edge, otherwise `None`. Corner-touching rectangles stay separate.
///
/// Merging an L-shaped overlap into its bounding box adds area; that is
/// acceptable because a region is evidence of damage, not an exact set.
fn merge_pair(a: Rect, b: Rect) -> Option<Rect> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    if a.intersect(&b).is_some() {
        return Some(a.union(&b));
    }
    let touches_x = a.right() == b.x || b.right() == a.x;
    let spans_y = a.y.max(b.y) < a.bottom().min(b.bottom());
    let touches_y = a.bottom() == b.y || b.bottom() == a.y;
    let spans_x = a.x.max(b.x) < a.right().min(b.right());
    if (touches_x && spans_y) || (touches_y && spans_x) {
        Some(a.union(&b))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn rect_accessors_and_edges() {
        let r = rect(10, 20, 30, 40);
        assert_eq!(r.right(), 40);
        assert_eq!(r.bottom(), 60);
        assert!(!r.is_empty());
        assert_eq!(r.size(), Size { w: 30, h: 40 });
        assert_eq!(r.area(), 1200);
        assert_eq!(Rect::from_size(Size { w: 5, h: 6 }), rect(0, 0, 5, 6));
        assert_eq!(Rect::EMPTY.right(), 0);
        assert!(Rect::EMPTY.is_empty());
    }

    #[test]
    fn rect_edges_saturate_instead_of_wrapping() {
        let r = rect(i32::MAX - 1, i32::MAX - 1, 10, 10);
        assert_eq!(r.right(), i32::MAX);
        assert_eq!(r.bottom(), i32::MAX);

        let low = rect(i32::MIN, i32::MIN, 0, 0);
        assert!(low.is_empty());
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = rect(0, 0, 100, 50);
        assert!(r.contains(Point { x: 0, y: 0 }));
        assert!(r.contains(Point { x: 99, y: 49 }));
        assert!(!r.contains(Point { x: 100, y: 49 }));
        assert!(!r.contains(Point { x: 99, y: 50 }));
        assert!(!r.contains(Point { x: -1, y: 0 }));
        assert!(!r.contains(Point { x: 0, y: -1 }));
    }

    #[test]
    fn rect_contains_respects_origin_offset() {
        let r = rect(-10, -20, 10, 10);
        assert!(r.contains(Point { x: -10, y: -20 }));
        assert!(r.contains(Point { x: -1, y: -11 }));
        assert!(!r.contains(Point { x: 0, y: -11 }));
        assert!(!r.contains(Point { x: -10, y: -10 }));
    }

    #[test]
    fn rect_contains_nothing_when_empty() {
        assert!(!Rect::EMPTY.contains(Point { x: 0, y: 0 }));
        assert!(!rect(5, 5, 0, 10).contains(Point { x: 5, y: 5 }));
        assert!(!rect(5, 5, 10, 0).contains(Point { x: 5, y: 5 }));
    }

    #[test]
    fn rect_intersect_overlap() {
        let a = rect(0, 0, 10, 10);
        let b = rect(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), Some(rect(5, 5, 5, 5)));
        assert_eq!(b.intersect(&a), Some(rect(5, 5, 5, 5)));
    }

    #[test]
    fn rect_intersect_containment() {
        let outer = rect(0, 0, 100, 100);
        let inner = rect(10, 10, 20, 20);
        assert_eq!(outer.intersect(&inner), Some(inner));
        assert_eq!(inner.intersect(&outer), Some(inner));
    }

    #[test]
    fn rect_intersect_touching_edges_is_none() {
        let a = rect(0, 0, 10, 10);
        assert_eq!(a.intersect(&rect(10, 0, 5, 5)), None);
        assert_eq!(a.intersect(&rect(0, 10, 5, 5)), None);
        assert_eq!(a.intersect(&rect(10, 10, 5, 5)), None);
    }

    #[test]
    fn rect_intersect_disjoint_is_none() {
        let a = rect(0, 0, 10, 10);
        assert_eq!(a.intersect(&rect(20, 20, 5, 5)), None);
        assert_eq!(a.intersect(&rect(-20, -20, 5, 5)), None);
    }

    #[test]
    fn rect_intersect_empty_is_none() {
        let a = rect(0, 0, 10, 10);
        assert_eq!(a.intersect(&Rect::EMPTY), None);
        assert_eq!(Rect::EMPTY.intersect(&a), None);
        assert_eq!(Rect::EMPTY.intersect(&Rect::EMPTY), None);
    }

    #[test]
    fn rect_union_bounding_box() {
        let a = rect(0, 0, 10, 10);
        let b = rect(5, 5, 10, 10);
        assert_eq!(a.union(&b), rect(0, 0, 15, 15));

        let neg = rect(-5, -5, 10, 10);
        let pos = rect(5, 5, 10, 10);
        assert_eq!(neg.union(&pos), rect(-5, -5, 20, 20));
    }

    #[test]
    fn rect_union_ignores_empty_operands() {
        let a = rect(3, 4, 10, 10);
        assert_eq!(a.union(&Rect::EMPTY), a);
        assert_eq!(Rect::EMPTY.union(&a), a);
        assert_eq!(Rect::EMPTY.union(&Rect::EMPTY), Rect::EMPTY);
    }

    #[test]
    fn rect_union_saturates_size() {
        let a = rect(i32::MIN, i32::MIN, u32::MAX, u32::MAX);
        let b = rect(i32::MAX, i32::MAX, u32::MAX, u32::MAX);
        let u = a.union(&b);
        assert_eq!(u.w, u32::MAX);
        assert_eq!(u.h, u32::MAX);
        assert_eq!(u.x, i32::MIN);
        assert_eq!(u.y, i32::MIN);
    }

    #[test]
    fn region_push_and_extend_ignore_empty() {
        let mut region = Region::empty();
        region.push(Rect::EMPTY);
        assert!(region.is_empty());
        assert_eq!(region.len(), 0);

        region.push(rect(0, 0, 10, 10));
        assert_eq!(region.len(), 1);

        let mut other = Region::from_rect(Rect::EMPTY);
        other.push(rect(20, 20, 5, 5));
        region.extend(&other);
        assert_eq!(region.len(), 2);
        assert_eq!(region.rects(), &[rect(0, 0, 10, 10), rect(20, 20, 5, 5)]);
    }

    #[test]
    fn region_from_rect_empty() {
        assert!(Region::from_rect(Rect::EMPTY).is_empty());
        assert_eq!(Region::from_rect(rect(1, 2, 3, 4)).len(), 1);
        assert_eq!(Region::default(), Region::empty());
    }

    #[test]
    fn region_bounds_none_when_empty_some_when_not() {
        assert_eq!(Region::empty().bounds(), None);
        let mut region = Region::empty();
        region.push(rect(5, 5, 5, 5));
        region.push(rect(20, 20, 5, 5));
        assert_eq!(region.bounds(), Some(rect(5, 5, 20, 20)));
    }

    #[test]
    fn region_clip_intersects_each_rect() {
        let mut region = Region::empty();
        region.push(rect(0, 0, 10, 10));
        region.push(rect(20, 20, 10, 10));
        region.push(rect(50, 50, 10, 10));

        let clipped = region.clip(&rect(5, 5, 10, 10));
        assert_eq!(clipped.rects(), &[rect(5, 5, 5, 5)]);

        assert!(region.clip(&Rect::EMPTY).is_empty());
        assert!(region.clip(&rect(100, 100, 10, 10)).is_empty());
        // The source region is untouched.
        assert_eq!(region.len(), 3);
    }

    #[test]
    fn region_coalesce_overlapping() {
        let mut region = Region::empty();
        region.push(rect(0, 0, 10, 10));
        region.push(rect(5, 5, 10, 10));
        region.coalesce();
        assert_eq!(region.rects(), &[rect(0, 0, 15, 15)]);
    }

    #[test]
    fn region_coalesce_adjacent_edges() {
        let mut horizontal = Region::empty();
        horizontal.push(rect(0, 0, 10, 10));
        horizontal.push(rect(10, 0, 10, 10));
        horizontal.coalesce();
        assert_eq!(horizontal.rects(), &[rect(0, 0, 20, 10)]);

        let mut vertical = Region::empty();
        vertical.push(rect(0, 0, 10, 10));
        vertical.push(rect(0, 10, 10, 10));
        vertical.coalesce();
        assert_eq!(vertical.rects(), &[rect(0, 0, 10, 20)]);
    }

    #[test]
    fn region_coalesce_chain() {
        let mut region = Region::empty();
        region.push(rect(0, 0, 10, 10));
        region.push(rect(10, 0, 10, 10));
        region.push(rect(20, 0, 10, 10));
        region.coalesce();
        assert_eq!(region.rects(), &[rect(0, 0, 30, 10)]);
    }

    #[test]
    fn region_coalesce_keeps_corner_touching_separate() {
        let mut region = Region::empty();
        region.push(rect(0, 0, 10, 10));
        region.push(rect(10, 10, 10, 10));
        region.coalesce();
        assert_eq!(region.len(), 2);
    }

    #[test]
    fn region_coalesce_merges_partial_edge_neighbours() {
        // A shared edge segment is adjacency, so the bounding box is used even
        // though it adds area (damage is evidence, not an exact set).
        let mut region = Region::empty();
        region.push(rect(0, 0, 10, 10));
        region.push(rect(5, 10, 10, 10));
        region.coalesce();
        assert_eq!(region.rects(), &[rect(0, 0, 15, 20)]);
    }

    #[test]
    fn region_coalesce_deduplicates() {
        let mut region = Region::empty();
        region.push(rect(1, 2, 3, 4));
        region.push(rect(1, 2, 3, 4));
        region.coalesce();
        assert_eq!(region.rects(), &[rect(1, 2, 3, 4)]);
    }

    #[test]
    fn region_simplified_is_sorted_and_coalesced() {
        let mut region = Region::empty();
        region.push(rect(20, 20, 5, 5));
        region.push(rect(0, 0, 10, 10));
        region.push(rect(10, 0, 10, 10));
        // Not mutated by simplified().
        assert_eq!(
            region.simplified(),
            vec![rect(0, 0, 20, 10), rect(20, 20, 5, 5)]
        );
        assert_eq!(region.len(), 3);

        // Deterministic order regardless of insertion order.
        let mut reversed = Region::empty();
        reversed.push(rect(20, 20, 5, 5));
        reversed.push(rect(0, 0, 20, 10));
        assert_eq!(reversed.simplified(), region.simplified());
    }

    #[test]
    fn region_simplified_of_empty_is_empty() {
        assert_eq!(Region::empty().simplified(), Vec::new());
    }
}
