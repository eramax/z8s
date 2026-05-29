use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VNet {
    #[serde(default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_vnet_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: VNetSpec,
    #[serde(default)]
    pub status: Option<VNetStatus>,
}

fn default_api_version() -> String { "z8s.io/v1".to_string() }
fn default_vnet_kind() -> String { "VNet".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VNetSpec {
    #[serde(default)]
    pub cidr: Option<String>,
    #[serde(default = "default_true")]
    pub internet_access: bool,
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_true() -> bool { true }
fn default_role() -> String { "spoke".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VNetStatus {
    pub cidr: String,
    #[serde(default)]
    pub pod_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subnet {
    #[serde(default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_subnet_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: SubnetSpec,
}

fn default_subnet_kind() -> String { "Subnet".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubnetSpec {
    pub vnet: String,
    pub cidr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nsg {
    #[serde(default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_nsg_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: NsgSpec,
}

fn default_nsg_kind() -> String { "NSG".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NsgSpec {
    pub target_vnets: Vec<String>,
    #[serde(default)]
    pub rules: Vec<NsgRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NsgRule {
    pub name: String,
    pub action: String,
    #[serde(default)]
    pub src_cidrs: Vec<String>,
    #[serde(default)]
    pub dst_cidrs: Vec<String>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub protocol: String,
    #[serde(default = "default_priority")]
    pub priority: u32,
}

fn default_priority() -> u32 { 1000 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hub {
    #[serde(default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_hub_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: HubSpec,
}

fn default_hub_kind() -> String { "Hub".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubSpec {
    pub vnet: String,
    #[serde(default)]
    pub spokes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spoke {
    #[serde(default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_spoke_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: SpokeSpec,
}

fn default_spoke_kind() -> String { "Spoke".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpokeSpec {
    pub vnet: String,
    pub hub: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTable {
    #[serde(default = "default_api_version")]
    pub api_version: String,
    #[serde(default = "default_routetable_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: RouteTableSpec,
}

fn default_routetable_kind() -> String { "RouteTable".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTableSpec {
    #[serde(default)]
    pub routes: Vec<RouteEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteEntry {
    pub destination: String,
    pub next_hop: String,
}
