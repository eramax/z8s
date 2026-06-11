//! Unit tests for types parsing and resource identity.

use super::*;

    use super::*;

    #[test]
    fn uid_includes_namespace() {
        let yaml = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: my-pod\n  namespace: production\nspec:\n  containers:\n  - name: c\n    image: alpine\n";
        let resources = parse_manifest_yaml(yaml).unwrap();
        let r = &resources[0];
        assert_eq!(r.uid(), "Pod/production/my-pod");
    }

    #[test]
    fn uid_defaults_namespace_to_default() {
        let yaml = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: my-pod\nspec:\n  containers:\n  - name: c\n    image: alpine\n";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources[0].uid(), "Pod/default/my-pod");
    }

    #[test]
    fn same_name_different_namespaces_get_different_uids() {
        let pod_a = parse_manifest_yaml(
            "apiVersion: v1\nkind: Pod\nmetadata:\n  name: app\n  namespace: ns-a\nspec:\n  containers:\n  - name: c\n    image: alpine\n",
        ).unwrap().remove(0);
        let pod_b = parse_manifest_yaml(
            "apiVersion: v1\nkind: Pod\nmetadata:\n  name: app\n  namespace: ns-b\nspec:\n  containers:\n  - name: c\n    image: alpine\n",
        ).unwrap().remove(0);
        assert_ne!(pod_a.uid(), pod_b.uid());
    }

    #[test]
    fn pv_namespace_is_empty() {
        let yaml = "apiVersion: v1\nkind: PersistentVolume\nmetadata:\n  name: my-pv\nspec:\n  capacity:\n    storage: 1Gi\n  accessModes:\n  - ReadWriteOnce\n  hostPath:\n    path: /data\n";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources[0].namespace(), "");
        assert_eq!(resources[0].uid(), "PersistentVolume//my-pv");
    }

    #[tokio::test]
    async fn store_namespaced_resources_do_not_collide() {
        use crate::store::{MemoryBackend, StoreBackend};
        let store = MemoryBackend::new();

        let cm_a_yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: config\n  namespace: ns-a\ndata:\n  key: value-a\n";
        let cm_b_yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: config\n  namespace: ns-b\ndata:\n  key: value-b\n";

        let res_a = parse_manifest_yaml(cm_a_yaml).unwrap().remove(0);
        let res_b = parse_manifest_yaml(cm_b_yaml).unwrap().remove(0);

        store.apply(res_a).await.unwrap();
        store.apply(res_b).await.unwrap();

        let all = store.get_by_kind("ConfigMap").await;
        assert_eq!(all.len(), 2);

        let a = store.get("ConfigMap/ns-a/config").await;
        let b = store.get("ConfigMap/ns-b/config").await;
        assert!(a.is_some());
        assert!(b.is_some());

        if let Some(AnyResource::ConfigMap(cm)) = a.map(|t| t.resource) {
            assert_eq!(cm.data.unwrap().get("key").unwrap(), "value-a");
        }
        if let Some(AnyResource::ConfigMap(cm)) = b.map(|t| t.resource) {
            assert_eq!(cm.data.unwrap().get("key").unwrap(), "value-b");
        }
    }

    #[test]
    fn parse_multi_document_yaml() {
        let yaml = "\
apiVersion: v1
kind: ConfigMap
metadata:
  name: cm1
  namespace: default
---
apiVersion: v1
kind: Secret
metadata:
  name: sec1
  namespace: default
";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].kind(), "ConfigMap");
        assert_eq!(resources[1].kind(), "Secret");
    }

    #[test]
    fn parse_pv_and_pvc() {
        let yaml = "\
apiVersion: v1
kind: PersistentVolume
metadata:
  name: pv1
spec:
  capacity:
    storage: 5Gi
  accessModes:
  - ReadWriteOnce
  hostPath:
    path: /data/pv1
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: pvc1
  namespace: default
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 1Gi
";
        let resources = parse_manifest_yaml(yaml).unwrap();
        assert_eq!(resources.len(), 2);
        assert!(matches!(&resources[0], AnyResource::PersistentVolume(_)));
        assert!(matches!(
            &resources[1],
            AnyResource::PersistentVolumeClaim(_)
        ));
    }

    #[test]
    fn parse_unsupported_kind_returns_error() {
        let yaml = "apiVersion: v1\nkind: UnknownThing\nmetadata:\n  name: x\n";
        assert!(parse_manifest_yaml(yaml).is_err());
    }

    #[test]
    fn extract_containers_from_pod_includes_init() {
        let yaml = "\
apiVersion: v1
kind: Pod
metadata:
  name: p
spec:
  initContainers:
  - name: init
    image: busybox
  containers:
  - name: app
    image: alpine
";
        let resource = parse_manifest_yaml(yaml).unwrap().remove(0);
        let containers = extract_containers(&resource);
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].name, "app");
        assert_eq!(containers[1].name, "init");
    }

    #[test]
    fn extract_containers_from_non_pod_is_empty() {
        let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: cm\n";
        let resource = parse_manifest_yaml(yaml).unwrap().remove(0);
        assert!(extract_containers(&resource).is_empty());
    }

    #[test]
    fn parse_quantity_ki() {
        assert_eq!(parse_quantity_bytes(&Quantity("128Ki".into())), 128 * 1024);
    }

    #[test]
    fn parse_quantity_mi() {
        assert_eq!(
            parse_quantity_bytes(&Quantity("256Mi".into())),
            256 * 1024 * 1024
        );
    }

    #[test]
    fn parse_quantity_gi() {
        assert_eq!(
            parse_quantity_bytes(&Quantity("1Gi".into())),
            1024 * 1024 * 1024
        );
    }

    #[test]
    fn parse_quantity_plain_bytes() {
        assert_eq!(parse_quantity_bytes(&Quantity("4096".into())), 4096);
    }

    #[test]
    fn parse_cpu_millicores() {
        let (quota, period) = parse_quantity_cpu(&Quantity("500m".into()));
        assert_eq!(quota, 50_000);
        assert_eq!(period, 100_000);
    }

    #[test]
    fn parse_cpu_cores() {
        let (quota, period) = parse_quantity_cpu(&Quantity("2".into()));
        assert_eq!(quota, 200_000);
        assert_eq!(period, 100_000);
    }
