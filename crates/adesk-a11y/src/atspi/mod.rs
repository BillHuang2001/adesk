//! The real AT-SPI2 backend: a window's accessibility tree, read over D-Bus.
//!
//! AT-SPI2 is a D-Bus protocol: an application exposes its widgets as objects with
//! the `org.a11y.atspi.Accessible` interface plus a set of *optional* ones
//! (`Component`, `Action`, `Text`, `Value`, ...), and the bus's registry object
//! lists every application that has joined it. This module turns that into the
//! crate's backend seam: [`AtspiSource::snapshot`] correlates a window with an
//! application's accessible frame and walks its subtree, and
//! [`AtspiSource::invoke`] performs an element's action.
//!
//! Three backends are exposed, differing only in *when* the bus is touched:
//!
//! - [`AtspiSource`] connects on construction and is what a caller that already
//!   knows the environment is usable drives directly.
//! - [`LazyAtspiSource`] is the `auto` backend: it connects on first use, caches
//!   the outcome (success *or* failure) and never reports a failure as anything
//!   but [`A11yError::Unavailable`]. A sandbox, a container or a CI job answers
//!   `not_supported` instead of hanging, and only ever pays for one connect
//!   attempt per runtime.
//! - [`UnavailableSource`] is the `off` backend behind `--accessibility off`: it
//!   never touches D-Bus at all.
//!
//! Everything that does not need a bus lives next door and is unit-tested without
//! one: [`map`] is the pure AT-SPI → AGP vocabulary mapping, [`correlate`] is the
//! window → frame ladder, [`dbus`] is the connection, addressing and error
//! classification, and [`walk`] is the element read and the bounded pre-order
//! traversal.

mod correlate;
mod dbus;
mod map;
mod walk;

use std::sync::Arc;

use async_trait::async_trait;
use atspi::connection::AccessibilityConnection;
use tokio::sync::OnceCell;
use tracing::debug;

use crate::error::{A11yError, Result};
use crate::source::{
    AccessibilitySource, ElementHandle, InvokeOutcome, SourceOptions, SourceSnapshot, WindowTarget,
};

/// The backend's `AccessibilitySource::name`.
///
/// This is the name of the *backend family*, so the connected, the lazily
/// connected and the disabled backend all report the same one: a diagnostic that
/// says "atspi" should not depend on whether the bus happened to be reachable.
/// [`UnavailableSource`] is the deliberate exception — see its own documentation.
const BACKEND_NAME: &str = "atspi";

/// The `atspi` backend, already connected to the accessibility bus.
///
/// Constructing one is the only operation that can wait on the environment, and
/// it is bounded: [`AtspiSource::connect`] answers [`A11yError::Unavailable`] when
/// there is no accessibility bus, rather than hanging a §5.11 request on a wedged
/// D-Bus (`docs/architecture.md` §12).
#[derive(Debug, Clone)]
pub struct AtspiSource {
    /// The connection every read runs over.
    connection: AccessibilityConnection,
}

impl AtspiSource {
    /// Connect to the accessibility bus.
    ///
    /// Returns [`A11yError::Unavailable`] when the environment has no
    /// accessibility bus — an absent session bus, an unreachable
    /// `org.a11y.Bus`, or a connect that did not complete in time. It never
    /// hangs: the connect itself is time-bounded (`dbus::CONNECT_TIMEOUT`).
    pub async fn connect() -> Result<AtspiSource> {
        Ok(AtspiSource {
            connection: dbus::connect().await?,
        })
    }

    /// The connected bus name, for diagnostics.
    ///
    /// This is the D-Bus unique name the daemon assigned the runtime's own
    /// connection (`":1.42"`), not an application's: it identifies *which*
    /// connection served a snapshot in a log. `None` only if the transport has no
    /// unique name at all.
    pub fn bus_name(&self) -> Option<String> {
        self.connection
            .connection()
            .unique_name()
            .map(|name| name.as_str().to_owned())
    }

    /// The underlying connection, for the sibling modules that do the reads.
    fn connection(&self) -> &AccessibilityConnection {
        &self.connection
    }
}

#[async_trait]
impl AccessibilitySource for AtspiSource {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn is_available(&self) -> bool {
        // A connected source is by definition usable; whether the environment
        // *has* a bus is what the lazy and disabled backends answer.
        true
    }

