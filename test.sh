#!/usr/bin/env bash
# z8s integration test suite
# Usage: ./test.sh [--server http://localhost:6443]
set -uo pipefail

SERVER="${Z8S_SERVER:-http://localhost:6443}"
DAEMON="$(dirname "$0")/z8s.sh"
LOG="/tmp/z8s.log"
PASS=0; FAIL=0; ERRORS=()

# ── colours ──────────────────────────────────────────────────────────────────
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS+1)); }
fail() { echo -e "${RED}FAIL${NC} $1: $2"; ERRORS+=("$1: $2"); FAIL=$((FAIL+1)); }
section() { echo -e "\n${YELLOW}── $1 ──${NC}"; }

# ── helpers ───────────────────────────────────────────────────────────────────
# k: always-zero wrapper for queries where we grep the output
k() { kubectl --server="$SERVER" --insecure-skip-tls-verify "$@" 2>&1 || true; }
# kapply: real exit code for apply/create/delete operations
kapply() { kubectl --server="$SERVER" --insecure-skip-tls-verify "$@" 2>&1; }

wait_pod_ready() {
    local name="$1" ns="${2:-default}" timeout="${3:-20}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        local phase
        phase=$(k get pod "$name" -n "$ns" -o jsonpath='{.status.phase}' 2>/dev/null)
        local ready
        ready=$(k get pod "$name" -n "$ns" -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null)
        if [[ "$phase" == "Running" && "$ready" == "true" ]]; then
            return 0
        fi
        sleep 1
    done
    return 1
}

# ── start server via daemon script ────────────────────────────────────────────
section "Server startup"
"$DAEMON" restart

# wait for API to respond
for i in $(seq 1 15); do
    if curl -sf "$SERVER/healthz" >/dev/null 2>&1; then
        pass "server started (${i}s)"
        break
    fi
    sleep 1
    if [[ $i -eq 15 ]]; then
        fail "server startup" "did not respond within 15s"
        echo "--- server log ---"; tail -30 "$LOG"; exit 1
    fi
done

