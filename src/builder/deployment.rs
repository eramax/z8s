use crate::builder::container::ContainerBuilder;
use crate::builder::pod::PodBuilder;
use crate::builder::ResourceBuilder;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::apps::v1::DeploymentSpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::api::core::v1::PodTemplateSpec;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct DeploymentBuilder {
    name: String,
    namespace: String,
    labels: BTreeMap<String, String>,
    annotations: BTreeMap<String, String>,
    replicas: i32,
    match_labels: BTreeMap<String, String>,
    pod_builder: Option<PodBuilder>,
    containers: Vec<ContainerBuilder>,
}

impl DeploymentBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: "default".into(),
            labels: BTreeMap::new(),
            annotations: BTreeMap::new(),
            replicas: 1,
            match_labels: BTreeMap::new(),
            pod_builder: None,
            containers: Vec::new(),
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

    pub fn match_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.match_labels.insert(key.into(), value.into());
        self
    }

    pub fn replicas(mut self, count: i32) -> Self {
        self.replicas = count;
        self
    }

    pub fn add_container(mut self, builder: ContainerBuilder) -> Self {
        self.containers.push(builder);
        self
    }

    pub fn pod_template(mut self, builder: PodBuilder) -> Self {
        self.pod_builder = Some(builder);
        self
    }

    pub fn build(&self) -> Deployment {
        let app_label = self.match_labels.get("app")
            .cloned()
            .unwrap_or_else(|| self.name.clone());

        let mut match_labels = if self.match_labels.is_empty() {
            let mut m = BTreeMap::new();
            m.insert("app".into(), app_label.clone());
            m
        } else {
            self.match_labels.clone()
        };

        let template_labels = {
            let mut m = BTreeMap::new();
            m.insert("app".into(), app_label);
            m
        };

        let container = self.containers.first()
            .map(|c| c.build())
            .unwrap_or_else(|| {
                ContainerBuilder::new("default", "alpine:latest")
                    .command(vec!["sleep".to_string()])
                    .args(vec!["infinity".to_string()])
                    .build()
            });

        Deployment {
            metadata: ObjectMeta {
                name: Some(self.name.clone()),
                namespace: Some(self.namespace.clone()),
                labels: if self.labels.is_empty() {
                    None
                } else {
                    Some(self.labels.clone())
                },
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                replicas: Some(self.replicas),
                selector: LabelSelector {
                    match_labels: Some(match_labels),
                    ..Default::default()
                },
                template: PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        labels: Some(template_labels),
                        ..Default::default()
                    }),
                    spec: Some(
                        self.pod_builder
                            .clone()
                            .unwrap_or_else(|| PodBuilder::new(&self.name))
                            .add_container(container)
                            .build()
                            .spec
                            .unwrap(),
                    ),
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

impl ResourceBuilder for DeploymentBuilder {
    type Output = Deployment;
    fn build(&self) -> Deployment {
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
