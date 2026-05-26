#!/usr/bin/env bash
# z8s integration test suite v2 — parallel, fast, selective
# Usage:
#   ./run-tests-v2.sh                  # run all tests
#   ./run-tests-v2.sh --only pod,svc   # run only pod + service tests
#   ./run-tests-v2.sh --skip scale     # run all except scale tests
#   ./run-tests-v2.sh --list           # list available test groups
#   ./run-tests-v2.sh --parallel 4     # max 4 parallel tests (default: 8)
set -uo pipefail

export PATH="/home/abb/.local/bin:/usr/local/bin:/usr/bin:/bin:$PATH"
SERVER="${Z8S_SERVER:-http://localhost:6443}"
DAEMON="$(dirname "$0")/../z8s.sh"
YAML_DIR="$(dirname "$0")"
LOG="/tmp/z8s.log"
PARALLEL="${PARALLEL:-8}"
PASS=0; FAIL=0; ERRORS=(); RESULTS_DIR=$(mktemp -d)
SELECTED=""; SKIPPED=""; LIST_ONLY=0

GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS+1)); }
fail() { local m="$1" d="${2:-}"; echo -e "${RED}FAIL${NC} $m${d:+: $d}"; ERRORS+=("$m${d:+: $d}"); FAIL=$((FAIL+1)); }

k() { kubectl --server="$SERVER" "$@" 2>&1 || true; }
kapply() { kubectl --server="$SERVER" "$@" 2>&1; }

wait_pod_ready() {
    local name="$1" ns="${2:-default}" timeout="${3:-60}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        phase=$(k get pod "$name" -n "$ns" -o jsonpath='{.status.phase}' 2>/dev/null)
        ready=$(k get pod "$name" -n "$ns" -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null)
        [[ "$phase" == "Running" && "$ready" == "true" ]] && return 0
        sleep 1
    done
    return 1
}

wait_deploy_ready() {
    local name="$1" ns="${2:-default}" replicas="${3:-1}" timeout="${4:-90}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        ready=$(k get deployment "$name" -n "$ns" -o jsonpath='{.status.readyReplicas}' 2>/dev/null)
        [[ "${ready:-0}" -ge "$replicas" ]] && return 0
        sleep 1
    done
    return 1
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --only) SELECTED="$2"; shift 2 ;;
            --skip) SKIPPED="$2"; shift 2 ;;
            --parallel) PARALLEL="$2"; shift 2 ;;
            --list) LIST_ONLY=1; shift ;;
            *) echo "Unknown: $1"; exit 1 ;;
        esac
    done
}
parse_args "$@"

