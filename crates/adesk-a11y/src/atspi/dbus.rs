//! The D-Bus plumbing shared by the AT-SPI backend.
//!
//! Three things live here: the connection to the accessibility bus (bounded in
//! time), the `({bus name}|{object path})` address an [`ElementHandle`] is spelled
//! with, and the classification of a D-Bus failure into either a *legitimate
//! absence* (this element has no such interface / method / property) or a real
//! backend error.
//!
//! The classification is the delicate part. A backend that treats every failed
//! call as "the element has no value" silently turns a broken bus into an empty
//! window, and one that treats every failure as fatal makes a toolkit that dropped
//! an optional interface mid-call look like an outage. Only the D-Bus error names
//! that actually mean "absent" are mapped to `None`/`Gone`; everything else is
//! propagated as [`A11yError::Backend`].
//!
//! [`ElementHandle`]: crate::ElementHandle

use std::time::Duration;

use atspi::connection::AccessibilityConnection;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::zbus::names::BusName;
use atspi::zbus::proxy::CacheProperties;
use atspi::zbus::zvariant::ObjectPath;
use atspi::zbus::{fdo::Error as FdoError, Connection, Error as ZbusError};
use atspi::ObjectRefOwned;

use crate::error::{A11yError, Result};

/// The time bound on connecting to the accessibility bus.
///
/// Connecting reads the a11y bus address off the session bus and may trigger D-Bus
/// activation, either of which can take arbitrarily long in a wedged environment;
/// the runtime must answer `not_supported` instead of hanging a request
/// (`docs/architecture.md` §12).
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Connect to the accessibility bus, never waiting longer than
/// [`CONNECT_TIMEOUT`].
///
/// Every failure — no session bus, no `org.a11y.Bus`, a timeout — is
/// [`A11yError::Unavailable`]: the environment has no accessibility bus to talk to,
/// which is a normal answer for the runtime (`docs/accessibility.md`, "Backend
/// seam").
pub(crate) async fn connect() -> Result<AccessibilityConnection> {
    match tokio::time::timeout(CONNECT_TIMEOUT, AccessibilityConnection::new()).await {
        Ok(Ok(connection)) => Ok(connection),
        Ok(Err(error)) => Err(A11yError::Unavailable(format!(
            "no accessibility bus: {error}"
        ))),
        Err(_elapsed) => Err(A11yError::Unavailable(format!(
            "connecting to the accessibility bus timed out after {}s",
            CONNECT_TIMEOUT.as_secs()
        ))),
    }
}

/// The bus name and object path that address one accessible element.
///
/// This is the whole of what an [`ElementHandle`] hides: the toolkit's own
/// addressing, spelled `"{bus_name}|{object_path}"`. Neither a D-Bus bus name nor
/// an object path may contain `|`, so a handle is unambiguous.
///
/// [`ElementHandle`]: crate::ElementHandle
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ElementAddress {
    /// Unique bus name of the application the element belongs to.
    pub(crate) name: BusName<'static>,
    /// Object path of the element in that application.
    pub(crate) path: ObjectPath<'static>,
}

impl ElementAddress {
    /// The address of the element an application's `ObjectRef` points at.
    ///
    /// `None` for a null reference (an element the toolkit has no name for), which
    /// is never addressable.
    pub(crate) fn of(reference: &ObjectRefOwned) -> Option<ElementAddress> {
        let name = reference.name()?.clone();
        Some(ElementAddress {
            name: BusName::from(name),
            path: reference.path().clone(),
        })
    }

    /// Parse the `"{bus_name}|{object_path}"` spelling a handle carries.
    ///
    /// `None` for anything that is not one of ours: a handle without a separator, a
    /// malformed bus name, or a malformed object path.
    pub(crate) fn parse(handle: &str) -> Option<ElementAddress> {
        let (name, path) = handle.split_once('|')?;
        Some(ElementAddress {
            name: BusName::try_from(name.to_owned()).ok()?,
            path: ObjectPath::try_from(path).ok()?.to_owned(),
        })
    }

    /// The handle spelling of this address.
    pub(crate) fn handle(&self) -> String {
        format!("{}|{}", self.name.as_str(), self.path.as_str())
    }
}

/// Build the `Accessible` interface proxy of one element.
///
/// This is the proxy every node read starts from; the optional interfaces a toolkit
/// may or may not implement on the same element get their own proxy, built only
/// after that element's interface list says the interface is there.
pub(crate) async fn accessible<'c>(
    connection: &'c Connection,
    address: &ElementAddress,
) -> atspi::zbus::Result<AccessibleProxy<'c>> {
    AccessibleProxy::builder(connection)
        .destination(address.name.clone())?
        .path(address.path.clone())?
        // A name, a description and a state set all change while an application
        // runs, so nothing may be served out of a property cache.
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

/// Wrap a D-Bus failure as the crate's backend error, naming the call it came
/// from so a client sees more than "something went wrong".
pub(crate) fn backend_error(what: &str, error: impl std::fmt::Display) -> A11yError {
    A11yError::Backend(format!("{what}: {error}"))
}

/// Whether a D-Bus failure says the element has no such **interface, method or
/// property** — a legitimate absence on an element that otherwise exists.
///
/// A toolkit answers with one of these when a duck-typed probe reaches an element
/// that does not implement the interface the probe assumed, or that dropped it
/// between two calls; the value is then simply absent.
pub(crate) fn is_absent_interface(error: &ZbusError) -> bool {
    match error {
        ZbusError::InterfaceNotFound => true,
        other => error_name(other).is_some_and(absent_interface_error),
    }
}

