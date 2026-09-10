//! Synthetic "dummy VLM" provider — no I/O, no network, never panics.
//!
//! [`DummyVlmProvider`] exists to drive the loop without an LLM: it either
//! replays a caller-supplied script ([`DummyMode::Fixed`]) or emits random
//! decisions drawn from a configurable action pool ([`DummyMode::Random`]).
//!
//! Unlike [`crate::provider::MockProvider`], which fails loudly when its script is
//! exhausted, the dummy provider always terminates cleanly: when a fixed script
//! runs out it returns [`AgentDecision::Finish`], and random mode finishes via a
//! configurable probability *and* a hard step budget. The `complete` path performs
//! no allocation-light I/O, no syscalls and no locking beyond its own mutex, so it
//! can never panic.
//!
//! Randomness is provided by a small, self-contained SplitMix64 generator, so the
//! crate needs no random-number dependency and a fixed seed reproduces an
//! identical decision sequence across runs.

use std::sync::Mutex;

use adesk_core::{AppId, Button, Position, WindowId};
use async_trait::async_trait;

use crate::context::AgentContext;
use crate::decision::{AgentDecision, ObserveCondition};
use crate::error::ProviderError;
use crate::provider::LlmProvider;

/// Default seed for [`DummyMode::Random`] (`0x5EED_5EED`).
///
/// The default is a fixed constant so an unconfigured dummy provider is fully
/// reproducible: the same seed always yields the same decision sequence.
pub const DEFAULT_SEED: u64 = 0x5EED_5EED;

/// Default probability of returning [`AgentDecision::Finish`] on a random step.
///
/// Chosen so a random walk ends well before the loop's default step budget while
/// still emitting a handful of actions.
pub const DEFAULT_FINISH_PROBABILITY: f64 = 0.15;

/// Default hard step budget for [`DummyMode::Random`].
///
/// Once this many pooled decisions have been emitted, the next call returns
/// [`AgentDecision::Finish`] unconditionally, so termination never depends on the
/// finish probability.
pub const DEFAULT_STEP_BUDGET: u32 = 10;

/// How [`DummyVlmProvider`] chooses each decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum DummyMode {
    /// Replay a caller-supplied script in order; [`AgentDecision::Finish`] once it
    /// runs out.
    #[value(name = "fixed")]
    Fixed,
    /// Pick a random decision from the configured pool each step, finishing via
    /// the finish probability or the step budget. This is the default.
    #[default]
    #[value(name = "random")]
    Random,
}

/// Configuration for [`DummyVlmProvider`].
///
/// Both modes read their own fields: [`DummyMode::Fixed`] uses `script`, while
/// [`DummyMode::Random`] uses `seed`, `finish_probability`, `step_budget` and
/// `pool`. An empty `pool` falls back to [`default_action_pool`].
#[derive(Debug, Clone, PartialEq)]
pub struct DummyConfig {
    /// Decision source.
    pub mode: DummyMode,
    /// Canned sequence replayed in [`DummyMode::Fixed`]; unused in
    /// [`DummyMode::Random`].
    pub script: Vec<AgentDecision>,
    /// PRNG seed for [`DummyMode::Random`] (default [`DEFAULT_SEED`]).
    pub seed: u64,
    /// Per-step probability of finishing early in [`DummyMode::Random`]
    /// (default [`DEFAULT_FINISH_PROBABILITY`]).
    pub finish_probability: f64,
    /// Hard cap on pooled decisions emitted in [`DummyMode::Random`]
    /// (default [`DEFAULT_STEP_BUDGET`]).
    pub step_budget: u32,
    /// Action pool for [`DummyMode::Random`]; empty = [`default_action_pool`].
    pub pool: Vec<AgentDecision>,
    /// Stable provider name (default `"dummy"`).
    pub name: String,
}

impl Default for DummyConfig {
    fn default() -> Self {
        Self {
            mode: DummyMode::Random,
            script: Vec::new(),
            seed: DEFAULT_SEED,
            finish_probability: DEFAULT_FINISH_PROBABILITY,
            step_budget: DEFAULT_STEP_BUDGET,
            pool: Vec::new(),
            name: String::from("dummy"),
        }
    }
}

