//! Kubernetes-compatible API types for z8s (split across `types/` modules).

#[path = "types/discovery.rs"]
mod discovery;
#[path = "types/rbac.rs"]
mod rbac;
#[path = "types/z8s_network.rs"]
mod z8s_network;
#[path = "types/any_resource.rs"]
mod any_resource;
#[path = "types/runtime.rs"]
mod runtime;
#[path = "types/helpers.rs"]
mod helpers;
#[path = "types/authz.rs"]
mod authz;
#[path = "types/scale.rs"]
mod scale;
#[path = "types/meta.rs"]
mod meta;
#[path = "types/workload.rs"]
mod workload;
#[path = "types/networking.rs"]
mod networking;
#[path = "types/storage.rs"]
mod storage;
#[path = "types/config.rs"]
mod config;
#[path = "types/core.rs"]
mod core;

pub use any_resource::AnyResource;
pub use authz::*;
pub use config::*;
pub use core::*;
pub use discovery::*;
pub use helpers::*;
pub use meta::*;
pub use networking::*;
pub use rbac::*;
pub use runtime::*;
pub use scale::*;
pub use storage::*;
pub use workload::*;
pub use z8s_network::*;

#[cfg(test)]
#[path = "types/tests.rs"]
mod tests;
