//! # Network Resources — Service, EndpointSlice, VNet, Subnet, NSG
//!
//! ## Service
//!
//! A Service exposes a set of Pods as a network endpoint.
//! Types: ClusterIP (internal), NodePort (external).
//!
//! ## EndpointSlice
//!
//! Tracks the IP addresses and ports of Pods matching a Service selector.
//!
//! ## VNet (Virtual Network)
//!
//! A VNet isolates a group of Pods into a virtual network with its own CIDR.
//!
//! ## Subnet
//!
//! A Subnet defines a CIDR range within a VNet.
//!
//! ## NSG (Network Security Group)
//!
//! An NSG defines allow/deny rules for traffic between subnets.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::meta::ObjectMeta;

// ── Service ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    #[serde(default = "default_svc_api_version")]
    pub api_version: String,
    #[serde(default = "default_svc_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<ServiceSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ServiceStatus>,
}

fn default_svc_api_version() -> String { "v1".into() }
fn default_svc_kind() -> String { "Service".into() }

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSpec {
    /// Service type: ClusterIP (default), NodePort, LoadBalancer.
    #[serde(rename = "type")]
    pub type_: Option<String>,
    /// ClusterIP address (auto-assigned or user-specified).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_ip: Option<String>,
    /// Selector for matching Pods.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<BTreeMap<String, String>>,
    /// Ports to expose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<ServicePort>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServicePort {
    pub name: String,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_balancer: Option<LoadBalancerStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct LoadBalancerStatus {}

// ── EndpointSlice ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSlice {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<Vec<Endpoint>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<EndpointPort>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<EndpointConditions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<ObjectReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointConditions {
    pub ready: bool,
    pub serving: bool,
    pub terminating: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct EndpointPort {
    pub name: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectReference {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    pub uid: Option<String>,
}

// ── VNet ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VNet {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: VNetSpec,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<VNetStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VNetSpec {
    pub cidr: Option<String>,
    pub internet_access: bool,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct VNetStatus {}

// ── Subnet ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Subnet {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: SubnetSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SubnetSpec {
    pub vnet: String,
    pub cidr: String,
}

// ── NSG ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Nsg {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: NsgSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NsgSpec {
    pub rules: Vec<NsgRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NsgRule {
    pub name: String,
    pub priority: i32,
    pub action: String,
    #[serde(rename = "srcCIDRs")]
    pub src_cidrs: Vec<String>,
    #[serde(rename = "dstCIDRs")]
    pub dst_cidrs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "srcPorts")]
    pub src_ports: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "dstPorts")]
    pub dst_ports: Option<Vec<String>>,
}

// ── NetworkPolicy ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicy {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<NetworkPolicySpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicySpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pod_selector: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingress: Option<Vec<NetworkPolicyIngressRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub egress: Option<Vec<NetworkPolicyEgressRule>>,
    #[serde(rename = "policyTypes")]
    pub policy_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NetworkPolicyIngressRule {}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NetworkPolicyEgressRule {}

// ── Ingress ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ingress {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<IngressSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<IngressStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct IngressSpec {}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct IngressStatus {}

// ── RouteTable ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteTable {
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: RouteTableSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RouteTableSpec {
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Route {
    pub dest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dev: Option<String>,
}