/// The default [`DummyMode::Random`] action pool: one valid decision per common
/// entry of the [`AgentDecision`] vocabulary, using placeholder ids
/// (`WindowId(1)` and `org.example.demo`).
///
/// The pool deliberately omits [`AgentDecision::Finish`] — finishing is driven by
/// the finish probability and the step budget — and `close_window`, which is
/// destructive.
pub fn default_action_pool() -> Vec<AgentDecision> {
    let window = WindowId(1);
    let center = Position::normalized(0.5, 0.5);
    vec![
        AgentDecision::ListApps { query: None },
        AgentDecision::ListWindows,
        AgentDecision::GetWindow { window_id: window },
        AgentDecision::LaunchApp {
            app_id: AppId::from("org.example.demo"),
            args: Vec::new(),
        },
        AgentDecision::ActivateWindow { window_id: window },
        AgentDecision::Capture {
            window_id: window,
            region: None,
            max_dimension: None,
        },
        AgentDecision::Observe {
            window_id: Some(window),
            after_action: None,
            until: ObserveCondition::Quiet { quiet_ms: 100 },
            timeout_ms: None,
            include_image: Some(false),
            max_dimension: None,
            region: None,
        },
        AgentDecision::Wait {
            window_id: Some(window),
            until: ObserveCondition::Change,
            timeout_ms: None,
        },
        AgentDecision::Click {
            window_id: window,
            position: center,
            button: Button::Left,
            count: 1,
        },
        AgentDecision::Type {
            window_id: Some(window),
            text: String::from("hello"),
        },
        AgentDecision::Keypress {
            window_id: Some(window),
            keys: vec![String::from("RETURN")],
        },
        AgentDecision::Scroll {
            window_id: window,
            position: center,
            dx: 0.0,
            dy: -3.0,
        },
    ]
}

