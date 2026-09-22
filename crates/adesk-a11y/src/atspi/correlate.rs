//! Window → accessible-frame correlation (`docs/accessibility.md`,
//! "Correlation").
//!
//! The agent addresses windows by `window_id`, while AT-SPI addresses applications
//! and the frames inside them, so a §5.11 request has to relate the two. The
//! correlation is done from the window model's own snapshot (the server fills
//! [`WindowTarget`] from `QueryState`) and never from state the compositor thread
//! owns.
//!
//! Failure is reported, never guessed: when no frame matches, the backend answers
//! [`A11yError::NotCorrelated`] rather than falling back to "the only application
//! on the bus", to the first frame, or to an empty tree — a wrong tree is worse
//! than a missing one, because the agent could read *and act on* another
//! application's widgets.

use atspi::connection::AccessibilityConnection;
use atspi::zbus::fdo::DBusProxy;
use atspi::zbus::names::BusName;
use atspi::zbus::Connection;
use atspi::ObjectRefOwned;
use tracing::{debug, trace};

use crate::error::{A11yError, Result};
use crate::source::WindowTarget;

use super::dbus::{
    self, backend_error, is_absent_interface, is_missing_element, is_missing_name, ElementAddress,
};
use super::map;

/// The accessible subtree a [`WindowTarget`] was correlated with.
pub(crate) struct Correlated {
    /// Accessible name of the application the window belongs to, when it has one.
    pub(crate) app_name: Option<String>,
    /// Reference to the element that roots the window's subtree — the window's own
    /// accessible frame, not the application.
    pub(crate) root: ObjectRefOwned,
}

/// Find the accessible subtree of `target` on the accessibility bus.
///
/// The correlation ladder is tried in order of confidence and stops at the first
/// hit (`docs/accessibility.md`, "Correlation"):
///
/// 1. **pid** — resolve each application's bus name to a process id through the bus
///    daemon and match the window's application. This is the strongest signal: it
///    identifies the application itself, not a string that may repeat.
/// 2. **title** — a top-level frame of some application whose accessible name is
///    exactly the window's title.
/// 3. **application identity** — an application whose name is the window's
///    `app_id` (or contains it), or contains the window's title; the window is then
///    that application's first top-level frame.
pub(crate) async fn correlate(
    connection: &AccessibilityConnection,
    target: &WindowTarget,
) -> Result<Correlated> {
    let applications = applications(connection).await?;

    if let Some(pid) = target.pid {
        if let Some(application) =
            application_with_pid(connection.connection(), &applications, pid).await?
        {
            return within_application(connection.connection(), &application, target).await;
        }
    }

    if let Some(title) = target.title.as_deref().filter(|title| !title.is_empty()) {
        for application in &applications {
            for frame in frames(connection.connection(), application).await? {
                if frame.name.as_deref() == Some(title) {
                    return Ok(Correlated {
                        app_name: element_name(connection.connection(), application).await?,
                        root: frame.reference,
                    });
                }
            }
        }
    }

    for application in &applications {
        let Some(name) = element_name(connection.connection(), application).await? else {
            continue;
        };
        if !matches_application(&name, target) {
            continue;
        }
        if let Some(frame) = frames(connection.connection(), application)
            .await?
            .into_iter()
            .next()
        {
            return Ok(Correlated {
                app_name: Some(name),
                root: frame.reference,
            });
        }
    }

    Err(A11yError::NotCorrelated(target.window_id))
}

/// The elements an application's root object points at: every application
/// registered on the accessibility bus.
async fn applications(connection: &AccessibilityConnection) -> Result<Vec<ObjectRefOwned>> {
    let registry = connection
        .root_accessible_on_registry()
        .await
        .map_err(|error| backend_error("addressing the accessibility registry", error))?;
    let applications = registry
        .get_children()
        .await
        .map_err(|error| backend_error("listing the accessible applications", error))?;

    Ok(applications
        .into_iter()
        .filter(|application| !application.is_null())
        .collect())
}

/// One of an application's top-level frames: the element the toolkit groups a
/// window's widgets, menus and dialogs under.
struct Frame {
    /// The frame's accessible name, when it reports one.
    name: Option<String>,
    /// Reference to the frame itself.
    reference: ObjectRefOwned,
}

/// The top-level frames of `application`, in the toolkit's reported order.
async fn frames(connection: &Connection, application: &ObjectRefOwned) -> Result<Vec<Frame>> {
    let Some(address) = ElementAddress::of(application) else {
        return Ok(Vec::new());
    };
    let proxy = dbus::accessible(connection, &address)
        .await
        .map_err(|error| backend_error("addressing an accessible application", error))?;

    let children = match proxy.get_children().await {
        Ok(children) => children,
        // The application left the bus between the two calls: it has no frames.
        Err(error) if is_missing_element(&error) => {
            debug!(application = %address.handle(), "an accessible application disappeared");
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(backend_error(
                "listing an application's accessible frames",
                error,
            ))
        }
    };

    let mut frames = Vec::new();
    for child in children {
        if child.is_null() {
            continue;
        }
        frames.push(Frame {
            name: element_name(connection, &child).await?,
            reference: child,
        });
    }
    Ok(frames)
}

