//! Store-backed union of all persisted resource kinds.

use serde::{Deserialize, Serialize};

use super::ObjectMeta;
use super::{
    ClusterRole, ClusterRoleBinding, ConfigMap, Deployment, EndpointSlice, Endpoints, Event,
    Ingress, Namespace, NetworkPolicy, Node, Nsg, PersistentVolume, PersistentVolumeClaim, Pod,
    Role, RoleBinding, RouteTable, Secret, Service, ServiceAccount, StorageClass, Subnet, VNet,
};

macro_rules! define_any_resource {
    (
        namespaced: [ $( $variant:ident($inner:ident) as $kind:expr ),+ $(,)? ];
        cluster_scoped: [ $( $cvariant:ident($cinner:ident) as $ckind:expr ),+ $(,)? ];
    ) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(tag = "resourceType")]
        pub enum AnyResource {
            $( $variant($inner), )+
            $( $cvariant($cinner), )+
        }

        impl AnyResource {
            pub fn metadata(&self) -> &ObjectMeta {
                match self {
                    $( Self::$variant(r) => &r.metadata, )+
                    $( Self::$cvariant(r) => &r.metadata, )+
                }
            }

            pub fn metadata_mut(&mut self) -> &mut ObjectMeta {
                match self {
                    $( Self::$variant(r) => &mut r.metadata, )+
                    $( Self::$cvariant(r) => &mut r.metadata, )+
                }
            }

            pub fn kind(&self) -> &'static str {
                match self {
                    $( Self::$variant(_) => $kind, )+
                    $( Self::$cvariant(_) => $ckind, )+
                }
            }

            pub fn name(&self) -> &str {
                self.metadata().name.as_deref().unwrap_or("<unnamed>")
            }

            pub fn namespace(&self) -> &str {
                match self {
                    $( Self::$variant(_) => {
                        self.metadata().namespace.as_deref().unwrap_or("default")
                    }, )+
                    $( Self::$cvariant(_) => "", )+
                }
            }

            pub fn uid(&self) -> String {
                format!("{}/{}/{}", self.kind(), self.namespace(), self.name())
            }

            pub fn from_yaml_value(
                value: serde_yaml::Value,
                kind: &str,
            ) -> anyhow::Result<Self> {
                match kind {
                    $(
                        $kind => Ok(Self::$variant(
                            serde_yaml::from_value(value)
                                .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", $kind, e))?
                        )),
                    )+
                    $(
                        $ckind => Ok(Self::$cvariant(
                            serde_yaml::from_value(value)
                                .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", $ckind, e))?
                        )),
                    )+
                    _ => anyhow::bail!("Unsupported resource kind: {}", kind),
                }
            }

            pub fn from_json_value(
                value: serde_json::Value,
                kind: &str,
            ) -> anyhow::Result<Self> {
                match kind {
                    $(
                        $kind => Ok(Self::$variant(serde_json::from_value(value)?)),
                    )+
                    $(
                        $ckind => Ok(Self::$cvariant(serde_json::from_value(value)?)),
                    )+
                    _ => anyhow::bail!("Unsupported kind: {}", kind),
                }
            }
        }
    };
}

define_any_resource! {
    namespaced: [
        Pod(Pod) as "Pod",
        Deployment(Deployment) as "Deployment",
        Service(Service) as "Service",
        ConfigMap(ConfigMap) as "ConfigMap",
        Secret(Secret) as "Secret",
        PersistentVolumeClaim(PersistentVolumeClaim) as "PersistentVolumeClaim",
        VNet(VNet) as "VNet",
        Subnet(Subnet) as "Subnet",
        Nsg(Nsg) as "NSG",
        RouteTable(RouteTable) as "RouteTable",
        Ingress(Ingress) as "Ingress",
        NetworkPolicy(NetworkPolicy) as "NetworkPolicy",
        Namespace(Namespace) as "Namespace",
        Node(Node) as "Node",
        Endpoints(Endpoints) as "Endpoints",
        EndpointSlice(EndpointSlice) as "EndpointSlice",
        Event(Event) as "Event",
        StorageClass(StorageClass) as "StorageClass",
        Role(Role) as "Role",
        RoleBinding(RoleBinding) as "RoleBinding",
        ServiceAccount(ServiceAccount) as "ServiceAccount",
    ];
    cluster_scoped: [
        PersistentVolume(PersistentVolume) as "PersistentVolume",
        ClusterRole(ClusterRole) as "ClusterRole",
        ClusterRoleBinding(ClusterRoleBinding) as "ClusterRoleBinding",
    ];
}
