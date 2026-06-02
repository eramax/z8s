//! Service, Ingress, NetworkPolicy, Endpoints, and EndpointSlice types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{IntOrString, LabelSelector, ObjectMeta, ObjectReference, Time};

fn default_service_api_version() -> String { "v1".to_string() }
fn default_service_kind() -> String { "Service".to_string() }
fn default_ingress_api_version() -> String { "networking.k8s.io/v1".to_string() }
fn default_ingress_kind() -> String { "Ingress".to_string() }
fn default_networkpolicy_api_version() -> String { "networking.k8s.io/v1".to_string() }
fn default_networkpolicy_kind() -> String { "NetworkPolicy".to_string() }
fn default_endpoints_api_version() -> String { "v1".to_string() }
fn default_endpoints_kind() -> String { "Endpoints".to_string() }
fn default_endpointslice_api_version() -> String { "discovery.k8s.io/v1".to_string() }
fn default_endpointslice_kind() -> String { "EndpointSlice".to_string() }

// ── Service ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServicePort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_port: Option<i32>,
    pub port: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_port: Option<IntOrString>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocate_load_balancer_node_ports: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "clusterIP")]
    pub cluster_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "clusterIPs")]
    pub cluster_ips: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalIPs")]
    pub external_ips: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_traffic_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_node_port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_traffic_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_families: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_family_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_balancer_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "loadBalancerIP")]
    pub load_balancer_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_balancer_source_ranges: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<ServicePort>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_not_ready_addresses: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_affinity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_affinity_config: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_distribution: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_balancer: Option<LoadBalancerStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancerStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingress: Option<Vec<LoadBalancerIngress>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancerIngress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<PortStatus>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub port: i32,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    #[serde(rename = "apiVersion", default = "default_service_api_version")]
    pub api_version: String,
    #[serde(default = "default_service_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<ServiceSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ServiceStatus>,
}
// ── Ingress ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressTLS {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hosts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<HTTPIngressRuleValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPIngressRuleValue {
    pub paths: Vec<HTTPIngressPath>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPIngressPath {
    pub backend: IngressBackend,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressBackend {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<ObjectReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<IngressServiceBackend>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressServiceBackend {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<ServiceBackendPort>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceBackendPort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_backend: Option<IngressBackend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingress_class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<IngressRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<Vec<IngressTLS>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngressStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_balancer: Option<LoadBalancerStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ingress {
    #[serde(rename = "apiVersion", default = "default_ingress_api_version")]
    pub api_version: String,
    #[serde(default = "default_ingress_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<IngressSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<IngressStatus>,
}

// ── NetworkPolicy ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicyPeer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_block: Option<IPBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace_selector: Option<LabelSelector>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pod_selector: Option<LabelSelector>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IPBlock {
    pub cidr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub except: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicyPort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<IntOrString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicyIngressRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Vec<NetworkPolicyPeer>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<NetworkPolicyPort>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicyEgressRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<NetworkPolicyPort>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<NetworkPolicyPeer>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicySpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub egress: Option<Vec<NetworkPolicyEgressRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingress: Option<Vec<NetworkPolicyIngressRule>>,
    pub pod_selector: LabelSelector,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicy {
    #[serde(rename = "apiVersion", default = "default_networkpolicy_api_version")]
    pub api_version: String,
    #[serde(default = "default_networkpolicy_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<NetworkPolicySpec>,
}

// ── Endpoints ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointAddress {
    pub ip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<ObjectReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointPort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub port: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSubset {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<EndpointAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_ready_addresses: Option<Vec<EndpointAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<EndpointPort>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Endpoints {
    #[serde(rename = "apiVersion", default = "default_endpoints_api_version")]
    pub api_version: String,
    #[serde(default = "default_endpoints_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subsets: Option<Vec<EndpointSubset>>,
}

// ── EndpointSlice ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSliceEndpoint {
    pub addresses: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<EndpointSliceConditions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<ObjectReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSliceConditions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serving: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminating: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSlicePort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSlice {
    #[serde(rename = "apiVersion", default = "default_endpointslice_api_version")]
    pub api_version: String,
    #[serde(default = "default_endpointslice_kind")]
    pub kind: String,
    pub address_type: String,
    pub endpoints: Vec<EndpointSliceEndpoint>,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<EndpointSlicePort>>,
}

