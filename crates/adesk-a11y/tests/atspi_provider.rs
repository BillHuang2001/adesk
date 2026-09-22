//! Integration test: the real AT-SPI2 backend, driven end to end against a mock
//! provider over a private bus.
//!
//! `src/atspi/` is unit-tested piece by piece without a bus (the pure vocabulary
//! mapping, the correlation ladder, the address spelling, the error
//! classification) and `src/atspi/mod.rs` probes connectivity once, but the
//! *request path* — `connect()` → correlate a [`WindowTarget`] → bounded pre-order
//! walk → `invoke` — has never been driven against a peer that actually answers.
//! That is what this file adds, with no display, no GPU, no network and no
//! installed GUI application:
//!
//! 1. The test starts a **private** `dbus-daemon` on a socket of its own and
//!    points `DBUS_SESSION_BUS_ADDRESS` at it, so the real
//!    `AtspiSource::connect()` runs unchanged — including the `org.a11y.Bus`
//!    handshake `zbus::Connection::session()` → `GetAddress` → connect to the
//!    returned address.
//! 2. On that bus it *serves* the AT-SPI objects the backend asks for: the bus's
//!    own `GetConnectionUnixProcessID` (so pid correlation has something to
//!    match), our `org.a11y.Bus`, the registry and its root `Accessible` (whose
//!    children are the applications), one application root, and a window frame
//!    with three children — a button, an entry and a slider — carrying the
//!    optional `Component`, `Action`, `Text` and `Value` interfaces the walk
//!    probes for.
//! 3. Then it runs the backend against it and asserts what came back: the
//!    correlated application name, the walked tree (roles, names, descriptions,
//!    states, window-relative bounds, actions, text and numeric values), the
//!    handle spelling, the bounds-and-truncation behaviour, the whole correlation
//!    ladder, and every `InvokeOutcome`.
//!
//! The provider mirrors the interfaces the backend *reads*, as `src/atspi/`
//! spells them, rather than the AT-SPI spec in general: `GetChildren`,
//! `GetInterfaces`, `GetRoleName`, `GetState` and the `Name`/`Description`
//! properties on `Accessible`, plus `GetExtents`, `GetActions`/`DoAction`,
//! `CharacterCount`/`GetText`, the `CurrentValue` property and
//! `GetApplicationBusAddress` (deliberately unsupported, so the client's
//! peer-to-peer initialization skips this application instead of dialling it).
//!
//! **Skip policy.** A container or CI job without `dbus-daemon` prints the reason
//! and returns early; the test never requires a real accessibility bus, a
//! toolkit or an application. `DBUS_SESSION_BUS_ADDRESS` is process-global, so the
//! tests in this file take turns on `BUS` and each restores the variable it found.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adesk_a11y::{
    AccessibilitySource, AtspiSource, ElementHandle, InvokeOutcome, SourceOptions, SourceSnapshot,
    WindowTarget,
};
use adesk_core::{AccessibleState, AppId, Rect, WindowId};
use atspi::{
    Action, CoordType, Interface, InterfaceSet, ObjectRef, ObjectRefOwned, State, StateSet,
};
use zbus::names::UniqueName;
use zbus::zvariant::ObjectPath;

/// The environment variable zbus reads the session bus address from, and the one
/// this test has to borrow.
const SESSION_BUS_ENV: &str = "DBUS_SESSION_BUS_ADDRESS";

/// How long the private bus may take to print its address.
const DAEMON_START_BOUND: Duration = Duration::from_secs(10);

/// How long the whole driven backend may take before the test calls it a hang.
///
/// Every individual call is bounded by the backend itself (`CONNECT_TIMEOUT`), so
/// this only turns a stall the backend cannot bound — a deadlock between test and
/// mock — into a failure instead of a hung suite.
const DRIVE_BOUND: Duration = Duration::from_secs(30);

// --- the mock provider's objects -------------------------------------------------

/// The registry's own root object: its children are the applications on the bus.
const REGISTRY_ROOT: &str = "/org/a11y/atspi/accessible/root";
/// The mock application's root object.
const APP_ROOT: &str = "/org/adesk/MockApp";
/// The mock window's accessible frame.
const FRAME: &str = "/org/adesk/MockApp/frame";
/// A push button inside the frame.
const BUTTON: &str = "/org/adesk/MockApp/frame/button";
/// A text entry inside the frame.
const ENTRY: &str = "/org/adesk/MockApp/frame/entry";
/// A slider inside the frame, to exercise the `Value` interface.
const SLIDER: &str = "/org/adesk/MockApp/frame/slider";

