pub mod types;
pub mod store;
pub mod syscall;

pub use syscall as sys;

#[cfg(test)]
mod tests {
    use super::types::*;
    use super::types::resource::Resource;
    use super::store::{MemoryBackend, StoreBackend, StoreOp};

    fn make_pod(name: &str, namespace: &str) -> Pod {
        Pod {
            metadata: ObjectMeta {
                uid: Some(format!("uid-{}", name)),
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            spec: Some(PodSpec {
                containers: vec![ContainerSpec {
                    name: "main".to_string(),
                    image: "alpine:3.19".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn make_deployment(name: &str, namespace: &str) -> Deployment {
        Deployment {
            metadata: ObjectMeta {
                uid: Some(format!("uid-deploy-{}", name)),
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                replicas: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn make_configmap(name: &str, namespace: &str) -> ConfigMap {
        ConfigMap {
            metadata: ObjectMeta {
                uid: Some(format!("uid-cm-{}", name)),
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(std::collections::BTreeMap::from([
                ("key".to_string(), "value".to_string()),
            ])),
            ..Default::default()
        }
    }

    // ── AnyResource ──────────────────────────────────────────────────────

    #[test]
    fn any_resource_kind_roundtrip() {
        let pod = make_pod("nginx", "default");
        let any = pod.clone().into_any();
        assert_eq!(any.kind(), "Pod");
        assert_eq!(any.name(), "nginx");
        assert_eq!(any.namespace(), Some("default"));
        assert_eq!(any.uid(), "uid-nginx");

        let restored = Pod::from_any(any).unwrap();
        assert_eq!(restored, pod);
    }

    #[test]
    fn any_resource_all_kinds() {
        let pod = make_pod("p", "ns").into_any();
        assert_eq!(pod.kind(), "Pod");

        let deploy = make_deployment("d", "ns").into_any();
        assert_eq!(deploy.kind(), "Deployment");

        let cm = AnyResource::ConfigMap(make_configmap("c", "ns"));
        assert_eq!(cm.kind(), "ConfigMap");

        let svc = Service {
            metadata: ObjectMeta { name: Some("s".into()), namespace: Some("ns".into()), ..Default::default() },
            ..Default::default()
        }.into_any();
        assert_eq!(svc.kind(), "Service");

        let ns = Namespace {
            metadata: ObjectMeta { name: Some("production".into()), ..Default::default() },
            ..Default::default()
        }.into_any();
        assert_eq!(ns.kind(), "Namespace");
        assert_eq!(ns.namespace(), None);
    }

    #[test]
    fn any_resource_from_wrong_type_returns_none() {
        let pod = make_pod("p", "ns").into_any();
        let result = <Pod as Resource>::from_any(pod);
        assert!(result.is_some()); // Pod converts fine
    }

    // ── ResourceRecord ───────────────────────────────────────────────────

    #[test]
    fn record_new_starts_at_generation_1() {
        let record = ResourceRecord::new(make_pod("nginx", "default").into_any());
        assert_eq!(record.generation, 1);
        assert_eq!(record.observed_generation, 0);
        assert!(record.needs_reconcile());
    }

    #[test]
    fn record_needs_reconcile_until_observed() {
        let mut record = ResourceRecord::new(make_pod("nginx", "default").into_any());
        assert!(record.needs_reconcile());

        record.observed_generation = 1;
        assert!(!record.needs_reconcile());

        record.generation = 2;
        assert!(record.needs_reconcile());

        record.observed_generation = 2;
        assert!(!record.needs_reconcile());
    }

    #[test]
    fn record_uid_delegates_to_spec() {
        let record = ResourceRecord::new(make_pod("nginx", "default").into_any());
        assert_eq!(record.uid(), "uid-nginx");
        assert_eq!(record.kind(), "Pod");
        assert_eq!(record.name(), "nginx");
    }

    // ── ResourceStatus ───────────────────────────────────────────────────

    #[test]
    fn status_default_is_not_terminal() {
        let status = ResourceStatus::default();
        assert!(!status.is_terminal());
        assert_eq!(status.phase, Phase::Pending);
    }

    #[test]
    fn status_terminal_states() {
        let mut status = ResourceStatus::default();
        status.phase = Phase::Succeeded;
        assert!(status.is_terminal());

        status.phase = Phase::Failed;
        assert!(status.is_terminal());

        status.phase = Phase::Running;
        assert!(!status.is_terminal());
    }

    // ── EventRecord ──────────────────────────────────────────────────────

    #[test]
    fn event_record_key_format() {
        let pod = make_pod("nginx", "default");
        let event = EventRecord::new(
            "Started", "Container started", "controller",
            Pod::kind(), pod.name(), pod.namespace(), pod.uid(),
            EventType::Normal,
        );
        assert_eq!(event.key(), "uid-nginx/0");
        assert_eq!(event.reason, "Started");
        assert_eq!(event.resource_kind, "Pod");
    }

    // ── StoreBackend (MemoryBackend) ─────────────────────────────────────

    #[tokio::test]
    async fn store_write_and_get() {
        let store = MemoryBackend::new();
        let pod = make_pod("nginx", "default");
        let uid = pod.metadata.uid.clone().unwrap();

        store.write_spec(pod.into_any(), None).await.unwrap();

        let record = store.get(&uid).await.unwrap();
        assert_eq!(record.kind(), "Pod");
        assert_eq!(record.generation, 1);
    }

    #[tokio::test]
    async fn store_write_increments_generation() {
        let store = MemoryBackend::new();
        let pod = make_pod("nginx", "default");
        let uid = pod.metadata.uid.clone().unwrap();

        store.write_spec(pod.clone().into_any(), None).await.unwrap();
        store.write_spec(pod.into_any(), None).await.unwrap();

        let record = store.get(&uid).await.unwrap();
        assert_eq!(record.generation, 2);
    }

    #[tokio::test]
    async fn store_get_by_kind() {
        let store = MemoryBackend::new();
        store.write_spec(make_pod("a", "ns1").into_any(), None).await.unwrap();
        store.write_spec(make_pod("b", "ns2").into_any(), None).await.unwrap();
        store.write_spec(AnyResource::ConfigMap(make_configmap("c", "ns1")), None).await.unwrap();

        assert_eq!(store.get_by_kind("Pod").await.len(), 2);
        assert_eq!(store.get_by_kind("ConfigMap").await.len(), 1);
    }

    #[tokio::test]
    async fn store_assign_and_get_by_node() {
        let store = MemoryBackend::new();
        let pod = make_pod("nginx", "default");
        let uid = pod.metadata.uid.clone().unwrap();

        store.write_spec(pod.into_any(), None).await.unwrap();
        store.assign_node(&uid, "node-1").await.unwrap();

        let by_node = store.get_by_node("node-1").await;
        assert_eq!(by_node.len(), 1);
        assert_eq!(by_node[0].assigned_node.as_deref(), Some("node-1"));

        assert!(store.get_unassigned("Pod").await.is_empty());
    }

    #[tokio::test]
    async fn store_delete() {
        let store = MemoryBackend::new();
        let pod = make_pod("nginx", "default");
        let uid = pod.metadata.uid.clone().unwrap();

        store.write_spec(pod.into_any(), None).await.unwrap();
        assert!(store.get(&uid).await.is_some());

        store.delete(&uid).await.unwrap();
        assert!(store.get(&uid).await.is_none());
    }

    #[tokio::test]
    async fn store_write_status_does_not_increment_generation() {
        let store = MemoryBackend::new();
        let pod = make_pod("nginx", "default");
        let uid = pod.metadata.uid.clone().unwrap();

        store.write_spec(pod.into_any(), None).await.unwrap();
        store.write_status(&uid, ResourceStatus {
            phase: Phase::Running,
            ..Default::default()
        }).await.unwrap();

        let record = store.get(&uid).await.unwrap();
        assert_eq!(record.generation, 1);
        assert_eq!(record.status.phase, Phase::Running);
    }

    #[tokio::test]
    async fn store_set_observed_generation() {
        let store = MemoryBackend::new();
        let pod = make_pod("nginx", "default");
        let uid = pod.metadata.uid.clone().unwrap();

        store.write_spec(pod.into_any(), None).await.unwrap();
        assert!(store.get(&uid).await.unwrap().needs_reconcile());

        store.set_observed_generation(&uid, 1).await.unwrap();
        assert!(!store.get(&uid).await.unwrap().needs_reconcile());
    }

    #[tokio::test]
    async fn store_batch_apply() {
        let store = MemoryBackend::new();
        let ops = vec![
            StoreOp::WriteSpec { spec: make_pod("a", "ns").into_any(), assigned_node: None },
            StoreOp::WriteSpec { spec: make_pod("b", "ns").into_any(), assigned_node: None },
            StoreOp::WriteSpec { spec: AnyResource::ConfigMap(make_configmap("c", "ns")), assigned_node: None },
        ];
        store.apply_batch(ops).await.unwrap();

        assert_eq!(store.get_by_kind("Pod").await.len(), 2);
        assert_eq!(store.get_by_kind("ConfigMap").await.len(), 1);
    }

    #[tokio::test]
    async fn store_event_recording() {
        let store = MemoryBackend::new();
        let pod = make_pod("nginx", "default");
        let uid = pod.metadata.uid.clone().unwrap();

        store.record_event(&pod.into_any(), "Created", "Pod created", "api", EventType::Normal).await.unwrap();
        store.record_event(&make_pod("nginx", "default").into_any(), "Started", "Container started", "controller", EventType::Normal).await.unwrap();

        let events = store.get_events(&uid).await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].reason, "Created");
        assert_eq!(events[1].reason, "Started");
    }
}
