//! # adesk-agent — multimodal GUI agent prototype
//!
//! The ADesk runtime's primary client: a provider-agnostic multimodal agent that
//! plans, acts and observes a headless Wayland desktop through the Agent GUI
//! Protocol (AGP, `docs/protocol.md`).
//!
//! ## Loop shape
//!
//! ```text
//! build bounded context ──▶ LlmProvider::complete ──▶ AgentDecision
//!         ▲                                                 │
//!         │                                                 ▼
//!   record action/events ◀── Observation ◀── AgentClient (AGP)
//! ```
//!
//! Every step: assemble a *bounded* [`AgentContext`] (one current image, one
//! previous keyframe, capped action/event/window/app lists), ask the provider for
//! one [`AgentDecision`], execute it through [`AgentClient`], record the resulting
//! [`adesk_core::Observation`] and the metrics, and repeat until `Finish`, the step
//! budget, or the failure budget.
//!
//! ## Seams (everything is replaceable in tests)
//!
//! - [`LlmProvider`] — the LLM. [`MockProvider`] replays a script and is the
//!   default provider in tests and dry runs.
//! - [`AgentClient`] — AGP operations. [`AgpClient`] wraps `adesk-client`; the
//!   `adesk_agent::testing::ScriptedClient` fake needs no socket or compositor.
//!
//! ## Module map
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`context`] | Bounded context assembly and its budgeting rules |
//! | [`decision`] | The decision vocabulary (`AgentDecision`, `ActionKind`) |
//! | [`client`] | `AgentClient` trait plus AGP request/result types |
//! | [`agp`] | Concrete `adesk-client` adapter (the only wire-coupled module) |
//! | [`provider`] | `LlmProvider` plus mock and OpenAI-compatible implementations |
//! | [`agent_loop`] | plan → act → observe → decide control flow, budgets, recovery |
//! | [`metrics`] | Counters, latency statistics, `MetricsReport` |
//! | [`scenario`] | Built-in scenarios, expectations, runner |
//! | [`report`] | `RunReport` artifact written by `--report` |
//! | [`error`] | `Error`, `ProviderError`, retry/recovery classification |
//!
//! The `adesk-agent` binary wires CLI → provider → client → loop and is the only
//! place that touches the filesystem for reports.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod agent_loop;
pub mod agp;
pub mod client;
pub mod context;
pub mod decision;
pub mod error;
pub mod metrics;
pub mod provider;
pub mod report;
pub mod scenario;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use agent_loop::{AgentLoop, LoopConfig, LoopOutcome, StepRecord, StepStatus};
pub use agp::AgpClient;
pub use client::{
    AgentClient, CaptureOutcome, CaptureRequest, ClickRequest, LaunchOutcome, ObserveOutcome,
    ObserveRequest, RuntimeInfo, ScrollRequest, TypeOutcome, WindowList, PROTOCOL_VERSION,
};
pub use context::{
    ActionRecord, AgentContext, AppSummary, ContextBudget, ContextBuilder, ContextInput,
    EventSummary, RuntimeSummary, TaskDescription, WindowSummary,
};
pub use decision::{ActionKind, AgentDecision, ObserveCondition};
pub use error::{Error, ErrorClass, ProviderError, Result};
pub use metrics::{
    estimate_visual_tokens, latency_stats, LatencyStats, Metrics, MetricsReport, StopReason,
};
pub use provider::{
    LlmProvider, MockProvider, OpenAiCompatProvider, OpenAiConfig, ProviderConfig, ProviderKind,
    ScriptEntry,
};
pub use report::RunReport;
pub use scenario::{
    Expectation, ExpectationResult, Scenario, ScenarioId, ScenarioReport, ScenarioRunner,
};
