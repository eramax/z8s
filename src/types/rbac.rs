//! RBAC and ServiceAccount types.

use serde::{Deserialize, Serialize};

use super::ObjectMeta;
use super::ObjectReference;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PolicyRule {
    #[serde(default)]
    pub api_groups: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub resource_names: Vec<String>,
    #[serde(default)]
    pub verbs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Role {
    #[serde(rename = "apiVersion", default = "default_role_api_version")]
    pub api_version: String,
    #[serde(default = "default_role_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

fn default_role_api_version() -> String {
    "rbac.authorization.k8s.io/v1".to_string()
}
fn default_role_kind() -> String {
    "Role".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoleRef {
    pub api_group: String,
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Subject {
    #[serde(default = "default_subject_kind")]
    pub kind: String,
    #[serde(default)]
    pub namespace: String,
    pub name: String,
}

fn default_subject_kind() -> String {
    "ServiceAccount".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoleBinding {
    #[serde(rename = "apiVersion", default = "default_role_api_version")]
    pub api_version: String,
    #[serde(default = "default_rolebinding_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(default)]
    pub subjects: Vec<Subject>,
    pub role_ref: RoleRef,
}

fn default_rolebinding_kind() -> String {
    "RoleBinding".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RoleList {
    pub items: Vec<Role>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RoleBindingList {
    pub items: Vec<RoleBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClusterRole {
    #[serde(rename = "apiVersion", default = "default_role_api_version")]
    pub api_version: String,
    #[serde(default = "default_clusterrole_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

fn default_clusterrole_kind() -> String {
    "ClusterRole".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterRoleBinding {
    #[serde(rename = "apiVersion", default = "default_role_api_version")]
    pub api_version: String,
    #[serde(default = "default_clusterrolebinding_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(default)]
    pub subjects: Vec<Subject>,
    pub role_ref: RoleRef,
}

fn default_clusterrolebinding_kind() -> String {
    "ClusterRoleBinding".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceAccount {
    #[serde(rename = "apiVersion", default = "default_sa_api_version")]
    pub api_version: String,
    #[serde(default = "default_sa_kind")]
    pub kind: String,
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<Vec<ObjectReference>>,
}

fn default_sa_api_version() -> String {
    "v1".to_string()
}
fn default_sa_kind() -> String {
    "ServiceAccount".to_string()
}
