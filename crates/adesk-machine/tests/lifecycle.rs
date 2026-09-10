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

/// One step of a scripted lifecycle, run against a fresh manager.
#[derive(Clone)]
enum Step {
    /// `create` succeeds and yields `Created`.
    Create(&'static str),
    /// `start` succeeds and yields `Running`.
    Start(&'static str),
    /// `restart` succeeds and yields `Running`.
    Restart(&'static str),
    /// `stop` succeeds and yields `Stopped`.
    Stop(&'static str),
    /// `remove` succeeds and reports the state observed just before removal.
    Remove(&'static str, bool, MachineState),
    /// `start` is illegal in `state`.
    StartIsIllegal(&'static str, &'static str),
    /// `stop` is illegal in `state`.
    StopIsIllegal(&'static str, &'static str),
    /// `remove` without `force` is illegal while the machine runs.
    RemoveRunningIsIllegal(&'static str),
    /// `list` reports no machines.
    ListIsEmpty,
    /// `status` fails with `NotFound`.
    StatusIsNotFound(&'static str),
}

/// Runs `steps` against a fresh manager, asserting each outcome.
async fn run_script(label: &str, steps: &[Step]) {
    let manager = manager();
    let mut created = 0u32;

    for step in steps {
        match step {
            Step::Create(machine) => {
                created += 1;
                let status = manager.create(&spec(machine)).await.unwrap();
                assert_eq!(status.name, name(machine), "{label}: create name");
                assert_eq!(
                    status.id,
                    MachineId::from(format!("machine-{created}")),
                    "{label}: create id"
                );
                assert_eq!(status.state, MachineState::Created, "{label}: create state");
                assert!(
                    !status.is_running(),
                    "{label}: a created machine is not running"
                );
            }
            Step::Start(machine) => {
                let status = manager.start(&name(machine)).await.unwrap();
                assert_eq!(status.state, MachineState::Running, "{label}: start state");
                assert!(status.is_running(), "{label}: start is running");
            }
            Step::Restart(machine) => {
                assert_eq!(
                    manager.restart(&name(machine)).await.unwrap().state,
                    MachineState::Running,
                    "{label}: restart state"
                );
            }
            Step::Stop(machine) => {
                assert_eq!(
                    manager.stop(&name(machine), 0).await.unwrap().state,
                    MachineState::Stopped,
                    "{label}: stop state"
                );
            }
            Step::Remove(machine, force, expected) => {
                let status = manager.remove(&name(machine), *force).await.unwrap();
                assert_eq!(
                    &status.state, expected,
                    "{label}: remove reports the pre-removal state"
                );
            }
            Step::StartIsIllegal(machine, state) => {
                let err = manager.start(&name(machine)).await.unwrap_err();
                match &err {
                    MachineError::InvalidState {
                        state: actual,
                        action,
                        ..
                    } => {
                        assert_eq!(actual.as_str(), *state, "{label}: illegal state");
                        assert_eq!(action.as_str(), "start", "{label}: illegal action");
                    }
                    other => panic!("{label}: expected an invalid-state error, got {other:?}"),
                }
            }
            Step::StopIsIllegal(machine, state) => {
                let err = manager.stop(&name(machine), 0).await.unwrap_err();
                match &err {
                    MachineError::InvalidState {
                        state: actual,
                        action,
                        ..
                    } => {
                        assert_eq!(actual.as_str(), *state, "{label}: illegal state");
                        assert_eq!(action.as_str(), "stop", "{label}: illegal action");
                    }
                    other => panic!("{label}: expected an invalid-state error, got {other:?}"),
                }
            }
            Step::RemoveRunningIsIllegal(machine) => {
                let err = manager.remove(&name(machine), false).await.unwrap_err();
                match &err {
                    MachineError::InvalidState { action, .. } => {
                        assert_eq!(action.as_str(), "remove", "{label}: illegal action");
                    }
                    other => panic!("{label}: expected an invalid-state error, got {other:?}"),
                }
            }
            Step::ListIsEmpty => {
                assert!(
                    manager.list().await.unwrap().is_empty(),
                    "{label}: list is empty"
                );
            }
            Step::StatusIsNotFound(machine) => {
                let err = manager.status(&name(machine)).await.unwrap_err();
                assert!(
                    matches!(err, MachineError::NotFound { .. }),
                    "{label}: status is not found, got {err:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn lifecycle_transitions_follow_the_state_machine() {
    let cases: &[(&str, &[Step])] = &[
        (
            "full lifecycle through restart and removal",
            &[
                Step::Create("adesk"),
                Step::Start("adesk"),
                Step::Restart("adesk"),
                Step::Stop("adesk"),
                Step::Remove("adesk", false, MachineState::Stopped),
                Step::ListIsEmpty,
                Step::StatusIsNotFound("adesk"),
            ],
        ),
        (
            "start is allowed from created and stopped",
            &[
                Step::Create("adesk"),
                Step::Start("adesk"),
                Step::Stop("adesk"),
                Step::Start("adesk"),
            ],
        ),
        (
            "illegal transitions are reported",
            &[
                Step::Create("adesk"),
                Step::StopIsIllegal("adesk", "created"),
                Step::Start("adesk"),
                Step::StartIsIllegal("adesk", "running"),
                Step::RemoveRunningIsIllegal("adesk"),
                // The rejected removal left the machine intact, so a forced one
                // still observes it running.
                Step::Remove("adesk", true, MachineState::Running),
            ],
        ),
    ];

    for (label, steps) in cases {
        run_script(label, steps).await;
    }
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
