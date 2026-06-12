pub mod assign;
pub mod index;
pub mod process;

use std::net::Ipv4Addr;
use std::sync::Arc;

use tokio::sync::Mutex;

use z8s_core::store::StoreBackend;
use z8s_core::types::{
    AnyResource, EventType, Phase, Resource, ResourceStatus,
};
use z8s_core::types::event::reasons;
use z8s_core::types::helpers::now_rfc3339;

use network::{Netmux, Ipv4Cidr, IpPool};
use network::plan::{self, PlanConfig};
use runtime::RuntimeProvider;
use runtime::spec::{ContainerConfig, ContainerConfigBuilder, ContainerSpec};

use crate::assign::{assign_unassigned_pods, reassign_dead_node_pods};
use crate::index::NodeIndex;
use crate::process::{ProcessTracker, TrackedPod};

#[derive(Debug, Clone)]
pub struct ControllerConfig {
    pub node_name: String,
    pub tick_interval_secs: u64,
    pub pod_cidr: String,
    pub service_cidr: String,
    pub cluster_domain: String,
    pub gateway: Ipv4Addr,
}

pub struct Controller {
    config: ControllerConfig,
    store: Arc<dyn StoreBackend>,
    runtime: Arc<dyn RuntimeProvider>,
    netmux: Arc<Mutex<Netmux>>,
    tracker: ProcessTracker,
    index: NodeIndex,
    ip_pool: IpPool,
    is_leader: bool,
}

impl Controller {
    pub fn new(
        config: ControllerConfig,
        store: Arc<dyn StoreBackend>,
        runtime: Arc<dyn RuntimeProvider>,
        netmux: Arc<Mutex<Netmux>>,
    ) -> anyhow::Result<Self> {
        let pod_cidr = Ipv4Cidr::parse(&config.pod_cidr)
            .ok_or_else(|| anyhow::anyhow!("invalid pod CIDR: {}", config.pod_cidr))?;
        let ip_pool = IpPool::new(pod_cidr);

        Ok(Self {
            config,
            store,
            runtime,
            netmux,
            tracker: ProcessTracker::new(),
            index: NodeIndex::new(),
            ip_pool,
            is_leader: false,
        })
    }

    pub fn set_leader(&mut self, is_leader: bool) {
        self.is_leader = is_leader;
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        tracing::info!(
            "controller starting on node {} (leader={})",
            self.config.node_name,
            self.is_leader
        );

        self.rebuild_index().await;

        loop {
            if let Err(e) = self.tick().await {
                tracing::error!("controller tick failed: {}", e);
            }
            tokio::time::sleep(std::time::Duration::from_secs(self.config.tick_interval_secs)).await;
        }
    }

    pub async fn tick(&mut self) -> anyhow::Result<()> {
        if self.is_leader {
            let assigned = assign_unassigned_pods(
                self.store.as_ref(),
                &mut self.index,
                &self.config.node_name,
                true,
            )
            .await?;
            if assigned > 0 {
                tracing::info!("assigned {} pods", assigned);
            }

            let reassigned = reassign_dead_node_pods(
                self.store.as_ref(),
                &mut self.index,
            )
            .await?;
            if reassigned > 0 {
                tracing::info!("reassigned {} pods from dead nodes", reassigned);
            }
        }

        let needing = self
            .store
            .get_needing_reconcile(&self.config.node_name)
            .await;

        for record in &needing {
            if let Err(e) = self.reconcile(record).await {
                tracing::error!(
                    "reconcile failed for {} ({}): {}",
                    record.kind(),
                    record.name(),
                    e
                );
            }
        }

        self.reconcile_network().await?;

        Ok(())
    }

    async fn reconcile(&mut self, record: &z8s_core::types::ResourceRecord) -> anyhow::Result<()> {
        match &record.spec {
            AnyResource::Pod(pod) => {
                if let Some(ref spec) = pod.spec {
                    self.reconcile_pod(record, spec).await?;
                }
            }
            AnyResource::Deployment(dep) => {
                tracing::debug!(
                    "deployment reconcile: {} (handled via owned pods)",
                    dep.name()
                );
            }
            _ => {}
        }
        Ok(())
    }

    async fn reconcile_pod(
        &mut self,
        record: &z8s_core::types::ResourceRecord,
        pod_spec: &z8s_core::types::PodSpec,
    ) -> anyhow::Result<()> {
        let uid = record.uid();
        let desired_running = !record.spec.is_deleting();
        let currently_tracked = self.tracker.contains(uid).await;

        if desired_running && !currently_tracked {
            self.start_pod(record, pod_spec).await?;
        } else if !desired_running && currently_tracked {
            self.stop_pod(record).await?;
        } else if desired_running && currently_tracked {
            self.check_pod_health(record).await?;
        }

        self.store
            .set_observed_generation(uid, record.generation)
            .await?;

        Ok(())
    }