/// The window title the mock frame reports, and the title the backend correlates on.
const WINDOW_TITLE: &str = "Mock Window";
/// The accessible name of the mock application (its desktop-file id, which is what
/// the third correlation rung matches too).
const APP_NAME: &str = "org.adesk.MockApp";

/// The frame's screen origin and size: every child bound is asserted relative to it.
const FRAME_EXTENTS: (i32, i32, i32, i32) = (100, 50, 800, 600);
/// The button's screen extents — 20/30 from the frame origin.
const BUTTON_EXTENTS: (i32, i32, i32, i32) = (120, 80, 60, 25);
/// The entry's screen extents.
const ENTRY_EXTENTS: (i32, i32, i32, i32) = (120, 130, 200, 30);
/// The slider's screen extents.
const SLIDER_EXTENTS: (i32, i32, i32, i32) = (120, 180, 200, 20);

/// The text the mock entry reports through the `Text` interface.
const ENTRY_TEXT: &str = "hello";
/// The value the mock slider reports through the `Value` interface.
const SLIDER_VALUE: f64 = 42.5;

/// One accessible element the mock provider serves.
///
/// The optional interfaces an element implements are *derived* from its data (a
/// bound makes it a `Component`, an action an `Action`, ...) so the interface list
/// the backend reads can never disagree with what is actually served.
#[derive(Default)]
struct Node {
    /// The toolkit's own role spelling, which the backend normalizes.
    role: &'static str,
    /// The element's accessible name.
    name: &'static str,
    /// Extended help text; an empty string is "no description" on the wire.
    description: &'static str,
    /// The AT-SPI state flags that are set.
    states: Vec<State>,
    /// Screen-coordinate extents, when the element has geometry.
    extents: Option<(i32, i32, i32, i32)>,
    /// Child object paths, in the toolkit's reported order.
    children: Vec<&'static str>,
    /// `(name, description, keybinding)` of every action the element exposes.
    actions: Vec<(&'static str, &'static str, &'static str)>,
    /// The element's whole text, when it is a text element.
    text: Option<&'static str>,
    /// The element's `Value` interface value, when it has one.
    value: Option<f64>,
    /// Whether the element is an application root (`org.a11y.atspi.Application`).
    application: bool,
}

impl Node {
    /// A bare element with nothing but its role and name.
    fn new(role: &'static str, name: &'static str) -> Node {
        Node {
            role,
            name,
            ..Node::default()
        }
    }

    /// The AT-SPI interfaces this element implements, as `GetInterfaces` reports them.
    fn interfaces(&self) -> Vec<Interface> {
        let mut interfaces = vec![Interface::Accessible];
        if self.application {
            interfaces.push(Interface::Application);
        }
        if self.extents.is_some() {
            interfaces.push(Interface::Component);
        }
        if !self.actions.is_empty() {
            interfaces.push(Interface::Action);
        }
        if self.text.is_some() {
            interfaces.push(Interface::Text);
        }
        if self.value.is_some() {
            interfaces.push(Interface::Value);
        }
        interfaces
    }

