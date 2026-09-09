//! Shared runtime context.
//!
//! [`ServerContext`] is the cheap-clone bundle every connection, dispatch arm
//! and background task receives: compositor handle, observer, app registry,
//! correlator, subscription registries, inspection cache, cursor tracker and
//! the shutdown token.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use adesk_app_registry::{AppRegistry, Correlator};
use adesk_compositor::CompositorHandle;
use adesk_core::Point;
use adesk_observer::ObserverService;

use crate::config::ServerConfig;
use crate::inspection::InspectionCache;
use crate::shutdown::ShutdownHandle;
use crate::subscriptions::{InspectRegistry, SubscriptionRegistry};

/// Cheap clone of the whole runtime state.
#[derive(Clone)]
pub struct ServerContext {
    /// Configuration the runtime was started with.
    pub config: Arc<ServerConfig>,
    /// Handle to the compositor thread.
    pub compositor: CompositorHandle,
    /// Temporal observation engine fed by the event pump.
    pub observer: ObserverService,
    /// Application registry (shared by list/get/launch).
    pub registry: Arc<AppRegistry>,
    /// Launch → window correlator; shared with the registry's clock.
    pub correlator: Arc<Mutex<Correlator>>,
    /// Event subscriptions of all connections (§5.6).
    pub subscriptions: SubscriptionRegistry,
    /// Inspector streams of all connections (§5.7).
    pub inspect_subscriptions: InspectRegistry,
    /// Cached inspection state exposed through `InspectionSource`.
    pub inspection: InspectionCache,
    /// Last pointer position the runtime commanded (output coordinates).
    pub cursor: CursorTracker,
    /// Shutdown coordination token.
    pub shutdown: ShutdownHandle,
    /// Instant the runtime started; the base of the monotonic `ts_ms` clock.
    pub started_at: Instant,
    next_connection_id: Arc<AtomicU64>,
}

impl ServerContext {
    /// Composes a context from already-started components.
    pub fn new(
        config: Arc<ServerConfig>,
        compositor: CompositorHandle,
        observer: ObserverService,
        registry: Arc<AppRegistry>,
        correlator: Arc<Mutex<Correlator>>,
    ) -> ServerContext {
        ServerContext {
            config,
            compositor,
            observer,
            registry,
            correlator,
            subscriptions: SubscriptionRegistry::new(),
            inspect_subscriptions: InspectRegistry::new(),
            inspection: InspectionCache::new(),
            cursor: CursorTracker::new(),
            shutdown: ShutdownHandle::new(),
            started_at: Instant::now(),
            next_connection_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Monotonic milliseconds since the runtime started (the `ts_ms` base of
    /// every observation and event).
    pub fn now_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    /// Uptime reported by `ping.uptime_ms`.
    pub fn uptime_ms(&self) -> u64 {
        self.now_ms()
    }

    /// Allocates the next connection id (monotonic, starts at 1).
    pub fn next_connection_id(&self) -> u64 {
        self.next_connection_id.fetch_add(1, Ordering::Relaxed)
    }
}

/// Last pointer position commanded by the runtime, in output coordinates.
///
/// The compositor's state snapshot does not carry the cursor, so the server
/// records every pointer position it sends; the inspector reads it back.
#[derive(Clone, Default)]
pub struct CursorTracker {
    position: Arc<RwLock<Option<Point>>>,
}

impl CursorTracker {
    /// An empty tracker (no pointer position commanded yet).
    pub fn new() -> CursorTracker {
        CursorTracker::default()
    }

    /// Records a pointer position.
    pub fn set(&self, position: Point) {
        *self.position.write().unwrap_or_else(|error| error.into_inner()) = Some(position);
    }

    /// The most recent pointer position, if any.
    pub fn get(&self) -> Option<Point> {
        *self.position.read().unwrap_or_else(|error| error.into_inner())
    }
}