    async fn snapshot(&self, target: &WindowTarget, opts: SourceOptions) -> Result<SourceSnapshot> {
        let correlated = correlate::correlate(self.connection(), target).await?;
        // Read the frame's screen origin once, then make every element bound in
        // the subtree relative to it.
        let origin = walk::screen_origin(self.connection().connection(), &correlated.root).await?;
        let (root, truncated) = walk::walk(
            self.connection().connection(),
            &correlated.root,
            origin,
            opts,
        )
        .await?;

        Ok(SourceSnapshot {
            app_name: correlated.app_name,
            root,
            truncated,
        })
    }

    async fn invoke(&self, handle: &ElementHandle, action: Option<&str>) -> Result<InvokeOutcome> {
        walk::invoke(self.connection().connection(), handle, action).await
    }
}

/// The `auto` backend: connect to the accessibility bus on first use.
///
/// The runtime is started long before anyone asks for an accessibility tree, and
/// most environments (a container, a CI job, a desktop with accessibility turned
/// off) have no bus at all. Connecting eagerly would make every startup pay for a
/// failed D-Bus round trip; connecting on every request would turn one wedged bus
/// into a per-request stall. So the first call connects, and its outcome —
/// success *or* failure — is cached for the process's lifetime.
///
/// Failure is never an error the caller has to interpret: [`is_available`]
/// answers `false` and `snapshot`/`invoke` answer [`A11yError::Unavailable`],
/// which the server reports as `not_supported` rather than as a broken backend.
///
/// Cheap to clone: clones share one cache, so N clones cost one connect attempt,
/// not N.
///
/// [`is_available`]: AccessibilitySource::is_available
#[derive(Debug, Clone, Default)]
pub struct LazyAtspiSource {
    /// The one connect attempt, and what it produced. `None` *inside* the cell
    /// means "connected and unavailable"; an empty cell means "not tried yet".
    connection: Arc<OnceCell<Option<AtspiSource>>>,
}

impl LazyAtspiSource {
    /// A backend that has not connected yet.
    pub fn new() -> LazyAtspiSource {
        LazyAtspiSource {
            connection: Arc::new(OnceCell::new()),
        }
    }

    /// Connect on first use and hand back the cached result.
    ///
    /// A connect failure is logged once, here, and cached like a success — the
    /// point of the cache is that a missing bus is decided once, not re-probed on
    /// every request.
    async fn resolve(&self) -> Option<&AtspiSource> {
        self.connection
            .get_or_init(|| async {
                match AtspiSource::connect().await {
                    Ok(source) => {
                        debug!(backend = BACKEND_NAME, bus = ?source.bus_name(), "connected to the accessibility bus");
                        Some(source)
                    }
                    Err(error) => {
                        // The reason matters for diagnostics and is exactly what
                        // the caller sees as `not_supported`.
                        debug!(backend = BACKEND_NAME, %error, "the accessibility bus is not available");
                        None
                    }
                }
            })
            .await
            .as_ref()
    }

    /// The error a disabled-by-environment backend reports, naming the backend so
    /// the `not_supported` answer says which one was missing.
    fn unavailable() -> A11yError {
        A11yError::Unavailable(format!(
            "the {BACKEND_NAME} backend has no accessibility bus"
        ))
    }
}

#[async_trait]
impl AccessibilitySource for LazyAtspiSource {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn is_available(&self) -> bool {
        self.resolve().await.is_some()
    }

    async fn snapshot(&self, target: &WindowTarget, opts: SourceOptions) -> Result<SourceSnapshot> {
        let Some(source) = self.resolve().await else {
            return Err(LazyAtspiSource::unavailable());
        };
        source.snapshot(target, opts).await
    }

    async fn invoke(&self, handle: &ElementHandle, action: Option<&str>) -> Result<InvokeOutcome> {
        let Some(source) = self.resolve().await else {
            return Err(LazyAtspiSource::unavailable());
        };
        source.invoke(handle, action).await
    }
}

/// The `off` backend: accessibility the environment cannot or must not have.
///
/// Two situations produce one: the runtime was started with `--accessibility off`,
/// and the `auto` backend found no bus (which the service reports through the same
/// `not_supported` answer, so an agent's fallback path is identical).
///
/// This backend answers synchronously and touches nothing — no D-Bus, no session
/// bus probe, no timeout — so it is also the honest answer for a caller that
/// already knows the environment: there is nothing to try, and trying would only
/// make a request slower.
pub struct UnavailableSource {
    /// Why the backend is unavailable, carried into every error.
    reason: String,
}