    /// The states as the `GetState` bitfield the toolkit sends.
    fn state_set(&self) -> StateSet {
        self.states.iter().copied().collect()
    }
}

/// Everything the mock provider serves, and what it was asked to do.
struct Provider {
    /// The address `org.a11y.Bus.GetAddress` answers with — the private bus itself.
    address: String,
    /// The unique name of the connection serving the objects: the name half of
    /// every `ObjectRef` the provider hands out.
    bus_name: String,
    /// One entry per served object path.
    nodes: BTreeMap<&'static str, Node>,
    /// `(path, action index)` of every `DoAction` the provider was asked for, in
    /// call order — so a test can assert what the backend *actually did*, not just
    /// what it reported back.
    performed: Mutex<Vec<(String, i32)>>,
}

impl Provider {
    /// The mock provider's whole tree.
    ///
    /// Registry root → application → window frame → a button (`Action`), an entry
    /// (`Text`) and a slider (`Value`).
    fn new(address: &str, bus_name: &str) -> Provider {
        let registry_root = Node {
            role: "application",
            name: "",
            children: vec![APP_ROOT],
            ..Node::default()
        };
        let application = Node {
            name: APP_NAME,
            application: true,
            children: vec![FRAME],
            ..Node::new("application", APP_NAME)
        };
        let frame = Node {
            description: "the mock window",
            states: vec![
                State::Showing,
                State::Visible,
                State::Enabled,
                State::Active,
            ],
            extents: Some(FRAME_EXTENTS),
            children: vec![BUTTON, ENTRY, SLIDER],
            ..Node::new("frame", WINDOW_TITLE)
        };
        let button = Node {
            description: "a button",
            states: vec![State::Enabled, State::Focusable, State::Sensitive],
            extents: Some(BUTTON_EXTENTS),
            actions: vec![("click", "Clicks the button", "")],
            ..Node::new("push button", "Press me")
        };
        let entry = Node {
            states: vec![State::Enabled, State::Editable, State::Sensitive],
            extents: Some(ENTRY_EXTENTS),
            text: Some(ENTRY_TEXT),
            ..Node::new("entry", "Search")
        };
        let slider = Node {
            states: vec![State::Enabled, State::Sensitive],
            extents: Some(SLIDER_EXTENTS),
            value: Some(SLIDER_VALUE),
            ..Node::new("slider", "Zoom")
        };

        Provider {
            address: address.to_owned(),
            bus_name: bus_name.to_owned(),
            nodes: BTreeMap::from([
                (REGISTRY_ROOT, registry_root),
                (APP_ROOT, application),
                (FRAME, frame),
                (BUTTON, button),
                (ENTRY, entry),
                (SLIDER, slider),
            ]),
            performed: Mutex::new(Vec::new()),
        }
    }

    /// The node at `path`.
    ///
    /// The provider only ever answers for the paths it registered, so a lookup
    /// that misses is a bug in this file rather than an input a test can produce.
    fn node(&self, path: &str) -> &Node {
        self.nodes
            .get(path)
            .expect("the mock provider serves only its own paths")
    }

    /// One `(bus name, object path)` reference, as AT-SPI spells them.
    fn object_ref(&self, path: &str) -> ObjectRefOwned {
        let name: UniqueName<'static> = UniqueName::try_from(self.bus_name.clone())
            .expect("the connection's own unique name is a valid unique name");
        let path = ObjectPath::try_from(path)
            .expect("every path the mock serves is a valid object path")
            .into_owned();
        ObjectRef::new_owned(name, path)
    }

    /// The references of the element's children, in order.
    fn child_refs(&self, path: &str) -> Vec<ObjectRefOwned> {
        self.node(path)
            .children
            .iter()
            .map(|child| self.object_ref(child))
            .collect()
    }

    /// Record that the backend asked for an action to be performed.
    fn record_action(&self, path: &str, index: i32) {
        self.performed
            .lock()
            .expect("the mock provider's action log is never poisoned")
            .push((path.to_owned(), index));
    }

    /// Every `DoAction` the provider was asked to perform, in call order.
    fn performed(&self) -> Vec<(String, i32)> {
        self.performed
            .lock()
            .expect("the mock provider's action log is never poisoned")
            .clone()
    }
}

/// The `org.a11y.Bus` interface: the one call that gets a client onto the a11y bus.
struct BusIface {
    /// The address handed to every client.
    address: String,
}

#[zbus::interface(name = "org.a11y.Bus")]
impl BusIface {
    /// `GetAddress`: the mock answers with the private bus it is serving on, which
    /// is exactly what the stack under test then connects to.
    fn get_address(&self) -> String {
        self.address.clone()
    }
}

/// The `org.a11y.atspi.Registry` interface.
///
/// Nothing in the request path calls it — the client only *dials* the name, which
/// is what makes `zbus` call `Properties.GetAll` on this object — but a bus that
/// advertised a registry with no methods at all would be a poor mock of one.
struct RegistryIface;

#[zbus::interface(name = "org.a11y.atspi.Registry")]
impl RegistryIface {
    /// `RegisterEvent`: the runtime never registers for events, so this does nothing.
    fn register_event(&self, _event: &str) {}

