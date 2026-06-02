# Module Plan: RBAC & in-cluster API access

> Part of [00-overview.md](./00-overview.md) · Current ~320 LOC (`api/handlers/rbac.rs`) — incomplete vs production needs

## Goal

**Pods (and humans) call the z8s API** — `GET` pods, `kubectl apply`, list services, patch deployments — and every request is **allowed or denied by Role / ClusterRole bindings**, not by “GET is always open” or “no bindings = allow all”.

Works with the unified **`z8s.io/v1` catalog** ([01-api.md](./01-api.md)): one authz engine, all plurals, all compat paths.

## Current state (gaps)

| Issue | Today | Risk |
|-------|--------|------|
| **Read bypass** | `GET`/`HEAD` never checked | Any bearer string can list secrets |
| **Open default** | No `RoleBinding` → allow all mutating | Misconfiguration exposes cluster |
| **`api_groups` ignored** | Only `resources` + `verbs` matched | Rules don’t match real PolicyRule shape |
| **`resource_names` ignored** | — | No per-object grants |
| **No ClusterRole** | Namespace `Role` only | Can’t model cluster-admin / node reader |
| **Broken role lookup** | `role_map.get(&(role_ref.name, namespace))` | Wrong key order vs insert `(ns, name)` |
| **Identity** | Bearer token = raw username string | Pods have no real ServiceAccount token |
| **SSAR** | Always `allowed: true` | `kubectl auth can-i` lies |
| **URI map** | Hand-rolled `uri_to_resource` | Incomplete; won’t scale to catalog |
| **List/watch verb** | `GET` mapped only to `get` | `list` / `watch` never enforced |
| **Subjects** | `User`, partial `ServiceAccount` | No `Group`; SA namespace often wrong |

## Target model (Kubernetes-compatible + z8s-native)

### Resources (all `z8s.io/v1` in store; compat paths on wire)

| Kind | Scope | Purpose |
|------|-------|---------|
| `Role` | namespaced | Policy rules in a namespace |
| `RoleBinding` | namespaced | Bind subjects → Role |
| `ClusterRole` | cluster | Cluster-wide rules |
| `ClusterRoleBinding` | cluster | Bind subjects → ClusterRole |
| `ServiceAccount` | namespaced | Identity for pods |

Normalize upstream `rbac.authorization.k8s.io/v1` YAML on admission to `z8s.io/v1` (same as other kinds).

### PolicyRule evaluation (complete)

```rust
pub struct AuthzRequest {
    pub subject: SubjectIdentity,
    pub verb: Verb,           // get, list, watch, create, update, patch, delete, apply
    pub api_group: String,    // "" for core, "z8s.io", "apps"
    pub resource: String,     // plural: pods, deployments, vnets
    pub namespace: Option<String>,
    pub name: Option<String>, // resourceNames constraint
    pub subresource: Option<String>, // log, exec, scale, status
}

pub fn rule_allows(rule: &PolicyRule, req: &AuthzRequest) -> bool {
    verbs_match(&rule.verbs, req.verb)
        && api_groups_match(&rule.api_groups, &req.api_group)
        && resources_match(&rule.resources, &req.resource, req.subresource.as_deref())
        && resource_names_match(&rule.resource_names, req.name.as_deref())
}
```

Verb mapping from HTTP + path:

| Request | verb |
|---------|------|
| `GET` collection | `list` |
| `GET` with `?watch=1` | `watch` |
| `GET` `/pods/{name}` | `get` |
| `POST` collection | `create` |
| `PUT`/`PATCH` | `update` / `patch` |
| `DELETE` | `delete` |
| Multi-doc YAML POST (apply) | `create` or `patch` per object |

### Default posture: **deny**

```text
if !authz_engine.has_any_policy() {
    // bootstrap only: allow loopback / unix admin socket
} else {
    require explicit allow
}
```

Optional config flag `--rbac-mode=permissive` for dev (log warning): same as today when no bindings.

## Pod identity: calling z8s from inside a workload

### 1. ServiceAccount on Pod spec

Standard field (already in Pod types):

```yaml
spec:
  serviceAccountName: ci-runner
```

