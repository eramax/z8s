//! YAML manifest parsing and quantity helpers.

use serde::Deserialize;

use super::{AnyResource, Container, Deployment, Pod, Quantity};

pub fn parse_manifest_yaml(yaml: &str) -> anyhow::Result<Vec<AnyResource>> {
    use anyhow::Context;
    let mut resources = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(yaml) {
        let value: serde_yaml::Value =
            serde_yaml::Value::deserialize(doc).context("Failed to parse YAML document")?;
        let kind = value
            .get("kind")
            .and_then(|k| k.as_str())
            .map(|s| s.to_string())
            .context("Missing 'kind' field in YAML")?;
        resources.push(AnyResource::from_yaml_value(value, &kind)?);
    }
    Ok(resources)
}

pub fn extract_containers(resource: &AnyResource) -> Vec<Container> {
    match resource {
        AnyResource::Pod(pod) => pod.spec.as_ref().map_or_else(Vec::new, |s| {
            let mut c = s.containers.clone();
            if let Some(init) = &s.init_containers {
                c.extend(init.clone());
            }
            c
        }),
        AnyResource::Deployment(deploy) => deploy
            .spec
            .as_ref()
            .and_then(|s| s.template.spec.as_ref())
            .map_or_else(Vec::new, |s| {
                let mut c = s.containers.clone();
                if let Some(init) = &s.init_containers {
                    c.extend(init.clone());
                }
                c
            }),
        _ => Vec::new(),
    }
}

pub fn parse_quantity_bytes(q: &Quantity) -> u64 {
    let s = q.0.trim();
    if let Some(rest) = s.strip_suffix("Ti") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(4)
    } else if let Some(rest) = s.strip_suffix("Gi") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(3)
    } else if let Some(rest) = s.strip_suffix("Mi") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(2)
    } else if let Some(rest) = s.strip_suffix("Ki") {
        rest.parse::<u64>().unwrap_or(0) * 1024
    } else if let Some(rest) = s.strip_suffix("Pi") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(5)
    } else if let Some(rest) = s.strip_suffix("Ei") {
        rest.parse::<u64>().unwrap_or(0) * 1024u64.pow(6)
    } else if let Some(rest) = s.strip_suffix('T') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000_000_000
    } else if let Some(rest) = s.strip_suffix('G') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000_000
    } else if let Some(rest) = s.strip_suffix('M') {
        rest.parse::<u64>().unwrap_or(0) * 1_000_000
    } else if let Some(rest) = s.strip_suffix('k') {
        rest.parse::<u64>().unwrap_or(0) * 1000
    } else {
        s.parse::<u64>().unwrap_or(0)
    }
}

pub fn parse_quantity_cpu(q: &Quantity) -> (i64, i64) {
    let s = q.0.trim();
    if let Some(rest) = s.strip_suffix('m') {
        let millicores = rest.parse::<i64>().unwrap_or(0);
        (millicores * 100, 100_000)
    } else {
        let cores = s.parse::<f64>().unwrap_or(0.0);
        ((cores * 100_000.0) as i64, 100_000)
    }
}
