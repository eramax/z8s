use crate::api::auth::{self, api_group_for_resource, verbs_for_http, AuthzRequest, TokenRegistry};
use crate::api::server::*;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

fn is_exempt_path(uri: &str) -> bool {
    matches!(
        uri,
        "/healthz" | "/readyz" | "/livez" | "/version" | "/openapi/v2" | "/openapi/v3"
    ) || uri.starts_with("/openapi/")
}

pub async fn authorize_middleware_with_store(
    headers: HeaderMap,
    request: Request,
    next: Next,
    store: Arc<dyn StoreBackend>,
    tokens: Arc<TokenRegistry>,
) -> Result<Response, StatusCode> {
    let method = request.method().as_str().to_string();
    let uri = request.uri().path().to_string();

    if is_exempt_path(&uri) {
        return Ok(next.run(request).await);
    }

    if !crate::config::rbac_enforced() {
        return Ok(next.run(request).await);
    }

    if !auth::has_any_rbac_policy(store.as_ref()).await {
        return Ok(next.run(request).await);
    }

    let Some((resource, namespace, name)) = crate::api::catalog::authz_from_path(&uri) else {
        return Ok(next.run(request).await);
    };

    let user = auth::extract_user(&headers, Some(tokens.as_ref())).await;
    let api_group = crate::api::catalog::rbac_api_group_for_plural(resource);
    let api_group = if api_group.is_empty() {
        api_group_for_resource(resource)
    } else {
        api_group
    };
    let verbs = verbs_for_http(&method, &uri);

    for verb in verbs {
        let req = AuthzRequest {
            user: &user,
            namespace,
            resource,
            verb,
            api_group,
            name,
        };
        if auth::authorize(store.as_ref(), &req).await {
            return Ok(next.run(request).await);
        }
    }

    tracing::warn!(
        "RBAC denied: user='{}' uri='{}' resource='{}' ns='{}'",
        user,
        uri,
        resource,
        namespace
    );
    Err(StatusCode::FORBIDDEN)
}

pub async fn authorize(
    store: &dyn StoreBackend,
    user: &str,
    namespace: &str,
    resource: &str,
    verb: &str,
) -> bool {
    let req = AuthzRequest {
        user,
        namespace,
        resource,
        verb,
        api_group: api_group_for_resource(resource),
        name: None,
    };
    auth::authorize(store, &req).await
}