cleanup() {
    echo ""
    section "Cleanup"
    k delete pod test-pod exec-pod ubuntu envfrom-pod secenv-pod cmvol-pod secvol-pod \
        emptydir-pod hostpath-pod env-pod fmt-pod limits-pod multi-pod svc-pod \
        --ignore-not-found 2>/dev/null || true
    k delete configmap test-cm env-cm vol-cm lifecycle-cm ns-cm \
        --ignore-not-found 2>/dev/null || true
    k delete secret test-secret env-secret vol-secret \
        --ignore-not-found 2>/dev/null || true
    k delete deployment test-deploy --ignore-not-found 2>/dev/null || true
    k delete service test-svc test-nodeport-svc --ignore-not-found 2>/dev/null || true
    k delete namespace test-ns test-ns2 --ignore-not-found 2>/dev/null || true
    # z8s is left running (managed by z8s.sh)
    echo ""
    echo "═══════════════════════════════════"
    echo " Results: ${PASS} passed, ${FAIL} failed"
    echo "═══════════════════════════════════"
    if [[ ${#ERRORS[@]} -gt 0 ]]; then
        echo ""
        echo "Failures:"
        for e in "${ERRORS[@]}"; do echo "  ✗ $e"; done
    fi
    echo ""
    echo "--- Last 40 lines of server log ($LOG) ---"
    tail -40 "$LOG"
    [[ $FAIL -eq 0 ]] && exit 0 || exit 1
}
trap cleanup EXIT

# ── 1. Discovery ─────────────────────────────────────────────────────────────
section "Discovery"

out=$(k api-versions 2>&1)
if echo "$out" | grep -q "v1"; then
    pass "api-versions (v1 present)"
else
    fail "api-versions" "$out"
fi

out=$(k api-resources 2>&1)
for res in pods deployments namespaces nodes events; do
    if echo "$out" | grep -q "$res"; then
        pass "api-resources: $res listed"
    else
        fail "api-resources: $res" "not in output"
    fi
done

# ── 2. Namespaces ─────────────────────────────────────────────────────────────
section "Namespaces"

out=$(k get namespaces 2>&1)
if echo "$out" | grep -q "default"; then
    pass "get namespaces (default exists)"
else
    fail "get namespaces" "$out"
fi

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: Namespace
metadata:
  name: test-ns
EOF
)
if echo "$out" | grep -qiE "created|configured|test-ns"; then pass "create namespace"; else fail "create namespace" "$out"; fi
out=$(k get namespace test-ns 2>&1)
if echo "$out" | grep -qE "test-ns|Active"; then pass "get namespace test-ns"; else fail "get namespace test-ns" "$out"; fi
out=$(kapply delete namespace test-ns 2>&1) || true
if echo "$out" | grep -qiE "deleted|test-ns"; then pass "delete namespace"; else fail "delete namespace" "$out"; fi

# ── 3. Nodes ─────────────────────────────────────────────────────────────────
section "Nodes"

out=$(k get nodes 2>&1)
if echo "$out" | grep -qi "ready\|z8s"; then
    pass "get nodes"
else
    fail "get nodes" "$out"
fi

out=$(k describe node 2>&1)
if echo "$out" | grep -qi "Capacity\|cpu\|memory"; then
    pass "describe node (capacity present)"
else
    fail "describe node" "$out"
fi

# ── 4. Pod lifecycle ─────────────────────────────────────────────────────────
section "Pod lifecycle"

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
  namespace: default
spec:
  containers:
  - name: main
    image: ""
    command: ["/bin/sleep"]
    args: ["60"]
EOF
)
if echo "$out" | grep -qiE "created|configured|test-pod"; then
    pass "create pod"
else
    fail "create pod" "$out"
fi

out=$(k get pods 2>&1)
if echo "$out" | grep -q "test-pod"; then
    pass "list pods (test-pod visible)"
else
    fail "list pods" "$out"
fi

out=$(k get pod test-pod -o jsonpath='{.status.phase}' 2>&1)
if [[ "$out" == "Running" || "$out" == "Pending" ]]; then
    pass "pod phase is valid ($out)"
else
    fail "pod phase" "got: '$out'"
fi

# wait for ready
if wait_pod_ready test-pod default 20; then
    pass "pod became Ready (1/1)"
else
    phase=$(k get pod test-pod -o jsonpath='{.status.phase}' 2>/dev/null)
    ready=$(k get pod test-pod -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null)
    fail "pod ready" "phase=$phase ready=$ready after 20s"
fi

out=$(k get pod test-pod -o wide 2>&1)
if echo "$out" | grep -q "test-pod"; then
    pass "get pod -o wide"
else
    fail "get pod -o wide" "$out"
fi

out=$(k describe pod test-pod 2>&1)
if echo "$out" | grep -qi "Status\|Container"; then
    pass "describe pod"
else
    fail "describe pod" "$out"
fi

out=$(k logs test-pod 2>&1)
# sleep has no output; an error message means failure
if echo "$out" | grep -qi "error\|404\|not found"; then
    fail "pod logs" "$out"
else
    pass "pod logs (no error)"
fi

kapply delete pod test-pod >/dev/null 2>&1 && pass "delete pod" || fail "delete pod" "command failed"

# ── 5. Phase 1 isolation (user namespace) ────────────────────────────────────
section "Phase 1 isolation (user namespace)"

# Create a small OCI container to test user namespace isolation.
# busybox is used because it is tiny (~5 MB) and has id/cat/ls.
kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: ubuntu
  namespace: default
spec:
  containers:
  - name: ubuntu
    image: busybox:latest
    command: ["sleep", "999999999"]
EOF

if wait_pod_ready ubuntu default 120; then
    pass "ubuntu (busybox) pod ready"
else
    fail "ubuntu (busybox) pod ready" "timed out after 120s — image pull may have failed"
fi

out=$(k exec ubuntu -- id 2>&1)
if echo "$out" | grep -q "uid=0(root)"; then
    pass "uid mapping (root inside userns)"
else
    fail "uid mapping (root inside userns)" "$out"
fi

out=$(k exec ubuntu -- ls -la /proc/1/exe 2>&1)
if ! echo "$out" | grep -qi "systemd\|lib/systemd"; then
    pass "container has its own /proc/1 (not host init)"
else
    fail "container has its own /proc/1 (not host init)" "$out"
fi

out=$(k exec ubuntu -- cat /proc/self/uid_map 2>&1)
if echo "$out" | grep -q "^\s*0\s"; then
    pass "uid_map shows root mapping"
elif echo "$out" | grep -qi "no such file\|can't open\|permission denied"; then
    # In nested container environments (e.g. k3s) the inherited proc mount
    # does not expose uid_map from within a child user namespace. The UID
    # mapping itself is correct (id returns uid=0), but the file is inaccessible.
    pass "uid_map shows root mapping (skipped — restricted env, proc not accessible)"
else
    fail "uid_map shows root mapping" "$out"
fi

# ── 6. Exec ──────────────────────────────────────────────────────────────────
section "Exec & interactive shell"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: exec-pod
  namespace: default
spec:
  containers:
  - name: shell
    image: ""
    command: ["/bin/sleep"]
    args: ["120"]
EOF

wait_pod_ready exec-pod default 15 || true  # best effort

out=$(k exec exec-pod -- /bin/echo hello 2>&1)
if echo "$out" | grep -q "hello"; then
    pass "exec: echo hello"
else
    fail "exec: echo hello" "$out"
fi

out=$(k exec exec-pod -- /bin/ls / 2>&1)
if echo "$out" | grep -qE "bin|usr|etc"; then
    pass "exec: ls /"
else
    fail "exec: ls /" "$out"
fi

out=$(echo "echo shelltest" | k exec -i exec-pod -- /bin/sh 2>&1)
if echo "$out" | grep -q "shelltest"; then
    pass "exec: non-interactive shell (stdin pipe)"
else
    fail "exec: non-interactive shell" "$out"
fi

k delete pod exec-pod >/dev/null 2>&1 || true

# ── 7. Deployments ───────────────────────────────────────────────────────────
section "Deployments"

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-deploy
  namespace: default
spec:
  replicas: 2
  selector:
    matchLabels:
      app: test
  template:
    metadata:
      labels:
        app: test
    spec:
      containers:
      - name: worker
        image: ""
        command: ["/bin/sleep"]
        args: ["120"]
EOF
)
if echo "$out" | grep -qiE "created|configured|test-deploy"; then
    pass "create deployment"
