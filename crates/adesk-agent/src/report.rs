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