    async fn start_pod(
        &mut self,
        record: &z8s_core::types::ResourceRecord,
        pod_spec: &z8s_core::types::PodSpec,
    ) -> anyhow::Result<()> {
        let uid = record.uid();
        let name = record.name();
        let namespace = record.spec.namespace().unwrap_or("default");

        let pod_ip = self
            .ip_pool
            .allocate()
            .ok_or_else(|| anyhow::anyhow!("no IPs available in pool"))?;

        tracing::info!("starting pod {} ({}) with IP {}", name, uid, pod_ip);

        let mut container_configs: Vec<ContainerConfig> = Vec::new();
        let mut container_ids: Vec<String> = Vec::new();

        for container in &pod_spec.containers {
            let container_id = format!("{}-{}", &uid[..8.min(uid.len())], container.name);

            let _rootfs = match self.runtime.unpack_image(&container.image, &container_id).await {
                Ok(path) => path,
                Err(e) => {
                    tracing::error!("failed to pull image {} for {}: {}", container.image, name, e);
                    let status = ResourceStatus {
                        phase: Phase::Failed,
                        message: Some(format!("image pull failed: {}", e)),
                        ..Default::default()
                    };
                    self.store.write_status(uid, status).await?;
                    self.ip_pool.release(pod_ip);
                    return Err(e);
                }
            };

            let _cgroup_path = self.runtime.create_pod_cgroup(uid)?;

            let mut env_pairs: Vec<(String, String)> = Vec::new();
            if let Some(ref env) = container.env {
                for e in env {
                    if let Some(ref val) = e.value {
                        env_pairs.push((e.name.clone(), val.clone()));
                    }
                }
            }
            env_pairs.push(("POD_IP".into(), pod_ip.to_string()));
            env_pairs.push(("POD_NAME".into(), name.to_string()));
            env_pairs.push(("POD_UID".into(), uid.to_string()));

            let mut builder = ContainerConfigBuilder::new(&container_id, &container.name, &container.image)
                .envs(env_pairs)
                .isolated_net(true);

            let mut combined_args: Vec<String> = Vec::new();
            if let Some(ref command) = container.command
                && !command.is_empty()
            {
                builder = builder.entrypoint(&command[0]);
                if command.len() > 1 {
                    combined_args.extend_from_slice(&command[1..]);
                }
            }
            if let Some(ref args) = container.args {
                combined_args.extend_from_slice(args);
            }
            if !combined_args.is_empty() {
                builder = builder.args(combined_args);
            }
            if let Some(ref dir) = container.working_dir {
                builder = builder.working_dir(dir);
            }

            if let Some(ref resources) = container.resources
                && let Some(ref limits) = resources.limits
            {
                if let Some(mem) = limits.get("memory")
                    && let Ok(bytes) = crate::assign::parse_memory(mem)
                {
                    builder = builder.memory_limit(bytes);
                }
                if let Some(cpu) = limits.get("cpu")
                    && let Ok(millicores) = cpu.trim_end_matches('m').parse::<i64>()
                {
                    let quota = millicores * 1000;
                    builder = builder.cpu_limit(quota, 100000);
                }
            }

            if let Some(ref sec) = container.security_context
                && let Some(sec_uid) = sec.run_as_user
            {
                let gid = sec.run_as_group.unwrap_or(sec_uid);
                builder = builder.run_as(sec_uid as u32, gid as u32);
            }

            container_configs.push(builder.build());
            container_ids.push(container_id);
        }

        let pod_spec_for_runtime = ContainerSpec {
            pod_name: name.to_string(),
            pod_uid: uid.to_string(),
            namespace: namespace.to_string(),
            hostname: name.to_string(),
            containers: container_configs,
            labels: record.spec.metadata().labels.clone().unwrap_or_default(),
            subnet: None,
        };

        match self.runtime.start_pod(&pod_spec_for_runtime).await {
            Ok(()) => {
                tracing::info!("pod {} containers started", name);
            }
            Err(e) => {
                tracing::error!("failed to start pod {}: {}", name, e);
                let status = ResourceStatus {
                    phase: Phase::Failed,
                    message: Some(format!("pod start failed: {}", e)),
                    ..Default::default()
                };
                self.store.write_status(uid, status).await?;
                self.ip_pool.release(pod_ip);
                return Err(e);
            }
        }

        self.tracker
            .insert(TrackedPod {
                uid: uid.to_string(),
                name: name.to_string(),
                namespace: namespace.to_string(),
                pod_ip: pod_ip.to_string(),
                pid: 0,
                container_ids,
            })
            .await;

        let status = ResourceStatus {
            phase: Phase::Running,
            pod_ip: Some(pod_ip.to_string()),
            host_ip: Some(self.config.gateway.to_string()),
            started_at: Some(now_rfc3339()),
            ..Default::default()
        };
        self.store.write_status(uid, status).await?;

        self.store
            .record_event(
                &record.spec,
                reasons::STARTED,
                &format!("pod {} started with IP {}", name, pod_ip),
                "controller",
                EventType::Normal,
            )
            .await?;

        self.index.increment(&self.config.node_name);

        tracing::info!("pod {} fully started with IP {}", name, pod_ip);
        Ok(())
    }

