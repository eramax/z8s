//! Kubernetes subresources (log, exec, scale) — exceptions to generic catalog CRUD.

pub mod deployment_scale;
pub mod pod;

pub fn routes() -> axum::Router<crate::api::server::AppState> {
    deployment_scale::routes().merge(pod::routes())
}
