//! Lifecycle integration tests: [`MachineManager`] driving the in-memory
//! [`MockRuntime`] through the full machine state machine.
//!
//! These tests exercise the manager end to end — create/start/stop/restart/
//! remove, name uniqueness, unknown names, illegal transitions and the registry
//! rebuild from backend truth — and assert the exact backend calls the manager
//! issues.

use adesk_machine::runtime::mock::RuntimeCall;
use adesk_machine::{
    ContainerRuntime, MachineError, MachineId, MachineManager, MachineName, MachineSpec,
    MachineState, MockRuntime,
};

/// A machine spec whose only significant field for these tests is the name.
fn spec(name: &str) -> MachineSpec {
    MachineSpec::new(name, "ghcr.io/adesk/machine:latest")
}

/// Shorthand for the manager's name key.
fn name(value: &str) -> MachineName {
    MachineName::from(value)
}

/// A manager over a fresh, deterministic in-memory backend.
fn manager() -> MachineManager<MockRuntime> {
    MachineManager::new(MockRuntime::new())
}

#[tokio::test]
async fn full_lifecycle_transitions_and_removal() {
    let manager = manager();

    let created = manager.create(&spec("adesk")).await.unwrap();
    assert_eq!(created.name, name("adesk"));
    assert_eq!(created.id, MachineId::from("machine-1"));
    assert_eq!(created.state, MachineState::Created);
    assert!(!created.is_running());

    let running = manager.start(&name("adesk")).await.unwrap();
    assert_eq!(running.state, MachineState::Running);
    assert!(running.is_running());

    // `restart` stops then starts, so it runs while the machine is `Running`.
    let restarted = manager.restart(&name("adesk")).await.unwrap();
    assert_eq!(restarted.state, MachineState::Running);

    let stopped = manager.stop(&name("adesk"), 0).await.unwrap();
    assert_eq!(stopped.state, MachineState::Stopped);

    // `remove` reports the status observed immediately before removal; a stopped
    // machine needs no force.
    let removed = manager.remove(&name("adesk"), false).await.unwrap();
    assert_eq!(removed.state, MachineState::Stopped);

    assert!(manager.list().await.unwrap().is_empty());
    assert!(matches!(
        manager.status(&name("adesk")).await.unwrap_err(),
        MachineError::NotFound { .. }
    ));
}

#[tokio::test]
async fn start_is_allowed_from_created_and_stopped() {
    let manager = manager();
    manager.create(&spec("adesk")).await.unwrap();

    // From `Created`.
    assert_eq!(
        manager.start(&name("adesk")).await.unwrap().state,
        MachineState::Running
    );

    // From `Stopped`.
    manager.stop(&name("adesk"), 0).await.unwrap();
    assert_eq!(
        manager.start(&name("adesk")).await.unwrap().state,
        MachineState::Running
    );
}

#[tokio::test]
async fn duplicate_name_is_rejected() {
    let manager = manager();
    manager.create(&spec("adesk")).await.unwrap();

    let err = manager.create(&spec("adesk")).await.unwrap_err();
    assert!(
        matches!(err, MachineError::Duplicate { ref name } if name == "adesk"),
        "unexpected error: {err:?}"
    );
    // The original machine is left untouched and no second one appeared.
    assert_eq!(manager.list().await.unwrap().len(), 1);
    assert_eq!(
        manager.status(&name("adesk")).await.unwrap().state,
        MachineState::Created
    );
}

#[tokio::test]
async fn unknown_names_are_not_found() {
    let manager = manager();
    let ghost = name("ghost");

    for err in [
        manager.start(&ghost).await.unwrap_err(),
        manager.stop(&ghost, 0).await.unwrap_err(),
        manager.status(&ghost).await.unwrap_err(),
        manager.remove(&ghost, true).await.unwrap_err(),
    ] {
        assert!(
            matches!(err, MachineError::NotFound { ref name } if name == "ghost"),
            "unexpected error: {err:?}"
        );
    }
}

#[tokio::test]
async fn illegal_transitions_are_reported() {
    let manager = manager();
    manager.create(&spec("adesk")).await.unwrap();

    // Stopping a machine that was never started is illegal.
    let err = manager.stop(&name("adesk"), 0).await.unwrap_err();
    assert!(
        matches!(err, MachineError::InvalidState { ref state, ref action, .. }
            if state == "created" && action == "stop"),
        "unexpected error: {err:?}"
    );

    manager.start(&name("adesk")).await.unwrap();

    // Starting an already-running machine is illegal.
    let err = manager.start(&name("adesk")).await.unwrap_err();
    assert!(
        matches!(err, MachineError::InvalidState { ref state, .. } if state == "running"),
        "unexpected error: {err:?}"
    );

    // Removing a running machine without force is illegal...
    let err = manager.remove(&name("adesk"), false).await.unwrap_err();
    assert!(
        matches!(err, MachineError::InvalidState { ref action, .. } if action == "remove"),
        "unexpected error: {err:?}"
    );

    // ...but the rejected removal left the machine intact, so a forced one works.
    manager.remove(&name("adesk"), true).await.unwrap();
}

#[tokio::test]
async fn list_rebuilds_the_registry_from_the_backend() {
    let manager = manager();
    manager.create(&spec("alpha")).await.unwrap();
    manager.create(&spec("bravo")).await.unwrap();

    // Mutate the backend behind the manager's back: drop `bravo` and add
    // `charlie`, leaving the cached registry stale.
    let backend = manager.runtime();
    let bravo = backend
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|status| status.name == name("bravo"))
        .expect("bravo is registered");
    backend.remove(&bravo.id, true).await.unwrap();
    backend.create(&spec("charlie")).await.unwrap();

    let mut listed: Vec<MachineName> = manager
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|status| status.name)
        .collect();
    listed.sort();
    assert_eq!(listed, vec![name("alpha"), name("charlie")]);

    // The rebuilt cache resolves the machine the manager had never created...
    assert_eq!(
        manager.status(&name("charlie")).await.unwrap().name,
        name("charlie")
    );
    // ...and has forgotten the one the backend no longer reports.
    assert!(matches!(
        manager.status(&name("bravo")).await.unwrap_err(),
        MachineError::NotFound { .. }
    ));
}

#[tokio::test]
async fn manager_issues_backend_calls_in_order() {
    let manager = manager();
    manager.create(&spec("adesk")).await.unwrap();
    manager.start(&name("adesk")).await.unwrap();
    manager.stop(&name("adesk"), 1_500).await.unwrap();

    // Each manager transition is one backend action followed by a status refresh.
    let id = MachineId::from("machine-1");
    assert_eq!(
        manager.runtime().calls(),
        vec![
            RuntimeCall::Create {
                name: name("adesk"),
            },
            RuntimeCall::Status { id: id.clone() },
            RuntimeCall::Start { id: id.clone() },
            RuntimeCall::Status { id: id.clone() },
            RuntimeCall::Stop {
                id: id.clone(),
                timeout_ms: 1_500,
            },
            RuntimeCall::Status { id },
        ]
    );
}

#[tokio::test]
async fn list_reports_every_machine_the_backend_knows() {
    let manager = manager();
    manager.create(&spec("alpha")).await.unwrap();
    manager.create(&spec("bravo")).await.unwrap();

    let names: Vec<MachineName> = manager
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|status| status.name)
        .collect();
    assert_eq!(names, vec![name("alpha"), name("bravo")]);
}
