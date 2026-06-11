//! Spawn pipeline scaffold (C1) — steps will migrate from `runtime.rs` incrementally.

use anyhow::Result;
use async_trait::async_trait;

use super::context::ContainerSpawnCtx;
use super::pipes::StdPipes;
use crate::cri::runtime::{ProcessSupervisor, RunningContainer};

/// Mutable state accumulated across spawn steps (extended as steps migrate).
pub struct SpawnState<'a> {
    pub ctx: ContainerSpawnCtx<'a>,
    /// Filled by [`super::steps::MergeEnvStep`].
    pub merged_env: Option<Vec<(String, String)>>,
    /// Filled by [`super::steps::CreatePipesStep`].
    pub pipes: Option<StdPipes>,
}

impl<'a> SpawnState<'a> {
    pub fn new(ctx: ContainerSpawnCtx<'a>) -> Self {
        Self {
            ctx,
            merged_env: None,
            pipes: None,
        }
    }
}

#[async_trait]
pub trait SpawnStep: Send + Sync {
    async fn apply(
        &self,
        supervisor: &ProcessSupervisor,
        state: &mut SpawnState<'_>,
    ) -> Result<()>;
}

/// Ordered spawn steps; today delegates to legacy `ProcessSupervisor` paths.
pub struct SpawnPipeline {
    steps: Vec<Box<dyn SpawnStep>>,
}

impl SpawnPipeline {
    pub fn legacy_isolated() -> Self {
        Self { steps: Vec::new() }
    }

    pub fn push_step(mut self, step: Box<dyn SpawnStep>) -> Self {
        self.steps.push(step);
        self
    }

    /// Run configured steps, then execute root-ns or user-ns spawn (behavior parity).
    pub async fn finish_isolated(
        &self,
        supervisor: &ProcessSupervisor,
        mut state: SpawnState<'_>,
    ) -> Result<RunningContainer> {
        for step in &self.steps {
            step.apply(supervisor, &mut state).await?;
        }
        use super::context::{IsolationStrategy, isolation_strategy};
        match isolation_strategy() {
            IsolationStrategy::RootNs => supervisor.spawn_root_ns_container(state).await,
            IsolationStrategy::UserNs => supervisor.spawn_userns_container(state).await,
        }
    }
}

