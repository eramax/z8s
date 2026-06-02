//! Compat CRUD routes driven by [`catalog::COMPAT_MOUNTS`] (A2).

use axum::Router;
use axum::routing::{MethodRouter, get};

use crate::api::catalog::{CompatMount, COMPAT_MOUNTS};
use crate::api::resource_handler;
use crate::api::server::AppState;

/// Build catalog-backed compat routes (`RouterBuilder::from_catalog` in the plan).
pub fn from_catalog() -> Router<AppState> {
    let namespaced_item = get(resource_handler::get_namespaced)
        .put(resource_handler::update_namespaced)
        .patch(resource_handler::update_namespaced)
        .delete(resource_handler::delete_namespaced);
    let namespaced_collection = get(resource_handler::list_namespaced)
        .post(resource_handler::create_namespaced);
    let cluster_item =
        get(resource_handler::get_cluster).delete(resource_handler::delete_cluster);
    let cluster_collection =
        get(resource_handler::list_cluster).post(resource_handler::create_cluster);
    let cluster_list = get(resource_handler::list_v1_plural);
    let cluster_list_create = get(resource_handler::list_v1_plural)
        .post(resource_handler::create_v1_plural);

    let mut router = Router::new();
    for mount in COMPAT_MOUNTS {
        router = mount_compat(
            router,
            mount,
            namespaced_collection.clone(),
            namespaced_item.clone(),
            cluster_list.clone(),
            cluster_list_create.clone(),
            cluster_item.clone(),
        );
    }

    router
        .route(
            "/apis/storage.k8s.io/v1/storageclasses",
            cluster_collection.clone(),
        )
        .route(
            "/apis/storage.k8s.io/v1/storageclasses/{name}",
            cluster_item.clone(),
        )
        .route(
            "/apis/rbac.authorization.k8s.io/v1/{plural}",
            cluster_list_create.clone(),
        )
        .route(
            "/apis/rbac.authorization.k8s.io/v1/{plural}/{name}",
            cluster_item.clone(),
        )
        .route(
            "/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/{plural}",
            namespaced_collection,
        )
        .route(
            "/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/{plural}/{name}",
            namespaced_item,
        )
}

/// Legacy name — prefer [`from_catalog`].
pub fn routes() -> Router<AppState> {
    from_catalog()
}

fn mount_compat(
    router: Router<AppState>,
    mount: &'static CompatMount,
    namespaced_collection: MethodRouter<AppState>,
    namespaced_item: MethodRouter<AppState>,
    cluster_list: MethodRouter<AppState>,
    cluster_list_create: MethodRouter<AppState>,
    cluster_item: MethodRouter<AppState>,
) -> Router<AppState> {
    let f = mount.flags;
    match mount.prefix {
        "/api/v1" => {
            let mut r = router;
            if f.cluster_list {
                let coll = if f.cluster_create {
                    cluster_list_create
                } else {
                    cluster_list
                };
                r = r.route("/api/v1/{plural}", coll);
            }
            if f.cluster_item {
                r = r.route("/api/v1/{plural}/{name}", cluster_item);
            }
            if f.namespaced {
                r = r
                    .route(
                        "/api/v1/namespaces/{namespace}/{plural}",
                        namespaced_collection,
                    )
                    .route(
                        "/api/v1/namespaces/{namespace}/{plural}/{name}",
                        namespaced_item,
                    );
            }
            r
        }
        "/apis/z8s.io/v1" => {
            let mut r = router;
            if f.cluster_list {
                let coll = if f.cluster_create {
                    cluster_list_create
                } else {
                    cluster_list
                };
                r = r.route("/apis/z8s.io/v1/{plural}", coll);
            }
            if f.cluster_item {
                r = r.route("/apis/z8s.io/v1/{plural}/{name}", cluster_item);
            }
            r
        }
        "/apis/apps/v1" => {
            let mut r = router;
            if f.cluster_list {
                r = r.route("/apis/apps/v1/{plural}", cluster_list);
            }
            if f.namespaced {
                r = r
                    .route(
                        "/apis/apps/v1/namespaces/{namespace}/{plural}",
                        namespaced_collection,
                    )
                    .route(
                        "/apis/apps/v1/namespaces/{namespace}/{plural}/{name}",
                        namespaced_item,
                    );
            }
            r
        }
        "/apis/networking.k8s.io/v1" => router
            .route(
                "/apis/networking.k8s.io/v1/namespaces/{namespace}/{plural}",
                namespaced_collection,
            )
            .route(
                "/apis/networking.k8s.io/v1/namespaces/{namespace}/{plural}/{name}",
                namespaced_item,
            ),
        "/apis/discovery.k8s.io/v1" => router
            .route(
                "/apis/discovery.k8s.io/v1/namespaces/{namespace}/{plural}",
                namespaced_collection,
            )
            .route(
                "/apis/discovery.k8s.io/v1/namespaces/{namespace}/{plural}/{name}",
                namespaced_item,
            ),
        _ => router,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::catalog::COMPAT_MOUNTS;

    #[test]
    fn compat_mount_prefixes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for m in COMPAT_MOUNTS {
            assert!(seen.insert(m.prefix), "duplicate mount {}", m.prefix);
        }
    }

    #[test]
    fn from_catalog_builds_router() {
        let _router: Router<AppState> = from_catalog();
    }
}
