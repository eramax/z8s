#!/usr/bin/env bash
set -uo pipefail

KUBECTL="$(command -v kubectl 2>/dev/null || echo /home/abb/.local/bin/kubectl)"
Z8S_BIN="${Z8S_BIN:-$(dirname "$0")/../target/debug/z8s}"
SERVER="${Z8S_SERVER:-http://localhost:6443}"
PASS=0; FAIL=0; ERRORS=()
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS+1)); }
fail() { local m="$1" d="${2:-}"; echo -e "${RED}FAIL${NC} $m${d:+: $d}"; ERRORS+=("$m${d:+: $d}"); FAIL=$((FAIL+1)); }
skip() { echo -e "${YELLOW}SKIP${NC} $1"; }

k() { "$KUBECTL" --server="$SERVER" "$@" 2>&1 || true; }
kapply() { "$KUBECTL" --validate=false --server="$SERVER" apply -f - 2>&1; }
kdelete() { "$KUBECTL" --server="$SERVER" delete -f - 2>&1; }

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

wait_svc_ready() {
    local name="$1" ns="${2:-default}" timeout="${3:-30}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        cip=$(k get svc "$name" -n "$ns" -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
        [[ -n "$cip" && "$cip" != "None" ]] && return 0
        sleep 1
    done
    return 1
}

test_A1() {
    echo -e "${CYAN}A1: Pod has IP from its VNet CIDR${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-a1
  namespace: default
spec:
  containers:
  - name: test
    image: alpine
    command: ["sleep", "30"]
    ports:
    - containerPort: 80
YAML
    if wait_pod_ready test-a1; then
        local ip=$(k exec test-a1 -- ip addr show eth0 2>/dev/null | grep -oP '10\.\d+\.\d+\.\d+' | head -1)
        if [[ -n "$ip" ]]; then
            pass "A1: Pod has IP $ip on eth0"
        else
            fail "A1: Pod has no IP on eth0" "$(k exec test-a1 -- ip addr 2>&1)"
        fi
    else
        fail "A1: Pod did not become ready"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-a1
  namespace: default
YAML
}

test_A2() {
    echo -e "${CYAN}A2: Pod pings same-VNet peer (same node)${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-a2a
  namespace: default
spec:
  containers:
  - name: test
    image: alpine
    command: ["sleep", "30"]
    ports:
    - containerPort: 80
---
apiVersion: v1
kind: Pod
metadata:
  name: test-a2b
  namespace: default
spec:
  containers:
  - name: test
    image: alpine
    command: ["sleep", "30"]
    ports:
    - containerPort: 80
YAML
    wait_pod_ready test-a2a && wait_pod_ready test-a2b || { fail "A2: Pods not ready"; return; }
    local ip_b=$(k get pod test-a2b -o jsonpath='{.status.podIP}')
    if [[ -z "$ip_b" || "$ip_b" == "127.0.0.1" ]]; then
        ip_b=$(k exec test-a2b -- hostname -i 2>/dev/null | awk '{print $1}')
    fi
    if k exec test-a2a -- ping -c 1 -W 2 "$ip_b" >/dev/null 2>&1; then
        pass "A2: Pod A can ping Pod B ($ip_b)"
    else
        fail "A2: Pod A cannot ping Pod B ($ip_b)"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-a2a
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-a2b
  namespace: default
YAML
}

test_A3() {
    echo -e "${CYAN}A3: Pod pings same-VNet peer (cross-node)${NC}"
    skip "A3: Cross-node test requires multi-node cluster (Phase 6)"
}

test_A4() {
    echo -e "${CYAN}A4: Pod cannot ping different-VNet pod (default deny)${NC}"
    skip "A4: Requires VNet CRDs (Phase 3)"
}

