//! Request multiplexing: distinct request ids and id-matched responses.
//!
//! `Client` may have many requests in flight on one connection; AGP responses
//! are matched by request id, so out-of-order replies are normal and harmless
//! (`docs/protocol.md` §1, `CONTEXT.md` → "Design Decisions").

mod common;

/// Many concurrent in-flight requests get distinct, monotonic ids.
///
/// Fire N `list_windows` calls concurrently (for example with
/// `futures::future::join_all` or `tokio::join!`) and assert `next_request()`
/// yields the ids 1..=N exactly once each — per-connection ids start at 1 and
/// are never reused. Reply to each id with a distinct marker (for example
/// 'active_window_id' equal to the id) and assert every future resolves with
/// its own marker, proving there is no cross-talk.
#[tokio::test]
async fn many_requests_in_flight_get_distinct_ids() {
    todo!(
        "connect with verify_version(false); spawn N concurrent list_windows calls; \
         collect N requests with next_request() and assert their ids are exactly 1..=N with no duplicates; \
         answer each id with a distinct marker; assert each future resolves with its own marker"
    );
}

/// Out-of-order responses are still matched to the right caller.
///
/// Send two requests, capture their ids, then answer the second id first and
/// the first id second; assert each future receives its own result object (a
/// distinct marker per id) and that neither resolves before its own response.
#[tokio::test]
async fn out_of_order_responses_match_by_id() {
    todo!(
        "send two requests and capture ids id1 and id2 with next_request(); \
         respond to id2 first with marker 2, then to id1 with marker 1; \
         assert the two futures resolve with marker 2 and marker 1 respectively (matched by id, not by arrival order)"
    );
}
