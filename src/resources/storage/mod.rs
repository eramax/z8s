pub mod configmap;
pub mod pv;
pub mod pvc;
pub mod secret;

pub use pv::PvResource;
pub use pvc::PvcResource;

pub trait StorageResource {
    fn namespace(&self) -> &str;
    fn name(&self) -> &str;
}
