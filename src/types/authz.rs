//! Authorization API types (SelfSubjectAccessReview).

use serde::{Deserialize, Serialize};

use super::ObjectMeta;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SelfSubjectAccessReview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ObjectMeta>,
    pub spec: SelfSubjectAccessReviewSpec,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<SubjectAccessReviewStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SelfSubjectAccessReviewSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_attributes: Option<ResourceAttributes>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceAttributes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subresource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubjectAccessReviewStatus {
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denied: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evaluation_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
