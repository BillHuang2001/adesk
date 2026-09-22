# ADesk — accessibility: the text view of the desktop

This document is the design record for ADesk's accessibility subsystem, the agent's
text-first view of a window's UI.
`docs/protocol.md` §5.11 is the normative wire contract; this file explains the model
and the decisions behind it.

## Goal

An ADesk agent should be able to read what a window *contains* — its labels, buttons,
text fields, lists and menus — without looking at a single pixel.
A form, a dialog or a document is better described as text than as an image, and text
costs orders of magnitude fewer tokens than the frame it was rendered into.

Because ADesk owns the whole desktop, it can own the accessibility path too instead of
making the agent reconstruct a UI from screenshots: the runtime exposes a window's
toolkit accessibility tree (AT-SPI2 over D-Bus) as a first-class, text-only
observation alongside the pixel one.

The principle is **see the desktop with pure text**: for any window whose application
publishes an accessibility tree, an agent enumerates its elements, reads their roles,
names, values and states, and acts on them by id — with the pixel observation (§5.4)
remaining available as the fallback and the cross-check, not the default.

## Model

An accessible element ("node") is described by:

- `role` — what the toolkit says the element is (`push_button`, `text`, `list_item`,
  `check_box`, ...).
- `name` — the human-readable label an assistive tool would announce.
- `value` — the element's text/value content, for value-bearing roles (`text`,
  `entry`, `slider`, `progress_bar`, ...); absent when the element has none.
- `states` — the element's boolean state flags (`enabled`, `focused`, `checked`,
  `expanded`, ...).
- `bounds` — where the element is, in window-relative pixels.
- `actions` — the actuation verbs the element exposes (`click`, `press`, `set_value`, ...).

Together they form a tree, rooted at the window's **own** accessible frame rather than
at the whole application: the agent asks about a window it can also list, activate and
capture through §5.3/§5.4, and the menus, popups and transient dialogs the toolkit
groups under that frame come with it. A `window_id` therefore names exactly one
accessibility tree.

Every node in a response carries a stable `AccessibleId`. Ids are assigned by the
runtime — the agent never sees an AT-SPI path or a D-Bus object path — and they are
what `invoke_accessible_action` addresses.

`bounds` is window-relative pixels, measured from the window's accessible frame, so an
element's rectangle is directly comparable with the coordinates §5.5 input uses and
with the rects §5.4 reports; an agent that wants to cross-check text against pixels
does not have to convert between two coordinate spaces.

`role` is the toolkit's own role name normalized to lowercase snake_case
(`push_button`, `page_tab_list`, `table_cell`), a free string rather than a closed
enum of the runtime's own invention. The accessibility vocabulary is large, sparsely
implemented and grows with every toolkit release; a closed enum would silently drop
every role the runtime did not know, and an unclassifiable role is precisely the case
where the element is still useful — the agent can read its name, value, states and
actions regardless. Keeping the name is lossless and forward compatible (a runtime
built before a toolkit gained a role still reports the node), and normalizing the
spelling on the runtime side keeps a filter such as `find_accessible role` stable
across toolkits that spell the same role differently.

## Backend seam

The subsystem talks to one seam, `AccessibilitySource`: connect, list the applications
and frames on the accessibility bus, walk a frame's subtree, resolve an element handle
back to a node, and invoke an element action. Two implementations ship behind it.

- The **AT-SPI backend** is the `atspi` crate over `zbus` — pure Rust D-Bus, no
  `libdbus`, no C dependency — talking to the session's accessibility bus
  (`org.a11y.Bus`).
- A **fixture backend** is a deterministic in-memory tree, used by `adesk-testkit` and
  by tools, so the whole §5.11 surface is exercisable with no desktop, no toolkit and
  no D-Bus session.

The connection is **lazy**: it is established the first time a §5.11 method actually
needs the bus. A runtime whose agent never reads accessibility never pays for a
connection, and a runtime started before the desktop's applications are ready connects
on first use instead of failing at startup. When there is no accessibility bus to
connect to, the seam reports the absence as a structured `not_supported` — a normal
answer, never a degraded or partially filled tree.

## Correlation

The agent addresses windows by `window_id`, while AT-SPI addresses applications and
frames, so the server has to relate the two. It does so from the window-model snapshot
it already holds (§3 `QueryState`) — never from state the compositor thread owns:

1. the window's title against the accessible *frame* names the bus exposes;
2. failing that, the window's application (`app_id` / app name / title) against the
   applications the bus exposes, and then that application's accessible frame.

Correlation failure is **reported, never guessed**: when no frame can be matched to
the window, the §5.11 method answers `not_supported` with a message naming the window.
The runtime does not fall back to "the only application on the bus", to the first
frame, or to an empty tree, because a wrong tree is worse than a missing one — the
agent could read, and act on, another application's widgets. A window whose
application publishes no accessibility tree at all (no toolkit bridge, or the bridge
disabled) is exactly the case the pixel observation covers.

## Text rendering

`accessibility_tree` returns both the structured `AccessibleTree` and a rendered
outline (`text`), because they answer different questions. The tree is what an agent
consumes when it must *address* an element — a node id, an action name, a state — and
the text is what it reads, or feeds to a model, when it must *understand* a window.
Returning only one would force the agent to either re-render the outline itself or
re-walk the tree for a name it had just read.

