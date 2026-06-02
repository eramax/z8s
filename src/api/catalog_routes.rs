//! Routes generated from the resource catalog (A2).

use axum::Router;
use axum::routing::{delete, get, patch, post, put};

use crate::api::resource_handler;
use crate::api::server::AppState;

pub fn routes() -> Router<AppState> {
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

    Router::new()
        // z8s.io cluster-scoped (vnets enriched in enrich.rs; subnets/nsgs/routetables generic)
        .route("/apis/z8s.io/v1/vnets", cluster_collection.clone())
        .route("/apis/z8s.io/v1/vnets/{name}", cluster_item.clone())
        .route("/apis/z8s.io/v1/subnets", cluster_collection.clone())
        .route("/apis/z8s.io/v1/subnets/{name}", cluster_item.clone())
        .route("/apis/z8s.io/v1/nsgs", cluster_collection.clone())
        .route("/apis/z8s.io/v1/nsgs/{name}", cluster_item.clone())
        .route("/apis/z8s.io/v1/routetables", cluster_collection.clone())
        .route("/apis/z8s.io/v1/routetables/{name}", cluster_item.clone())
        .route(
            "/api/v1/{plural}",
            get(resource_handler::list_v1_plural).post(resource_handler::create_v1_plural),
        )
        .route(
            "/api/v1/{plural}/{name}",
            get(resource_handler::get_cluster).delete(resource_handler::delete_cluster),
        )
        .route(
            "/api/v1/namespaces/{namespace}/{plural}",
            namespaced_collection.clone(),
        )
        .route(
            "/api/v1/namespaces/{namespace}/{plural}/{name}",
            namespaced_item.clone(),
        )
        .route(
            "/apis/apps/v1/{plural}",
            get(resource_handler::list_v1_plural),
        )
        .route(
            "/apis/apps/v1/namespaces/{namespace}/{plural}",
            namespaced_collection.clone(),
        )
        .route(
            "/apis/apps/v1/namespaces/{namespace}/{plural}/{name}",
            namespaced_item.clone(),
        )
        .route(
            "/apis/networking.k8s.io/v1/namespaces/{namespace}/{plural}",
            namespaced_collection.clone(),
        )
        .route(
            "/apis/networking.k8s.io/v1/namespaces/{namespace}/{plural}/{name}",
            namespaced_item.clone(),
        )
        .route(
            "/apis/discovery.k8s.io/v1/namespaces/{namespace}/{plural}",
            namespaced_collection.clone(),
        )
        .route(
            "/apis/discovery.k8s.io/v1/namespaces/{namespace}/{plural}/{name}",
            namespaced_item.clone(),
        )
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
            get(resource_handler::list_v1_plural).post(resource_handler::create_v1_plural),
        )
        .route(
            "/apis/rbac.authorization.k8s.io/v1/{plural}/{name}",
            get(resource_handler::get_cluster).delete(resource_handler::delete_cluster),
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