impl UnavailableSource {
    /// A backend that always reports [`A11yError::Unavailable`] with `reason`.
    ///
    /// The reason reaches the client through `adesk_core::Error`'s message, so it
    /// should read as an explanation ("accessibility is disabled"), not as a
    /// backend name.
    pub fn new(reason: impl Into<String>) -> UnavailableSource {
        UnavailableSource {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl AccessibilitySource for UnavailableSource {
    fn name(&self) -> &'static str {
        // Deliberately NOT `"atspi"`: `name` is what a diagnostic prints, and
        // "atspi: unavailable" is indistinguishable from a lazily connected
        // backend that merely failed to connect. The runtime spells this mode
        // `off` (`--accessibility off`), so the backend reports the same word.
        "off"
    }

    async fn is_available(&self) -> bool {
        false
    }

    async fn snapshot(
        &self,
        _target: &WindowTarget,
        _opts: SourceOptions,
    ) -> Result<SourceSnapshot> {
        Err(A11yError::Unavailable(self.reason.clone()))
    }

    async fn invoke(
        &self,
        _handle: &ElementHandle,
        _action: Option<&str>,
    ) -> Result<InvokeOutcome> {
        Err(A11yError::Unavailable(self.reason.clone()))
    }
}

/// The `auto` backend as the trait object the service consumes.
///
/// This is what an unconfigured runtime serves §5.11 from: the real AT-SPI backend
/// when the environment has a bus, `not_supported` when it does not.
pub fn auto_source() -> Arc<dyn AccessibilitySource> {
    Arc::new(LazyAtspiSource::new())
}

/// The `off` backend as the trait object the service consumes.
///
/// `reason` is carried into every `not_supported` answer, so it should explain the
/// mode ("accessibility is disabled") rather than name a backend.
pub fn unavailable_source(reason: impl Into<String>) -> Arc<dyn AccessibilitySource> {
    Arc::new(UnavailableSource::new(reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::WindowId;
    use std::time::Duration;

    /// The outer bound on a test's first lazy call.
    ///
    /// It is deliberately larger than `dbus::CONNECT_TIMEOUT`: the backend is
    /// allowed to spend its own 5 s budget trying to connect, and a call that
    /// exceeds this bound is a regression to an *unbounded* wait, which is what
    /// the timeout turns into a test failure instead of a hung suite.
    const FIRST_CALL_BOUND: Duration = Duration::from_secs(30);

    /// The bound on a call that must answer without touching D-Bus at all.
    const IMMEDIATE: Duration = Duration::from_secs(1);

    /// A target that cannot correlate with anything: no pid, no title, no app id.
    fn uncorrelatable() -> WindowTarget {
        WindowTarget::new(WindowId(1))
    }

    /// A handle nobody could have produced on this bus.
    fn foreign_handle() -> ElementHandle {
        ElementHandle::from(":1.0|/org/a11y/atspi/accessible/root")
    }

    /// Whether `error` is the backend saying the environment has no bus.
    fn is_unavailable(error: &A11yError) -> bool {
        matches!(error, A11yError::Unavailable(_))
    }

    /// Exercise the connected backend as far as it can be exercised without a
    /// real application on the bus.
    ///
    /// The registry round trip is the part that proves the connection is real: a
    /// target with no correlation key must reach the applications and the frames
    /// the bus exposes and then report that nothing matched, rather than guessing
    /// a tree or failing with a D-Bus error.
    async fn exercise(source: &AtspiSource) {
        assert!(source.is_available().await);
        assert!(
            source.bus_name().is_some_and(|name| name.starts_with(':')),
            "a connected bus client has a unique name"
        );

        let error = source
            .snapshot(&uncorrelatable(), SourceOptions::new(1, 1))
            .await
            .expect_err("a target with no correlation key matches no accessible frame");
        assert!(
            matches!(error, A11yError::NotCorrelated(_)),
            "the registry was reached, so the answer is a correlation miss, not a bus error: {error}"
        );
    }

    /// The sandbox has no accessibility bus, so this is detection-gated in the
    /// same way `adesk-recorder`'s hardware tests and the compositor's `GL` tests
    /// are: with no bus the test prints why and returns, and with a bus it
    /// genuinely drives the backend.
    #[tokio::test]
    async fn the_backend_connects_to_a_real_accessibility_bus() {
        match tokio::time::timeout(FIRST_CALL_BOUND, AtspiSource::connect()).await {
            Ok(Ok(source)) => exercise(&source).await,
            Ok(Err(error)) => {
                println!(
                    "no accessibility bus in this environment, skipping the AT-SPI test: {error}"
                );
            }
            Err(_elapsed) => panic!("connecting to the accessibility bus must be time-bounded"),
        }
    }

    #[tokio::test]
    async fn the_lazy_backend_is_bounded_and_reports_unavailable_without_a_bus() {
        let source = LazyAtspiSource::new();
        let available = tokio::time::timeout(FIRST_CALL_BOUND, source.is_available())
            .await
            .expect("the lazy connect must be time-bounded");

        if available {
            println!("an accessibility bus is present, skipping the unavailable-path assertions");
            return;
        }

        // The failed connect is cached, so these answer immediately — the bound is
        // there to catch a regression to re-probing D-Bus on every request.
        let error = tokio::time::timeout(
            IMMEDIATE,
            source.snapshot(&uncorrelatable(), SourceOptions::new(1, 1)),
        )
        .await
        .expect("a snapshot on the unavailable path must not wait on D-Bus")
        .expect_err("no bus means no tree");
        assert!(is_unavailable(&error), "expected Unavailable, got {error}");

        let error = tokio::time::timeout(IMMEDIATE, source.invoke(&foreign_handle(), None))
            .await
            .expect("an invoke on the unavailable path must not wait on D-Bus")
            .expect_err("no bus means no action");
        assert!(is_unavailable(&error), "expected Unavailable, got {error}");

        // And the decision really is cached: a second availability probe is now
        // free.
        assert!(!source.is_available().await);
    }

    #[tokio::test]
    async fn the_off_backend_answers_immediately_and_touches_no_bus() {
        let source = UnavailableSource::new("accessibility is disabled");
        assert_eq!(source.name(), "off");
        assert_ne!(
            source.name(),
            BACKEND_NAME,
            "a disabled backend must not be reported as the connected one"
        );

        assert!(
            !tokio::time::timeout(IMMEDIATE, source.is_available())
                .await
                .expect("availability must be immediate"),
            "the off backend is never available"
        );

        let error = tokio::time::timeout(
            IMMEDIATE,
            source.snapshot(&uncorrelatable(), SourceOptions::new(1, 1)),
        )
        .await
        .expect("the off backend never waits")
        .expect_err("the off backend has no tree");
        assert!(
            matches!(&error, A11yError::Unavailable(reason) if reason == "accessibility is disabled"),
            "the reason is handed to the caller verbatim, got {error}"
        );

        let error =
            tokio::time::timeout(IMMEDIATE, source.invoke(&foreign_handle(), Some("click")))
                .await
                .expect("the off backend never waits")
                .expect_err("the off backend performs no action");
        assert!(is_unavailable(&error), "expected Unavailable, got {error}");
    }

    #[tokio::test]
    async fn the_selectors_return_working_backend_trait_objects() {
        let auto: Arc<dyn AccessibilitySource> = auto_source();
        assert_eq!(auto.name(), BACKEND_NAME);
        assert!(
            tokio::time::timeout(FIRST_CALL_BOUND, auto.is_available())
                .await
                .is_ok(),
            "the auto backend must be usable from behind the trait object, and bounded"
        );

        let off: Arc<dyn AccessibilitySource> = unavailable_source("accessibility is off");
        assert_eq!(off.name(), "off");
        assert!(!off.is_available().await);
        let error = off
            .snapshot(&uncorrelatable(), SourceOptions::new(1, 1))
            .await
            .expect_err("the off backend has no tree");
        assert!(
            matches!(&error, A11yError::Unavailable(reason) if reason == "accessibility is off"),
            "got {error}"
        );
    }

    #[test]
    fn clones_of_the_lazy_backend_share_one_connect_attempt() {
        let source = LazyAtspiSource::new();
        let clone = source.clone();

        assert!(
            Arc::ptr_eq(&source.connection, &clone.connection),
            "clones must share the cache, not each hold their own"
        );
    }
}