    /// `DeregisterEvent`: the counterpart of the above.
    fn deregister_event(&self, _event: &str) {}
}

/// The `org.a11y.atspi.Accessible` interface of one element.
struct AccessibleIface {
    provider: Arc<Provider>,
    path: &'static str,
}

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl AccessibleIface {
    /// `GetChildren`: the element's children, as `(bus name, object path)` pairs.
    fn get_children(&self) -> Vec<ObjectRefOwned> {
        self.provider.child_refs(self.path)
    }

    /// `GetInterfaces`: the optional interfaces this element implements.
    fn get_interfaces(&self) -> InterfaceSet {
        self.provider
            .node(self.path)
            .interfaces()
            .into_iter()
            .collect()
    }

    /// `GetRoleName`: the toolkit's own role spelling.
    fn get_role_name(&self) -> String {
        self.provider.node(self.path).role.to_owned()
    }

    /// `GetState`: the state flags that are set.
    fn get_state(&self) -> StateSet {
        self.provider.node(self.path).state_set()
    }

    /// The `Name` property.
    #[zbus(property, name = "Name")]
    fn accessible_name(&self) -> String {
        self.provider.node(self.path).name.to_owned()
    }

    /// The `Description` property.
    #[zbus(property)]
    fn description(&self) -> String {
        self.provider.node(self.path).description.to_owned()
    }
}

/// The `org.a11y.atspi.Component` interface of one element.
struct ComponentIface {
    /// The element's screen extents.
    extents: (i32, i32, i32, i32),
}

#[zbus::interface(name = "org.a11y.atspi.Component")]
impl ComponentIface {
    /// `GetExtents`: the mock has one rectangle and reports it for every
    /// coordinate type — the backend only ever asks for `Screen`.
    fn get_extents(&self, _coord_type: CoordType) -> (i32, i32, i32, i32) {
        self.extents
    }
}

/// The `org.a11y.atspi.Action` interface of one element.
struct ActionIface {
    provider: Arc<Provider>,
    path: &'static str,
}

#[zbus::interface(name = "org.a11y.atspi.Action")]
impl ActionIface {
    /// `GetActions`: the actions the element exposes.
    fn get_actions(&self) -> Vec<Action> {
        self.provider
            .node(self.path)
            .actions
            .iter()
            .map(|(name, description, keybinding)| Action {
                name: (*name).to_owned(),
                description: (*description).to_owned(),
                keybinding: (*keybinding).to_owned(),
            })
            .collect()
    }

    /// `DoAction`: records the invocation and reports that it was performed.
    fn do_action(&self, index: i32) -> bool {
        self.provider.record_action(self.path, index);
        true
    }
}

/// The `org.a11y.atspi.Text` interface of one element.
struct TextIface {
    /// The element's whole text.
    text: String,
}

#[zbus::interface(name = "org.a11y.atspi.Text")]
impl TextIface {
    /// The `CharacterCount` property — a property, not a method: that is how the
    /// `Text` interface spells it and how the backend's text read asks for it.
    #[zbus(property)]
    fn character_count(&self) -> i32 {
        self.text.chars().count() as i32
    }

    /// `GetText`: the characters in `[start_offset, end_offset)`.
    fn get_text(&self, start_offset: i32, end_offset: i32) -> String {
        let start = start_offset.max(0) as usize;
        let length = (end_offset - start_offset).max(0) as usize;
        self.text.chars().skip(start).take(length).collect()
    }
}

/// The `org.a11y.atspi.Value` interface of one element.
struct ValueIface {
    /// The element's current value.
    value: f64,
}

#[zbus::interface(name = "org.a11y.atspi.Value")]
impl ValueIface {
    /// The `CurrentValue` property.
    #[zbus(property)]
    fn current_value(&self) -> f64 {
        self.value
    }
}

/// The `org.a11y.atspi.Application` interface of the mock application.
struct ApplicationIface;

#[zbus::interface(name = "org.a11y.atspi.Application")]
impl ApplicationIface {
    /// The `ToolkitName` property.
    #[zbus(property)]
    fn toolkit_name(&self) -> String {
        "adesk-mock".to_owned()
    }

    /// The `Version` property.
    #[zbus(property)]
    fn version(&self) -> String {
        "1.0".to_owned()
    }

    /// `GetApplicationBusAddress`: the mock is not a peer-to-peer provider.
    ///
    /// Reporting an error is the honest mock of a toolkit that does not offer a
    /// private socket, and it is what makes the client's peer initialization skip
    /// this application (`atspi`'s `Peers::initialize_peers`) instead of dialling a
    /// socket that does not exist.
    fn get_application_bus_address(&self) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::NotSupported(
            "the mock provider is not a peer-to-peer peer".to_owned(),
        ))
    }
}

// --- the harness -----------------------------------------------------------------

/// The private bus daemon, killed when the harness is dropped.
struct PrivateDaemon {
    child: Child,
    /// The socket the daemon listens on, removed with it.
    socket: PathBuf,
    /// The address the daemon printed (its listen address plus a GUID).
    address: String,
}

