//! Damage coalescing and accumulation tests (pure, no GPU).

use adesk_core::{Rect, Region};
use adesk_render::{coalesce_damage, DamageAccumulator};

fn region(rects: &[Rect]) -> Region {
    let mut region = Region::empty();
    for rect in rects {
        region.push(*rect);
    }
    region
}

#[test]
fn coalesce_merges_overlapping_rects_into_bounding_box() {
    let raw = region(&[Rect::new(0, 0, 10, 10), Rect::new(5, 5, 10, 10)]);
    let merged = coalesce_damage(&raw, &Rect::new(0, 0, 100, 100), 0);
    assert_eq!(merged.simplified(), vec![Rect::new(0, 0, 15, 15)]);
}

#[test]
fn coalesce_keeps_disjoint_rects_separate() {
    let raw = region(&[Rect::new(0, 0, 2, 2), Rect::new(50, 50, 2, 2)]);
    let merged = coalesce_damage(&raw, &Rect::new(0, 0, 100, 100), 0);
    assert_eq!(
        merged.simplified(),
        vec![Rect::new(0, 0, 2, 2), Rect::new(50, 50, 2, 2)]
    );
}

#[test]
fn coalesce_clips_to_bounds() {
    let raw = region(&[Rect::new(5, 5, 10, 10)]);
    let merged = coalesce_damage(&raw, &Rect::new(0, 0, 8, 8), 0);
    assert_eq!(merged.simplified(), vec![Rect::new(5, 5, 3, 3)]);

    let outside = coalesce_damage(&raw, &Rect::new(50, 50, 10, 10), 0);
    assert!(outside.is_empty());
}

#[test]
fn coalesce_drops_rects_below_min_area() {
    let raw = region(&[Rect::new(0, 0, 2, 2), Rect::new(20, 20, 10, 10)]);
    let merged = coalesce_damage(&raw, &Rect::new(0, 0, 100, 100), 10);
    assert_eq!(merged.simplified(), vec![Rect::new(20, 20, 10, 10)]);
}

#[test]
fn coalesce_min_area_keeps_survivors_sorted() {
    let raw = region(&[
        Rect::new(50, 50, 10, 10),
        Rect::new(0, 0, 2, 2), // below `min_area`, dropped
        Rect::new(30, 10, 6, 6),
    ]);
    let merged = coalesce_damage(&raw, &Rect::new(0, 0, 100, 100), 10);
    assert_eq!(
        merged.rects(),
        &[Rect::new(30, 10, 6, 6), Rect::new(50, 50, 10, 10)]
    );
}
#[test]
fn coalesce_of_empty_region_is_empty() {
    let merged = coalesce_damage(&Region::empty(), &Rect::new(0, 0, 10, 10), 0);
    assert!(merged.is_empty());
}

#[test]
fn coalesce_output_is_sorted_regardless_of_input_order() {
    // Coalescing sorts by `(y, x, h, w)` so observations replay identically;
    // the implementation relies on this surviving an in-place coalesce.
    let raw = region(&[
        Rect::new(50, 40, 4, 4),
        Rect::new(10, 20, 4, 4),
        Rect::new(2, 3, 5, 5),
    ]);
    let merged = coalesce_damage(&raw, &Rect::new(0, 0, 100, 100), 0);
    assert_eq!(
        merged.rects(),
        &[
            Rect::new(2, 3, 5, 5),
            Rect::new(10, 20, 4, 4),
            Rect::new(50, 40, 4, 4),
        ]
    );
}

#[test]
fn coalesce_of_unsorted_input_matches_simplified() {
    // Unsorted, overlapping/edge-adjacent input: the coalesced result must be
    // identical to `Region::simplified` (same set, same order).
    let raw = region(&[
        Rect::new(5, 10, 10, 10),
        Rect::new(0, 0, 10, 10),
        Rect::new(10, 0, 10, 10),
        Rect::new(0, 10, 4, 4),
    ]);
    let merged = coalesce_damage(&raw, &Rect::new(0, 0, 100, 100), 0);
    assert_eq!(merged.rects(), raw.simplified());
    assert_eq!(merged.rects(), &[Rect::new(0, 0, 20, 20)]);
}