CRI / admission ensures SA exists in namespace (create `default` if missing).

### 2. Token issuance (new: `auth/token.rs` ~100 LOC)

On pod start (leader stores tokens in redb; replicated via gossip optional):

```text
system:serviceaccount:default:ci-runner
  → opaque token (32 bytes) or HS256 JWT
  → mounted at /var/run/secrets/z8s.io/serviceaccount/token
```

Also write:

| File | Content |
|------|---------|
| `namespace` | `default` |
| `name` | `ci-runner` |
| `ca.crt` | optional for TLS |
| `cluster` | `http://10.43.0.1:6443` (ClusterIP of `z8s-api` EdgeService) |

**Request auth:**

```http
Authorization: Bearer <token>
```

Server resolves token → `SubjectIdentity::ServiceAccount { namespace, name }` → canonical user string `system:serviceaccount:default:ci-runner` for binding match.

Rotate token on pod restart; revoke on pod delete.

### 3. In-cluster API endpoint

Planner / bootstrap creates **`z8s.io/v1` EdgeService**:

```yaml
kind: EdgeService
metadata:
  name: kubernetes   # optional alias for compat
  namespace: default
spec:
  selector:
    z8s.io/component: apiserver
  ports:
    - port: 443
      targetPort: 6443
  exposure:
    mode: ClusterIP
```

DNS (existing netmux DNS): `kubernetes.default.svc.cluster.local` → ClusterIP (kubectl-from-pod compat).

Document in README:

```bash
# inside pod
curl -H "Authorization: Bearer $(cat /var/run/secrets/z8s.io/serviceaccount/token)" \
  http://kubernetes.default.svc.cluster.local/api/v1/namespaces/default/pods
kubectl apply -f manifest.yaml --server=... --token=...
```

### 4. Example: CI pod that lists and applies

```yaml
apiVersion: z8s.io/v1
kind: Role
metadata:
  name: deployer
  namespace: default
rules:
  - apiGroups: ["z8s.io", ""]
    resources: ["pods", "deployments"]
    verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
  - apiGroups: ["z8s.io"]
    resources: ["edgeservices"]
    verbs: ["get", "list"]
---
apiVersion: z8s.io/v1
kind: RoleBinding
metadata:
  name: ci-deployer
  namespace: default
subjects:
  - kind: ServiceAccount
    name: ci-runner
    namespace: default
roleRef:
  kind: Role
  name: deployer
  apiGroup: z8s.io
---
apiVersion: v1
kind: Pod
metadata:
  name: ci
spec:
  serviceAccountName: ci-runner
  containers:
    - name: kubectl
      image: bitnami/kubectl
      command: ["sleep", "infinity"]
```

Read-only observer role (no create/patch):

```yaml
rules:
  - apiGroups: ["z8s.io", ""]
    resources: ["pods", "events"]
    verbs: ["get", "list", "watch"]
```

## Authz engine architecture

```
Request
   │
   ▼
┌─────────────┐
│ Authn       │  Bearer / SA token → SubjectIdentity
└──────┬──────┘
       ▼
┌─────────────┐
│ PathResolve │  catalog::resolve(uri) → AuthzRequest skeleton
└──────┬──────┘
       ▼
┌─────────────┐
│ AuthzEngine │  bindings index → rules → allow/deny
└──────┬──────┘
       ▼
   handler
```

### `AuthzEngine` (~150 LOC)

- **Index** (rebuilt on Role/Binding change, not per request):
  - `by_sa: (ns, name) → Vec<CompiledRule>`
  - `by_user: name → Vec<CompiledRule>`
  - `by_group: name → Vec<CompiledRule>`
- **ClusterRoleBinding** applies cluster rules + namespaced rules where `rules` include namespace via RoleBinding only for namespaced resources.
- **Aggregation**: union of all matching bindings; deny wins optional (Kubernetes: deny not in Role; use separate deny roles later).

### Middleware (replace `authorize_middleware_with_store`)

```rust
pub async fn rbac_middleware(req: Request, next: Next, state: AuthState) -> Result<Response, StatusCode> {
    if is_exempt(&req) { return Ok(next.run(req).await); }  // /healthz, /version only

    let subject = state.authn.authenticate(req.headers()).await?;
    let authz_req = state.resolver.from_request(&req)?;
    if !state.engine.decide(&subject, &authz_req).await {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(req).await)
}
```

