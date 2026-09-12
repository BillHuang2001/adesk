# ADesk — notifications and reactive agent wakeups

This document is the design record for ADesk's notification subsystem and its
agent-facing event inbox.
`docs/protocol.md` §5.6/§5.9/§5.10 is the normative wire contract; this file explains
the model and the decisions behind it.

## Goal

An ADesk agent should be able to stay entirely idle — issuing no polls, rendering no
pixels, burning no tokens — until something worth reacting to happens, and then be
woken with the structured content of that event.
Because ADesk owns the whole desktop, it can own the notification path and expose it
as a standard, programmable event source for the agent, instead of making the agent
scrape the screen.

## Two capabilities

1. **Notifications** — a structured, programmable inbox (§5.9).
   Any client may `post_notification`; the runtime assigns a `notification_id`,
   stores the notification, and publishes a `notification` event.
   `list_notifications` / `close_notification` / `invoke_notification_action` round
   out the lifecycle.
2. **Event waits** — `wait_for_events` (§5.10), one blocking request that answers as
   soon as any matching event (a notification, a window lifecycle change, a title
   change, ...) is published, or a timeout elapses.

Together they are the "stay idle / wake on event" loop: the agent calls
`wait_for_events` with the kinds it cares about, sleeps, and is handed the events.

## Why notifications ride the single event stream

Notifications are `RuntimeEvent`s, not a side channel: a §5.9 handler reserves a
`seq` from the compositor's single counter (`RuntimeCommand::ReserveSeq`, the same
source `AppLaunched` uses) and publishes the event on the same broadcast the
compositor uses (`docs/architecture.md` §2). That means:

- one global monotonic `seq` domain covers notifications too, so a `notification`
  frame and a `surface_commit` frame are causally orderable;
- `subscribe_events` and `wait_for_events` see notifications for free;
- the event pump, the subscription fan-out and the event inbox need no special case
  for them.

The notification *store* (id allocation, dismissal state) is a separate, synchronous
concern owned by `adesk-notify` and mutated only by the §5.9 request handlers — never
by the event pump, so a store mutation and the event it publishes cannot disagree.

## The "user message" case

A message to the agent is just a notification: `post_notification` with
`source: "user"` (or any name), `category: "message"` and the text in `body`.
The agent wakes on the `notification` kind and turns the body into a task.
Nothing special is needed — a chat message and a desktop notification are the same
object with different metadata.

## Sources

The programmable source is `post_notification`. Any AGP client — a tool, a test, the
viewer, or the agent itself — is a notification source, and the method is the
standard interface it uses.
A future standard-app path (a D-Bus `org.freedesktop.Notifications` service, so
ordinary Linux applications inside the desktop can notify through the interface they
already use) plugs in behind the same `adesk-notify` store without changing AGP;
that bridge is out of v1 scope.

## Components

- `adesk-core` — the notification domain vocabulary (`NotificationId`,
  `Notification`, `NotificationUrgency`, `NotificationAction`,
  `NotificationCloseReason`), the `RuntimeEvent` notification variants and the new
  `EventKind`s and `ErrorCode::UnknownNotification`.
- `adesk-notify` — the runtime's notification store and event inbox:
  `NotificationService` (post/list/close/invoke over an `Arc<Inner>`, cheap clone)
  and the `wait_for_events` waiter.
- `adesk-proto` — the §5.9/§5.10 methods and the three notification event payloads.
- `adesk-server` — owns one `NotificationService` in `ServerContext`, serves the
  methods, reserves `seq` and publishes the events, and feeds the inbox from the
  event pump.
- `adesk-client` — typed SDK methods and `AgpEvent` notification variants.
- `adesk-agent` — the idle/`watch` loop: `wait_for_events` awakens a task.

## Non-goals (v1)

- D-Bus / `org.freedesktop.Notifications` bridging.
- Auto-expiry of notifications (`timeout_ms` is stored and echoed, not enforced).
- Rendering notifications into the viewer's frame stream (the viewer stays a desktop
  mirror; a notification UI is future work).
- Delivery guarantees beyond a bounded inbox (a slow or absent consumer never blocks
  a producer).