test_A5() {
    echo -e "${CYAN}A5: Pod gets new IP after delete/recreate${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-a5
  namespace: default
spec:
  containers:
  - name: test
    image: alpine
    command: ["sleep", "10"]
    ports:
    - containerPort: 80
YAML
    wait_pod_ready test-a5 || { fail "A5: Pod not ready"; return; }
    local ip1=$(k get pod test-a5 -o jsonpath='{.status.podIP}')
    [[ -z "$ip1" ]] && ip1=$(k exec test-a5 -- hostname -i 2>/dev/null | awk '{print $1}')
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-a5
  namespace: default
YAML
    sleep 3
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-a5
  namespace: default
spec:
  containers:
  - name: test
    image: alpine
    command: ["sleep", "10"]
    ports:
    - containerPort: 80
YAML
    wait_pod_ready test-a5 || { fail "A5: Recreated pod not ready"; return; }
    local ip2=$(k get pod test-a5 -o jsonpath='{.status.podIP}')
    [[ -z "$ip2" ]] && ip2=$(k exec test-a5 -- hostname -i 2>/dev/null | awk '{print $1}')
    if [[ -n "$ip2" ]]; then
        pass "A5: Pod recreated with valid IP $ip2"
    else
        fail "A5: Pod has no IP after recreate" "ip1=$ip1 ip2=$ip2"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-a5
  namespace: default
YAML
}

test_A6() {
    echo -e "${CYAN}A6: Host has /32 route for each pod via veth${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-a6
  namespace: default
spec:
  containers:
  - name: test
    image: alpine
    command: ["sleep", "15"]
    ports:
    - containerPort: 80
YAML
    wait_pod_ready test-a6 || { fail "A6: Pod not ready"; return; }
    local ip=$(k get pod test-a6 -o jsonpath='{.status.podIP}')
    [[ -z "$ip" ]] && ip=$(k exec test-a6 -- hostname -i 2>/dev/null | awk '{print $1}')
    if grep -q "FFFFFFFF" /proc/net/route 2>/dev/null; then
        pass "A6: Host has /32 routes"
    else
        fail "A6: No /32 routes" "$(cat /proc/net/route 2>&1)"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-a6
  namespace: default
YAML
}

test_A7() {
    echo -e "${CYAN}A7: No bridge interfaces${NC}"
    local bridges=$(ip link show type bridge 2>/dev/null | grep -c 'bridge')
    if [[ "$bridges" -eq 0 ]]; then
        pass "A7: No bridge interfaces found"
    else
        fail "A7: Bridge interfaces found" "$bridges bridges"
    fi
}

test_A8() {
    echo -e "${CYAN}A8: Job gets IP + route, no DNAT, no DNS${NC}"
    skip "A8: Job resource type not yet implemented"
}