Exempt: `/healthz`, `/readyz`, `/livez`, `/version` (not `/api`).

Enforce on **all** methods including GET and WebSocket upgrade (exec needs `create` on `pods/exec` subresource).

### SelfSubjectAccessReview / SubjectAccessReview

Implement real checks:

```rust
pub async fn self_subject_access_review(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SelfSubjectAccessReview>,
) -> Json<SelfSubjectAccessReview> {
    let subject = s.authn.authenticate(&headers).await?;
    let req = body.spec.into_authz_request();
    let allowed = s.engine.decide(&subject, &req).await;
    // populate status.allowed, status.reason
}
```

Enables from pod:

```bash
kubectl auth can-i create deployments --namespace=default
```

### Catalog integration

Replace `uri_to_resource` with:

```rust
impl ResourceCatalog {
    pub fn authz_from_path(method: &Method, path: &str, query: &str) -> Option<AuthzRequest> { ... }
}
```

All `z8s.io/v1` plurals + compat aliases resolve to the same `resource` string used in `PolicyRule.resources`.

**PolicyRule.resources** accepts:

- plural (`pods`) and singular alias (`pod`)
- `*` for all resources in matched apiGroups
- subresources: `pods/log`, `pods/exec` or `pods` + verb `get` with subresource `log` (pick one style, document)

**PolicyRule.api_groups**:

- `""` or `"*"` → core compat (`v1`)
- `"z8s.io"` → all z8s kinds
- `"apps"` → deployments (if still compat-shimmed)

## Bootstrap & operations

### Cluster bootstrap roles (manifests in `/etc/z8s/manifests/rbac/`)

| Object | Purpose |
|--------|---------|
| `ClusterRole/cluster-admin` | `*` verbs on `*` resources |
| `ClusterRole/view` | get, list, watch |
| `ClusterRole/edit` | view + write namespaced |
| `ClusterRoleBinding/cluster-admin` | Subject: User `admin` or break-glass token |
| `RoleBinding/default-view` | Optional: default SA read-only |

### Human admin (off-cluster)

- `kubectl` with `--token` or client cert (future)
- Or `X-Remote-User: admin` behind reverse proxy
- Bind `User/Group` in ClusterRoleBinding

### Pod workload (in-cluster)

- Only **ServiceAccount** tokens mounted by CRI
- Permissions **only** via RoleBinding / ClusterRoleBinding

### Node join tokens (not API RBAC)

Cluster **join tokens** (`z8s.jt.*`) are separate from ServiceAccount API tokens:

| | Join token | ServiceAccount token |
|---|------------|----------------------|
| **Issued by** | `z8s node <name> token` on main ([06-store-sync.md](./06-store-sync.md)) | CRI on pod start |
| **Used for** | `z8s join` / gossip WebSocket `Authorization: Bearer` | HTTP API to z8s |
| **Checked by** | `ws` handshake on main | RBAC `AuthzEngine` |
| **Stored as** | `join_tokens/{name}` hash in cluster redb | SA secret file in pod |

Never use a join token with `kubectl`; never use an SA token for gossip join.

## Apply YAML from pods

Multi-document POST / compat `kubectl apply`:

1. Parse each document → `AnyResource`
2. For each: `admission::normalize` + `authz.decide(subject, create|patch, …)`
3. If any denied → `403` with body listing failed GVK/name (partial apply optional: deny all by default)
4. All allowed → `apply_batch` to store

Verb selection:

- URL exists + same resourceVersion → `patch` or `update`
- else → `create`

## LOC budget