# ── Test registry ──────────────────────────────────────────────────────────
declare -A TESTS=()
test() { TESTS["$1"]="$2"; }
run_test() {
    local name="$1" func="$2"
    local out_file="$RESULTS_DIR/${name//\//_}.out"
    echo "START:$name" > "$out_file"
    ($func >> "$out_file" 2>&1)
    local rc=$?
    echo "EXIT:$name=$rc" >> "$out_file"
    if [[ $rc -eq 0 ]]; then
        echo "PASS:$name" >> "$RESULTS_DIR/results"
    else
        echo "FAIL:$name" >> "$RESULTS_DIR/results"
    fi
    return $rc
}
run_parallel() {
    local -a names=("$@")
    local pids=() running=0 i=0
    for name in "${names[@]}"; do
        local func="${TESTS[$name]}"
        if [[ $running -ge $PARALLEL ]]; then
            wait -n 2>/dev/null; running=$((running - 1))
        fi
        echo "  launching $name ($func)" >&2
        (run_test "$name" "$func") &
        pids+=($!); running=$((running + 1))
    done
    wait 2>/dev/null
}
show_results() {
    while IFS=: read -r status name; do
        local out_file="$RESULTS_DIR/${name//\//_}.out"
        local first_err
        first_err=$(head -1 "$out_file" 2>/dev/null)
        if [[ "$status" == "PASS" ]]; then
            pass "$name"
        else
            fail "$name" "${first_err:-see $out_file}"
        fi
        echo ""
        echo "--- $name output ---"
        cat "$out_file"
        echo "---"
        echo ""
    done < "$RESULTS_DIR/results"
}
cleanup() {
    rm -rf "$RESULTS_DIR"
    echo ""
    echo "════════════════════════════════════════════"
    echo " Results: ${PASS} passed, ${FAIL} failed"
    [[ ${#ERRORS[@]} -gt 0 ]] && { echo ""; echo " Failures:"; for e in "${ERRORS[@]}"; do echo "  ✗ $e"; done; }
    echo "════════════════════════════════════════════"
    [[ $FAIL -eq 0 ]] && exit 0 || exit 1
}
trap cleanup EXIT

# ── Test functions ──────────────────────────────────────────────────────────

setup() {
    echo "══ Setup: start server + apply YAMLs ══"
    "$DAEMON" restart
    for i in $(seq 1 15); do
        curl -sf "$SERVER/healthz" >/dev/null 2>&1 && break
        sleep 1
        [[ $i -eq 15 ]] && { echo "Server failed to start"; exit 1; }
    done
    for f in "$YAML_DIR"/*.yaml; do
        kapply apply --validate=false -f "$f" 2>/dev/null || true
    done
}

wait_all() {
    echo "══ Wait for readiness ══"
    for pod in alpine-pod:default:60 postgres-pod:default:120 python-pod:default:60 logger-pod:default:60 ubuntu-pod:z8s-test:120; do
        IFS=: read -r name ns timeout <<< "$pod"
        wait_pod_ready "$name" "$ns" "$timeout" && echo "  pod $name Ready" || echo "  pod $name FAIL"
    done
    for dep in http-echo:default:1:90 python-deploy:default:2:90 alpine-deploy:default:2:90 whoami:default:1:90 logger-deploy:default:2:90 cluster-dashboard:default:1:120 hostinfo:default:1:120 nginx-deploy:default:2:90 postgres-deploy:default:1:120 ubuntu-deploy:z8s-test:1:90 nginx-hello:default:1:10; do
        IFS=: read -r name ns reps timeout <<< "$dep"
        wait_deploy_ready "$name" "$ns" "$reps" "$timeout" && echo "  deploy $name Ready" || echo "  deploy $name FAIL (timeout)"
    done
}

# ── Individual test groups ──────────────────────────────────────────────────

test_namespace() {
    k get ns z8s-test 2>&1 | grep -q "z8s-test" || return 1
    k get ns z8s-prod 2>&1 | grep -q "z8s-prod" || return 1
    return 0
}

test_configmap() {
    [[ "$(k get configmap app-config -n default -o jsonpath='{.data.APP_ENV}' 2>/dev/null)" == "production" ]] || return 1
    [[ "$(k get configmap app-config -n z8s-test -o jsonpath='{.data.APP_ENV}' 2>/dev/null)" == "staging" ]] || return 1
    return 0
}

test_secret() {
    k get secret app-secret -n z8s-test -o json 2>&1 | grep -q "Opaque" || return 1
    return 0
}

test_pod_alpine() {
    local e r
    e=$(k exec alpine-pod -- sh -c env 2>&1) || { echo "exec env failed: $e"; return 1; }
    echo "$e" | grep -q "DIRECT_ENV=direct-value" || { echo "MISSING DIRECT_ENV"; return 1; }
    echo "$e" | grep -q "APP_ENV=production" || { echo "MISSING APP_ENV"; return 1; }
    echo "$e" | grep -q "DB_PASSWORD=password123" || { echo "MISSING DB_PASSWORD"; return 1; }
    r=$(k exec alpine-pod -- sh -c 'echo "x" > /tmp/test && cat /tmp/test' 2>&1) || { echo "write failed: $r"; return 1; }
    echo "$r" | grep -q "x" || { echo "readback failed"; return 1; }
    echo "env vars OK, write/read OK"
    return 0
}

test_pod_ubuntu() {
    local u e
    u=$(k exec -n z8s-test ubuntu-pod -- id 2>&1) || { echo "exec id failed: $u"; return 1; }
    echo "$u" | grep -qE "uid=1000|uid=1001" || { echo "unexpected uid: $u"; return 1; }
    e=$(k exec -n z8s-test ubuntu-pod -- sh -c env 2>&1) || { echo "exec env failed: $e"; return 1; }
    echo "$e" | grep -q "APP_ENV=staging" || { echo "MISSING APP_ENV=staging"; return 1; }
    echo "$e" | grep -q "DB_PASSWORD=test-pass" || { echo "MISSING DB_PASSWORD"; return 1; }
    echo "uid=$u, envFrom OK"
    return 0
}

test_pod_python() {
    local r
    r=$(k exec python-pod -- wget -q -O- -T 3 http://127.0.0.1:8080/ 2>&1) || true
    echo "$r" | grep -qiE "directory listing|http|html" || { echo "HTTP check failed: $(echo "$r" | head -c 200)"; return 1; }
    echo "HTTP server responds OK"
    return 0
}

test_pod_postgres() {
    local r
    r=$(k exec postgres-pod -- psql -U admin -d testdb -c "SELECT 1 AS ok;" 2>&1) || true
    echo "$r" | grep -q "1" || { echo "psql failed: $(echo "$r" | head -c 200)"; return 1; }
    echo "psql query OK"
    return 0
}

test_pod_ubuntu() {
    local u
    u=$(k exec -n z8s-test ubuntu-pod -- id 2>&1) || true
    echo "$u" | grep -qE "uid=1000|uid=1001" || return 1
    # envFrom
    local e
    e=$(k exec -n z8s-test ubuntu-pod -- sh -c env 2>&1) || true
    echo "$e" | grep -q "APP_ENV=staging" || return 1
    echo "$e" | grep -q "DB_PASSWORD=test-pass" || return 1
    return 0
}

test_pod_python() {
    local r
    r=$(k exec python-pod -- python3 -c "import urllib.request; print(urllib.request.urlopen('http://127.0.0.1:18080/').read().decode())" 2>&1) || true
    echo "$r" | grep -qiE "directory listing|http|html" || return 1
    return 0
}

test_pod_postgres() {
    local r
    r=$(k exec postgres-pod -- psql -U admin -d testdb -c "SELECT 1 AS ok;" 2>&1) || true
    echo "$r" | grep -q "1" || { echo "psql failed: $(echo "$r" | head -c 300)"; return 1; }
    echo "psql query OK"
    return 0
}

test_deploy_alpine() {
    local reps pod e
    reps=$(k get deployment alpine-deploy -n default -o jsonpath='{.spec.replicas}' 2>/dev/null)
    [[ "$reps" == "2" ]] || { echo "expected 2 replicas, got $reps"; return 1; }
    pod=$(k get pods -n default -o name 2>/dev/null | grep -o 'alpine-deploy-pod-[^ ]*' | head -1)
    [[ -n "$pod" ]] || { echo "no alpine-deploy pod found"; return 1; }
    e=$(k exec -n default "$pod" -- sh -c env 2>&1) || { echo "exec env failed: $e"; return 1; }
    echo "$e" | grep -q "APP_ENV=production" || { echo "MISSING APP_ENV in deploy pod"; return 1; }
    echo "$e" | grep -q "DEPLOY_NAME=alpine-deploy" || { echo "MISSING DEPLOY_NAME"; return 1; }
    echo "replicas=2, env vars OK"
    return 0
}

test_deploy_ubuntu() {
    local reps
    reps=$(k get deployment ubuntu-deploy -n z8s-test -o jsonpath='{.spec.replicas}' 2>/dev/null)
    [[ "$reps" == "1" ]] || { echo "expected 1 replica, got $reps"; return 1; }
    echo "replicas=1"
    return 0
}

test_deploy_python() {
    local reps
    reps=$(k get deployment python-deploy -n default -o jsonpath='{.spec.replicas}' 2>/dev/null)
    [[ "$reps" == "2" ]] || { echo "expected 2 replicas, got $reps"; return 1; }
    echo "replicas=2"
    return 0
}

test_deploy_nginx() {
    local reps pod r
    reps=$(k get deployment nginx-deploy -n default -o jsonpath='{.spec.replicas}' 2>/dev/null)
    [[ "$reps" == "2" ]] || { echo "expected 2 replicas, got $reps"; return 1; }
    pod=$(k get pods -n default -o name 2>/dev/null | grep -o 'nginx-deploy-pod-[^ ]*' | head -1)
    [[ -n "$pod" ]] || { echo "no nginx-deploy pod found"; return 1; }
    r=$(k exec -n default "$pod" -- sh -c 'wget -q -O- -T 3 http://127.0.0.1:80/' 2>&1) || true
    echo "$r" | grep -qiE "nginx|html|welcome|Hostname" || { echo "HTTP check failed: $(echo "$r" | head -c 200)"; return 1; }
    echo "replicas=2, HTTP OK"
    return 0
}

test_deploy_info() {
    echo "=== checking info pods ==="
    return 0
}

test_deploy_python() {
    [[ "$(k get deployment python-deploy -n default -o jsonpath='{.spec.replicas}' 2>/dev/null)" == "2" ]] || return 1
    return 0
}

test_deploy_nginx() {
    [[ "$(k get deployment nginx-deploy -n default -o jsonpath='{.spec.replicas}' 2>/dev/null)" == "2" ]] || return 1
    local pod
    pod=$(k get pods -n default -o name 2>/dev/null | grep -o 'nginx-deploy-pod-[^ ]*' | head -1)
    [[ -n "$pod" ]] || return 1
    local r
    r=$(k exec -n default "$pod" -- sh -c 'wget -q -O- -T 3 http://127.0.0.1:80/ 2>&1') || true
    echo "$r" | grep -qiE "nginx|html|welcome|Hostname" || return 1
    return 0
}

test_deploy_info() {
    for dep in whoami http-echo hostinfo cluster-dashboard; do
        local pod
        pod=$(k get pods -n default -o name 2>/dev/null | grep -o "${dep}-pod-[^ ]*" | head -1)
        [[ -n "$pod" ]] || return 1
        local r
        case "$dep" in
            whoami)   r=$(k exec -n default "$pod" -- sh -c 'wget -q -O- -T 3 http://127.0.0.1:80/' 2>&1) || true; echo "$r" | grep -qiE "Hostname|hostname" || return 1 ;;
            http-echo) r=$(k exec -n default "$pod" -- sh -c 'wget -q -O- -T 3 http://127.0.0.1:5678/' 2>&1) || true; echo "$r" | grep -qiE "hello from z8s" || return 1 ;;
            hostinfo) r=$(k exec -n default "$pod" -- sh -c 'wget -q -O- -T 3 http://127.0.0.1:8080/' 2>&1) || true; echo "$r" | grep -qiE "hostname|Hostname" || return 1 ;;
            cluster-dashboard) r=$(k exec -n default "$pod" -- sh -c 'wget -q -O- -T 3 http://127.0.0.1:80/' 2>&1) || true; echo "$r" | grep -qiE "dashboard|cluster|html" || return 1 ;;
        esac
    done
    return 0
}

test_scale() {
    local t0 t1
    t0=$(date +%s)
    kapply scale deployment alpine-deploy --replicas=4 >/dev/null 2>&1 || true
    wait_deploy_ready alpine-deploy default 4 30 || { echo "scale 2->4 failed"; return 1; }
    t1=$(date +%s)
    echo "  alpine 2->4: $((t1-t0))s"
    t0=$(date +%s)
    kapply scale deployment alpine-deploy --replicas=1 >/dev/null 2>&1 || true
    wait_deploy_ready alpine-deploy default 1 30 || { echo "scale 4->1 failed"; return 1; }
    t1=$(date +%s)
    echo "  alpine 4->1: $((t1-t0))s"
    t0=$(date +%s)
    kapply scale deployment python-deploy --replicas=3 >/dev/null 2>&1 || true
    wait_deploy_ready python-deploy default 3 60 || { echo "scale python 2->3 failed"; return 1; }
    t1=$(date +%s)
    echo "  python 2->3: $((t1-t0))s"
    kapply scale deployment python-deploy --replicas=2 >/dev/null 2>&1 || true
    wait_deploy_ready python-deploy default 2 30 || true
    t0=$(date +%s)
    kapply scale deployment alpine-deploy --replicas=100 >/dev/null 2>&1 || true
    wait_deploy_ready alpine-deploy default 100 180 || { echo "scale 2->100 failed"; return 1; }
    t1=$(date +%s)
    echo "  alpine 2->100: $((t1-t0))s"
    local count
    count=$(k get pods -n default 2>/dev/null | grep -c 'alpine-deploy-pod-' || true)
    echo "  pods visible: $count/100"
    kapply scale deployment alpine-deploy --replicas=1 >/dev/null 2>&1 || true
    wait_deploy_ready alpine-deploy default 1 60 || true
    return 0
}

test_service() {
    kapply apply -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: svc-client
  namespace: default
spec:
  containers:
  - name: client
    image: alpine:latest
    command: ["sleep", "infinity"]
EOF
    wait_pod_ready svc-client default 30 2>/dev/null || { echo "svc-client not ready"; return 1; }
    local ok=0 err=0
    for svc in python-svc:8080:"directory listing|http|html" \
               nginx-svc:80:"nginx|html|welcome|Hostname" \
               whoami-svc:80:"Hostname|hostname" \
               http-echo-svc:5678:"hello from z8s" \
               hostinfo-svc:8080:"hostname|Hostname" \
               nginx-hello-svc:80:"Server|server|html"; do
        IFS=: read -r name port expected <<< "$svc"
        local ip r
        ip=$(k get svc "$name" -n default -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
        [[ -z "$ip" ]] && ip="$name"
        local ok_this=0
        for try in 1 2 3; do
            r=$(k exec svc-client -- wget -q -O- -T 3 "http://${ip}:${port}/" 2>&1) || true
            if echo "$r" | grep -qiE "$expected"; then ok=$((ok+1)); ok_this=1; break; fi
            sleep 1
        done
        [[ $ok_this -eq 0 ]] && { echo "  FAIL $name ($ip:$port): $(echo "$r" | head -c 100)"; err=$((err+1)); }
    done
    local svc_env
    svc_env=$(k exec svc-client -- sh -c env 2>&1) || { echo "exec env in svc-client failed"; return 1; }
    echo "$svc_env" | grep -q "PYTHON_SVC_SERVICE_HOST" && ok=$((ok+1)) || { echo "  MISSING PYTHON_SVC_SERVICE_HOST"; err=$((err+1)); }
    echo "$svc_env" | grep -q "POSTGRES_SVC_SERVICE_HOST" && ok=$((ok+1)) || { echo "  MISSING POSTGRES_SVC_SERVICE_HOST"; err=$((err+1)); }
    k delete pod svc-client --ignore-not-found 2>/dev/null || true
    echo "services: $ok ok, $err failed"
    [[ $err -eq 0 ]] || return 1
    return 0
}

test_volume_basic() {
    local r
    r=$(k exec alpine-pod -- sh -c 'echo "x" > /tmp/test && cat /tmp/test' 2>&1) || true
    echo "$r" | grep -q "x" || { echo "emptyDir write/read failed: $(echo "$r" | head -c 200)"; return 1; }
    echo "emptyDir write/read OK"
    return 0
}

test_volume_persistence() {
    # emptyDir: data lost on pod recreate
    local w
    w=$(k exec alpine-pod -- sh -c 'echo "persist" > /tmp/persist' 2>&1) || true
    k delete pod alpine-pod --wait=true --timeout=30s 2>/dev/null || true
    sleep 3
    kapply apply --validate=false -f "$YAML_DIR/03-pod-alpine.yaml" 2>/dev/null || true
    wait_pod_ready alpine-pod default 60 || return 1
    local r
    r=$(k exec alpine-pod -- cat /tmp/persist 2>&1) || true
    echo "$r" | grep -qiE "no such file|cannot open|not found" && { echo "  emptyDir correctly ephemeral"; return 0; }
    echo "  emptyDir data survived (may be mount issue)"
    return 1
}

test_pv_pvc() {
    k get pv pv-test 2>&1 | grep -q "pv-test" || return 1
    k get pv pv-test-2 2>&1 | grep -q "pv-test-2" || return 1
    [[ "$(k get pv pv-test -o jsonpath='{.spec.capacity.storage}' 2>/dev/null)" == "1Gi" ]] || return 1
    [[ "$(k get pv pv-test-2 -o jsonpath='{.spec.capacity.storage}' 2>/dev/null)" == "5Gi" ]] || return 1
    k get pvc pvc-test -n default 2>&1 | grep -q "pvc-test" || return 1
    k get pvc pvc-test -n z8s-test 2>&1 | grep -q "pvc-test" || return 1
    return 0
}

test_security() {
    # PID isolation
    local p
    p=$(k exec alpine-pod -- ls -la /proc/1/exe 2>&1) || true
    echo "$p" | grep -qiE "systemd|lib/systemd" && return 1
    # Non-root pod
    kapply apply -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata: { name: sec-pod, namespace: default }
spec:
  securityContext: { runAsUser: 12345, runAsNonRoot: true }
  containers:
  - name: sec
    image: alpine:latest
    command: ["sleep", "60"]
    env:
    - name: USER
      value: testuser
EOF
    wait_pod_ready sec-pod default 60 || return 1
    local u
    u=$(k exec sec-pod -- id 2>&1) || true
    echo "$u" | grep -q "uid=12345" || return 1
    local e
    e=$(k exec sec-pod -- sh -c env 2>&1) || true
    echo "$e" | grep -q "USER=testuser" || return 1
    local w
    w=$(k exec sec-pod -- touch /etc/root-test 2>&1) || true
    echo "$w" | grep -qiE "permission denied|read-only" || return 1
    k delete pod sec-pod --ignore-not-found 2>/dev/null || true
    return 0
}

test_logs() {
    local l
    l=$(k logs logger-pod -n default 2>&1) || { echo "kubectl logs failed: $(echo "$l" | head -c 200)"; return 1; }
    echo "$l" | grep -q "INFO" || { echo "no INFO lines in logs"; return 1; }
    echo "$l" | grep -q "DEBUG" || { echo "no DEBUG lines (stderr)"; }
    echo "$l" | grep -qE "[0-9]{4}-[0-9]{2}" || { echo "no timestamps in logs"; }
    echo "logs OK"
    return 0
}

test_all_namespaces() {
    local err=""
    k get pods --all-namespaces 2>&1 | grep -q "alpine-pod" || err="$err missing alpine-pod;"
    k get pods --all-namespaces 2>&1 | grep -q "postgres-pod" || err="$err missing postgres-pod;"
    k get deployments --all-namespaces 2>&1 | grep -q "alpine-deploy" || err="$err missing alpine-deploy;"
    k get pv 2>&1 | grep -q "pv-test" || err="$err missing pv-test;"
    k get pvc --all-namespaces 2>&1 | grep -q "pvc-test" || err="$err missing pvc-test;"
    if [[ -n "$err" ]]; then echo "FAIL: $err"; return 1; fi
    echo "all resources visible"
    return 0
}

test_security() {
    local p u e w
    # PID isolation
    p=$(k exec alpine-pod -- ls -la /proc/1/exe 2>&1) || true
    echo "$p" | grep -qiE "systemd|lib/systemd" && { echo "FAIL: /proc/1/exe points to host init"; return 1; }
    echo "PID isolation OK"
    # non-root pod
    kapply apply -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata: { name: sec-pod, namespace: default }
spec:
  securityContext: { runAsUser: 12345, runAsNonRoot: true }
  containers:
  - name: sec
    image: alpine:latest
    command: ["sleep", "60"]
    env:
    - name: USER
      value: testuser
EOF
    wait_pod_ready sec-pod default 60 || { echo "sec-pod not ready"; return 1; }
    u=$(k exec sec-pod -- id 2>&1) || { echo "exec id failed: $u"; return 1; }
    echo "$u" | grep -q "uid=12345" || { echo "unexpected uid: $u"; return 1; }
    echo "non-root uid OK"
    e=$(k exec sec-pod -- sh -c env 2>&1) || { echo "exec env failed: $e"; return 1; }
    echo "$e" | grep -q "USER=testuser" || { echo "MISSING USER env var"; return 1; }
    echo "env var OK"
    w=$(k exec sec-pod -- touch /etc/root-test 2>&1) || true
    echo "$w" | grep -qiE "permission denied|read-only" || { echo "non-root could write to /etc: $w"; return 1; }
    echo "write protection OK"
    k delete pod sec-pod --ignore-not-found 2>/dev/null || true
    echo "security checks passed"
    return 0
}

test_kubectl_cmds() {
    # describe
    k describe pod alpine-pod -n default 2>&1 | grep -q "alpine-pod" || return 1
    k describe deployment alpine-deploy -n default 2>&1 | grep -q "alpine-deploy" || return 1
    k describe svc python-svc -n default 2>&1 | grep -q "python-svc" || return 1
    # expose
    kapply expose deployment nginx-deploy --name=nginx-exposed --port=80 --target-port=80 -n default 2>&1 | grep -qE "created|service" || return 1
    k get svc nginx-exposed -n default 2>&1 | grep -q "nginx-exposed" || return 1
    k delete svc nginx-exposed -n default --ignore-not-found 2>/dev/null || true
    return 0
}

test_api_formats() {
    k get pod alpine-pod -o json 2>&1 | python3 -c "import sys,json; d=json.load(sys.stdin); exit(0 if d.get('kind')=='Pod' else 1)" 2>/dev/null || return 1
    k get pod alpine-pod -o yaml 2>&1 | grep -q "^kind:" || return 1
    [[ "$(k get pod alpine-pod -o jsonpath='{.metadata.name}' 2>/dev/null)" == "alpine-pod" ]] || return 1
    return 0
}

test_all_namespaces() {
    k get pods --all-namespaces 2>&1 | grep -q "alpine-pod" || return 1
    k get pods --all-namespaces 2>&1 | grep -q "postgres-pod" || return 1
    k get deployments --all-namespaces 2>&1 | grep -q "alpine-deploy" || return 1
    k get pv 2>&1 | grep -q "pv-test" || return 1
    k get pvc --all-namespaces 2>&1 | grep -q "pvc-test" || return 1
    return 0
}

test_health() {
    for ep in healthz readyz livez; do
        [[ "$(curl -sf "$SERVER/$ep" 2>&1)" == "ok" ]] || return 1
    done
    curl -sf "$SERVER/version" 2>&1 | grep -q "z8s" || return 1
    return 0
}

# ── Register tests ─────────────────────────────────────────────────────────
test "namespace"       test_namespace
test "configmap"       test_configmap
test "secret"          test_secret
test "pod:alpine"      test_pod_alpine
test "pod:ubuntu"      test_pod_ubuntu
test "pod:python"      test_pod_python
test "pod:postgres"    test_pod_postgres
test "deploy:alpine"   test_deploy_alpine
test "deploy:ubuntu"   test_deploy_ubuntu
test "deploy:python"   test_deploy_python
test "deploy:nginx"    test_deploy_nginx
test "deploy:info"     test_deploy_info
test "scale"           test_scale
test "service"         test_service
test "volume:basic"    test_volume_basic
test "volume:persist"  test_volume_persistence
test "pv-pvc"          test_pv_pvc
test "security"        test_security
test "logs"            test_logs
test "kubectl"         test_kubectl_cmds
test "api"             test_api_formats
test "all-ns"          test_all_namespaces
test "health"          test_health

# ── Test ordering (phases) ────────────────────────────────────────────────
PHASE1=(namespace configmap secret pv-pvc health)           # fast, independent
PHASE2=(pod:alpine pod:ubuntu pod:python pod:postgres)      # depends on pods
PHASE3=(deploy:alpine deploy:ubuntu deploy:python deploy:nginx deploy:info)
PHASE4=(service kubectl)                                    # needs client pod
PHASE5=(scale)                                              # modifies replicas
PHASE6=(volume:basic volume:persist)                        # modifies pods
PHASE7=(security logs api all-ns)                           # independent

ALL_TESTS=("${PHASE1[@]}" "${PHASE2[@]}" "${PHASE3[@]}" "${PHASE4[@]}" "${PHASE5[@]}" "${PHASE6[@]}" "${PHASE7[@]}")

# Resolve selected/skipped
resolve_tests() {
    local -a result=()
    for t in "${ALL_TESTS[@]}"; do
        local include=1
        if [[ -n "$SELECTED" ]]; then
            include=0
            IFS=',' read -ra SEL <<< "$SELECTED"
            for s in "${SEL[@]}"; do
                s="${s// /}"; [[ "$t" == "$s" || "$t" == "$s:"* || "$s" == "$t" ]] && { include=1; break; }
            done
        fi
        if [[ -n "$SKIPPED" ]]; then
            IFS=',' read -ra SKP <<< "$SKIPPED"
            for s in "${SKP[@]}"; do
                s="${s// /}"; [[ "$t" == "$s" || "$t" == "$s:"* || "$s" == "$t" ]] && { include=0; break; }
            done
        fi
        [[ $include -eq 1 ]] && result+=("$t")
    done
    echo "${result[@]}"
}

# ── Main ──────────────────────────────────────────────────────────────────
if [[ $LIST_ONLY -eq 1 ]]; then
    echo "Available test groups:"
    echo "  Phase 1: ${PHASE1[*]}"
    echo "  Phase 2: ${PHASE2[*]}"
    echo "  Phase 3: ${PHASE3[*]}"
    echo "  Phase 4: ${PHASE4[*]}"
    echo "  Phase 5: ${PHASE5[*]}"
    echo "  Phase 6: ${PHASE6[*]}"
    echo "  Phase 7: ${PHASE7[*]}"
    echo ""
    echo "Usage:"
    echo "  $0                              # run all (full setup)"
    echo "  $0 --only pod,svc               # run only pod + service (skip setup)"
    echo "  $0 --skip scale,volume          # run all except scale + volume"
    echo "  $0 --parallel 4                 # max 4 parallel"
    exit 0
fi

# Setup only for full runs (--only skips server restart but still applies YAMLs)
if [[ -z "$SELECTED" ]]; then
    setup
    wait_all
else
    echo "══ Quick setup (apply YAMLs, skip restart) ══"
    for f in "$YAML_DIR"/*.yaml; do
        kapply apply --validate=false -f "$f" 2>/dev/null || true
    done
    echo "══ Quick wait (30s max per pod) ══"
    for pod in alpine-pod:default:30 postgres-pod:default:60 python-pod:default:30 logger-pod:default:30 ubuntu-pod:z8s-test:60; do
        IFS=: read -r name ns timeout <<< "$pod"
        wait_pod_ready "$name" "$ns" "$timeout" && echo "  pod $name Ready" || echo "  pod $name not ready (continuing)"
    done
fi

RESULT_TESTS=$(resolve_tests)
echo "══ Running tests (parallel=${PARALLEL}) ══"

# shellcheck disable=SC2048
for phase in 1 2 3 4 5 6 7; do
    var="PHASE${phase}[@]"
    phase_tests=("${!var}")
    to_run=()
    for t in "${phase_tests[@]}"; do
        for r in $RESULT_TESTS; do
            if [[ "$t" == "$r" ]]; then
                to_run+=("$t")
                break
            fi
        done
    done
    [[ ${#to_run[@]} -eq 0 ]] && continue
    echo "  Phase ${phase}: ${to_run[*]}"
    run_parallel "${to_run[@]}"
done

show_results
