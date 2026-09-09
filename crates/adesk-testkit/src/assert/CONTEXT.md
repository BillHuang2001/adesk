# assert — image and event assertions

## Intent

Assertion helpers for ADesk tests: pixel/image comparisons and `RuntimeEvent` stream predicates.
Assertions panic with detailed messages (`assert_eq!` semantics); everything else returns `Result`.

## API Surface

- `ImageAssert<'a>` — `new`, `pixel`, `region_avg`, `matches_pattern` (+`_tol`), `matches_solid`, `differs_from` (+`_tol`), `save_png`, `dump_on_failure`.
- `EventAssert` — `tap`, `from_receiver`, `try_recv`, `drain`, `seen`, `wait_for`, `wait_for_kind`, `wait_for_expected`, `wait_ordered`, `expect_none`, `assert_seen_order`.
- `Expected` — common event predicates (`WindowCreated`, `SurfaceCommit(WindowId)`, ...) plus `custom(name, predicate)`.

## Constraints

- Every wait takes a `Duration` deadline and returns `TestkitError::Timeout` on expiry; broadcast `Lagged`/`Closed` map to `TestkitError::Lagged`/`ConnectionClosed`.
- Pixel comparisons are exact against `FillPattern::at(x, y, size)` (see `../fill.rs`) so client and assertion share one ground truth; tolerance variants compare per channel.
- No sleeps: waits wrap `tokio::sync::broadcast::Receiver::recv` in `tokio::time::timeout`.
- `EventAssert::seen` records every event observed, in arrival order, for `assert_seen_order`.

## Routing Table

Leaf module: `image.rs` (pixels), `event.rs` (event streams).
