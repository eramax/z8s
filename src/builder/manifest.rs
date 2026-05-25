use crate::api::AnyResource;
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::api::apps::v1::Deployment;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RawManifest {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    metadata: RawMetadata,
    #[serde(default)]
    spec: Option<serde_yaml::Value>,
}

#[derive(Debug, Deserialize)]
struct RawMetadata {
    name: Option<String>,
    namespace: Option<String>,
    #[serde(default)]
    labels: Option<std::collections::BTreeMap<String, String>>,
}

pub struct ManifestBuilder;

impl ManifestBuilder {
    pub fn from_yaml(yaml: &str) -> Result<Vec<AnyResource>> {
        let mut resources = Vec::new();

        for doc in serde_yaml::Deserializer::from_str(yaml) {
            let raw: RawManifest =
                serde_yaml::Value::deserialize(doc)
                    .and_then(|v| RawManifest::deserialize(v))
                    .context("Failed to parse manifest")?;

            let resource = match raw.kind.as_str() {
                "Pod" => Self::build_pod(&raw)?,
                "Deployment" => Self::build_deployment(&raw)?,
                "Service" => Self::build_service(&raw)?,
                "ConfigMap" => Self::build_configmap(&raw)?,
                "Secret" => Self::build_secret(&raw)?,
                kind => anyhow::bail!("Unsupported resource kind: {}", kind),
            };

            resources.push(resource);
        }

        Ok(resources)
    }

    fn build_pod(_raw: &RawManifest) -> Result<AnyResource> {
        let pod: Pod = serde_yaml::from_value(serde_yaml::Value::default())?;
        Ok(AnyResource::Pod(pod))
    }

    fn build_deployment(_raw: &RawManifest) -> Result<AnyResource> {
        let deploy: Deployment = serde_yaml::from_value(serde_yaml::Value::default())?;
        Ok(AnyResource::Deployment(deploy))
    }

    fn build_service(_raw: &RawManifest) -> Result<AnyResource> {
        let svc: k8s_openapi::api::core::v1::Service =
            serde_yaml::from_value(serde_yaml::Value::default())?;
        Ok(AnyResource::Service(svc))
    }

    fn build_configmap(_raw: &RawManifest) -> Result<AnyResource> {
        let cm: k8s_openapi::api::core::v1::ConfigMap =
            serde_yaml::from_value(serde_yaml::Value::default())?;
        Ok(AnyResource::ConfigMap(cm))
    }

    fn build_secret(_raw: &RawManifest) -> Result<AnyResource> {
        let secret: k8s_openapi::api::core::v1::Secret =
            serde_yaml::from_value(serde_yaml::Value::default())?;
        Ok(AnyResource::Secret(secret))
    }
}
