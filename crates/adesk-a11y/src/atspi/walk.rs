//! Reading an accessible element and walking a window's subtree.
//!
//! Everything here works from one element's *interface list*: AT-SPI is a set of
//! optional interfaces (`Component`, `Action`, `Text`, `Value`, ...) that a toolkit
//! may or may not implement on a given element, so a read never guesses — it asks
//! `GetInterfaces` once and then only calls the interfaces that are actually
//! there. A probe that still fails with "no such interface / method / property" is
//! an absence (the interface went away between the two calls); every other failure
//! is a real backend error and is propagated.
//!
//! The walk itself is depth-first and pre-order, bounded by
//! [`SourceOptions::max_depth`] and [`SourceOptions::max_nodes`]. A bound is never
//! an error: the partial tree is returned and
//! [`SourceSnapshot::truncated`] says so.
//!
//! [`SourceSnapshot::truncated`]: crate::SourceSnapshot::truncated

use atspi::proxy::action::ActionProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::proxy::text::TextProxy;
use atspi::proxy::value::ValueProxy;
use atspi::zbus::proxy::{CacheProperties, ProxyImpl};
use atspi::zbus::{Connection, Proxy};
use atspi::{CoordType, Interface, ObjectRefOwned};
use tracing::trace;

use adesk_core::Rect;

use crate::error::{A11yError, Result};
use crate::source::{ElementHandle, InvokeOutcome, SourceNode, SourceOptions};

use super::dbus::{self, backend_error, is_absent_interface, is_missing_element, ElementAddress};
use super::map;

/// The screen-coordinate origin of the correlated window root.
///
/// Every element bound is made relative to this origin, so it is read once per
/// snapshot. An element with no `Component` interface (or no geometry at all)
/// reports no origin, which is the same as `(0, 0)`: the bounds are then taken as
/// the toolkit reported them, which is what a Wayland toolkit does anyway.
pub(crate) async fn screen_origin(
    connection: &Connection,
    root: &ObjectRefOwned,
) -> Result<(i32, i32)> {
    let Some(address) = ElementAddress::of(root) else {
        return Ok((0, 0));
    };
    let proxy = dbus::accessible(connection, &address)
        .await
        .map_err(|error| backend_error("addressing the window's accessible frame", error))?;
    let interfaces = proxy
        .get_interfaces()
        .await
        .map_err(|error| backend_error("reading the window's accessible interfaces", error))?;

    if !interfaces.contains(Interface::Component) {
        return Ok((0, 0));
    }
    Ok(component_extents(connection, &address)
        .await?
        .map_or((0, 0), |(x, y, _, _)| (x, y)))
}

/// Walk `root`'s subtree depth-first in pre-order, bounded by `opts`.
///
/// Returns the truncated tree and whether a bound cut it. The walk is iterative
/// and bounded in node count, so a hostile or deeply nested application cannot
/// overflow the stack or run away (`docs/architecture.md` §12).
pub(crate) async fn walk(
    connection: &Connection,
    root: &ObjectRefOwned,
    origin: (i32, i32),
    opts: SourceOptions,
) -> Result<(SourceNode, bool)> {
    /// One step of the walk: read an element, or close the element it is inside.
    enum Step {
        Read {
            reference: ObjectRefOwned,
            depth: u32,
        },
        Close,
    }

    let mut steps = vec![Step::Read {
        reference: root.clone(),
        depth: 0,
    }];
    // The elements being built, outermost first. A child is appended to its parent
    // as soon as it is complete, so at most one node per open ancestor is ever held.
    let mut open: Vec<SourceNode> = Vec::new();
    let mut read: u32 = 0;
    let budget = opts.max_nodes.max(1);
    let mut truncated = false;

    while let Some(step) = steps.pop() {
        match step {
            Step::Close => close(&mut open),
            Step::Read { reference, depth } => {
                if read >= budget {
                    truncated = true;
                    break;
                }
                let Some(address) = ElementAddress::of(&reference) else {
                    return Err(A11yError::Backend(
                        "an accessible element has no bus name".into(),
                    ));
                };
                let (node, children) = read_element(connection, &address, origin).await?;
                read += 1;
                open.push(node);

                if children.is_empty() {
                    close(&mut open);
                } else if depth >= opts.max_depth {
                    // The depth bound stops here; the element's children are not
                    // walked, which is exactly what `truncated` reports.
                    truncated = true;
                    close(&mut open);
                } else {
                    steps.push(Step::Close);
                    for child in children.into_iter().rev() {
                        steps.push(Step::Read {
                            reference: child,
                            depth: depth + 1,
                        });
                    }
                }
            }
        }
    }

    // A walk cut short by the node budget leaves its ancestors open; fold them.
    while open.len() > 1 {
        close(&mut open);
    }
    let Some(root) = open.pop() else {
        return Err(A11yError::Backend(
            "the accessibility walk produced no window root".into(),
        ));
    };
    Ok((root, truncated))
}

