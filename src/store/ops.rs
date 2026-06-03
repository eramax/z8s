use crate::store::{AnyResource, ResourceState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreChange {
    Created,
    Updated,
    Deleted,
}

#[derive(Debug, Clone)]
pub enum StoreOp {
    Upsert(AnyResource),
    UpsertWithState(AnyResource, Option<ResourceState>),
    Delete(AnyResource),
}

#[derive(Debug, Clone)]
pub enum StoreEvent {
    Applied {
        resource: AnyResource,
        change: StoreChange,
    },
    Deleted {
        resource: AnyResource,
    },
}

impl StoreEvent {
    pub fn kind(&self) -> &str {
        match self {
            StoreEvent::Applied { resource, .. } | StoreEvent::Deleted { resource } => {
                resource.kind()
            }
        }
    }

    pub fn uid(&self) -> String {
        match self {
            StoreEvent::Applied { resource, .. } | StoreEvent::Deleted { resource } => {
                resource.uid()
            }
        }
    }
}