| Component | Target LOC |
|-----------|------------|
| `auth/authn.rs` (token + headers) | 100 |
| `auth/authz.rs` (engine + rule match) | 150 |
| `auth/index.rs` (binding index) | 80 |
| `auth/middleware.rs` | 40 |
| `auth/ssar.rs` | 50 |
| `cri/sa_mount.rs` (token mount) | 60 |
| RBAC kinds in catalog (CRUD via generic handler) | 0 extra |
| **Total new auth/** | **~480** |

Remove duplicate logic from `handlers/rbac.rs`; keep thin route registration.

## Implementation phases

| Phase | Deliverable |
|-------|-------------|
| **R1** | Fix role lookup bug; match `api_groups`; deny-by-default when bindings exist |
| **R2** | Enforce GET (list/get/watch); catalog path resolver |
| **R3** | ClusterRole + ClusterRoleBinding kinds + index |
| **R4** | ServiceAccount kind + token issue/revoke on pod lifecycle |
| **R5** | Real SSAR; `kubectl auth can-i` from pod |
| **R6** | Multi-doc apply authz; exec/log subresource rules |
| **R7** | Bootstrap manifests + `z8s-api` ClusterIP service |
| **R8** | **cluster-dashboard E2E** — SA token + Role allows deploy; in-pod `kubectl get pods` passes |

## Testing

### Unit & integration (baseline)

- Unit: `rule_allows` matrix (verb, apiGroup, resource, resourceNames)
- Integration ([tests/test_rbac.sh](../../tests/test_rbac.sh) today — curl + `X-Remote-User`):
  - SA `deploy-bot` + `pod-viewer` → GET/LIST pods OK, CREATE/DELETE forbidden
  - Upgrade to `pod-manager` → CREATE/DELETE pods OK
  - No binding → mutating calls 403 (once deny-by-default is on)
- SSAR matches middleware decision (after **R5**)

### E2E: `cluster-dashboard` + in-pod `kubectl` (required)

**Goal:** Prove RBAC allows a real workload that talks to the z8s API via **ServiceAccount token** (not `X-Remote-User`), runs **`kubectl`**, and can deploy the **cluster-dashboard** stack.

**Fixture:** `tests/rbac/cluster-dashboard-stack.yaml` (consolidate from [tests/13-deployment-info.yaml](../../tests/13-deployment-info.yaml) dashboard section + RBAC objects below).

**Script:** `tests/test-rbac-cluster-dashboard.sh` — add to `run-tests.sh` after RBAC phases **R4–R5** (SA token mount + real Bearer auth).

#### Manifests (apply order)

**1. ServiceAccount**

```yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: cluster-dashboard
  namespace: default
```

**2. Role `cluster-dashboard-operator`** — enough for dashboard UI + `kubectl get/describe` cluster state

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cluster-dashboard-operator
  namespace: default
rules:
  - apiGroups: ["", "apps", "z8s.io"]
    resources:
      - pods
      - deployments
      - services
      - replicasets
      - events
    verbs: ["get", "list", "watch"]
  # If dashboard runs kubectl apply for demos, add create/patch (narrow):
  - apiGroups: ["apps"]
    resources: ["deployments"]
    verbs: ["get", "list", "watch", "create", "update", "patch"]
  - apiGroups: [""]
    resources: ["pods", "services"]
    verbs: ["get", "list", "watch", "create", "update", "patch"]
```

Tune verbs to **minimum** the image needs after inspecting `eramax/cluster-dashboard` — start read-only in CI, widen only if tests fail.

**3. RoleBinding**

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cluster-dashboard-operator
  namespace: default
subjects:
  - kind: ServiceAccount
    name: cluster-dashboard
    namespace: default
roleRef:
  kind: Role
  name: cluster-dashboard-operator
  apiGroup: rbac.authorization.k8s.io
```

**4. Deployment + Service** (user workload)

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: cluster-dashboard
  namespace: default
  labels:
    app: cluster-dashboard
    type: info-deploy
spec:
  replicas: 1
  selector:
    matchLabels:
      app: cluster-dashboard
  template:
    metadata:
      labels:
        app: cluster-dashboard
    spec:
      serviceAccountName: cluster-dashboard
      containers:
        - name: dashboard
          image: eramax/cluster-dashboard:latest
          ports:
            - containerPort: 80
          env:
            # z8s in-cluster API (after EdgeService kubernetes.default — 04-netmux / 07 R7)
            - name: KUBERNETES_SERVICE_HOST
              value: kubernetes.default.svc.cluster.local
            - name: KUBERNETES_SERVICE_PORT
              value: "443"
            # kubectl in image uses mounted SA token (R4):
            # /var/run/secrets/z8s.io/serviceaccount/token
            - name: KUBECONFIG
              value: /var/run/secrets/z8s.io/serviceaccount/kubeconfig
---
apiVersion: v1
kind: Service
metadata:
  name: cluster-dashboard-svc
  namespace: default
  labels:
    app: cluster-dashboard
spec:
  type: ClusterIP
  selector:
    app: cluster-dashboard
  ports:
    - name: http
      port: 80
      targetPort: 80
      protocol: TCP
```

**CRI responsibility ([02-cri.md](./02-cri.md), **R4**): when `serviceAccountName` is set, mount:

| Path | Content |
|------|---------|
| `token` | SA bearer token |
| `namespace`, `name` | SA identity |
| `kubeconfig` | minimal kubeconfig pointing at `https://kubernetes.default.svc.cluster.local` + `token` file |

Image `eramax/cluster-dashboard:latest` expects standard in-cluster config so **`kubectl get pods -A`** works without host `kubectl` kubeconfig.

#### Test script assertions

`tests/test-rbac-cluster-dashboard.sh`:

| Step | Action | Pass condition |
|------|--------|----------------|
| 0 | `kubectl apply -f tests/rbac/cluster-dashboard-stack.yaml` | SA, Role, Binding, Deployment, Service created |
| 1 | Wait Deployment `cluster-dashboard` Ready | `kubectl wait --for=condition=Available` |
| 2 | Negative (optional pre-check) | Pod **without** SA or wrong Role → `kubectl exec … kubectl get pods` fails or API 403 |
| 3 | `kubectl exec deploy/cluster-dashboard -c dashboard -- kubectl get pods -n default` | exit 0, lists pods |
| 4 | `kubectl exec … -- kubectl get deployments -n default` | includes `cluster-dashboard` |
| 5 | `kubectl exec … -- kubectl auth can-i list pods --namespace=default` | `yes` (after **R5** SSAR) |
| 6 | HTTP | `kubectl run curl --rm -it --image=curlimages/curl -- curl -s cluster-dashboard-svc.default.svc.cluster.local` or port-forward → dashboard UI responds (optional) |
| 7 | Deny test | Second SA `no-access` + busybox → `kubectl get pods` **fails** 403 |
| 8 | Cleanup | delete stack |

**Prerequisites:** z8s main running with RBAC enforced on GET+mutate; in-cluster API DNS; scheduler started dashboard pod ([05-scheduler.md](./05-scheduler.md)).

**CI gate:** `FAIL=0` on this script is required before marking RBAC phase complete.

#### Negative: Role too weak

Apply Deployment **without** RoleBinding → pod runs but:

```bash
kubectl exec deploy/cluster-dashboard -- kubectl get pods
# expect: Forbidden / connection error / exit != 0
```

Proves enforcement is on **API token**, not only admission.

#### Relation to existing tests

| Asset | Role |
|-------|------|
| [tests/test-cluster-dashboard.sh](../../tests/test-cluster-dashboard.sh) | Image/port probe only — **no RBAC** |
| [tests/13-deployment-info.yaml](../../tests/13-deployment-info.yaml) | Contains dashboard Deployment/Service — move RBAC into `tests/rbac/` |
| [tests/test_rbac.sh](../../tests/test_rbac.sh) | Header-based SA simulation — keep; dashboard test uses **real mounted token** |

### Implementation phase add-on

| Phase | Deliverable |
|-------|-------------|
| **R8** | `tests/rbac/cluster-dashboard-stack.yaml` + `test-rbac-cluster-dashboard.sh`; CRI `kubeconfig` generator; wire `run-tests.sh` |

## Security notes

- Tokens are secrets — file mode `0400`, memory zeroize on revoke
- Exec subresource must require explicit `pods/exec` **create** (or `*` with care)
- Secrets: separate rule `resources: ["secrets"]` — never include in default view role
- Rate-limit auth failures per source IP (optional, netmux or middleware)

---

*Previous: [06-store-sync.md](./06-store-sync.md) · Index: [00-overview.md](./00-overview.md)*