/// Fold the innermost open node into its parent.
///
/// The walk root sits alone at the bottom of the stack and has no parent to fold
/// into; a one-deep stack is therefore left alone and becomes the snapshot root.
fn close(open: &mut Vec<SourceNode>) {
    if open.len() < 2 {
        return;
    }
    let Some(done) = open.pop() else {
        return;
    };
    if let Some(parent) = open.last_mut() {
        parent.children.push(done);
    }
}

/// Read one element: its node, and the references of its children.
async fn read_element(
    connection: &Connection,
    address: &ElementAddress,
    origin: (i32, i32),
) -> Result<(SourceNode, Vec<ObjectRefOwned>)> {
    let proxy = dbus::accessible(connection, address)
        .await
        .map_err(|error| backend_error("addressing an accessible element", error))?;

    let role = map::role_name(
        &proxy
            .get_role_name()
            .await
            .map_err(|error| backend_error("reading an accessible role", error))?,
    );
    let name = proxy
        .name()
        .await
        .map_err(|error| backend_error("reading an accessible name", error))?;
    let description = map::text_value(
        &proxy
            .description()
            .await
            .map_err(|error| backend_error("reading an accessible description", error))?,
    );
    let states = map::states(
        proxy
            .get_state()
            .await
            .map_err(|error| backend_error("reading an accessible state", error))?,
    );
    let interfaces = proxy
        .get_interfaces()
        .await
        .map_err(|error| backend_error("reading an element's interfaces", error))?;

    let bounds = read_bounds(connection, address, interfaces, origin).await?;
    let actions = read_actions(connection, address, interfaces).await?;
    let value = read_value(connection, address, interfaces).await?;

    let children = proxy
        .get_children()
        .await
        .map_err(|error| backend_error("listing an element's children", error))?;

    let node = SourceNode {
        role,
        name,
        description,
        value,
        states,
        bounds,
        actions,
        handle: ElementHandle(address.handle()),
        children: Vec::new(),
    };

    Ok((
        node,
        children
            .into_iter()
            .filter(|child| !child.is_null())
            .collect(),
    ))
}