    async fn stop_pod(
        &mut self,
        record: &z8s_core::types::ResourceRecord,
    ) -> anyhow::Result<()> {
        let uid = record.uid();
        let name = record.name();

        tracing::info!("stopping pod {} ({})", name, uid);

        if let Some(tracked) = self.tracker.remove(uid).await {
            for cid in &tracked.container_ids {
                if let Err(e) = self.runtime.stop_container(cid).await {
                    tracing::warn!("failed to stop container {}: {}", cid, e);
                }
            }

            if let Ok(ip) = tracked.pod_ip.parse::<Ipv4Addr>() {
                self.ip_pool.release(ip);
            }

            let mut g = self.netmux.lock().await;
            if let Err(e) = g.detach_pod(uid) {
                tracing::warn!("failed to detach pod network {}: {}", uid, e);
            }
        }

        if let Err(e) = self.runtime.remove_cgroup(uid) {
            tracing::warn!("failed to remove cgroup for {}: {}", uid, e);
        }

        let status = ResourceStatus {
            phase: Phase::Succeeded,
            finished_at: Some(now_rfc3339()),
            ..Default::default()
        };
        self.store.write_status(uid, status).await?;

        self.store
            .record_event(
                &record.spec,
                reasons::KILLING,
                &format!("pod {} stopped", name),
                "controller",
                EventType::Normal,
            )
            .await?;

        self.index.decrement(&self.config.node_name);

        tracing::info!("pod {} fully stopped", name);
        Ok(())
    }

    async fn check_pod_health(
        &mut self,
        record: &z8s_core::types::ResourceRecord,
    ) -> anyhow::Result<()> {
        let uid = record.uid();
        let name = record.name();

        let alive = self.runtime.is_pod_alive(name).await;
        if !alive {
            tracing::warn!("pod {} appears dead, marking as Failed", name);

            if let Some(tracked) = self.tracker.remove(uid).await
                && let Ok(ip) = tracked.pod_ip.parse::<Ipv4Addr>()
            {
                self.ip_pool.release(ip);
            }

            let status = ResourceStatus {
                phase: Phase::Failed,
                message: Some("container process exited".into()),
                finished_at: Some(now_rfc3339()),
                ..Default::default()
            };
            self.store.write_status(uid, status).await?;
            self.index.decrement(&self.config.node_name);
        }

        Ok(())
    }

    async fn reconcile_network(&self) -> anyhow::Result<()> {
        let snap = self.store.snapshot().await;

        let plan_cfg = PlanConfig {
            node_name: self.config.node_name.clone(),
            pod_cidr: Ipv4Cidr::parse(&self.config.pod_cidr)
                .ok_or_else(|| anyhow::anyhow!("invalid pod CIDR"))?,
            service_cidr: Ipv4Cidr::parse(&self.config.service_cidr)
                .ok_or_else(|| anyhow::anyhow!("invalid service CIDR"))?,
            cluster_domain: self.config.cluster_domain.clone(),
            gateway: self.config.gateway,
            peers: vec![],
        };

        let desired = plan::plan(&snap, &plan_cfg);

        let mut g = self.netmux.lock().await;
        let ops = network::reconcile(&desired, g.current());
        if !ops.is_empty() {
            tracing::debug!("applying {} network ops", ops.len());
            g.apply(&ops, false)?;
        }

        Ok(())
    }

    async fn rebuild_index(&mut self) {
        let nodes = self.store.get_by_kind("Node").await;
        for record in &nodes {
            if let AnyResource::Node(node) = &record.spec {
                self.index.upsert_node(node.name());
            }
        }

        let my_pods = self.store.get_by_node(&self.config.node_name).await;
        for record in &my_pods {
            if record.status.phase == Phase::Running {
                self.index.increment(&self.config.node_name);
            }
        }

        tracing::info!(
            "index rebuilt: {} nodes, {} local pods",
            self.index.node_count(),
            self.index.total_pods()
        );
    }
}