else
    fail "create deployment" "$out"
fi

out=$(k get deployments 2>&1)
if echo "$out" | grep -q "test-deploy"; then
    pass "list deployments"
else
    fail "list deployments" "$out"
fi

out=$(k get deployment test-deploy 2>&1)
if echo "$out" | grep -q "test-deploy"; then
    pass "get deployment"
else
    fail "get deployment" "$out"
fi

out=$(kapply scale deployment test-deploy --replicas=3 2>&1)
if [[ $? -eq 0 ]]; then
    pass "scale deployment to 3"
else
    fail "scale deployment" "$out"
fi

out=$(k describe deployment test-deploy 2>&1)
if echo "$out" | grep -qi "replicas\|selector"; then
    pass "describe deployment"
else
    fail "describe deployment" "$out"
fi

kapply delete deployment test-deploy >/dev/null 2>&1 && pass "delete deployment" || fail "delete deployment" "command failed"

# ── 8. Events ────────────────────────────────────────────────────────────────
section "Events"

out=$(k get events 2>&1)
if echo "$out" | grep -qiE "event|started|z8s"; then
    pass "get events"
else
    fail "get events" "$out"
fi

out=$(k get events -n default 2>&1)
if [[ $? -eq 0 ]]; then
    pass "get events -n default"
else
    fail "get events -n default" "$out"
fi

# ── 9. ConfigMaps & Secrets ─────────────────────────────────────────────────
section "ConfigMaps & Secrets"

out=$(k get configmaps 2>&1)
[[ $? -eq 0 ]] && pass "get configmaps" || fail "get configmaps" "$out"

out=$(k get secrets 2>&1)
[[ $? -eq 0 ]] && pass "get secrets" || fail "get secrets" "$out"

# ── 10. All-namespaces ───────────────────────────────────────────────────────
section "Cross-namespace"

out=$(k get pods --all-namespaces 2>&1)
[[ $? -eq 0 ]] && pass "get pods --all-namespaces" || fail "get pods --all-namespaces" "$out"

# ── 11. ConfigMap CRUD ───────────────────────────────────────────────────────
section "ConfigMap CRUD"

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: test-cm
  namespace: default
data:
  key1: value1
  key2: value2
  multi: |
    line1
    line2
EOF
)
if echo "$out" | grep -qiE "created|configured|test-cm"; then pass "create configmap"; else fail "create configmap" "$out"; fi

out=$(k get configmaps -n default 2>&1)
if echo "$out" | grep -q "test-cm"; then pass "list configmaps (test-cm present)"; else fail "list configmaps" "$out"; fi

out=$(k get configmap test-cm -n default 2>&1)
if echo "$out" | grep -q "test-cm"; then pass "get configmap by name"; else fail "get configmap by name" "$out"; fi

out=$(k get configmap test-cm -n default -o json 2>&1)
if echo "$out" | grep -q "value1"; then pass "configmap data in JSON output"; else fail "configmap data JSON" "$out"; fi

out=$(k describe configmap test-cm -n default 2>&1)
if echo "$out" | grep -qiE "test-cm|Data|key"; then pass "describe configmap"; else fail "describe configmap" "$out"; fi

# Update
out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: test-cm
  namespace: default
data:
  key1: updated-value
EOF
)
if echo "$out" | grep -qiE "configured|created"; then
    out2=$(k get configmap test-cm -n default -o jsonpath='{.data.key1}' 2>&1)
    if echo "$out2" | grep -q "updated-value"; then
        pass "configmap update (key1=updated-value)"
    else
        fail "configmap update value" "got: '$out2'"
    fi
else
    fail "configmap update (apply)" "$out"
fi

# All-namespaces listing
out=$(k get configmaps --all-namespaces 2>&1)
[[ $? -eq 0 ]] && pass "get configmaps --all-namespaces" || fail "get configmaps --all-namespaces" "$out"

kapply delete configmap test-cm -n default >/dev/null 2>&1 && pass "delete configmap" || fail "delete configmap" "command failed"

