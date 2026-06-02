//! Pre-fork spawn pipeline steps (C1).

use anyhow::Result;
use async_trait::async_trait;

use super::context::ContainerSpawnCtx;
use super::pipeline::{SpawnState, SpawnStep};
use crate::cri::rootfs;
use crate::cri::runtime::ProcessSupervisor;

struct PrepareVolumesStep;

#[async_trait]
impl SpawnStep for PrepareVolumesStep {
    async fn apply(
        &self,
        _supervisor: &ProcessSupervisor,
        state: &mut SpawnState<'_>,
    ) -> Result<()> {
        let ContainerSpawnCtx { rootfs_path, volumes, .. } = &state.ctx;
        if !volumes.is_empty() {
            crate::cri::volumes::scrub_rootfs_volume_mounts(rootfs_path, volumes);
            crate::cri::volumes::stage_volumes_in_rootfs(rootfs_path, volumes);
        }
        Ok(())
    }
}

struct MergeEnvStep;

#[async_trait]
impl SpawnStep for MergeEnvStep {
    async fn apply(
        &self,
        _supervisor: &ProcessSupervisor,
        state: &mut SpawnState<'_>,
    ) -> Result<()> {
        let ContainerSpawnCtx {
            env_vars,
            rootfs_path,
            ..
        } = &state.ctx;
        state.merged_env = Some(ProcessSupervisor::merge_env(env_vars, rootfs_path));
        Ok(())
    }
}

struct PrepareRootfsStep;

#[async_trait]
impl SpawnStep for PrepareRootfsStep {
    async fn apply(
        &self,
        _supervisor: &ProcessSupervisor,
        state: &mut SpawnState<'_>,
    ) -> Result<()> {
        rootfs::prepare_rootfs(state.ctx.rootfs_path)?;
        Ok(())
    }
}

impl super::pipeline::SpawnPipeline {
    pub fn isolated_default() -> Self {
        Self::legacy_isolated()
            .push_step(Box::new(PrepareVolumesStep))
            .push_step(Box::new(PrepareRootfsStep))
            .push_step(Box::new(MergeEnvStep))
    }
}
