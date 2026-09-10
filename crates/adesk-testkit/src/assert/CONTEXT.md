# assert — image and event assertions

## Intent

Assertion helpers for ADesk tests: pixel/image comparisons and `RuntimeEvent` stream predicates.
Assertions panic with detailed messages (`assert_eq!` semantics); everything else returns `Result`.

## API Surface

- `ImageAssert<'a>` — `new`, `pixel`, `region_avg`, `matches_pattern` (+`_tol`), `matches_solid`, `differs_from` (+`_tol`), `save_png`, `dump_on_failure`.
- `EventAssert` — `tap`, `from_receiver`, `try_recv`, `drain`, `seen`, `wait_for`, `wait_for_kind`, `wait_for_expected`, `expect_none`, `assert_seen_order`.
- `Expected` — common event predicates (`WindowCreated`, `SurfaceCommit(WindowId)`, ...) plus `custom(name, predicate)`.

## Constraints

- Every wait takes a `Duration` deadline and returns `TestkitError::Timeout` on expiry; broadcast `Lagged`/`Closed` map to `TestkitError::Lagged`/`ConnectionClosed`.
- Pixel comparisons are exact against `FillPattern::at(x, y, size)` (see `../fill.rs`) so client and assertion share one ground truth; tolerance variants compare per channel.
- No sleeps: waits wrap `tokio::sync::broadcast::Receiver::recv` in `tokio::time::timeout`.
- `EventAssert::seen` records every event observed, in arrival order, for `assert_seen_order`.

## Notes for Agents

- `image.rs` is the crate's only image I/O module and the only user of the `image` crate.
- PNG is encode-only: `ImageAssert::save_png` repacks stride-padded pixels into a tightly packed RGBA8 `Vec`, hands it to `image::RgbaImage::from_raw` and calls `save`. There is no PNG decoder anywhere in `adesk-testkit` — the round-trip self-test inspects only the 8-byte PNG signature.
- All comparisons go per logical pixel through `adesk_core::ImageBuffer::pixel`, so row padding and the stride value itself are never compared; there is no byte-level/row-memcmp diff, no diff image output and no percentage-threshold helper.
- `matches_pattern*` is exact against `FillPattern::at(x, y, size)`; the `_tol`/`matches_solid` variants are per-channel `abs_diff <= tolerance`. `differs_from*` is the negation of the private `equal_within(left, right, tolerance)`.

## Known Issues

- None.
## Routing Table

Leaf module: `image.rs` (pixels), `event.rs` (event streams).