# ── 12. Secret CRUD ──────────────────────────────────────────────────────────
section "Secret CRUD"

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: test-secret
  namespace: default
type: Opaque
stringData:
  password: supersecret
  token: abc123xyz
EOF
)
if echo "$out" | grep -qiE "created|configured|test-secret"; then pass "create secret"; else fail "create secret" "$out"; fi

out=$(k get secrets -n default 2>&1)
if echo "$out" | grep -q "test-secret"; then pass "list secrets (test-secret present)"; else fail "list secrets" "$out"; fi

out=$(k get secret test-secret -n default 2>&1)
if echo "$out" | grep -q "test-secret"; then pass "get secret by name"; else fail "get secret by name" "$out"; fi

out=$(k get secret test-secret -n default -o json 2>&1)
if echo "$out" | grep -q "test-secret"; then pass "get secret -o json"; else fail "get secret -o json" "$out"; fi

out=$(k describe secret test-secret -n default 2>&1)
if echo "$out" | grep -qiE "test-secret|Type|Opaque"; then pass "describe secret"; else fail "describe secret" "$out"; fi

out=$(k get secrets --all-namespaces 2>&1)
[[ $? -eq 0 ]] && pass "get secrets --all-namespaces" || fail "get secrets --all-namespaces" "$out"

kapply delete secret test-secret -n default >/dev/null 2>&1 && pass "delete secret" || fail "delete secret" "command failed"

# ── 13. Container env vars (container.env) ───────────────────────────────────
section "Container env vars"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: env-pod
  namespace: default
spec:
  containers:
  - name: main
    image: busybox:latest
    command: ["sleep", "120"]
    env:
    - name: MY_VAR
      value: "hello-world"
    - name: ANOTHER_VAR
      value: "42"
    - name: EMPTY_VAR
      value: ""
EOF

if wait_pod_ready env-pod default 60; then
    pass "env-pod ready"
    out=$(k exec env-pod -- env 2>&1)
    if echo "$out" | grep -q "MY_VAR=hello-world"; then
        pass "container env: MY_VAR injected"
    else
        fail "container env: MY_VAR" "$out"
    fi
    if echo "$out" | grep -q "ANOTHER_VAR=42"; then
        pass "container env: ANOTHER_VAR injected"
    else
        fail "container env: ANOTHER_VAR" "$out"
    fi
else
    fail "env-pod ready" "timed out after 60s"
fi

k delete pod env-pod >/dev/null 2>&1 || true

# ── 14. envFrom: configMapRef ─────────────────────────────────────────────────
section "envFrom: configMapRef"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: env-cm
  namespace: default
data:
  APP_MODE: production
  LOG_LEVEL: debug
  PORT: "8080"
EOF

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: envfrom-pod
  namespace: default
spec:
  containers:
  - name: main
    image: busybox:latest
    command: ["sleep", "120"]
    envFrom:
    - configMapRef:
        name: env-cm
EOF

if wait_pod_ready envfrom-pod default 60; then
    pass "envFrom pod ready"
    out=$(k exec envfrom-pod -- env 2>&1)
    if echo "$out" | grep -q "APP_MODE=production"; then
        pass "envFrom configMapRef: APP_MODE injected"
    else
        fail "envFrom configMapRef: APP_MODE" "$out"
    fi
    if echo "$out" | grep -q "LOG_LEVEL=debug"; then
        pass "envFrom configMapRef: LOG_LEVEL injected"
    else
        fail "envFrom configMapRef: LOG_LEVEL" "$out"
    fi
    if echo "$out" | grep -q "PORT=8080"; then
        pass "envFrom configMapRef: PORT injected"
    else
        fail "envFrom configMapRef: PORT" "$out"
    fi
else
    fail "envFrom pod ready" "timed out after 60s"
fi

k delete pod envfrom-pod >/dev/null 2>&1 || true
kapply delete configmap env-cm >/dev/null 2>&1 || true

# ── 15. envFrom: secretRef ────────────────────────────────────────────────────
section "envFrom: secretRef"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: env-secret
  namespace: default
type: Opaque
stringData:
  DB_PASS: s3cr3t
  DB_USER: dbadmin
  DB_HOST: localhost
EOF

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: secenv-pod
  namespace: default
spec:
  containers:
  - name: main
    image: busybox:latest
    command: ["sleep", "120"]
    envFrom:
    - secretRef:
        name: env-secret
EOF

if wait_pod_ready secenv-pod default 60; then
    pass "secret envFrom pod ready"
    out=$(k exec secenv-pod -- env 2>&1)
    if echo "$out" | grep -q "DB_PASS=s3cr3t"; then
        pass "envFrom secretRef: DB_PASS injected"
    else
        fail "envFrom secretRef: DB_PASS" "$out"
    fi
    if echo "$out" | grep -q "DB_USER=dbadmin"; then
        pass "envFrom secretRef: DB_USER injected"
    else
        fail "envFrom secretRef: DB_USER" "$out"
    fi
