//! # z8s Core — Shared Types, Store, Events, and Syscalls
//!
//! This is the foundation crate that all other z8s crates depend on.
//! It contains:
//!
//! - **Types** — All Kubernetes-compatible resource types (Pod, Service, etc.)
//! - **Store** — Persistent storage backend (redb) with in-memory index
//! - **Events** — StoreEventHub for reactive event distribution
//! - **Syscalls** — Direct Linux kernel calls (no nix dependency)
//!
//! ## Design Principles
//!
//! 1. **Zero internal dependencies** — this crate depends only on external crates
//! 2. **Types are immutable** — once created, resource types don't change
//! 3. **Store is the source of truth** — all state flows through the store
//! 4. **Events drive reactivity** — controllers subscribe to store events
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use core::types::{Pod, Resource, ResourceRecord};
//!
//! // Create a Pod
//! let pod = Pod { metadata: ObjectMeta::new("nginx", "default"), ... };
//!
//! // Convert to ResourceRecord for DB storage
//! let record = ResourceRecord::new(pod.into_any());
//!
//! // Store it
//! store.write_spec(record.spec, None).await?;
//! ```

pub mod types;
pub mod store;
pub mod syscall;

// Re-export commonly used items
pub use store::{StoreBackend, StoreEventHub, StoreOp, StoreEvent, StoreChange, StoreSnapshot};
pub use syscall as sys;