impl Drop for PrivateDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// A private session bus with the mock AT-SPI2 provider serving on it.
struct MockBus {
    /// The private bus daemon. Held only so it outlives the provider and is killed
    /// (and its socket removed) when the test is done.
    _daemon: PrivateDaemon,
    /// The provider's own connection: held so the served objects stay up.
    _connection: zbus::Connection,
    provider: Arc<Provider>,
    /// What `DBUS_SESSION_BUS_ADDRESS` held before this test borrowed it.
    previous: Option<std::ffi::OsString>,
}

impl Drop for MockBus {
    fn drop(&mut self) {
        // The file's tests are serialised on `BUS`, so nothing else can be using
        // the variable: put it back exactly as it was found.
        match &self.previous {
            Some(value) => std::env::set_var(SESSION_BUS_ENV, value),
            None => std::env::remove_var(SESSION_BUS_ENV),
        }
    }
}

impl MockBus {
    /// Start a private bus, serve the mock provider on it, and point the
    /// session-bus environment at it.
    async fn start() -> Result<MockBus, String> {
        let previous = std::env::var_os(SESSION_BUS_ENV);
        let daemon = start_private_daemon().await?;

        let connection = zbus::connection::Builder::address(daemon.address.as_str())
            .map_err(|error| format!("parsing the private bus address: {error}"))?
            .build()
            .await
            .map_err(|error| format!("connecting to the private bus: {error}"))?;
        connection
            .request_name("org.a11y.Bus")
            .await
            .map_err(|error| format!("owning org.a11y.Bus: {error}"))?;
        connection
            .request_name("org.a11y.atspi.Registry")
            .await
            .map_err(|error| format!("owning org.a11y.atspi.Registry: {error}"))?;

        let bus_name = connection
            .unique_name()
            .map(|name| name.as_str().to_owned())
            .ok_or_else(|| "the private bus gave the provider no unique name".to_owned())?;
        let provider = Arc::new(Provider::new(&daemon.address, &bus_name));
        serve(&connection, &provider).await?;

        // Only now is the environment usable: everything the backend needs is up.
        std::env::set_var(SESSION_BUS_ENV, &daemon.address);

        Ok(MockBus {
            _daemon: daemon,
            _connection: connection,
            provider,
            previous,
        })
    }
}

/// Serve every object of `provider` on `connection`.
async fn serve(connection: &zbus::Connection, provider: &Arc<Provider>) -> Result<(), String> {
    let server = connection.object_server();
    server
        .at(
            "/org/a11y/bus",
            BusIface {
                address: provider.address.clone(),
            },
        )
        .await
        .map_err(|error| format!("serving org.a11y.Bus: {error}"))?;
    server
        .at("/org/a11y/atspi/registry", RegistryIface)
        .await
        .map_err(|error| format!("serving the registry: {error}"))?;

    for (path, node) in &provider.nodes {
        server
            .at(
                *path,
                AccessibleIface {
                    provider: Arc::clone(provider),
                    path,
                },
            )
            .await
            .map_err(|error| format!("serving {path}: {error}"))?;
        if let Some(extents) = node.extents {
            server
                .at(*path, ComponentIface { extents })
                .await
                .map_err(|error| format!("serving {path}'s geometry: {error}"))?;
        }
        if !node.actions.is_empty() {
            server
                .at(
                    *path,
                    ActionIface {
                        provider: Arc::clone(provider),
                        path,
                    },
                )
                .await
                .map_err(|error| format!("serving {path}'s actions: {error}"))?;
        }
        if let Some(text) = node.text {
            server
                .at(
                    *path,
                    TextIface {
                        text: text.to_owned(),
                    },
                )
                .await
                .map_err(|error| format!("serving {path}'s text: {error}"))?;
        }
        if let Some(value) = node.value {
            server
                .at(*path, ValueIface { value })
                .await
                .map_err(|error| format!("serving {path}'s value: {error}"))?;
        }
        if node.application {
            server
                .at(*path, ApplicationIface)
                .await
                .map_err(|error| format!("serving {path}'s application interface: {error}"))?;
        }
    }

    Ok(())
}

/// Start a private session bus, or say why one could not be started.
async fn start_private_daemon() -> Result<PrivateDaemon, String> {
    let socket = unique_socket_path();
    let listen = format!("unix:path={}", socket.display());

    let spawned = tokio::time::timeout(
        DAEMON_START_BOUND,
        tokio::task::spawn_blocking(move || spawn_daemon(socket, &listen)),
    )
    .await;

    match spawned {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(format!("starting dbus-daemon: {error}")),
        Err(_elapsed) => Err(format!(
            "dbus-daemon did not print an address within {}s",
            DAEMON_START_BOUND.as_secs()
        )),
    }
}