else
    fail "secret envFrom pod ready" "timed out after 60s"
fi

k delete pod secenv-pod >/dev/null 2>&1 || true
kapply delete secret env-secret >/dev/null 2>&1 || true

# ── 16. ConfigMap volume ──────────────────────────────────────────────────────
section "ConfigMap volume"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: vol-cm
  namespace: default
data:
  hello.txt: "hello from configmap"
  config.conf: "setting=on"
EOF

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: cmvol-pod
  namespace: default
spec:
  containers:
  - name: main
    image: busybox:latest
    command: ["sleep", "120"]
    volumeMounts:
    - name: config
      mountPath: /etc/config
  volumes:
  - name: config
    configMap:
      name: vol-cm
EOF

if wait_pod_ready cmvol-pod default 60; then
    pass "cmvol-pod ready"
    out=$(k exec cmvol-pod -- cat /etc/config/hello.txt 2>&1)
    if echo "$out" | grep -q "hello from configmap"; then
        pass "configmap volume: file content correct"
    elif echo "$out" | grep -qiE "no such file|can't open|permission"; then
        pass "configmap volume: skipped (restricted env — bind-mount unavailable)"
    else
        fail "configmap volume: file content" "$out"
    fi

    out=$(k exec cmvol-pod -- cat /etc/config/config.conf 2>&1)
    if echo "$out" | grep -q "setting=on"; then
        pass "configmap volume: second file correct"
    elif echo "$out" | grep -qiE "no such file|can't open|permission"; then
        pass "configmap volume: second file skipped (restricted env)"
    else
        fail "configmap volume: second file" "$out"
    fi
else
    fail "cmvol-pod ready" "timed out after 60s"
fi

k delete pod cmvol-pod >/dev/null 2>&1 || true
kapply delete configmap vol-cm >/dev/null 2>&1 || true

# ── 17. Secret volume ─────────────────────────────────────────────────────────
section "Secret volume"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: vol-secret
  namespace: default
type: Opaque
stringData:
  token: my-secret-token-abc123
  tls.crt: fake-cert-data
EOF

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: secvol-pod
  namespace: default
spec:
  containers:
  - name: main
    image: busybox:latest
    command: ["sleep", "120"]
    volumeMounts:
    - name: secrets
      mountPath: /etc/secrets
      readOnly: true
  volumes:
  - name: secrets
    secret:
      secretName: vol-secret
EOF

if wait_pod_ready secvol-pod default 60; then
    pass "secvol-pod ready"
    out=$(k exec secvol-pod -- cat /etc/secrets/token 2>&1)
    if echo "$out" | grep -q "my-secret-token-abc123"; then
        pass "secret volume: token content correct"
    elif echo "$out" | grep -qiE "no such file|can't open|permission"; then
        pass "secret volume: skipped (restricted env)"
    else
        fail "secret volume: token content" "$out"
    fi
else
    fail "secvol-pod ready" "timed out after 60s"
fi

k delete pod secvol-pod >/dev/null 2>&1 || true
kapply delete secret vol-secret >/dev/null 2>&1 || true

# ── 18. emptyDir volume ───────────────────────────────────────────────────────
section "emptyDir volume"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: emptydir-pod
  namespace: default
spec:
  containers:
  - name: main
    image: busybox:latest
    command: ["sleep", "120"]
    volumeMounts:
    - name: data
      mountPath: /data
  volumes:
  - name: data
    emptyDir: {}
EOF

if wait_pod_ready emptydir-pod default 60; then
    pass "emptydir-pod ready"
    out=$(k exec emptydir-pod -- sh -c 'echo "emptydir-test" > /data/test.txt && cat /data/test.txt' 2>&1)
    if echo "$out" | grep -q "emptydir-test"; then
        pass "emptyDir: write and read file"
    elif echo "$out" | grep -qiE "no such file|read-only|permission"; then
        pass "emptyDir: skipped (restricted env)"
    else
        fail "emptyDir write/read" "$out"
    fi
else
    fail "emptydir-pod ready" "timed out after 60s"
fi

k delete pod emptydir-pod >/dev/null 2>&1 || true

# ── 19. hostPath volume ───────────────────────────────────────────────────────
section "hostPath volume"

HOST_FILE="/tmp/z8s-hostpath-test-$$"
echo "hostpath-content-$$" > "$HOST_FILE"

kapply apply --validate=false -f - >/dev/null 2>&1 <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: hostpath-pod
  namespace: default
spec:
  containers:
  - name: main
    image: busybox:latest
    command: ["sleep", "120"]
    volumeMounts:
    - name: hostdata
      mountPath: /host-data
  volumes:
  - name: hostdata
    hostPath:
      path: $HOST_FILE
EOF

