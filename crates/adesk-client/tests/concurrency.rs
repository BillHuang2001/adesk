//! Request multiplexing: distinct request ids and id-matched responses.
//!
//! `Client` may have many requests in flight on one connection; AGP responses
//! are matched by request id, so out-of-order replies are normal and harmless
//! (`docs/protocol.md` §1, `CONTEXT.md` → "Design Decisions").

mod common;

use std::time::Duration;

use adesk_core::WindowId;
use common::{connect, MockServer, TIMEOUT};
use serde_json::json;

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
    const N: usize = 16;

    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let requests = futures::future::join_all((0..N).map(|_| client.list_windows()));
    let (results, ()) = tokio::time::timeout(TIMEOUT, async {
        tokio::join!(requests, async {
            let mut ids = Vec::with_capacity(N);
            for _ in 0..N {
                let (id, method, params) = server.next_request().await;
                assert_eq!(method, "list_windows");
                assert_eq!(params, json!({}), "list_windows takes no params");
                ids.push(id);
            }
            ids.sort_unstable();
            assert_eq!(
                ids,
                (1..=N as u64).collect::<Vec<_>>(),
                "per-connection ids are 1..=N with no duplicates"
            );

            // A distinct marker per request id.
            for id in 1..=N as u64 {
                server
                    .respond(id, json!({"windows": [], "active_window_id": id}))
                    .await;
            }
        })
    })
    .await
    .expect("all requests complete");

    let mut markers: Vec<u64> = results
        .into_iter()
        .map(|result| {
            let list = result.expect("list_windows succeeds");
            assert!(list.windows.is_empty());
            u64::from(
                list.active_window_id
                    .expect("the marker is the active window id"),
            )
        })
        .collect();
    markers.sort_unstable();
    assert_eq!(
        markers,
        (1..=N as u64).collect::<Vec<_>>(),
        "every future resolved with its own marker (no cross-talk)"
    );
}

/// Out-of-order responses are still matched to the right caller.
///
/// Send two requests, capture their ids, then answer the second id first and
/// the first id second; assert each future receives its own result object (a
/// distinct marker per id) and that neither resolves before its own response.
#[tokio::test]
async fn out_of_order_responses_match_by_id() {
    let mut server = MockServer::start().await;
    let client = connect(&mut server).await;

    let mut first = Box::pin(client.list_windows());
    let mut second = Box::pin(client.list_windows());

    // Drive each future once so both requests reach the server.
    assert!(futures::poll!(&mut first).is_pending());
    assert!(futures::poll!(&mut second).is_pending());

    let (id1, method1, _) = server.next_request().await;
    let (id2, method2, _) = server.next_request().await;
    assert_eq!(
        (method1.as_str(), method2.as_str()),
        ("list_windows", "list_windows")
    );
    assert_ne!(id1, id2, "concurrent requests get distinct ids");
    assert_eq!(
        (id1, id2),
        (1, 2),
        "per-connection ids start at 1 and increase"
    );

    // Answer the second request first.
    server
        .respond(id2, json!({"windows": [], "active_window_id": 2}))
        .await;
    assert!(
        futures::poll!(&mut first).is_pending(),
        "the first caller must not resolve before its own response arrives"
    );
    let second_list = tokio::time::timeout(Duration::from_secs(5), &mut second)
        .await
        .expect("the second caller resolves once its response arrives")
        .expect("list_windows succeeds");
    assert_eq!(second_list.active_window_id, Some(WindowId(2)));

    // Now the first request, which arrived second.
    server
        .respond(id1, json!({"windows": [], "active_window_id": 1}))
        .await;
    let first_list = tokio::time::timeout(Duration::from_secs(5), &mut first)
        .await
        .expect("the first caller resolves once its response arrives")
        .expect("list_windows succeeds");
    assert_eq!(first_list.active_window_id, Some(WindowId(1)));
}
