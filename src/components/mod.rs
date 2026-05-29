use async_trait::async_trait;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use crate::types::{AnyResource, ResourceStore, ResourceTracker};
use crate::cri::RuntimeProvider;
use crate::net::NetworkEngine;
use crate::scheduler::process::ProcessTracker;
use crate::storage::StorageProvisioner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceCategory {
    Compute,
    Network,
    Storage,
}

#[async_trait]
pub trait Component: Send + Sync + 'static {
    fn kind(&self) -> &'static str;
    fn category(&self) -> ResourceCategory;

    async fn reconcile(&self, ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()>;
    async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()>;
    async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()>;
}

pub struct ReconcileContext {
    pub store: Arc<ResourceStore>,
    pub pipeline: Arc<ReconciliationPipeline>,
    pub cri: Arc<dyn RuntimeProvider>,
    pub net: Arc<dyn NetworkEngine>,
    pub process_tracker: Arc<ProcessTracker>,
    pub vol: Arc<dyn StorageProvisioner>,
}

pub struct ComponentRegistry {
    components: HashMap<&'static str, Box<dyn Component>>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self {
            components: HashMap::new(),
        }
    }

    pub fn register(&mut self, component: Box<dyn Component>) {
        self.components.insert(component.kind(), component);
    }

    pub fn get(&self, kind: &str) -> Option<&dyn Component> {
        self.components.get(kind).map(|c| c.as_ref())
    }

    pub fn by_category(&self, cat: ResourceCategory) -> Vec<&dyn Component> {
        self.components
            .values()
            .filter(|c| c.category() == cat)
            .map(|c| c.as_ref())
            .collect()
    }

    pub async fn reconcile_all(&self, ctx: &ReconcileContext) {
        let trackers = ctx.store.get_all().await;
        for tracker in &trackers {
            if let Some(component) = self.get(tracker.resource.kind()) {
        if let Err(e) = component.reconcile(ctx, tracker).await {
            tracing::error!("Reconcile failed for {}: {}", tracker.resource.uid(), e);
        }
            }
        }
    }

    pub async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) {
        let kind = resource.kind();

        if let Some(component) = self.get(kind) {
            if let Err(e) = component.on_apply(ctx, resource).await {
                tracing::error!("on_apply failed for {}: {}", resource.uid(), e);
            }
        }

        let stage_ctx = StageContext {
            store: ctx.store.clone(),
            cri: ctx.cri.clone(),
            net: ctx.net.clone(),
        };
        ctx.pipeline.dispatch_created(resource, &stage_ctx).await;
    }

    pub async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) {
        let kind = resource.kind();

        if let Some(component) = self.get(kind) {
            if let Err(e) = component.on_delete(ctx, resource).await {
                tracing::error!("on_delete failed for {}: {}", resource.uid(), e);
            }
        }

        let stage_ctx = StageContext {
            store: ctx.store.clone(),
            cri: ctx.cri.clone(),
            net: ctx.net.clone(),
        };
        ctx.pipeline.dispatch_deleted(kind, &resource.uid(), &stage_ctx).await;
    }
}

#[derive(Debug, Clone)]
pub enum ResourceEvent {
    Created(AnyResource),
    Updated(AnyResource),
    Deleted {
        kind: String,
        namespace: String,
        name: String,
        uid: String,
    },
}

impl ResourceEvent {
    pub fn kind(&self) -> &str {
        match self {
            ResourceEvent::Created(r) | ResourceEvent::Updated(r) => r.kind(),
            ResourceEvent::Deleted { kind, .. } => kind,
        }
    }
}

#[async_trait]
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &str;
    fn interests(&self) -> &[&str];

    async fn on_created(&self, resource: &AnyResource, ctx: &StageContext) -> Result<()>;
    async fn on_deleted(&self, kind: &str, uid: &str, ctx: &StageContext) -> Result<()>;
}

pub struct StageContext {
    pub store: Arc<ResourceStore>,
    pub cri: Arc<dyn RuntimeProvider>,
    pub net: Arc<dyn NetworkEngine>,
}

pub struct ReconciliationPipeline {
    stages: Vec<Box<dyn PipelineStage>>,
}

impl ReconciliationPipeline {
    pub fn builder() -> PipelineBuilder {
        PipelineBuilder::new()
    }

    pub async fn dispatch_created(&self, resource: &AnyResource, ctx: &StageContext) {
        let kind = resource.kind();
        for stage in &self.stages {
            if !stage.interests().contains(&kind) {
                continue;
            }
            if let Err(e) = stage.on_created(resource, ctx).await {
                tracing::error!(
                    "Pipeline stage '{}' failed on {} create: {}",
                    stage.name(),
                    kind,
                    e
                );
            }
        }
    }

    pub async fn dispatch_deleted(&self, kind: &str, uid: &str, ctx: &StageContext) {
        for stage in &self.stages {
            if !stage.interests().contains(&kind) {
                continue;
            }
            if let Err(e) = stage.on_deleted(kind, uid, ctx).await {
                tracing::error!(
                    "Pipeline stage '{}' failed on {} delete: {}",
                    stage.name(),
                    kind,
                    e
                );
            }
        }
    }
}

pub struct PipelineBuilder {
    stages: Vec<Box<dyn PipelineStage>>,
}

impl PipelineBuilder {
    pub fn new() -> Self {
        Self { stages: vec![] }
    }

    pub fn stage(mut self, stage: Box<dyn PipelineStage>) -> Self {
        self.stages.push(stage);
        self
    }

    pub fn build(self) -> ReconciliationPipeline {
        ReconciliationPipeline {
            stages: self.stages,
        }
    }
}

pub mod compute;
pub mod network;
pub mod storage;
