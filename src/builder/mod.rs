pub mod container;
pub mod deployment;
pub mod manifest;
pub mod pod;

use std::collections::BTreeMap;

pub trait ResourceBuilder {
    type Output;
    fn build(&self) -> Self::Output;
    fn name(&self) -> &str;
    fn namespace(&self) -> &str;
    fn labels(&self) -> &BTreeMap<String, String>;
}