/// Build one optional-interface proxy for an element.
async fn interface_proxy<'c, P>(
    connection: &'c Connection,
    address: &ElementAddress,
) -> atspi::zbus::Result<P>
where
    P: ProxyImpl<'c> + From<Proxy<'c>>,
{
    P::builder(connection)
        .destination(address.name.clone())?
        .path(address.path.clone())?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

/// The element's extents in `Screen` coordinates, when it has a `Component`
/// interface.
async fn component_extents(
    connection: &Connection,
    address: &ElementAddress,
) -> Result<Option<(i32, i32, i32, i32)>> {
    let proxy: ComponentProxy<'_> = interface_proxy(connection, address)
        .await
        .map_err(|error| backend_error("addressing an element's geometry", error))?;

    match proxy.get_extents(CoordType::Screen).await {
        Ok(extents) => Ok(Some(extents)),
        Err(error) if is_absent_interface(&error) => {
            trace!(element = %address.handle(), "an element has no geometry");
            Ok(None)
        }
        Err(error) => Err(backend_error("reading an element's extents", error)),
    }
}

/// The element's window-relative bounds, when it reports any.
async fn read_bounds(
    connection: &Connection,
    address: &ElementAddress,
    interfaces: atspi::InterfaceSet,
    origin: (i32, i32),
) -> Result<Option<Rect>> {
    if !interfaces.contains(Interface::Component) {
        return Ok(None);
    }
    Ok(component_extents(connection, address)
        .await?
        .and_then(|extents| map::relative_bounds(extents, origin)))
}

/// The names of the actions the element exposes.
async fn read_actions(
    connection: &Connection,
    address: &ElementAddress,
    interfaces: atspi::InterfaceSet,
) -> Result<Vec<String>> {
    if !interfaces.contains(Interface::Action) {
        return Ok(Vec::new());
    }
    Ok(action_names(connection, address).await?.unwrap_or_default())
}

/// The action names an element exposes, or `None` when it turns out to have no
/// `Action` interface after all.
async fn action_names(
    connection: &Connection,
    address: &ElementAddress,
) -> Result<Option<Vec<String>>> {
    let proxy: ActionProxy<'_> = interface_proxy(connection, address)
        .await
        .map_err(|error| backend_error("addressing an element's actions", error))?;

    match proxy.get_actions().await {
        Ok(actions) => Ok(Some(
            actions.into_iter().map(|action| action.name).collect(),
        )),
        Err(error) if is_absent_interface(&error) => {
            trace!(element = %address.handle(), "an element exposes no actions");
            Ok(None)
        }
        Err(error) if is_missing_element(&error) => {
            trace!(element = %address.handle(), "an element vanished while its actions were read");
            Ok(None)
        }
        Err(error) => Err(backend_error("reading an element's actions", error)),
    }
}

/// The element's value: its full text when it is a text element, else its
/// `Value` interface's current value.
///
/// The interface list decides what to ask for, so the whole text of an element
/// that is not a text element is never read (`docs/accessibility.md`, "Model").
async fn read_value(
    connection: &Connection,
    address: &ElementAddress,
    interfaces: atspi::InterfaceSet,
) -> Result<Option<String>> {
    if interfaces.contains(Interface::Text) {
        if let Some(text) = text_content(connection, address).await? {
            return Ok(Some(text));
        }
    }
    if interfaces.contains(Interface::Value) {
        return numeric_value(connection, address).await;
    }
    Ok(None)
}

/// The element's whole `Text` content, or `None` when it has none.
async fn text_content(connection: &Connection, address: &ElementAddress) -> Result<Option<String>> {
    let proxy: TextProxy<'_> = interface_proxy(connection, address)
        .await
        .map_err(|error| backend_error("addressing an element's text", error))?;

    let count = match proxy.character_count().await {
        Ok(count) => count,
        Err(error) if is_absent_interface(&error) || is_missing_element(&error) => {
            trace!(element = %address.handle(), "an element has no text");
            return Ok(None);
        }
        Err(error) => return Err(backend_error("reading an element's text length", error)),
    };
    if count <= 0 {
        return Ok(None);
    }

    match proxy.get_text(0, count).await {
        Ok(text) => Ok(map::text_value(&text)),
        Err(error) if is_absent_interface(&error) || is_missing_element(&error) => {
            trace!(element = %address.handle(), "an element's text vanished");
            Ok(None)
        }
        Err(error) => Err(backend_error("reading an element's text", error)),
    }
}

/// The element's `Value` interface current value, rendered as a string.
async fn numeric_value(
    connection: &Connection,
    address: &ElementAddress,
) -> Result<Option<String>> {
    let proxy: ValueProxy<'_> = interface_proxy(connection, address)
        .await
        .map_err(|error| backend_error("addressing an element's value", error))?;

    match proxy.current_value().await {
        Ok(value) if value.is_finite() => Ok(map::text_value(&value.to_string())),
        // A value that is not a number carries no information for an agent.
        Ok(_) => Ok(None),
        Err(error) if is_absent_interface(&error) || is_missing_element(&error) => {
            trace!(element = %address.handle(), "an element has no value");
            Ok(None)
        }
        Err(error) => Err(backend_error("reading an element's value", error)),
    }
}

/// Invoke an action of the element a handle addresses.
///
/// A handle that is not one of ours, or whose element has left the bus, is
/// [`InvokeOutcome::Gone`]; an element that exists but does not expose the
/// requested action is [`InvokeOutcome::NoSuchAction`]. Neither is an error: the
/// service turns them into the §5.11 answer the caller's own `AccessibleId` names.
pub(crate) async fn invoke(
    connection: &Connection,
    handle: &ElementHandle,
    action: Option<&str>,
) -> Result<InvokeOutcome> {
    let Some(address) = ElementAddress::parse(handle.as_str()) else {
        return Ok(InvokeOutcome::Gone);
    };
    let proxy: ActionProxy<'_> = interface_proxy(connection, &address)
        .await
        .map_err(|error| backend_error("addressing an accessible element", error))?;

    let actions = match proxy.get_actions().await {
        Ok(actions) => actions,
        // The element is there but has no `Action` interface at all, so it exposes
        // no action to invoke.
        Err(error) if is_absent_interface(&error) => return Ok(InvokeOutcome::NoSuchAction),
        Err(error) if is_missing_element(&error) => return Ok(InvokeOutcome::Gone),
        Err(error) => return Err(backend_error("reading an element's actions", error)),
    };

    let index = match action {
        Some(requested) => match actions.iter().position(|known| known.name == requested) {
            Some(index) => index,
            None => return Ok(InvokeOutcome::NoSuchAction),
        },
        None if actions.is_empty() => return Ok(InvokeOutcome::NoSuchAction),
        // With no action named, the toolkit's convention is that the first one is
        // the element's default action.
        None => 0,
    };
    let invoked = actions[index].name.clone();

    match proxy.do_action(index as i32).await {
        Ok(performed) => {
            if !performed {
                // The action exists and was addressed, but the toolkit performed
                // nothing (a disabled widget, a refused dialog). There is no AGP
                // outcome for "refused", and the element does expose the action, so
                // the invocation is reported with the name that was addressed.
                trace!(action = %invoked, "the toolkit reported the action was not performed");
            }
            Ok(InvokeOutcome::Invoked(invoked))
        }
        Err(error) if is_absent_interface(&error) => Ok(InvokeOutcome::NoSuchAction),
        Err(error) if is_missing_element(&error) => Ok(InvokeOutcome::Gone),
        Err(error) => Err(backend_error("invoking an element action", error)),
    }
}
