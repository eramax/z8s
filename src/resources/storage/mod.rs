pub mod configmap;
pub mod pv;
pub mod pvc;
pub mod secret;


pub trait StorageResource {
    fn namespace(&self) -> &str;
    fn name(&self) -> &str;
}