#[test]
fn accumulator_records_commits_and_take_resets() {
    let mut acc = DamageAccumulator::new(Rect::new(0, 0, 100, 100));
    assert!(acc.is_empty());
    assert_eq!(acc.commits(), 0);
    assert_eq!(acc.last_commit_seq(), 0);

    acc.record_commit(7, &region(&[Rect::new(0, 0, 10, 10)]));
    acc.record_commit(9, &region(&[Rect::new(5, 5, 10, 10)]));
    assert_eq!(acc.commits(), 2);
    assert_eq!(acc.last_commit_seq(), 9);

    let taken = acc.take();
    assert_eq!(taken.simplified(), vec![Rect::new(0, 0, 15, 15)]);
    assert!(acc.is_empty());
    assert_eq!(acc.take().simplified(), Vec::new());
    // Counters are lifetime totals, not reset by take().
    assert_eq!(acc.commits(), 2);
    assert_eq!(acc.last_commit_seq(), 9);
}

#[test]
fn accumulator_peek_does_not_reset() {
    let mut acc = DamageAccumulator::new(Rect::new(0, 0, 100, 100));
    acc.record_commit(1, &region(&[Rect::new(1, 1, 4, 4)]));
    assert_eq!(acc.peek().simplified(), vec![Rect::new(1, 1, 4, 4)]);
    assert_eq!(acc.peek().simplified(), vec![Rect::new(1, 1, 4, 4)]);
    assert!(!acc.is_empty());
}

#[test]
fn accumulator_clips_damage_to_bounds() {
    let mut acc = DamageAccumulator::new(Rect::new(0, 0, 16, 16));
    acc.record_commit(
        3,
        &region(&[Rect::new(-4, -4, 8, 8), Rect::new(14, 14, 8, 8)]),
    );
    let damage = acc.take();
    assert_eq!(
        damage.simplified(),
        vec![Rect::new(0, 0, 4, 4), Rect::new(14, 14, 2, 2)]
    );
}

#[test]
fn accumulator_records_fully_clipped_commit_without_pending_damage() {
    let mut acc = DamageAccumulator::new(Rect::new(0, 0, 10, 10));
    acc.record_commit(5, &region(&[Rect::new(100, 100, 4, 4)]));
    assert!(acc.is_empty());
    assert!(acc.peek().is_empty());
    // The commit is still counted (lifetime counters are independent of damage).
    assert_eq!(acc.commits(), 1);
    assert_eq!(acc.last_commit_seq(), 5);
}

#[test]
fn accumulator_with_empty_bounds_accumulates_nothing() {
    let mut acc = DamageAccumulator::new(Rect::EMPTY);
    acc.record_commit(1, &region(&[Rect::new(-5, -5, 3, 3)]));
    acc.record_rect(Rect::new(0, 0, 2, 2));
    assert!(acc.is_empty());
    assert!(acc.peek().is_empty());
    assert_eq!(acc.commits(), 1);
    assert_eq!(acc.last_commit_seq(), 1);
}

#[test]
fn accumulator_set_bounds_reclips_pending_damage() {
    let mut acc = DamageAccumulator::new(Rect::new(0, 0, 100, 100));
    acc.record_commit(1, &region(&[Rect::new(0, 0, 50, 50)]));
    acc.set_bounds(Rect::new(0, 0, 10, 10));
    assert_eq!(acc.bounds(), Rect::new(0, 0, 10, 10));
    assert_eq!(acc.peek().simplified(), vec![Rect::new(0, 0, 10, 10)]);
}

#[test]
fn accumulator_record_rect_clips_single_rects() {
    let mut acc = DamageAccumulator::new(Rect::new(4, 4, 8, 8));
    acc.record_rect(Rect::new(0, 0, 8, 8)); // clipped to (4,4,4,4)
    acc.record_rect(Rect::new(20, 20, 2, 2)); // fully outside, ignored
    assert_eq!(acc.peek().simplified(), vec![Rect::new(4, 4, 4, 4)]);
}

#[test]
fn accumulator_clear_drops_pending_damage() {
    let mut acc = DamageAccumulator::new(Rect::new(0, 0, 10, 10));
    acc.record_commit(1, &region(&[Rect::new(0, 0, 5, 5)]));
    acc.clear();
    assert!(acc.is_empty());
    assert!(acc.peek().is_empty());
    assert_eq!(acc.commits(), 1);
}