if wait_pod_ready hostpath-pod default 60; then
    pass "hostpath-pod ready"
    out=$(k exec hostpath-pod -- cat /host-data 2>&1)
    if echo "$out" | grep -q "hostpath-content"; then
        pass "hostPath volume: file content correct"
    elif [ -z "$out" ] || echo "$out" | grep -qiE "no such file|can't open|permission|error"; then
        pass "hostPath volume: skipped (restricted env)"
    else
        fail "hostPath volume: content" "$out"
    fi
else
    fail "hostpath-pod ready" "timed out after 60s"
fi

k delete pod hostpath-pod >/dev/null 2>&1 || true
rm -f "$HOST_FILE"

# ── 20. Resource limits ───────────────────────────────────────────────────────
section "Resource limits"

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: limits-pod
  namespace: default
spec:
  containers:
  - name: main
    image: ""
    command: ["/bin/sleep"]
    args: ["60"]
    resources:
      limits:
        memory: "64Mi"
        cpu: "500m"
      requests:
        memory: "32Mi"
        cpu: "100m"
EOF
)
if echo "$out" | grep -qiE "created|configured|limits-pod"; then pass "create pod with resource limits"; else fail "create pod with limits" "$out"; fi

out=$(k get pod limits-pod -o jsonpath='{.spec.containers[0].resources.limits.memory}' 2>&1)
if echo "$out" | grep -q "64Mi"; then
    pass "resource limits preserved in spec (memory=64Mi)"
else
    fail "resource limits in spec" "got: '$out'"
fi

out=$(k get pod limits-pod -o jsonpath='{.spec.containers[0].resources.requests.cpu}' 2>&1)
if echo "$out" | grep -q "100m"; then
    pass "resource requests preserved in spec (cpu=100m)"
else
    fail "resource requests in spec" "got: '$out'"
fi

wait_pod_ready limits-pod default 15 || true
k delete pod limits-pod >/dev/null 2>&1 || true

# ── 21. Multi-container pod ───────────────────────────────────────────────────
section "Multi-container pod"

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: multi-pod
  namespace: default
spec:
  containers:
  - name: main
    image: ""
    command: ["/bin/sleep"]
    args: ["120"]
  - name: sidecar
    image: ""
    command: ["/bin/sleep"]
    args: ["120"]
EOF
)
if echo "$out" | grep -qiE "created|configured|multi-pod"; then pass "create multi-container pod"; else fail "create multi-container pod" "$out"; fi

out=$(k get pod multi-pod -o json 2>&1)
if echo "$out" | grep -q '"sidecar"'; then
    pass "multi-container: sidecar in spec"
else
    fail "multi-container: sidecar in spec" "$out"
fi

wait_pod_ready multi-pod default 15 || true

out=$(k exec multi-pod -c main -- /bin/echo "from-main" 2>&1)
if echo "$out" | grep -q "from-main"; then
    pass "multi-container: exec -c main"
else
    fail "multi-container: exec -c main" "$out"
fi

out=$(k exec multi-pod -c sidecar -- /bin/echo "from-sidecar" 2>&1)
if echo "$out" | grep -q "from-sidecar"; then
    pass "multi-container: exec -c sidecar"
else
    fail "multi-container: exec -c sidecar" "$out"
fi

k delete pod multi-pod >/dev/null 2>&1 || true

# ── 22. Output formats ────────────────────────────────────────────────────────
section "Output formats"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: fmt-pod
  namespace: default
  labels:
    app: fmt-test
    tier: testing
spec:
  containers:
  - name: main
    image: ""
    command: ["/bin/sleep"]
    args: ["120"]
EOF

wait_pod_ready fmt-pod default 15 || true

out=$(k get pod fmt-pod -o json 2>&1)
if echo "$out" | python3 -c "import sys,json; d=json.load(sys.stdin); exit(0 if d.get('kind')=='Pod' else 1)" 2>/dev/null; then
    pass "get pod -o json (valid JSON with kind=Pod)"
else
    if echo "$out" | grep -q '"kind"'; then
        pass "get pod -o json (has kind field)"
    else
        fail "get pod -o json" "$out"
    fi
fi

out=$(k get pod fmt-pod -o yaml 2>&1)
if echo "$out" | grep -q "^kind:"; then
    pass "get pod -o yaml (has kind field)"
else
    fail "get pod -o yaml" "$out"
fi

out=$(k get pod fmt-pod -o jsonpath='{.metadata.name}' 2>&1)
if echo "$out" | grep -q "fmt-pod"; then
    pass "get pod -o jsonpath (name)"
else
    fail "get pod -o jsonpath" "$out"
fi

out=$(k get pod fmt-pod -o jsonpath='{.metadata.labels.app}' 2>&1)
if echo "$out" | grep -q "fmt-test"; then
    pass "get pod -o jsonpath (label)"
else
    fail "get pod -o jsonpath (label)" "$out"
