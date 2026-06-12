//! # Store — Persistent Storage Backend
//!
//! The store is the single source of truth for all resource state.
//! It provides:
//!
//! - **CRUD operations** — write_spec, write_status, get, delete
//! - **Reactive events** — StoreEventHub notifies subscribers on changes
//! - **In-memory index** — fast lookups by kind, node, assignment status
//! - **Batch operations** — single transaction for multiple writes
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────┐
//! │   API Server │───write_spec()───┐
//! └──────────────┘                 │
//!                                  ▼
//! ┌──────────────┐         ┌──────────────┐
//! │  Controller  │───write_status()──▶│    Store     │
//! └──────────────┘         │  (redb +     │
//!                          │   index)     │
//! ┌──────────────┐         │              │
//! │   Scheduler  │───assign_node()──▶│              │
//! └──────────────┘         └──────┬───────┘
//!                                 │
//!                          emit_applied() / emit_deleted()
//!                                 │
//!                          ┌──────▼───────┐
//!                          │ StoreEventHub│
//!                          │  (broadcast) │
//!                          └──────┬───────┘
//!                    ┌────────────┼────────────┐
//!                    ▼            ▼            ▼
//!               ┌────────┐  ┌────────┐  ┌────────┐
//!               │  API   │  │  Sync  │  │  DNS   │
//!               │ (watch)│  │(gossip)│  │ (cache)│
//!               └────────┘  └────────┘  └────────┘
//! ```
//!
//! ## StoreBackend Trait
//!
//! All store operations go through this trait. Implementations:
//! - `RedbBackend` — persistent storage using redb
//! - `MemoryBackend` — in-memory storage for testing

mod backend;
mod redb;
mod memory;
mod hub;
mod ops;
mod snapshot;

pub use backend::StoreBackend;
pub use redb::RedbBackend;
pub use memory::MemoryBackend;
pub use hub::StoreEventHub;
pub use ops::{StoreOp, StoreEvent, StoreChange};
pub use snapshot::StoreSnapshot;
