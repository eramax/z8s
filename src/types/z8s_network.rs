//! z8s.io networking resources (VNet, Subnet, NSG, RouteTable).

use serde::{Deserialize, Serialize};

use super::ObjectMeta;

fn default_api_version() -> String {
    "z8s.io/v1".to_string()
}
fn default_vnet_kind() -> String {
    "VNet".to_string()
}
fn default_subnet_kind() -> String {
    "Subnet".to_string()
}
fn default_nsg_kind() -> String {
    "NSG".to_string()
}
fn default_routetable_kind() -> String {
    "RouteTable".to_string()
}
fn default_true() -> bool {
    true
}
fn default_role() -> String {
    "spoke".to_string()
}
fn default_priority() -> u32 {
    1000
}
fn default_header_operator() -> String {
    "eq".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VNet {
    #[serde(rename = "apiVersion", default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_vnet_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: VNetSpec,
    #[serde(default)]
    pub status: Option<VNetStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VNetSpec {
    #[serde(default)]
    pub cidr: Option<String>,
    #[serde(default = "default_true")]
    pub internet_access: bool,
    #[serde(default = "default_role")]
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct VNetStatus {
    pub cidr: String,
    #[serde(default)]
    pub pod_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Subnet {
    #[serde(rename = "apiVersion", default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_subnet_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: SubnetSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubnetSpec {
    pub vnet: String,
    pub cidr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Nsg {
    #[serde(rename = "apiVersion", default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_nsg_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: NsgSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NsgSpec {
    pub target_vnets: Vec<String>,
    #[serde(default)]
    pub rules: Vec<NsgRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NsgRule {
    pub name: String,
    pub action: String,
    #[serde(alias = "src_cidrs", default)]
    pub srcCIDRs: Vec<String>,
    #[serde(alias = "dst_cidrs", default)]
    pub dstCIDRs: Vec<String>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub protocol: String,
    #[serde(default = "default_priority")]
    pub priority: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RouteTable {
    #[serde(rename = "apiVersion", default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_routetable_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: RouteTableSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RouteTableSpec {
    #[serde(default)]
    pub rules: Vec<RouteRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RouteRule {
    pub name: String,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub headers: Vec<HeaderMatch>,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct HeaderMatch {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_header_operator")]
    pub operator: String,
}