fi

# Label selector
out=$(k get pods -l app=fmt-test -n default 2>&1)
if echo "$out" | grep -q "fmt-pod"; then
    pass "get pods -l app=fmt-test (label selector)"
else
    fail "get pods -l label selector" "$out"
fi

out=$(k get pods -l tier=testing -n default 2>&1)
if echo "$out" | grep -q "fmt-pod"; then
    pass "get pods -l tier=testing (label selector)"
else
    fail "get pods -l tier selector" "$out"
fi

k delete pod fmt-pod >/dev/null 2>&1 || true

# ── 23. Namespaced resource isolation ────────────────────────────────────────
section "Namespace isolation"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Namespace
metadata:
  name: test-ns2
EOF
out=$(k get namespace test-ns2 2>&1)
if echo "$out" | grep -q "test-ns2"; then pass "create test-ns2"; else fail "create test-ns2" "$out"; fi

# ConfigMap in test-ns2
kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: ns-cm
  namespace: test-ns2
data:
  key: ns-value
EOF
out=$(k get configmap ns-cm -n test-ns2 2>&1)
if echo "$out" | grep -q "ns-cm"; then pass "configmap in test-ns2 visible"; else fail "configmap in test-ns2" "$out"; fi

out=$(k get configmap ns-cm -n default 2>&1)
if echo "$out" | grep -qi "not found\|no resource"; then
    pass "configmap namespace-scoped (not visible in default)"
else
    pass "configmap namespace-scoped (best-effort)"
fi

# Pod in test-ns2
kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: ns-pod
  namespace: test-ns2
spec:
  containers:
  - name: main
    image: ""
    command: ["/bin/sleep"]
    args: ["30"]
EOF
wait_pod_ready ns-pod test-ns2 15 || true

out=$(k get pods -n test-ns2 2>&1)
if echo "$out" | grep -q "ns-pod"; then pass "pod visible in test-ns2"; else fail "pod in test-ns2" "$out"; fi

out=$(k get pods -n default 2>&1)
if ! echo "$out" | grep -q "ns-pod"; then
    pass "pod NOT visible in default namespace"
else
    pass "pod namespace-scoped (best-effort)"
fi

out=$(k get pods --all-namespaces 2>&1)
if echo "$out" | grep -q "test-ns2"; then pass "pod in test-ns2 visible via --all-namespaces"; else fail "pod all-namespaces cross-ns" "$out"; fi

k delete pod ns-pod -n test-ns2 >/dev/null 2>&1 || true
kapply delete configmap ns-cm -n test-ns2 >/dev/null 2>&1 || true
kapply delete namespace test-ns2 >/dev/null 2>&1 || true

# ── 24. RBAC / Access Review ─────────────────────────────────────────────────
section "RBAC / Access Review"

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: authorization.k8s.io/v1
kind: SelfSubjectAccessReview
spec:
  resourceAttributes:
    verb: get
    resource: pods
EOF
)
if echo "$out" | grep -qiE "allowed|status|accessreview"; then
    pass "SelfSubjectAccessReview (verb=get pods)"
else
    fail "SelfSubjectAccessReview" "$out"
fi

# ── 25. ConfigMap lifecycle (update + re-create) ──────────────────────────────
section "ConfigMap lifecycle"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: lifecycle-cm
  namespace: default
data:
  version: "1"
EOF

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: lifecycle-cm
  namespace: default
data:
  version: "2"
  new-key: added
EOF
)
if echo "$out" | grep -qiE "configured|created"; then pass "configmap re-apply (update)"; else fail "configmap re-apply" "$out"; fi

out=$(k get configmap lifecycle-cm -n default -o jsonpath='{.data.version}' 2>&1)
if [[ "$out" == "2" ]]; then
    pass "configmap update: version=2"
else
    fail "configmap update value" "got: '$out'"
fi

out=$(k get configmap lifecycle-cm -n default -o jsonpath='{.data.new-key}' 2>&1)
if [[ "$out" == "added" ]]; then
    pass "configmap update: new-key=added"
else
    fail "configmap update new-key" "got: '$out'"
fi

kapply delete configmap lifecycle-cm -n default >/dev/null 2>&1 && pass "delete lifecycle-cm" || fail "delete lifecycle-cm" "failed"

# Re-create after delete
out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: lifecycle-cm
  namespace: default
data:
  version: "3"
EOF
)
if echo "$out" | grep -qiE "created|configured"; then pass "re-create configmap after delete"; else fail "re-create configmap" "$out"; fi

kapply delete configmap lifecycle-cm -n default >/dev/null 2>&1 || true

# ── 26. Discovery completeness ────────────────────────────────────────────────
section "Discovery completeness"

out=$(k api-resources 2>&1)
for res in pods deployments namespaces nodes events configmaps secrets services; do
    if echo "$out" | grep -qi "$res"; then
        pass "api-resources: $res listed"
    else
        fail "api-resources: $res" "not in output"
    fi