/// Whether a D-Bus failure says the element is **gone**: its application left the
/// bus, or the object path no longer exists.
pub(crate) fn is_missing_element(error: &ZbusError) -> bool {
    error_name(error).is_some_and(missing_element_error)
}

/// Whether an `fdo` failure says the **bus name has no owner** any more.
///
/// The bus daemon replies with a different error type than an application does
/// (`zbus::fdo::Error` rather than `zbus::Error`), so resolving an application to a
/// process id needs its own classifier; a name that has left the bus between the
/// registry enumeration and this call is simply not a candidate for correlation.
pub(crate) fn is_missing_name(error: &FdoError) -> bool {
    match error {
        FdoError::ZBus(inner) => is_missing_element(inner),
        FdoError::ServiceUnknown(_) | FdoError::NameHasNoOwner(_) => true,
        _ => false,
    }
}
/// The D-Bus error name a reply carried, when the failure was a remote error
/// rather than a transport or client-side one.
fn error_name(error: &ZbusError) -> Option<&str> {
    match error {
        ZbusError::MethodError(name, _, _) => Some(name.as_str()),
        _ => None,
    }
}

/// The error names that mean "no such interface / method / property".
fn absent_interface_error(name: &str) -> bool {
    matches!(
        name,
        "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.UnknownProperty"
            | "org.freedesktop.DBus.Error.NotSupported"
    )
}

/// The error names that mean "that element is not there any more".
fn missing_element_error(name: &str) -> bool {
    matches!(
        name,
        "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.NoReply"
            | "org.freedesktop.DBus.Error.Disconnected"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handle_round_trips_through_an_element_address() {
        let address = ElementAddress::parse(":1.42|/org/a11y/atspi/accessible/root")
            .expect("a well-formed handle parses");
        assert_eq!(address.name.as_str(), ":1.42");
        assert_eq!(address.path.as_str(), "/org/a11y/atspi/accessible/root");
        assert_eq!(address.handle(), ":1.42|/org/a11y/atspi/accessible/root");
    }

    #[test]
    fn a_handle_that_is_not_one_of_ours_does_not_parse() {
        assert_eq!(ElementAddress::parse(""), None);
        assert_eq!(ElementAddress::parse(":1.42"), None, "no separator");
        assert_eq!(
            ElementAddress::parse(":1.42|not/a/path"),
            None,
            "an object path starts with a slash"
        );
        assert_eq!(ElementAddress::parse(":1.42|"), None, "an empty path");
        assert_eq!(
            ElementAddress::parse("not a bus name|/org/a11y/atspi/accessible/root"),
            None
        );
    }

    #[test]
    fn an_absent_interface_is_not_a_real_failure() {
        assert!(absent_interface_error(
            "org.freedesktop.DBus.Error.UnknownInterface"
        ));
        assert!(absent_interface_error(
            "org.freedesktop.DBus.Error.UnknownMethod"
        ));
        assert!(absent_interface_error(
            "org.freedesktop.DBus.Error.UnknownProperty"
        ));

        assert!(!absent_interface_error(
            "org.freedesktop.DBus.Error.AccessDenied"
        ));
        assert!(!absent_interface_error(
            "org.freedesktop.DBus.Error.NoReply"
        ));
        assert!(!absent_interface_error("com.example.CustomError"));
    }

    #[test]
    fn a_gone_element_is_recognized_by_its_error_name() {
        assert!(missing_element_error(
            "org.freedesktop.DBus.Error.ServiceUnknown"
        ));
        assert!(missing_element_error(
            "org.freedesktop.DBus.Error.NameHasNoOwner"
        ));
        assert!(missing_element_error(
            "org.freedesktop.DBus.Error.UnknownObject"
        ));

        // A wedged toolkit is not a vanished element: it must not be reported as
        // `Gone`, because the element may well still be there.
        assert!(!missing_element_error(
            "org.freedesktop.DBus.Error.LimitsExceeded"
        ));
        assert!(!missing_element_error(
            "org.freedesktop.DBus.Error.UnknownMethod"
        ));
    }

    #[test]
    fn a_gone_bus_name_is_recognized_by_its_fdo_error() {
        // The bus daemon answers with `fdo` errors, so its `NameHasNoOwner` and
        // `ServiceUnknown` must classify like an application's `UnknownObject`.
        assert!(is_missing_name(&FdoError::ServiceUnknown("gone".into())));
        assert!(is_missing_name(&FdoError::NameHasNoOwner("gone".into())));

        // A `zbus` failure only says the name is gone when it is one of *those*
        // names; an absent interface is an absence, not a vanished peer.
        assert!(!is_missing_name(&FdoError::ZBus(
            ZbusError::InterfaceNotFound
        )));

        assert!(!is_missing_name(&FdoError::AccessDenied("nope".into())));
        assert!(!is_missing_name(&FdoError::LimitsExceeded("wedged".into())));
    }

    #[test]
    fn a_backend_error_names_the_call_it_came_from() {
        let error = backend_error("reading an element's extents", "boom");
        assert!(matches!(
            &error,
            A11yError::Backend(message) if message == "reading an element's extents: boom"
        ));
    }
}
