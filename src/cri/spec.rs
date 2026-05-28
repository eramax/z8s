use std::collections::{BTreeMap, HashMap};

use crate::cri::health::ProbeConfig;

#[derive(Debug, Clone)]
pub struct ContainerSpec {
    pub pod_name: String,
    pub pod_uid: String,
    pub namespace: String,
    pub hostname: String,
    pub containers: Vec<ContainerConfig>,
    pub cgroup_path: String,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ContainerConfig {
    pub container_id: String,
    pub container_name: String,
    pub image: String,
    pub rootfs_path: String,
    pub is_native: bool,
    pub entrypoint: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
    pub env: Vec<(String, String)>,
    pub volumes: Vec<ResolvedVolume>,
    pub memory_limit_bytes: Option<i64>,
    pub memory_low_bytes: Option<i64>,
    pub cpu_quota: Option<i64>,
    pub cpu_period: Option<i64>,
    pub run_as_user: Option<u32>,
    pub run_as_group: Option<u32>,
    pub privileged: bool,
    pub extra_capabilities: Vec<String>,
    pub isolated_net: bool,
    pub published_ports: HashMap<u16, u16>,
    pub probes: Vec<ProbeConfig>,
}

pub use crate::cri::volumes::ResolvedVolume;
pub struct ContainerSpecBuilder {
    spec: ContainerSpec,
}

impl ContainerSpecBuilder {
    pub fn new(pod_name: &str, pod_uid: &str, namespace: &str) -> Self {
        Self {
            spec: ContainerSpec {
                pod_name: pod_name.to_string(),
                pod_uid: pod_uid.to_string(),
                namespace: namespace.to_string(),
                hostname: pod_name.to_string(),
                containers: vec![],
                cgroup_path: String::new(),
                labels: BTreeMap::new(),
            },
        }
    }

    pub fn labels(&mut self, labels: BTreeMap<String, String>) -> &mut Self {
        self.spec.labels = labels;
        self
    }

    pub fn hostname(&mut self, hostname: &str) -> &mut Self {
        self.spec.hostname = hostname.to_string();
        self
    }

    pub fn add_container(&mut self, config: ContainerConfig) -> &mut Self {
        self.spec.containers.push(config);
        self
    }

    pub fn cgroup(&mut self, path: &str) -> &mut Self {
        self.spec.cgroup_path = path.to_string();
        self
    }

    pub fn build(self) -> ContainerSpec {
        self.spec
    }
}

pub struct ContainerConfigBuilder {
    cfg: ContainerConfig,
}

impl ContainerConfigBuilder {
    pub fn new(container_id: &str, name: &str) -> Self {
        Self {
            cfg: ContainerConfig {
                container_id: container_id.to_string(),
                container_name: name.to_string(),
                image: String::new(),
                rootfs_path: String::new(),
                is_native: false,
                entrypoint: String::new(),
                args: vec![],
                working_dir: None,
                env: vec![],
                volumes: vec![],
                memory_limit_bytes: None,
                memory_low_bytes: None,
                cpu_quota: None,
                cpu_period: None,
                run_as_user: None,
                run_as_group: None,
                privileged: false,
                extra_capabilities: vec![],
                isolated_net: false,
                published_ports: HashMap::new(),
                probes: vec![],
            },
        }
    }

    pub fn image(&mut self, image: &str, rootfs_path: &str) -> &mut Self {
        self.cfg.image = image.to_string();
        self.cfg.rootfs_path = rootfs_path.to_string();
        self.cfg.is_native = image.is_empty() || image == "host" || image.starts_with("host://");
        self
    }

    pub fn native(&mut self) -> &mut Self {
        self.cfg.is_native = true;
        self
    }

    pub fn entrypoint(&mut self, ep: &str, args: Vec<String>) -> &mut Self {
        self.cfg.entrypoint = ep.to_string();
        self.cfg.args = args;
        self
    }

    pub fn working_dir(&mut self, dir: &str) -> &mut Self {
        self.cfg.working_dir = Some(dir.to_string());
        self
    }

    pub fn env(&mut self, vars: Vec<(String, String)>) -> &mut Self {
        self.cfg.env = vars;
        self
    }

    pub fn add_env(&mut self, key: &str, value: &str) -> &mut Self {
        self.cfg.env.push((key.to_string(), value.to_string()));
        self
    }

    pub fn add_env_if_missing(&mut self, key: &str, value: &str) -> &mut Self {
        if !self.cfg.env.iter().any(|(k, _)| k == key) {
            self.cfg.env.push((key.to_string(), value.to_string()));
        }
        self
    }

    pub fn merge_env(&mut self, vars: Vec<(String, String)>) -> &mut Self {
        for (key, value) in vars {
            if !self.cfg.env.iter().any(|(k, _)| k == &key) {
                self.cfg.env.push((key, value));
            }
        }
        self
    }

    pub fn volumes(&mut self, vols: Vec<ResolvedVolume>) -> &mut Self {
        self.cfg.volumes = vols;
        self
    }

    pub fn add_volume(&mut self, host_path: &str, container_path: &str, read_only: bool) -> &mut Self {
        self.cfg.volumes.push(ResolvedVolume {
            host_path: host_path.to_string(),
            container_path: container_path.to_string(),
            read_only,
        });
        self
    }

    pub fn memory_limit(&mut self, bytes: i64) -> &mut Self {
        self.cfg.memory_limit_bytes = Some(bytes);
        self
    }

    pub fn memory_low(&mut self, bytes: i64) -> &mut Self {
        self.cfg.memory_low_bytes = Some(bytes);
        self
    }

    pub fn cpu_limit(&mut self, quota: i64, period: i64) -> &mut Self {
        self.cfg.cpu_quota = Some(quota);
        self.cfg.cpu_period = Some(period);
        self
    }

    pub fn resource_limits(
        &mut self,
        memory_limit: Option<i64>,
        memory_low: Option<i64>,
        cpu_quota: Option<i64>,
        cpu_period: Option<i64>,
    ) -> &mut Self {
        self.cfg.memory_limit_bytes = memory_limit;
        self.cfg.memory_low_bytes = memory_low;
        self.cfg.cpu_quota = cpu_quota;
        self.cfg.cpu_period = cpu_period;
        self
    }

    pub fn run_as(&mut self, uid: Option<u32>, gid: Option<u32>) -> &mut Self {
        self.cfg.run_as_user = uid;
        self.cfg.run_as_group = gid;
        self
    }

    pub fn privileged(&mut self, yes: bool) -> &mut Self {
        self.cfg.privileged = yes;
        self
    }

    pub fn capabilities(&mut self, caps: Vec<String>) -> &mut Self {
        self.cfg.extra_capabilities = caps;
        self
    }

    pub fn isolated_network(&mut self, yes: bool) -> &mut Self {
        self.cfg.isolated_net = yes;
        self
    }

    pub fn publish_port(&mut self, container_port: u16, host_port: u16) -> &mut Self {
        self.cfg.published_ports.insert(container_port, host_port);
        self
    }

    pub fn published_ports(&mut self, ports: HashMap<u16, u16>) -> &mut Self {
        self.cfg.published_ports = ports;
        self
    }

    pub fn probes(&mut self, probes: Vec<ProbeConfig>) -> &mut Self {
        self.cfg.probes = probes;
        self
    }

    pub fn add_probe(&mut self, probe: ProbeConfig) -> &mut Self {
        self.cfg.probes.push(probe);
        self
    }

    pub fn build(self) -> ContainerConfig {
        self.cfg
    }
}