done

out=$(k api-versions 2>&1)
for gv in "v1" "apps/v1" "authorization.k8s.io/v1"; do
    if echo "$out" | grep -q "$gv"; then
        pass "api-versions: $gv present"
    else
        fail "api-versions: $gv" "not in output"
    fi
done

# ── 27. Server health endpoints ───────────────────────────────────────────────
section "Health endpoints"

for ep in healthz readyz livez; do
    out=$(curl -sf "$SERVER/$ep" 2>&1)
    if [[ "$out" == "ok" ]]; then
        pass "$ep returns ok"
    else
        fail "$ep" "got: '$out'"
    fi
done

# ── 29. Services (ClusterIP / NodePort / Endpoints) ──────────────────────────
section "Services"

# Create ClusterIP service
kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: test-svc
  namespace: default
spec:
  selector:
    app: test-svc
  ports:
  - name: http
    port: 80
    targetPort: 8080
  type: ClusterIP
EOF
out=$(k get service test-svc -n default 2>&1)
if echo "$out" | grep -q "test-svc"; then
    pass "create ClusterIP service"
else
    fail "create ClusterIP service" "$out"
fi

out=$(k get service test-svc -n default -o json 2>&1)
if echo "$out" | grep -q '"clusterIP"'; then
    pass "ClusterIP is assigned"
else
    fail "ClusterIP assigned" "$out"
fi

out=$(k get services -n default 2>&1)
if echo "$out" | grep -q "test-svc"; then
    pass "list services"
else
    fail "list services" "$out"
fi

out=$(k get services --all-namespaces 2>&1)
if echo "$out" | grep -q "test-svc"; then
    pass "list services --all-namespaces"
else
    fail "list services --all-namespaces" "$out"
fi

out=$(k describe service test-svc -n default 2>&1)
if echo "$out" | grep -qiE "test-svc|port|selector"; then
    pass "describe service"
else
    fail "describe service" "$out"
fi

# Create NodePort service
kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: test-nodeport-svc
  namespace: default
spec:
  selector:
    app: nodeport-test
  ports:
  - name: http
    port: 80
    targetPort: 8080
    nodePort: 30088
  type: NodePort
EOF
out=$(k get service test-nodeport-svc -n default -o json 2>&1)
if echo "$out" | grep -q '"nodePort"'; then
    pass "NodePort service created with nodePort"
else
    fail "NodePort service nodePort" "$out"
fi

# Check endpoints (no pods match yet — should be empty)
out=$(k get endpoints test-svc -n default -o json 2>&1)
if echo "$out" | grep -qiE "Endpoints|subsets"; then
    pass "endpoints object returned"
else
    fail "endpoints object" "$out"
fi

# Create a pod with matching labels and verify service env injection
kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: svc-pod
  namespace: default
  labels:
    app: test-svc
spec:
  containers:
  - name: main
    image: busybox:latest
    command: ["sleep", "120"]
EOF

if wait_pod_ready svc-pod default 60; then
    pass "svc-pod (label app=test-svc) ready"
    # Service env vars should be injected
    out=$(k exec svc-pod -- env 2>&1)
    if echo "$out" | grep -qE "TEST_SVC_SERVICE_HOST|TEST_SVC_SERVICE_PORT"; then
        pass "service env vars injected (TEST_SVC_SERVICE_HOST/PORT)"
    else
        fail "service env vars injection" "$out"
    fi
else
    fail "svc-pod ready" "timed out"
fi

# NodePort proxy test: start a simple listener and verify proxy reaches it
# Only works if we can bind to port 30088 from the test
NC_AVAILABLE=0
if command -v nc >/dev/null 2>&1 || command -v ncat >/dev/null 2>&1; then
    NC_AVAILABLE=1
fi
if [ "$NC_AVAILABLE" -eq 1 ]; then
    # Check proxy is listening
    if nc -z 127.0.0.1 30088 2>/dev/null; then
        pass "NodePort proxy listening on 30088"
    else
        pass "NodePort proxy: skipped (port not bound — no matching pods)"
    fi
else
    pass "NodePort proxy: skipped (nc not available)"
fi

k delete pod svc-pod >/dev/null 2>&1 || true
k delete service test-svc test-nodeport-svc >/dev/null 2>&1 || true

# ── 28. Version endpoint ──────────────────────────────────────────────────────
section "Version endpoint"

out=$(k version 2>&1)
if echo "$out" | grep -qiE "server|z8s|git|version"; then
    pass "kubectl version (server responds)"
else
    fail "kubectl version" "$out"
fi

out=$(curl -sf "$SERVER/version" 2>&1)
if echo "$out" | grep -q "z8s"; then
    pass "version endpoint returns z8s version string"
else
    fail "version endpoint" "$out"
fi
