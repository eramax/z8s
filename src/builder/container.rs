use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, ResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ContainerBuilder {
    name: String,
    image: String,
    command: Vec<String>,
    args: Vec<String>,
    ports: Vec<ContainerPort>,
    env: Vec<EnvVar>,
    memory_limit: Option<String>,
    cpu_limit: Option<String>,
    memory_request: Option<String>,
    cpu_request: Option<String>,
}

impl ContainerBuilder {
    pub fn new(name: impl Into<String>, image: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            image: image.into(),
            command: Vec::new(),
            args: Vec::new(),
            ports: Vec::new(),
            env: Vec::new(),
            memory_limit: None,
            cpu_limit: None,
            memory_request: None,
            cpu_request: None,
        }
    }

    pub fn command(mut self, cmd: Vec<impl Into<String>>) -> Self {
        self.command = cmd.into_iter().map(|c| c.into()).collect();
        self
    }

    pub fn args(mut self, a: Vec<impl Into<String>>) -> Self {
        self.args = a.into_iter().map(|a| a.into()).collect();
        self
    }

    pub fn port(mut self, container_port: i32) -> Self {
        self.ports.push(ContainerPort {
            container_port,
            ..Default::default()
        });
        self
    }

    pub fn port_with_name(
        mut self,
        name: impl Into<String>,
        container_port: i32,
    ) -> Self {
        self.ports.push(ContainerPort {
            name: Some(name.into()),
            container_port,
            ..Default::default()
        });
        self
    }

    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push(EnvVar {
            name: name.into(),
            value: Some(value.into()),
            ..Default::default()
        });
        self
    }

    pub fn memory_limit(mut self, limit: impl Into<String>) -> Self {
        self.memory_limit = Some(limit.into());
        self
    }

    pub fn cpu_limit(mut self, limit: impl Into<String>) -> Self {
        self.cpu_limit = Some(limit.into());
        self
    }

    pub fn memory_request(mut self, req: impl Into<String>) -> Self {
        self.memory_request = Some(req.into());
        self
    }

    pub fn cpu_request(mut self, req: impl Into<String>) -> Self {
        self.cpu_request = Some(req.into());
        self
    }

    pub fn build(&self) -> Container {
        let mut resources = ResourceRequirements::default();

        let mut limits = BTreeMap::new();
        if let Some(ref mem) = self.memory_limit {
            limits.insert("memory".into(), Quantity(format!("{}", mem)));
        }
        if let Some(ref cpu) = self.cpu_limit {
            limits.insert("cpu".into(), Quantity(format!("{}", cpu)));
        }
        let has_limits = !limits.is_empty();
        if has_limits {
            resources.limits = Some(limits);
        }

        let mut requests = BTreeMap::new();
        if let Some(ref mem) = self.memory_request {
            requests.insert("memory".into(), Quantity(format!("{}", mem)));
        }
        if let Some(ref cpu) = self.cpu_request {
            requests.insert("cpu".into(), Quantity(format!("{}", cpu)));
        }
        let has_requests = !requests.is_empty();
        if has_requests {
            resources.requests = Some(requests);
        }

        Container {
            name: self.name.clone(),
            image: Some(self.image.clone()),
            command: if self.command.is_empty() {
                None
            } else {
                Some(self.command.clone())
            },
            args: if self.args.is_empty() {
                None
            } else {
                Some(self.args.clone())
            },
            ports: if self.ports.is_empty() {
                None
            } else {
                Some(self.ports.clone())
            },
            env: if self.env.is_empty() {
                None
            } else {
                Some(self.env.clone())
            },
            resources: if has_limits || has_requests {
                Some(resources)
            } else {
                None
            },
            ..Default::default()
        }
    }
}