/// A tiny self-contained SplitMix64 PRNG.
///
/// Deterministic, dependency-free and `unsafe`-free: identical seeds produce
/// identical streams, which is what makes [`DummyMode::Random`] reproducible.
/// `next_bounded` uses a plain modulo, which carries a negligible bias for the
/// tiny pools a dummy provider draws from.
#[derive(Debug, Clone)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Golden-ratio increment added on every draw.
    const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
    /// First finalizer multiplier.
    const MIX1: u64 = 0xBF58_476D_1CE4_E5B9;
    /// Second finalizer multiplier.
    const MIX2: u64 = 0x94D0_49BB_1331_11EB;

    /// Seed the generator.
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Next 64-bit output.
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(Self::GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(Self::MIX1);
        z = (z ^ (z >> 27)).wrapping_mul(Self::MIX2);
        z ^ (z >> 31)
    }

    /// Uniform value in `0..bound`; `bound` must be positive.
    fn next_bounded(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0, "next_bounded needs a positive bound");
        (self.next_u64() % bound as u64) as usize
    }

    /// Uniform value in `0.0..1.0`.
    fn next_f64(&mut self) -> f64 {
        // 53 significant bits give an exact double in [0, 1).
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

/// Mutable replay/PRNG state behind a mutex so `complete(&self)` can advance it.
#[derive(Debug)]
struct DummyState {
    cursor: usize,
    steps: u64,
    contexts: Vec<AgentContext>,
    rng: SplitMix64,
}

/// Synthetic "dummy VLM" [`LlmProvider`].
///
/// * **Fixed mode** (built by [`DummyVlmProvider::fixed`]) replays `script` in
///   order and, once exhausted, returns a successful [`AgentDecision::Finish`]
///   instead of failing — the loop always terminates cleanly.
/// * **Random mode** (built by [`DummyVlmProvider::random`]) draws a decision from
///   `pool` each step using a seeded SplitMix64 generator. It finishes when the
///   finish-probability roll succeeds or once `step_budget` pooled decisions have
///   been emitted, whichever happens first, so termination is guaranteed and
///   reproducible for a fixed seed.
///
/// Every call records the [`AgentContext`] it received, mirroring
/// [`crate::provider::MockProvider`], so tests can inspect what the loop sent.
#[derive(Debug)]
pub struct DummyVlmProvider {
    config: DummyConfig,
    state: Mutex<DummyState>,
}

impl DummyVlmProvider {
    /// Build a provider from a [`DummyConfig`].
    ///
    /// An empty `pool` is resolved to [`default_action_pool`] so random mode
    /// always has something to draw from.
    pub fn from_config(mut config: DummyConfig) -> Self {
        if config.pool.is_empty() {
            config.pool = default_action_pool();
        }
        let state = DummyState {
            cursor: 0,
            steps: 0,
            contexts: Vec::new(),
            rng: SplitMix64::new(config.seed),
        };
        Self {
            config,
            state: Mutex::new(state),
        }
    }

    /// Build a fixed-mode provider that replays `decisions` in order.
    pub fn fixed(decisions: impl IntoIterator<Item = AgentDecision>) -> Self {
        Self::from_config(DummyConfig {
            mode: DummyMode::Fixed,
            script: decisions.into_iter().collect(),
            ..DummyConfig::default()
        })
    }

    /// Build a random-mode provider with the documented defaults and `seed`.
    pub fn random(seed: u64) -> Self {
        Self::from_config(DummyConfig {
            mode: DummyMode::Random,
            seed,
            ..DummyConfig::default()
        })
    }

    /// The resolved configuration (including the effective action pool).
    pub fn config(&self) -> &DummyConfig {
        &self.config
    }

    /// Override the provider name (shows up in reports).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.config.name = name.into();
        self
    }

    /// Contexts received so far, in call order (cloned snapshot).
    pub fn contexts(&self) -> Vec<AgentContext> {
        self.lock().contexts.clone()
    }

    /// Number of contexts received so far.
    pub fn context_count(&self) -> usize {
        self.lock().contexts.len()
    }

    /// Decisions left before the fallback `Finish`.
    ///
    /// In [`DummyMode::Fixed`] this is the un-replayed script length; in
    /// [`DummyMode::Random`] it is how many pooled decisions can still be emitted
    /// before the step budget forces [`AgentDecision::Finish`].
    pub fn remaining(&self) -> usize {
        let state = self.lock();
        match self.config.mode {
            DummyMode::Fixed => self.config.script.len().saturating_sub(state.cursor),
            DummyMode::Random => {
                (u64::from(self.config.step_budget).saturating_sub(state.steps)) as usize
            }
        }
    }

    /// Rewind the fixed-script cursor, reset the random step count and PRNG, and
    /// forget recorded contexts.
    pub fn reset(&mut self) {
        let mut state = self.lock();
        state.cursor = 0;
        state.steps = 0;
        state.contexts.clear();
        state.rng = SplitMix64::new(self.config.seed);
    }

    /// Poison-tolerant lock helper.
    fn lock(&self) -> std::sync::MutexGuard<'_, DummyState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl LlmProvider for DummyVlmProvider {
    async fn complete(&self, ctx: &AgentContext) -> Result<AgentDecision, ProviderError> {
        // Record the context and advance state under one lock; there is no await,
        // so the guard never spans a suspension point.
        let mut state = self.lock();
        state.contexts.push(ctx.clone());

        match self.config.mode {
            DummyMode::Fixed => match self.config.script.get(state.cursor) {
                Some(decision) => {
                    state.cursor += 1;
                    Ok(decision.clone())
                }
                None => Ok(AgentDecision::Finish {
                    success: true,
                    summary: format!(
                        "dummy VLM: fixed script exhausted ({} decisions); finishing",
                        self.config.script.len()
                    ),
                }),
            },
            DummyMode::Random => {
                if state.steps >= u64::from(self.config.step_budget) {
                    return Ok(AgentDecision::Finish {
                        success: true,
                        summary: format!(
                            "dummy VLM: step budget ({}) reached; finishing",
                            self.config.step_budget
                        ),
                    });
                }
                if state.rng.next_f64() < self.config.finish_probability {
                    return Ok(AgentDecision::Finish {
                        success: true,
                        summary: String::from("dummy VLM: random finish"),
                    });
                }
                if self.config.pool.is_empty() {
                    // Unreachable via the constructors (empty pool resolves to the
                    // default), but keeps `complete` panic-free regardless.
                    return Ok(AgentDecision::Finish {
                        success: true,
                        summary: String::from("dummy VLM: empty action pool; finishing"),
                    });
                }
                let index = state.rng.next_bounded(self.config.pool.len());
                state.steps += 1;
                Ok(self.config.pool[index].clone())
            }
        }
    }

    fn name(&self) -> &str {
        &self.config.name
    }

    fn supports_images(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::decision::ActionKind;
    use crate::provider::test_context;

    /// Draw `n` decisions from a provider.
    async fn drain(provider: &DummyVlmProvider, n: usize) -> Vec<AgentDecision> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(provider.complete(&test_context("draw")).await.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn fixed_replays_supplied_decisions_in_order() {
        let decisions = vec![
            AgentDecision::ListWindows,
            AgentDecision::ListApps {
                query: Some(String::from("edit")),
            },
        ];
        let provider = DummyVlmProvider::fixed(decisions.clone());

        assert_eq!(provider.config().mode, DummyMode::Fixed);
        assert_eq!(provider.remaining(), 2);
        assert_eq!(
            provider.complete(&test_context("one")).await.unwrap(),
            decisions[0]
        );
        assert_eq!(provider.remaining(), 1);
        assert_eq!(
            provider.complete(&test_context("two")).await.unwrap(),
            decisions[1]
        );
        assert_eq!(provider.remaining(), 0);
    }

    #[tokio::test]
    async fn fixed_falls_back_to_finish_without_panicking() {
        let provider = DummyVlmProvider::fixed(vec![AgentDecision::ListWindows]);
        assert_eq!(
            provider.complete(&test_context("t")).await.unwrap(),
            AgentDecision::ListWindows
        );

        // Every call past the end keeps returning a successful Finish — never a
        // panic and never a repeated script entry.
        for _ in 0..3 {
            match provider.complete(&test_context("t")).await.unwrap() {
                AgentDecision::Finish { success, summary } => {
                    assert!(success);
                    assert!(summary.contains("exhausted"), "summary: {summary}");
                }
                other => panic!("expected Finish, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn empty_fixed_script_finishes_immediately() {
        let provider = DummyVlmProvider::fixed(Vec::new());
        assert!(matches!(
            provider.complete(&test_context("t")).await.unwrap(),
            AgentDecision::Finish { success: true, .. }
        ));
    }

    #[tokio::test]
    async fn random_is_reproducible_for_a_fixed_seed() {
        let first = DummyVlmProvider::random(42);
        let second = DummyVlmProvider::random(42);
        assert_eq!(drain(&first, 12).await, drain(&second, 12).await);
    }

    #[tokio::test]
    async fn random_differs_across_seeds() {
        let config = |seed| DummyConfig {
            mode: DummyMode::Random,
            seed,
            finish_probability: 0.0,
            step_budget: 64,
            ..DummyConfig::default()
        };
        let first = DummyVlmProvider::from_config(config(1));
        let second = DummyVlmProvider::from_config(config(2));

        let seq_first = drain(&first, 16).await;
        let seq_second = drain(&second, 16).await;
        assert_ne!(seq_first, seq_second);
        // With finish probability 0 nothing should have finished within the budget.
        assert!(seq_first
            .iter()
            .all(|d| !matches!(d, AgentDecision::Finish { .. })));
    }

    #[tokio::test]
    async fn random_reaches_finish_within_the_step_budget() {
        let provider = DummyVlmProvider::from_config(DummyConfig {
            mode: DummyMode::Random,
            seed: 7,
            finish_probability: 0.0,
            step_budget: 5,
            ..DummyConfig::default()
        });

        for _ in 0..5 {
            let decision = provider.complete(&test_context("t")).await.unwrap();
            assert!(
                !matches!(decision, AgentDecision::Finish { .. }),
                "pooled decision expected before the budget, got {decision:?}"
            );
        }
        assert_eq!(provider.remaining(), 0);

        // The (budget + 1)-th call is guaranteed to finish, and stays finished.
        for _ in 0..2 {
            match provider.complete(&test_context("t")).await.unwrap() {
                AgentDecision::Finish { success, summary } => {
                    assert!(success);
                    assert!(summary.contains("step budget"), "summary: {summary}");
                }
                other => panic!("expected Finish, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn random_finishes_immediately_when_probability_is_one() {
        let provider = DummyVlmProvider::from_config(DummyConfig {
            mode: DummyMode::Random,
            finish_probability: 1.0,
            ..DummyConfig::default()
        });
        assert!(matches!(
            provider.complete(&test_context("t")).await.unwrap(),
            AgentDecision::Finish { success: true, .. }
        ));
    }

    #[tokio::test]
    async fn random_uses_a_custom_pool() {
        let provider = DummyVlmProvider::from_config(DummyConfig {
            mode: DummyMode::Random,
            finish_probability: 0.0,
            step_budget: 3,
            pool: vec![AgentDecision::ListWindows],
            ..DummyConfig::default()
        });
        assert_eq!(provider.config().pool.len(), 1);
        for _ in 0..3 {
            assert_eq!(
                provider.complete(&test_context("t")).await.unwrap(),
                AgentDecision::ListWindows
            );
        }
        assert!(matches!(
            provider.complete(&test_context("t")).await.unwrap(),
            AgentDecision::Finish { .. }
        ));
    }

    #[tokio::test]
    async fn name_and_image_support() {
        let provider = DummyVlmProvider::random(1);
        assert_eq!(provider.name(), "dummy");
        assert!(provider.supports_images());
        assert_eq!(provider.with_name("dummy-a").name(), "dummy-a");
    }

    #[tokio::test]
    async fn contexts_are_recorded_in_call_order() {
        let provider = DummyVlmProvider::fixed(vec![AgentDecision::ListWindows]);
        provider.complete(&test_context("first")).await.unwrap();
        provider.complete(&test_context("second")).await.unwrap();

        assert_eq!(provider.context_count(), 2);
        let contexts = provider.contexts();
        assert_eq!(contexts[0].task, "first");
        assert_eq!(contexts[1].task, "second");
    }

    #[tokio::test]
    async fn reset_rewinds_fixed_and_random_state() {
        let mut fixed =
            DummyVlmProvider::fixed(vec![AgentDecision::ListWindows, AgentDecision::ListWindows]);
        fixed.complete(&test_context("t")).await.unwrap();
        assert_eq!(fixed.context_count(), 1);
        fixed.reset();
        assert_eq!(fixed.remaining(), 2);
        assert_eq!(fixed.context_count(), 0);

        let mut random = DummyVlmProvider::from_config(DummyConfig {
            mode: DummyMode::Random,
            finish_probability: 0.0,
            step_budget: 4,
            ..DummyConfig::default()
        });
        let first = random.complete(&test_context("t")).await.unwrap();
        random.reset();
        assert_eq!(random.remaining(), 4);
        assert_eq!(
            random.complete(&test_context("t")).await.unwrap(),
            first,
            "reset must re-seed the generator"
        );
    }

    #[test]
    fn dummy_config_default_is_documented() {
        let config = DummyConfig::default();
        assert_eq!(config.mode, DummyMode::Random);
        assert!(config.script.is_empty());
        assert_eq!(config.seed, DEFAULT_SEED);
        assert_eq!(config.finish_probability, DEFAULT_FINISH_PROBABILITY);
        assert_eq!(config.step_budget, DEFAULT_STEP_BUDGET);
        assert!(
            config.pool.is_empty(),
            "empty pool defers to the default pool"
        );
        assert_eq!(config.name, "dummy");
    }

    #[test]
    fn random_constructor_applies_documented_defaults() {
        let provider = DummyVlmProvider::random(99);
        let config = provider.config();
        assert_eq!(config.mode, DummyMode::Random);
        assert_eq!(config.seed, 99);
        assert_eq!(config.finish_probability, DEFAULT_FINISH_PROBABILITY);
        assert_eq!(config.step_budget, DEFAULT_STEP_BUDGET);
        assert_eq!(config.pool.len(), default_action_pool().len());
        assert!(config.script.is_empty());
        assert_eq!(config.name, "dummy");
    }

    /// Every default pooled decision must be a syntactically valid
    /// [`AgentDecision`] — proven by a lossless serde round trip.
    #[test]
    fn default_pool_decisions_round_trip_through_serde() {
        let provider = DummyVlmProvider::random(DEFAULT_SEED);
        let pool = &provider.config().pool;
        assert!(!pool.is_empty());
        for decision in pool {
            let json = serde_json::to_string(decision).expect("pool serializes");
            let back: AgentDecision = serde_json::from_str(&json).expect("pool parses");
            assert_eq!(&back, decision);
        }
    }

    #[test]
    fn default_pool_covers_the_decision_vocabulary() {
        let provider = DummyVlmProvider::random(0);
        let kinds: Vec<ActionKind> = provider
            .config()
            .pool
            .iter()
            .map(AgentDecision::kind)
            .collect();
        for expected in [
            ActionKind::ListApps,
            ActionKind::ListWindows,
            ActionKind::GetWindow,
            ActionKind::LaunchApp,
            ActionKind::ActivateWindow,
            ActionKind::Capture,
            ActionKind::Observe,
            ActionKind::Wait,
            ActionKind::Click,
            ActionKind::TypeText,
            ActionKind::Keypress,
            ActionKind::Scroll,
        ] {
            assert!(
                kinds.contains(&expected),
                "default pool is missing {expected:?}"
            );
        }
        // Neither `finish` nor the destructive `close_window` belong in the pool.
        assert!(!kinds.contains(&ActionKind::Finish));
        assert!(!kinds.contains(&ActionKind::CloseWindow));
    }

    #[test]
    fn prng_is_deterministic_and_bounded() {
        let mut first = SplitMix64::new(0);
        let mut second = SplitMix64::new(0);
        let mut seen = HashSet::new();
        for _ in 0..64 {
            let value = first.next_u64();
            assert_eq!(value, second.next_u64(), "same seed must match");
            seen.insert(value);
        }
        assert_eq!(
            seen.len(),
            64,
            "the 64-bit stream must not repeat over 64 draws"
        );

        let mut rng = SplitMix64::new(123);
        for _ in 0..1000 {
            assert!(rng.next_bounded(7) < 7);
            let unit = rng.next_f64();
            assert!((0.0..1.0).contains(&unit), "unit interval, got {unit}");
        }
    }
}