The outline is one line per node, `\n`-separated, two spaces of indent per level, with
the root at depth 0. A node renders as `{role} "{name}"` and then, in a fixed order,
` value="{value}"` when the node has a value, ` states=[{s1},{s2}]` and
` actions=[{a1},{a2}]` when those lists are non-empty, ` bounds={x},{y},{w},{h}` when
the geometry is known, and ` id={id}`. Children follow their parent, one per line:

```text
frame "Open File" bounds=0,0,640,480 id=1
  dialog "Open File" bounds=10,10,620,460 id=2
    text "File name:" bounds=0,0,80,24 id=3
    entry "" value="report.txt" states=[focused] actions=[set_value] bounds=88,0,400,24 id=4
    push_button "Open" actions=[click] id=5
    push_button "Cancel" states=[enabled,sensitive] id=6
```

Inside names and values, `\`, `"`, newline and tab are escaped as `\\`, `\"`, `\n` and
`\t`, so a line is always exactly one node and a quoted field is always unambiguous.
That is what lets the outline be read line by line without a parser that knows
anything about accessibility, and it is why the grammar is fixed rather than
free-form: the agent can grep it, diff it between two observations, or hand it to a
model verbatim.

The §5.11 projection flags (`include_text`, `include_bounds`, `include_states`,
`include_actions`) plus the `max_depth`/`max_nodes` bounds are a deliberate
detail-versus-tokens control: a cheap overview of a large dialog costs one request
with the optional sections off, and drilling into one subtree costs another.

## Action invocation

`invoke_accessible_action` is runtime-native actuation, not synthesized input: it
invokes the element's accessibility `Action` through the toolkit, which performs the
action on the widget itself. This is the same principle as `activate_window` (§5.3),
which moves focus directly instead of sending `Alt+Tab`.

Actuating through the toolkit is what makes the method usable when an element has no
visible geometry, sits off-screen inside a scrolled view, or would need a pointer click
at guessed coordinates — and because it is not a click path, the element's own
activation semantics (a `check_box` toggling, a `page_tab` being selected, a
`push_button` being pressed) come from the toolkit rather than from the agent's
inference. The response carries an ordinary AGP `action_id`, so an agent can reference
the invocation with `after_action` (§5.4) exactly like any other action.

Backend availability is checked **before** the node id is resolved, so with no
accessibility backend *every* `invoke_accessible_action` answers `not_supported`,
whatever `node_id` it carries — even an id the runtime never handed out. A runtime with
no backend knows nothing about any element, so `unknown_accessible` is reserved for an
available backend that does not recognise the id, and `invalid_request` for an action
name the element does not expose.

## Handle stability

An `AccessibleId` is assigned per backend element handle and retained by the runtime's
id registry, so the same element keeps the same id across requests — including across
an `accessibility_tree` that surfaced it and a `find_accessible` that matched it — for
as long as it lives. Ids are monotonic, never reused, and runtime-scoped like the
action registry and the notification store, so an id is not tied to the connection
that first saw it.

A handle that no longer resolves — the element was destroyed, its application exited,
or the accessed frame changed — answers `unknown_accessible`, never a stale node and
never a different element. An indistinguishable "the element is gone" answer is what
makes ids safe to cache between observations.

## Components

- `adesk-core` — the accessibility value vocabulary (`AccessibleId`,
  `AccessibleState`, `AccessibleNode`, `AccessibleTree`, `AccessibleMatch`) and
  `ErrorCode::UnknownAccessible`.
- `adesk-a11y` — the service: the `AccessibilitySource` seam, the AT-SPI and fixture
  backends, the element-handle → `AccessibleId` registry, window → accessible
  correlation and the text renderer.
- `adesk-proto` — the §5.11 methods and their wire payloads.
- `adesk-server` — owns one `AccessibilityService` in `ServerContext.accessibility`,
  serves the methods, correlates windows from `QueryState`, and selects the backend
  through its `--accessibility` option.
- `adesk-client` — typed SDK methods for the three methods.
- `adesk-testkit` — injects a deterministic fixture source, so an AGP test asserts a
  known tree over a real socket with no accessibility bus present.

## Non-goals (v1)

- **Accessibility events.** There is no accessibility `RuntimeEvent` and no
  accessibility `EventKind`: the text view is read strictly on demand (§5.11), so a
  change inside a window is something an agent re-reads, not something it is pushed.
  The event pump and the observer are untouched.
- **Being an accessibility provider.** ADesk exposes accessibility *to the agent*; it
  does not implement AT-SPI for other assistive tools and exports no
  `org.a11y.atspi` service of its own.
- **ATK / AT-SPI1 bridges.** Only the modern AT-SPI2 D-Bus stack is spoken.
- **AT-SPI "device" interfaces.** The subsystem reads and actuates elements; it does
  not claim accessibility device ownership and emits no device events.
- **Viewer and GTK front-end support.** Neither VAP nor `adesk-viewer-gui` carries
  accessibility: the viewer mirrors pixels and injects human input, and the agent's
  text view is an AGP concern.