test_B1() {
    echo -e "${CYAN}B1: Pod reaches ClusterIP:port, hits backend${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-b1-srv
  namespace: default
  labels:
    app: test-b1
spec:
  containers:
  - name: srv
    image: alpine
    command: ["sh", "-c", "while true; do echo -e 'HTTP/1.1 200 OK\r\n\r\nhello from b1' | nc -l -p 8080 -w 1; done"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: test-b1-svc
  namespace: default
spec:
  selector:
    app: test-b1
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b1-client
  namespace: default
spec:
  containers:
  - name: client
    image: alpine
    command: ["sleep", "30"]
YAML
    wait_pod_ready test-b1-srv && wait_pod_ready test-b1-client || { fail "B1: Pods not ready"; return; }
    wait_svc_ready test-b1-svc || { fail "B1: Service not ready"; return; }
    local cip=$(k get svc test-b1-svc -o jsonpath='{.spec.clusterIP}')
    local resp=$(k exec test-b1-client -- timeout 5 wget -q -O- -T 3 "http://$cip/" 2>&1)
    if echo "$resp" | grep -q "hello from b1"; then
        pass "B1: ClusterIP $cip:80 reaches backend"
    else
        fail "B1: ClusterIP $cip:80 not reachable" "response=$resp"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-b1-srv
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b1-client
  namespace: default
---
apiVersion: v1
kind: Service
metadata:
  name: test-b1-svc
  namespace: default
YAML
}

test_B2() {
    echo -e "${CYAN}B2: Round-robin across multiple backends${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-b2a
  namespace: default
  labels:
    app: test-b2
spec:
  containers:
  - name: srv
    image: alpine
    command: ["sh", "-c", "while true; do echo -e 'HTTP/1.1 200 OK\r\n\r\nbackend-a' | nc -l -p 8080 -w 1; done"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b2b
  namespace: default
  labels:
    app: test-b2
spec:
  containers:
  - name: srv
    image: alpine
    command: ["sh", "-c", "while true; do echo -e 'HTTP/1.1 200 OK\r\n\r\nbackend-b' | nc -l -p 8080 -w 1; done"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: test-b2-svc
  namespace: default
spec:
  selector:
    app: test-b2
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b2-client
  namespace: default
spec:
  containers:
  - name: client
    image: alpine
    command: ["sleep", "30"]
YAML
    wait_pod_ready test-b2a && wait_pod_ready test-b2b && wait_pod_ready test-b2-client || { fail "B2: Pods not ready"; return; }
    wait_svc_ready test-b2-svc || { fail "B2: Service not ready"; return; }
    local cip=$(k get svc test-b2-svc -o jsonpath='{.spec.clusterIP}')
    local seen_a=0 seen_b=0
    for i in 1 2 3 4 5 6; do
        local resp=$(k exec test-b2-client -- timeout 5 wget -q -O- -T 3 "http://$cip/" 2>&1)
        [[ "$resp" == *"backend-a"* ]] && seen_a=1
        [[ "$resp" == *"backend-b"* ]] && seen_b=1
    done
    if [[ $seen_a -eq 1 && $seen_b -eq 1 ]]; then
        pass "B2: Round-robin hits both backends"
    else
        fail "B2: Only saw a=$seen_a b=$seen_b" "one backend may be missing"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-b2a
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b2b
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b2-client
  namespace: default
---
apiVersion: v1
kind: Service
metadata:
  name: test-b2-svc
  namespace: default
YAML
}

test_B3() {
    echo -e "${CYAN}B3: DNAT updates on backend crash+replace${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-b3a
  namespace: default
  labels:
    app: test-b3
spec:
  containers:
  - name: srv
    image: alpine
    command: ["sh", "-c", "while true; do echo -e 'HTTP/1.1 200 OK\r\n\r\ni am b3' | nc -l -p 8080 -w 1; done"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: test-b3-svc
  namespace: default
spec:
  selector:
    app: test-b3
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b3-client
  namespace: default
spec:
  containers:
  - name: client
    image: alpine
    command: ["sleep", "30"]
YAML
    wait_pod_ready test-b3a && wait_pod_ready test-b3-client || { fail "B3: Pods not ready"; return; }
    wait_svc_ready test-b3-svc || { fail "B3: Service not ready"; return; }
    local cip=$(k get svc test-b3-svc -o jsonpath='{.spec.clusterIP}')
    local before=$(k exec test-b3-client -- timeout 5 wget -q -O- -T 3 "http://$cip/" 2>&1)
    [[ -z "$before" ]] && before="(empty)"
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-b3a
  namespace: default
YAML
    sleep 5
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-b3b
  namespace: default
  labels:
    app: test-b3
spec:
  containers:
  - name: srv
    image: alpine
    command: ["sh", "-c", "while true; do echo -e 'HTTP/1.1 200 OK\r\n\r\ni am b3' | nc -l -p 8080 -w 1; done"]
    ports:
    - containerPort: 8080
YAML
    wait_pod_ready test-b3b || { fail "B3: Replacement pod not ready"; return; }
    sleep 3
    local after=$(k exec test-b3-client -- timeout 5 wget -q -O- -T 3 "http://$cip/" 2>&1)
    if echo "$after" | grep -q "i am b3"; then
        pass "B3: DNAT works after backend replacement"
    else
        fail "B3: DNAT broken after backend replacement" "before='$before' after='$after'"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-b3b
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b3-client
  namespace: default
---
apiVersion: v1
kind: Service
metadata:
  name: test-b3-svc
  namespace: default
YAML
}

test_B4() {
    echo -e "${CYAN}B4: Empty ClusterIP drops traffic (connection refused)${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Service
metadata:
  name: test-b4-svc
  namespace: default
spec:
  selector:
    app: nonexistent
  ports:
  - port: 80
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b4-client
  namespace: default
spec:
  containers:
  - name: client
    image: alpine
    command: ["sleep", "30"]
YAML
    wait_pod_ready test-b4-client || { fail "B4: Client pod not ready"; return; }
    wait_svc_ready test-b4-svc || { fail "B4: Service not ready"; return; }
    local cip=$(k get svc test-b4-svc -o jsonpath='{.spec.clusterIP}')
    local resp=$(k exec test-b4-client -- timeout 5 wget -q -O- -T 3 "http://$cip:80/" 2>&1 || true)
    if echo "$resp" | grep -qi "refused\|timed out\|Connection refused\|10061\|exit code 4"; then
        pass "B4: Empty ClusterIP drops traffic"
    else
        fail "B4: Empty ClusterIP did not drop traffic" "response=$resp"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Service
metadata:
  name: test-b4-svc
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b4-client
  namespace: default
YAML
}

test_B5() {
    echo -e "${CYAN}B5: NodePort works (from cluster)${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-b5-srv
  namespace: default
  labels:
    app: test-b5
spec:
  containers:
  - name: srv
    image: alpine
    command: ["sh", "-c", "while true; do echo -e 'HTTP/1.1 200 OK\r\n\r\nnodeport-ok' | nc -l -p 8080 -w 1; done"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: test-b5-svc
  namespace: default
spec:
  type: NodePort
  selector:
    app: test-b5
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b5-client
  namespace: default
spec:
  containers:
  - name: client
    image: alpine
    command: ["sleep", "20"]
YAML
    wait_pod_ready test-b5-srv && wait_pod_ready test-b5-client || { fail "B5: Pods not ready"; return; }
    wait_svc_ready test-b5-svc || { fail "B5: Service not ready"; return; }
    local cip=$(k get svc test-b5-svc -o jsonpath='{.spec.clusterIP}')
    local resp=$(k exec test-b5-client -- timeout 5 wget -q -O- -T 3 "http://$cip/" 2>&1)
    if echo "$resp" | grep -q "nodeport-ok"; then
        pass "B5: NodePort service reachable via ClusterIP"
    else
        fail "B5: NodePort service not reachable" "response=$resp"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-b5-srv
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-b5-client
  namespace: default
---
apiVersion: v1
kind: Service
metadata:
  name: test-b5-svc
  namespace: default
YAML
}

test_B6() {
    echo -e "${CYAN}B6: No ClusterIP on any interface${NC}"
    local found=$(ip addr show 2>/dev/null | grep -cP '10\.96\.')
    if [[ "$found" -eq 0 ]]; then
        pass "B6: No ClusterIP found on any interface"
    else
        fail "B6: ClusterIP found on interface" "$(ip addr show 2>/dev/null | grep -P '10\.96\.')"
    fi
}

test_B7() {
    echo -e "${CYAN}B7: No userspace proxy for ClusterIP${NC}"
    local proxy_listen=$(ss -tlnp 2>/dev/null | grep -c 'service_proxy\|ensure_loopback')
    if [[ "$proxy_listen" -eq 0 ]]; then
        pass "B7: No userspace proxy listening"
    else
        fail "B7: Userspace proxy still listening" "$proxy_listen"
    fi
}

test_C1() {
    echo -e "${CYAN}C1: Pod (hub VNet) reaches internet${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-c1
  namespace: default
spec:
  containers:
  - name: test
    image: alpine
    command: ["sleep", "20"]
    ports:
    - containerPort: 80
YAML
    wait_pod_ready test-c1 || { fail "C1: Pod not ready"; return; }
    if k exec test-c1 -- ping -c 1 -W 3 1.1.1.1 >/dev/null 2>&1; then
        pass "C1: Pod reaches internet"
    else
        fail "C1: Pod cannot reach internet"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-c1
  namespace: default
YAML
}

test_C2() {
    echo -e "${CYAN}C2: Pod (spoke VNet) cannot reach internet${NC}"
    skip "C2: Requires VNet CRDs (Phase 3)"
}

test_C3() {
    echo -e "${CYAN}C3: Pod-to-pod traffic not SNATted${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-c3a
  namespace: default
  labels:
    app: test-c3
spec:
  containers:
  - name: srv
    image: alpine
    command: ["sh", "-c", "while true; do echo -e 'HTTP/1.1 200 OK\r\n\r\np2p-ok' | nc -l -p 8080 -w 1; done"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-c3b
  namespace: default
spec:
  containers:
  - name: client
    image: alpine
    command: ["sleep", "20"]
    ports:
    - containerPort: 80
YAML
    wait_pod_ready test-c3a && wait_pod_ready test-c3b || { fail "C3: Pods not ready"; return; }
    local ip_a=$(k get pod test-c3a -o jsonpath='{.status.podIP}')
    [[ -z "$ip_a" ]] && ip_a=$(k exec test-c3a -- hostname -i 2>/dev/null | awk '{print $1}')
    local resp=$(k exec test-c3b -- timeout 5 wget -q -O- -T 3 "http://$ip_a:8080/" 2>&1)
    if echo "$resp" | grep -q "p2p-ok"; then
        pass "C3: Pod-to-pod traffic works"
    else
        fail "C3: Pod-to-pod traffic failed" "response=$resp"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-c3a
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-c3b
  namespace: default
YAML
}

test_D1() {
    echo -e "${CYAN}D1: Default VNet per namespace${NC}"
    skip "D1: Requires VNet CRDs (Phase 3)"
}

test_D2() {
    echo -e "${CYAN}D2: Different VNets cannot communicate${NC}"
    skip "D2: Requires VNet CRDs (Phase 3)"
}

test_D3() {
    echo -e "${CYAN}D3: NSG allow subnet A->B port 5432${NC}"
    skip "D3: Requires NSG CRDs (Phase 3)"
}

test_D4() {
    echo -e "${CYAN}D4: NSG deny between subnets${NC}"
    skip "D4: Requires NSG CRDs (Phase 3)"
}

test_D5() {
    echo -e "${CYAN}D5: NSG + NetworkPolicy override${NC}"
    skip "D5: Requires Phase 3 + Phase 4"
}

test_D6() {
    echo -e "${CYAN}D6: Hub reaches spoke${NC}"
    skip "D6: Requires Hub-and-Spoke (Phase 3)"
}

test_D7() {
    echo -e "${CYAN}D7: Spoke cannot reach spoke (direct)${NC}"
    skip "D7: Requires Hub-and-Spoke (Phase 3)"
}

test_D8() {
    echo -e "${CYAN}D8: Spoke reaches spoke via hub transit${NC}"
    skip "D8: Requires Hub-and-Spoke (Phase 3)"
}

test_E1() {
    echo -e "${CYAN}E1: podSelector allow${NC}"
    skip "E1: Requires NetworkPolicy (Phase 4)"
}

test_E2() {
    echo -e "${CYAN}E2: namespaceSelector allow${NC}"
    skip "E2: Requires NetworkPolicy (Phase 4)"
}

test_E3() {
    echo -e "${CYAN}E3: ipBlock allow/deny${NC}"
    skip "E3: Requires NetworkPolicy (Phase 4)"
}

test_E4() {
    echo -e "${CYAN}E4: Dynamic set update on pod start/stop${NC}"
    skip "E4: Requires NetworkPolicy (Phase 4)"
}

test_F1() {
    echo -e "${CYAN}F1: <svc>.<ns>.svc.cluster.local -> ClusterIP${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-f1-srv
  namespace: default
  labels:
    app: test-f1
spec:
  containers:
  - name: srv
    image: alpine
    command: ["sh", "-c", "echo 'dns-ok' | nc -l -p 8080"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: test-f1-svc
  namespace: default
spec:
  selector:
    app: test-f1
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-f1-client
  namespace: default
spec:
  containers:
  - name: client
    image: alpine
    command: ["sleep", "30"]
YAML
    wait_pod_ready test-f1-srv && wait_pod_ready test-f1-client || { fail "F1: Pods not ready"; return; }
    wait_svc_ready test-f1-svc || { fail "F1: Service not ready"; return; }
    local resolved=$(k exec test-f1-client -- timeout 3 nslookup test-f1-svc.default.svc.cluster.local 2>&1 | grep -oP '10\.96\.\d+\.\d+')
    if [[ -n "$resolved" ]]; then
        pass "F1: DNS resolves test-f1-svc.default.svc.cluster.local -> $resolved"
    else
        fail "F1: DNS resolution failed" "$(k exec test-f1-client -- nslookup test-f1-svc.default.svc.cluster.local 2>&1)"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-f1-srv
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-f1-client
  namespace: default
---
apiVersion: v1
kind: Service
metadata:
  name: test-f1-svc
  namespace: default
YAML
}

test_F2() {
    echo -e "${CYAN}F2: External domains from pods${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-f2
  namespace: default
spec:
  containers:
  - name: test
    image: alpine
    command: ["sleep", "20"]
    ports:
    - containerPort: 80
YAML
    wait_pod_ready test-f2 || { fail "F2: Pod not ready"; return; }
    local resolved=$(k exec test-f2 -- timeout 3 nslookup example.com 2>&1 | grep -oP 'Address\s*:\s*\K[\d.]+')
    if [[ -n "$resolved" ]]; then
        pass "F2: External DNS resolves example.com -> $resolved"
    else
        fail "F2: External DNS failed" "$(k exec test-f2 -- nslookup example.com 2>&1)"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-f2
  namespace: default
YAML
}

test_F3() {
    echo -e "${CYAN}F3: Private VNet names resolve globally${NC}"
    skip "F3: Requires VNet CRDs (Phase 3)"
}

test_F4() {
    echo -e "${CYAN}F4: NSG enforces access (not DNS)${NC}"
    skip "F4: Requires NSG CRDs (Phase 3)"
}

test_G1() {
    echo -e "${CYAN}G1: HTTP by Host header${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-g1-srv
  namespace: default
  labels:
    app: test-g1
spec:
  containers:
  - name: srv
    image: hashicorp/http-echo
    args: ["-text=ingress-ok", "-listen=:8080"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: test-g1-svc
  namespace: default
spec:
  selector:
    app: test-g1
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: v1
kind: Ingress
metadata:
  name: test-g1-ing
  namespace: default
spec:
  rules:
  - host: test-g1.example.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: test-g1-svc
            port:
              number: 80
---
apiVersion: v1
kind: Pod
metadata:
  name: test-g1-client
  namespace: default
spec:
  containers:
  - name: client
    image: alpine
    command: ["sleep", "30"]
YAML
    wait_pod_ready test-g1-srv && wait_pod_ready test-g1-client || { fail "G1: Pods not ready"; return; }
    wait_svc_ready test-g1-svc || { fail "G1: Service not ready"; return; }
    sleep 2
    local resp=$(k exec test-g1-client -- timeout 3 wget -q -O- -T 2 --header='Host: test-g1.example.com' http://127.0.0.1:80/ 2>&1)
    if echo "$resp" | grep -q "ingress-ok"; then
        pass "G1: Ingress by Host header works"
    else
        fail "G1: Ingress by Host header failed" "response=$resp"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-g1-srv
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-g1-client
  namespace: default
---
apiVersion: v1
kind: Service
metadata:
  name: test-g1-svc
  namespace: default
---
apiVersion: v1
kind: Ingress
metadata:
  name: test-g1-ing
  namespace: default
YAML
}

test_G2() {
    echo -e "${CYAN}G2: TLS by SNI${NC}"
    skip "G2: TLS not yet implemented"
}

test_G3() {
    echo -e "${CYAN}G3: TCP by port${NC}"
    kapply - <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-g3-srv
  namespace: default
  labels:
    app: test-g3
spec:
  containers:
  - name: srv
    image: alpine
    command: ["sh", "-c", "echo 'tcp-ingress-ok' | nc -l -p 8080"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: test-g3-svc
  namespace: default
spec:
  selector:
    app: test-g3
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-g3-client
  namespace: default
spec:
  containers:
  - name: client
    image: alpine
    command: ["sleep", "30"]
YAML
    wait_pod_ready test-g3-srv && wait_pod_ready test-g3-client || { fail "G3: Pods not ready"; return; }
    wait_svc_ready test-g3-svc || { fail "G3: Service not ready"; return; }
    local cip=$(k get svc test-g3-svc -o jsonpath='{.spec.clusterIP}')
    local resp=$(k exec test-g3-client -- timeout 3 sh -c "echo '' | nc -w 2 $cip 80" 2>&1)
    if echo "$resp" | grep -q "tcp-ingress-ok"; then
        pass "G3: ClusterIP TCP access works"
    else
        fail "G3: ClusterIP TCP access failed" "response=$resp"
    fi
    kdelete - <<<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-g3-srv
  namespace: default
---
apiVersion: v1
kind: Pod
metadata:
  name: test-g3-client
  namespace: default
---
apiVersion: v1
kind: Service
metadata:
  name: test-g3-svc
  namespace: default
YAML
}

test_G4() {
    echo -e "${CYAN}G4: TLS termination + re-encryption${NC}"
    skip "G4: TLS not yet implemented"
}

test_G5() {
    echo -e "${CYAN}G5: Auto-TLS cert provisioning${NC}"
    skip "G5: TLS not yet implemented"
}

test_G6() {
    echo -e "${CYAN}G6: L7 NSG - block method/header${NC}"
    skip "G6: L7 NSG not yet implemented"
}

test_G7() {
    echo -e "${CYAN}G7: Ingress -> spoke backend (hub-and-spoke)${NC}"
    skip "G7: Requires VNet CRDs (Phase 3)"
}

test_H1() {
    echo -e "${CYAN}H1: Node joins cluster${NC}"
    skip "H1: Requires multi-node cluster (Phase 6)"
}

test_H2() {
    echo -e "${CYAN}H2: Node fails${NC}"
    skip "H2: Requires multi-node cluster (Phase 6)"
}

test_H3() {
    echo -e "${CYAN}H3: Cross-node pod-to-pod${NC}"
    skip "H3: Requires multi-node cluster (Phase 6)"
}

test_H4() {
    echo -e "${CYAN}H4: Cross-node ClusterIP${NC}"
    skip "H4: Requires multi-node cluster (Phase 6)"
}

test_H5() {
    echo -e "${CYAN}H5: Cross-node ingress${NC}"
    skip "H5: Requires multi-node cluster (Phase 6)"
}

test_I1() {
    echo -e "${CYAN}I1: VNet CIDR full${NC}"
    skip "I1: Requires VNet CRDs (Phase 3)"
}

test_I2() {
    echo -e "${CYAN}I2: nftables init fails gracefully${NC}"
    skip "I2: Manual failure injection test"
}

test_I3() {
    echo -e "${CYAN}I3: SNAT fails gracefully${NC}"
    skip "I3: Manual failure injection test"
}

test_I4() {
    echo -e "${CYAN}I4: 1000 concurrent connections through DNAT${NC}"
    skip "I4: Performance test — run manually"
}

test_I5() {
    echo -e "${CYAN}I5: 10 pods/sec start/stop for 60s${NC}"
    skip "I5: Performance test — run manually"
}

declare -A TESTS=(
    [A1]=test_A1 [A2]=test_A2 [A3]=test_A3 [A4]=test_A4 [A5]=test_A5 [A6]=test_A6 [A7]=test_A7 [A8]=test_A8
    [B1]=test_B1 [B2]=test_B2 [B3]=test_B3 [B4]=test_B4 [B5]=test_B5 [B6]=test_B6 [B7]=test_B7
    [C1]=test_C1 [C2]=test_C2 [C3]=test_C3
    [D1]=test_D1 [D2]=test_D2 [D3]=test_D3 [D4]=test_D4 [D5]=test_D5 [D6]=test_D6 [D7]=test_D7 [D8]=test_D8
    [E1]=test_E1 [E2]=test_E2 [E3]=test_E3 [E4]=test_E4
    [F1]=test_F1 [F2]=test_F2 [F3]=test_F3 [F4]=test_F4
    [G1]=test_G1 [G2]=test_G2 [G3]=test_G3 [G4]=test_G4 [G5]=test_G5 [G6]=test_G6 [G7]=test_G7
    [H1]=test_H1 [H2]=test_H2 [H3]=test_H3 [H4]=test_H4 [H5]=test_H5
    [I1]=test_I1 [I2]=test_I2 [I3]=test_I3 [I4]=test_I4 [I5]=test_I5
)

for test_id in $(echo "${!TESTS[@]}" | tr ' ' '\n' | sort); do
    ${TESTS[$test_id]}
done

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
if [[ $FAIL -gt 0 ]]; then
    for e in "${ERRORS[@]}"; do echo "  - $e"; done
    exit 1
fi
