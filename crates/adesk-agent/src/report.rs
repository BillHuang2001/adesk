//! The run artifact written by `--report <path>`.
//!
//! A [`RunReport`] is the complete, machine-readable record of one agent run:
//! the task, which provider answered, the socket used, aggregated [`MetricsReport`],
//! the optional [`ScenarioReport`], and the per-step history. Everything is plain
//! serde JSON so runs can be diffed across commits.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agent_loop::StepRecord;
use crate::metrics::MetricsReport;
use crate::scenario::ScenarioReport;
use crate::Result;

/// Complete record of one `adesk-agent` invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    /// Task goal that was executed.
    pub task: String,
    /// Provider name that produced the decisions (`mock`, `openai`, ...).
    pub provider: String,
    /// AGP socket path used, when a real runtime was contacted.
    #[serde(default)]
    pub socket: Option<String>,
    /// Aggregated metrics.
    pub metrics: MetricsReport,
    /// Scenario evaluation, when the run used `--scenario`.
    #[serde(default)]
    pub scenario: Option<ScenarioReport>,
    /// Per-step history.
    #[serde(default)]
    pub history: Vec<StepRecord>,
}

impl RunReport {
    /// Pretty-printed JSON.
    pub fn to_json_pretty(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Write the report to `path`, creating parent directories as needed.
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, self.to_json_pretty()?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{LatencyStats, StopReason};
    use std::collections::BTreeMap;

    fn sample() -> RunReport {
        RunReport {
            task: "open the settings dialog".into(),
            provider: "mock".into(),
            socket: Some("/run/user/1000/adesk.sock".into()),
            metrics: MetricsReport {
                task: "open the settings dialog".into(),
                success: true,
                stop_reason: StopReason::Finished,
                steps: 3,
                decisions: 3,
                actions: 2,
                actions_by_kind: BTreeMap::new(),
                runtime_ops: 1,
                input_actions: 1,
                gpu_readbacks: 0,
                images_sent: 1,
                visual_tokens: 85,
                decision_latency: LatencyStats::default(),
                failures: 0,
                failures_by_kind: BTreeMap::new(),
                recoveries: 0,
                failure_rate: 0.0,
                recovery_rate: 0.0,
                elapsed_ms: 42,
            },
            scenario: None,
            history: Vec::new(),
        }
    }

    #[test]
    fn json_round_trips() {
        let report = sample();
        let json = report.to_json_pretty().unwrap();
        assert!(json.contains("\n  \"task\""), "expected pretty JSON");
        assert_eq!(serde_json::from_str::<RunReport>(&json).unwrap(), report);
    }

    /// `write` creates missing parent directories and stores exactly the JSON
    /// `to_json_pretty` produces.
    #[test]
    fn write_creates_parent_directories() {
        let root = std::env::temp_dir().join(format!(
            "adesk-agent-report-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("nested").join("run.json");
        let report = sample();
        report.write(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            report.to_json_pretty().unwrap()
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