/// Spawn `dbus-daemon` on `socket` and read back the address it printed.
///
/// The daemon runs in the foreground (`--nofork`) so the test owns its lifetime,
/// and prints its address on stdout, which is where `dbus-launch` reads it from too.
fn spawn_daemon(socket: PathBuf, listen: &str) -> Result<PrivateDaemon, String> {
    let _ = std::fs::remove_file(&socket);
    let mut last_error = "no dbus-daemon candidate was tried".to_owned();

    for candidate in daemon_candidates() {
        let mut child = match Command::new(&candidate)
            .args([
                "--session",
                "--address",
                listen,
                "--print-address=1",
                "--nofork",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                last_error = format!("{}: {error}", candidate.display());
                continue;
            }
        };

        let stdout = child.stdout.take().expect("the daemon's stdout is piped");
        let mut address = String::new();
        match std::io::BufReader::new(stdout).read_line(&mut address) {
            Ok(_) if !address.trim().is_empty() => {
                return Ok(PrivateDaemon {
                    child,
                    socket,
                    address: address.trim().to_owned(),
                });
            }
            Ok(_) => {
                last_error = format!("{} printed no address", candidate.display());
            }
            Err(error) => {
                last_error = format!("{}: reading its address: {error}", candidate.display());
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    Err(last_error)
}

/// Where `dbus-daemon` may be found: on `PATH` first, then the Nix profile.
fn daemon_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("dbus-daemon"),
        PathBuf::from("/run/current-system/sw/bin/dbus-daemon"),
    ]
}

/// A socket path that cannot collide with an earlier run's leftovers.
fn unique_socket_path() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "adesk-a11y-mock-{}-{nanos}-{count}.sock",
        std::process::id()
    ))
}

/// A mock bus, or `None` — printing the reason — when this environment has no
/// `dbus-daemon` to start one with.
async fn mock_bus_or_skip() -> Option<MockBus> {
    match MockBus::start().await {
        Ok(bus) => Some(bus),
        Err(reason) => {
            println!("cannot start a private bus, skipping the mock AT-SPI2 test: {reason}");
            None
        }
    }
}

/// The window the mock frame is: matched by pid (the strongest rung), with the
/// title and the app id filled in as the runtime fills them from `QueryState`.
fn window_target() -> WindowTarget {
    WindowTarget {
        title: Some(WINDOW_TITLE.to_owned()),
        app_id: Some(AppId::from(APP_NAME)),
        pid: Some(std::process::id() as i32),
        ..WindowTarget::new(WindowId(7))
    }
}

/// The frame's handle, as the backend must have spelled it.
fn handle_of(provider: &Provider, path: &str) -> ElementHandle {
    ElementHandle::from(format!("{}|{path}", provider.bus_name))
}

/// `DBUS_SESSION_BUS_ADDRESS` is process-global: the tests in this file take turns.
static BUS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_backend_walks_and_acts_on_a_mock_atspi_provider() {
    let _turn = BUS.lock().await;
    let Some(bus) = mock_bus_or_skip().await else {
        return;
    };

    let driven = tokio::time::timeout(DRIVE_BOUND, drive_the_backend(&bus)).await;
    assert!(
        driven.is_ok(),
        "driving the backend against the mock provider did not finish within {}s",
        DRIVE_BOUND.as_secs()
    );
}

/// The whole request path, against the mock provider `bus` serves.
async fn drive_the_backend(bus: &MockBus) {
    let source = AtspiSource::connect()
        .await
        .expect("the mock provider serves an accessibility bus");
    assert!(source.is_available().await);
    assert_eq!(source.name(), "atspi");
    assert!(
        source.bus_name().is_some_and(|name| name.starts_with(':')),
        "a connected bus client has a unique name"
    );

    let snapshot = source
        .snapshot(&window_target(), SourceOptions::new(12, 2_000))
        .await
        .expect("the mock window correlates and walks");

    a_correlated_window_is_walked_whole(bus, &source, &snapshot).await;
    every_correlation_rung_finds_the_window(&source).await;
    the_bounds_are_window_relative_and_bounded(&source).await;
    every_invoke_outcome_is_reported(bus, &source, &snapshot).await;
    the_auto_backend_reaches_the_same_tree(&source, &snapshot).await;
}

