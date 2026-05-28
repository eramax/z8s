use async_trait::async_trait;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

use crate::api::types::AnyResource;
use crate::components::{PipelineStage, StageContext};

pub struct DnsStage {
    records: Arc<Mutex<HashMap<String, String>>>,
}

impl DnsStage {
    pub fn new() -> Self {
        Self {
            records: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl PipelineStage for DnsStage {
    fn name(&self) -> &str {
        "dns"
    }

    fn interests(&self) -> &[&str] {
        &["Pod", "Service"]
    }

    async fn on_created(&self, resource: &AnyResource, _ctx: &StageContext) -> Result<()> {
        match resource {
            AnyResource::Pod(pod) => {
                let name = pod.metadata.name.as_deref().unwrap_or("unknown");
                let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
                let pod_ip = pod.status.as_ref()
                    .and_then(|s| s.pod_ip.as_deref())
                    .unwrap_or("127.0.0.1");
                let fqdn = format!("{}.{}.pod.{}.svc.{}",
                    name, ns, ns,
                    crate::config::get().cluster_domain);

                let mut records = self.records.lock().await;
                records.insert(fqdn.clone(), pod_ip.to_string());
                info!("DNS: registered {}.{}/{} → {} ({})", name, ns, pod_ip, fqdn, records.len());
            }
            AnyResource::Service(svc) => {
                let name = svc.metadata.name.as_deref().unwrap_or("unknown");
                let ns = svc.metadata.namespace.as_deref().unwrap_or("default");
                let cluster_ip = svc.spec.as_ref()
                    .and_then(|s| s.cluster_ip.as_deref())
                    .unwrap_or("");
                if !cluster_ip.is_empty() && cluster_ip != "None" {
                    let fqdn = format!("{}.{}.svc.{}",
                        name, ns,
                        crate::config::get().cluster_domain);
                    let mut records = self.records.lock().await;
                    records.insert(fqdn.clone(), cluster_ip.to_string());
                    info!("DNS: registered svc {}/{} → {} ({})", name, ns, cluster_ip, fqdn);
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_deleted(&self, kind: &str, uid: &str, _ctx: &StageContext) -> Result<()> {
        match kind {
            "Pod" | "Service" => {
                let mut records = self.records.lock().await;
                let keys: Vec<String> = records.keys()
                    .filter(|k| k.contains(uid))
                    .cloned()
                    .collect();
                for key in keys {
                    records.remove(&key);
                    info!("DNS: removed record for {} ({})", uid, key);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
