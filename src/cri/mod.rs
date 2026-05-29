pub mod cgroup;
pub mod exec;
pub mod health;
pub mod image;
pub mod oci;
pub mod rootfs;
pub mod runtime;
pub mod spec;
pub mod volumes;

use async_trait::async_trait;
use anyhow::Result;
use std::collections::HashMap;

use crate::cri::spec::ContainerSpec;

#[async_trait]
pub trait RuntimeProvider: Send + Sync {
    async fn start_pod(&self, spec: &ContainerSpec) -> Result<()>;
    async fn stop_pod(&self, spec: &ContainerSpec) -> Result<()>;
    async fn stop_container(&self, container_id: &str) -> Result<()>;
    async fn is_pod_alive(&self, pod_name: &str) -> bool;
    async fn is_pod_ready(&self, pod_name: &str) -> bool;
    async fn backend_connect_port(&self, pod_name: &str, port: u16) -> u16;
    async fn get_container_logs(&self, pod_name: &str, container_name: &str) -> Vec<String>;
    async fn pod_restart_counts(&self, pod_name: &str) -> HashMap<String, u32>;
    async fn unpack_image(&self, image_ref: &str, container_id: &str) -> Result<String>;
    fn create_pod_cgroup(&self, pod_uid: &str) -> Result<String>;
    fn remove_cgroup(&self, pod_uid: &str) -> Result<()>;
}