/// The walked tree: the application, the frame, and the three children with their
/// roles, names, descriptions, states, bounds, actions and values.
async fn a_correlated_window_is_walked_whole(
    bus: &MockBus,
    source: &AtspiSource,
    snapshot: &SourceSnapshot,
) {
    assert_eq!(source.name(), "atspi");
    assert!(
        source.is_available().await,
        "the mock provider is reachable while the tree is walked"
    );
    assert_eq!(
        snapshot.app_name.as_deref(),
        Some(APP_NAME),
        "the application's accessible name is correlated with the window"
    );
    assert!(
        !snapshot.truncated,
        "a bounded walk of this tree is complete"
    );

    let root = &snapshot.root;
    assert_eq!(root.role, "frame");
    assert_eq!(root.name, WINDOW_TITLE);
    assert_eq!(root.description.as_deref(), Some("the mock window"));
    assert_eq!(
        root.states,
        vec![
            AccessibleState::Active,
            AccessibleState::Enabled,
            AccessibleState::Showing,
            AccessibleState::Visible,
        ],
        "the AT-SPI flags that have an AGP counterpart, sorted by wire name"
    );
    assert_eq!(
        root.bounds,
        Some(Rect {
            x: 0,
            y: 0,
            w: 800,
            h: 600,
        }),
        "the frame is the origin every other bound is relative to"
    );
    assert_eq!(
        root.handle,
        handle_of(&bus.provider, FRAME),
        "a handle is the bus name and the object path it addresses"
    );
    assert!(root.actions.is_empty(), "the frame exposes no actions");

    assert_eq!(root.children.len(), 3, "in the toolkit's reported order");

    let button = &root.children[0];
    assert_eq!(button.role, "push_button", "a toolkit role is normalized");
    assert_eq!(button.name, "Press me");
    assert_eq!(button.description.as_deref(), Some("a button"));
    assert_eq!(button.actions, vec!["click".to_owned()]);
    assert_eq!(
        button.bounds,
        Some(Rect {
            x: 20,
            y: 30,
            w: 60,
            h: 25,
        })
    );
    assert_eq!(
        button.states,
        vec![
            AccessibleState::Enabled,
            AccessibleState::Focusable,
            AccessibleState::Sensitive,
        ]
    );
    assert_eq!(button.children.len(), 0);
    assert_eq!(button.handle, handle_of(&bus.provider, BUTTON));

    let entry = &root.children[1];
    assert_eq!(entry.role, "entry");
    assert_eq!(entry.name, "Search");
    assert_eq!(
        entry.description, None,
        "an empty accessible field is no field at all"
    );
    assert_eq!(
        entry.value.as_deref(),
        Some(ENTRY_TEXT),
        "a text element's value is its whole text"
    );
    assert_eq!(
        entry.states,
        vec![
            AccessibleState::Editable,
            AccessibleState::Enabled,
            AccessibleState::Sensitive,
        ]
    );

    let slider = &root.children[2];
    assert_eq!(slider.role, "slider");
    assert_eq!(slider.name, "Zoom");
    assert_eq!(
        slider.value.as_deref(),
        Some("42.5"),
        "an element without text reports its Value interface"
    );
}

/// The runtime's own backend selection reaches the same tree: `auto_source()` is
/// the `LazyAtspiSource` the server installs when accessibility is on, and with a
/// bus present it must connect on first use and walk the window.
async fn the_auto_backend_reaches_the_same_tree(source: &AtspiSource, snapshot: &SourceSnapshot) {
    let auto = adesk_a11y::auto_source();
    assert_eq!(auto.name(), "atspi");
    assert!(
        auto.is_available().await,
        "the mock bus is present, so the auto backend is usable"
    );

    let walked = auto
        .snapshot(&window_target(), SourceOptions::new(1, 10))
        .await
        .expect("the auto backend walks the mock window");
    assert_eq!(walked.root.handle, snapshot.root.handle);
    assert_eq!(walked.app_name, snapshot.app_name);
    assert_eq!(walked.root.name, WINDOW_TITLE);

    // The connected source is not special-cased: the auto backend's sub-tree is
    // the same one, reached through its own lazily established connection.
    assert!(source.is_available().await);
}