/// The accessible `name` of an element.
///
/// `None` when the element reports no name, has gone away, or does not expose the
/// `Accessible` interface after all; any other failure is a real backend error and
/// is propagated, because turning it into "no name" would silently corrupt the
/// correlation.
async fn element_name(connection: &Connection, element: &ObjectRefOwned) -> Result<Option<String>> {
    let Some(address) = ElementAddress::of(element) else {
        return Ok(None);
    };
    let proxy = dbus::accessible(connection, &address)
        .await
        .map_err(|error| backend_error("addressing an accessible element", error))?;

    match proxy.name().await {
        Ok(name) => Ok(map::text_value(&name)),
        Err(error) if is_missing_element(&error) || is_absent_interface(&error) => {
            trace!(element = %address.handle(), "an accessible element has no name");
            Ok(None)
        }
        Err(error) => Err(backend_error("reading an accessible name", error)),
    }
}

/// The application whose process id is `pid`, when one is registered.
async fn application_with_pid(
    connection: &Connection,
    applications: &[ObjectRefOwned],
    pid: i32,
) -> Result<Option<ObjectRefOwned>> {
    let bus = DBusProxy::new(connection)
        .await
        .map_err(|error| backend_error("addressing the bus daemon", error))?;

    for application in applications {
        let Some(name) = application.name() else {
            continue;
        };
        match bus
            .get_connection_unix_process_id(BusName::from(name.clone()))
            .await
        {
            Ok(found) if i64::from(found) == i64::from(pid) => {
                return Ok(Some(application.clone()))
            }
            Ok(_) => continue,
            // An application that left the bus between the enumeration and this
            // call is simply not a candidate; it can never be the window's.
            Err(error) if is_missing_name(&error) => {
                debug!("an accessible application disappeared while resolving its pid");
                continue;
            }
            Err(error) => {
                return Err(backend_error(
                    "resolving an accessible application's process id",
                    error,
                ))
            }
        }
    }

    Ok(None)
}

/// Correlate inside a known application: its window frame when it has one, ending
/// at the application's own root when it publishes no frame at all.
///
/// The window's title, when there is one, picks between the application's frames;
/// a title that matches none of them leaves the first frame, which is what a
/// single-window application always exposes.
async fn within_application(
    connection: &Connection,
    application: &ObjectRefOwned,
    target: &WindowTarget,
) -> Result<Correlated> {
    let app_name = element_name(connection, application).await?;
    let frames = frames(connection, application).await?;
    let root = target
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .and_then(|title| {
            frames
                .iter()
                .find(|frame| frame.name.as_deref() == Some(title))
        })
        .or_else(|| frames.first())
        .map_or_else(|| application.clone(), |frame| frame.reference.clone());

    Ok(Correlated { app_name, root })
}

/// Whether an application's accessible name is the window's application.
///
/// The name is matched against the desktop-file id exactly first, then as a
/// case-insensitive substring (`"Calculator"` against `"org.gnome.Calculator"`),
/// and against the window's title as a substring — the third rung of the
/// correlation ladder (`docs/accessibility.md`, "Correlation").
fn matches_application(application: &str, target: &WindowTarget) -> bool {
    if let Some(app_id) = target.app_id.as_ref() {
        if matches_app_id(application, app_id.as_str()) {
            return true;
        }
    }
    target
        .title
        .as_deref()
        .is_some_and(|title| !title.is_empty() && application.contains(title))
}

/// Whether an application name identifies a desktop-file id.
fn matches_app_id(application: &str, app_id: &str) -> bool {
    application.eq_ignore_ascii_case(app_id)
        || application.to_lowercase().contains(&app_id.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{AppId, WindowId};

    fn target() -> WindowTarget {
        WindowTarget {
            title: Some("Calculator".into()),
            app_id: Some(AppId::from("org.gnome.Calculator")),
            ..WindowTarget::new(WindowId(4))
        }
    }

    #[test]
    fn an_application_name_matches_its_desktop_file_id() {
        assert!(matches_app_id(
            "org.gnome.Calculator",
            "org.gnome.Calculator"
        ));
        assert!(matches_app_id(
            "org.gnome.Calculator",
            "org.gnome.calculator"
        ));
        assert!(matches_app_id(
            "org.gnome.Calculator (Nightly)",
            "org.gnome.Calculator"
        ));
        assert!(!matches_app_id("Files", "org.gnome.Calculator"));
    }

    #[test]
    fn an_application_matches_on_its_id_or_on_the_window_title() {
        assert!(matches_application("org.gnome.Calculator", &target()));
        assert!(matches_application("Calculator — 12", &target()));
        assert!(!matches_application("Files", &target()));
    }

    #[test]
    fn an_empty_title_never_matches_every_application() {
        // `contains("")` is true for every string, so an empty title must not be
        // allowed to correlate with whatever happens to be on the bus.
        let target = WindowTarget {
            title: Some(String::new()),
            app_id: None,
            ..WindowTarget::new(WindowId(4))
        };
        assert!(!matches_application("Files", &target));
    }

    #[test]
    fn a_target_without_any_key_correlates_with_nothing() {
        // No pid, no title and no app id: the ladder has nothing to match on, which
        // is why a request for such a window must end in `not_supported`.
        let target = WindowTarget::new(WindowId(4));
        assert!(!matches_application("Files", &target));
    }
}
