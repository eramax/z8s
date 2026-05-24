use crate::builder::container::ContainerBuilder;
use crate::builder::ResourceBuilder;
use k8s_openapi::api::core::v1::{Container, Pod, PodSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct PodBuilder {
    name: String,
    namespace: String,
    labels: BTreeMap<String, String>,
    annotations: BTreeMap<String, String>,
    containers: Vec<Container>,
    init_containers: Vec<Container>,
    restart_policy: Option<String>,
    node_name: Option<String>,
    service_account: Option<String>,
}

impl PodBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: "default".into(),
            labels: BTreeMap::new(),
            annotations: BTreeMap::new(),
            containers: Vec::new(),
            init_containers: Vec::new(),
            restart_policy: None,
            node_name: None,
            service_account: None,
        }
    }

    pub fn namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }

    pub fn label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    pub fn labels(mut self, labels: BTreeMap<String, String>) -> Self {
        self.labels = labels;
        self
    }

    pub fn annotation(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.annotations.insert(key.into(), value.into());
        self
    }

    pub fn add_container(mut self, container: Container) -> Self {
        self.containers.push(container);
        self
    }

    pub fn add_container_builder(mut self, builder: ContainerBuilder) -> Self {
        self.containers.push(builder.build());
        self
    }

    pub fn add_init_container(mut self, container: Container) -> Self {
        self.init_containers.push(container);
        self
    }

    pub fn restart_policy(mut self, policy: impl Into<String>) -> Self {
        self.restart_policy = Some(policy.into());
        self
    }

    pub fn node_name(mut self, name: impl Into<String>) -> Self {
        self.node_name = Some(name.into());
        self
    }

    pub fn service_account(mut self, name: impl Into<String>) -> Self {
        self.service_account = Some(name.into());
        self
    }

    pub fn build(&self) -> Pod {
        Pod {
            metadata: ObjectMeta {
                name: Some(self.name.clone()),
                namespace: Some(self.namespace.clone()),
                labels: if self.labels.is_empty() {
                    None
                } else {
                    Some(self.labels.clone())
                },
                annotations: if self.annotations.is_empty() {
                    None
                } else {
                    Some(self.annotations.clone())
                },
                ..Default::default()
            },
            spec: Some(PodSpec {
                containers: self.containers.clone(),
                init_containers: if self.init_containers.is_empty() {
                    None
                } else {
                    Some(self.init_containers.clone())
                },
                restart_policy: self.restart_policy.clone(),
                node_name: self.node_name.clone(),
                service_account_name: self.service_account.clone(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

impl ResourceBuilder for PodBuilder {
    type Output = Pod;
    fn build(&self) -> Pod {
        self.build()
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn namespace(&self) -> &str {
        &self.namespace
    }
    fn labels(&self) -> &BTreeMap<String, String> {
        &self.labels
    }
}
