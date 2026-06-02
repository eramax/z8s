//! Kind-specific API response enrichment (A2).

mod deployment;
mod node;
mod pod;
mod service;
mod vnet;

pub use deployment::*;
pub use node::*;
pub use pod::*;
pub use service::*;
pub use vnet::*;
