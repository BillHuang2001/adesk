//! Built-in scenario definitions and the runner.
//!
//! Run with: `cargo test -p adesk-agent --features test-support`
#![cfg(feature = "test-support")]

use adesk_agent::testing::ScriptedClient;
use adesk_agent::{Expectation, LoopConfig, Scenario, ScenarioId, ScenarioRunner};

/// Every `ScenarioId` has a definition with a task, a non-empty script, and at
/// least one expectation; `as_str()` values are unique and match the CLI values.
#[test]
#[ignore = "phase 2: scenarios not implemented"]
fn every_scenario_is_defined() {
    let _ = ScenarioId::all();
    let _ = Scenario::builtin;
    let _ = Expectation::NoFailedSteps;
    todo!("phase 2: assert id/script/expectations for all eight scenarios")
}

/// `launch`: list apps → launch → wait for the window; expects `Finished{true}`
/// and `ActionSeen(LaunchApp)`.
#[tokio::test]
#[ignore = "phase 2: scenarios not implemented"]
async fn launch_scenario_passes_with_script() {
    todo!("phase 2: run with ScriptedClient + scenario script")
}

/// `activate`: two windows, activate the inactive one via `activate_window`
/// (asserting no synthetic input), expect `ActionSeen(ActivateWindow)`.
#[tokio::test]
#[ignore = "phase 2: scenarios not implemented"]
async fn activate_scenario_uses_runtime_native_focus() {
    todo!("phase 2: assert ActivateWindow and no Keypress/Click")
}

/// `click`: normalized position click + quiet observation, capped readbacks.
#[tokio::test]
#[ignore = "phase 2: scenarios not implemented"]
async fn click_scenario_observes_quiet() {
    todo!("phase 2: assert Click + Observe and MaxReadbacks")
}

/// `type`: `type_text` into the focused window + quiet observation.
#[tokio::test]
#[ignore = "phase 2: scenarios not implemented"]
async fn type_scenario_types_into_focus() {
    todo!("phase 2: assert TypeText + Observe")
}

/// `scroll`: pointer axis scroll + change observation.
#[tokio::test]
#[ignore = "phase 2: scenarios not implemented"]
async fn scroll_scenario_scrolls_and_observes() {
    todo!("phase 2: assert Scroll + Observe")
}

/// `dialog`: popup appears → dismiss → verify disappearance.
#[tokio::test]
#[ignore = "phase 2: scenarios not implemented"]
async fn dialog_scenario_handles_popups() {
    todo!("phase 2: script popup_appeared observation, dismissal, verification")
}

/// `navigation`: click a link, observe the change, verify the title changed.
#[tokio::test]
#[ignore = "phase 2: scenarios not implemented"]
async fn navigation_scenario_crosses_pages() {
    todo!("phase 2: assert title_changed observation reached")
}

/// `error_recovery`: injected `unknown_window` → refresh → continue → finish.
#[tokio::test]
#[ignore = "phase 2: scenarios not implemented"]
async fn error_recovery_scenario_recovers() {
    todo!("phase 2: assert recoveries >= 1 and Finished{true}")
}

/// The runner honours `min(runner.max_steps, scenario.max_steps)` and reports
/// failed expectations instead of erroring.
#[tokio::test]
#[ignore = "phase 2: scenarios not implemented"]
async fn runner_enforces_step_budget_and_reports_failures() {
    let _ = (ScenarioRunner::new(LoopConfig::default()), ScriptedClient::new());
    todo!("phase 2: assert budget + failed expectation reporting")
}
