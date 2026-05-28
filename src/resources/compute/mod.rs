pub mod deployment;
pub mod pod;
pub mod spec_builder;

use crate::api::AnyResource;
use k8s_openapi::api::core::v1::PodSpec;

pub trait ComputeResource {
    fn pod_spec(&self) -> Option<&PodSpec>;
    fn restart_policy(&self) -> &str;
    fn is_ephemeral(&self) -> bool;
    fn namespace(&self) -> &str;
    fn name(&self) -> &str;
    fn labels(&self) -> &std::collections::BTreeMap<String, String>;
    fn uid(&self) -> &str;
    fn resource_ref(&self) -> &AnyResource;
}