/// The correlation ladder: pid, then title, then application identity, and a miss
/// for a window that carries no key at all.
async fn every_correlation_rung_finds_the_window(source: &AtspiSource) {
    // The title rung: no pid and no app id, so the frame's name has to match.
    let by_title = WindowTarget {
        title: Some(WINDOW_TITLE.to_owned()),
        ..WindowTarget::new(WindowId(8))
    };
    let snapshot = source
        .snapshot(&by_title, SourceOptions::new(12, 2_000))
        .await
        .expect("the window's title correlates with its accessible frame");
    assert_eq!(snapshot.root.name, WINDOW_TITLE);

    // The application-identity rung: only the app id is known, so the window is the
    // application's first top-level frame.
    let by_app = WindowTarget {
        app_id: Some(AppId::from(APP_NAME)),
        ..WindowTarget::new(WindowId(9))
    };
    let snapshot = source
        .snapshot(&by_app, SourceOptions::new(12, 2_000))
        .await
        .expect("the application's name correlates with its first frame");
    assert_eq!(snapshot.app_name.as_deref(), Some(APP_NAME));
    assert_eq!(snapshot.root.role, "frame");

    // A window the runtime can describe in no way correlates with nothing: the
    // backend never falls back to "the only application on the bus".
    let error = source
        .snapshot(&WindowTarget::new(WindowId(10)), SourceOptions::new(1, 1))
        .await
        .expect_err("a target with no correlation key matches no frame");
    assert!(
        matches!(error, adesk_a11y::A11yError::NotCorrelated(WindowId(10))),
        "the registry was reached, so this is a correlation miss: {error}"
    );
}

/// The walk's bounds: a depth bound truncates instead of failing, and the node
/// budget does the same.
async fn the_bounds_are_window_relative_and_bounded(source: &AtspiSource) {
    let shallow = source
        .snapshot(&window_target(), SourceOptions::new(0, 2_000))
        .await
        .expect("a depth bound is not an error");
    assert!(
        shallow.truncated,
        "the frame has children the depth bound did not walk"
    );
    assert_eq!(shallow.root.role, "frame");
    assert!(shallow.root.children.is_empty());

    let budgeted = source
        .snapshot(&window_target(), SourceOptions::new(12, 2))
        .await
        .expect("a node budget is not an error");
    assert!(budgeted.truncated, "two nodes cannot hold this tree");
    assert_eq!(budgeted.root.children.len(), 1, "the root plus one child");
}

/// Every `InvokeOutcome` the backend can report, against the mock button.
async fn every_invoke_outcome_is_reported(
    bus: &MockBus,
    source: &AtspiSource,
    snapshot: &SourceSnapshot,
) {
    let button = snapshot.root.children[0].handle.clone();

    let outcome = source
        .invoke(&button, Some("click"))
        .await
        .expect("invoking a known action is not an error");
    assert_eq!(outcome, InvokeOutcome::Invoked("click".to_owned()));
    assert_eq!(
        bus.provider.performed(),
        vec![(BUTTON.to_owned(), 0)],
        "the toolkit really was asked to perform the action"
    );

    // No action named: the toolkit's first action is the element's default.
    let outcome = source
        .invoke(&button, None)
        .await
        .expect("the default action is invocable");
    assert_eq!(outcome, InvokeOutcome::Invoked("click".to_owned()));
    assert_eq!(bus.provider.performed().len(), 2);

    // An action the element does not expose is reported, never invented.
    let outcome = source
        .invoke(&button, Some("toggle"))
        .await
        .expect("an unknown action is an outcome, not an error");
    assert_eq!(outcome, InvokeOutcome::NoSuchAction);
    assert_eq!(
        bus.provider.performed().len(),
        2,
        "an unknown action never reaches the toolkit"
    );

    // An element without an `Action` interface exposes no action at all.
    let outcome = source
        .invoke(&handle_of(&bus.provider, ENTRY), None)
        .await
        .expect("an element without actions is an outcome, not an error");
    assert_eq!(outcome, InvokeOutcome::NoSuchAction);

    // A handle that is not one of ours never reaches the bus.
    let outcome = source
        .invoke(&ElementHandle::from("not-an-address"), None)
        .await
        .expect("a foreign handle is an outcome, not an error");
    assert_eq!(outcome, InvokeOutcome::Gone);

    // A well-formed handle on an element that is no longer there is gone.
    let vanished = ElementHandle::from(format!(":9.99|{FRAME}"));
    let outcome = source
        .invoke(&vanished, Some("click"))
        .await
        .expect("a vanished element is an outcome, not an error");
    assert_eq!(outcome, InvokeOutcome::Gone);
}
