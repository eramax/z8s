pub mod dns_stage;
pub mod service;

use crate::api::types::AnyResource;

pub trait NetworkResource {
    fn namespace(&self) -> &str;
    fn name(&self) -> &str;
    fn resource_ref(&self) -> &AnyResource;
}
