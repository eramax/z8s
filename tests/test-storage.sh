#!/usr/bin/env bash
set -uo pipefail

SERVER="${Z8S_SERVER:-http://localhost:6443}"
NS="storage-test"
GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $1"; }
fail() { echo -e "${RED}FAIL${NC} $1"; exit 1; }
k()   { kubectl --server="$SERVER" "$@" 2>/dev/null; }
check() { local m="$1"; shift; "$@" && pass "$m" || fail "$m"; }

PV_BASE="/var/lib/z8s/pv"
PVC_NAME="storage-test-claim"

cleanup() {
  for pid in $(fuser "${SP:-12345}/tcp" 2>/dev/null | xargs); do kill "$pid" 2>/dev/null || true; done
  kubectl delete ns "$NS" --ignore-not-found --wait=false 2>/dev/null || true
}

cleanup

kubectl delete ns "$NS" --ignore-not-found --wait=false 2>/dev/null || true
sleep 1
kubectl create ns "$NS" 2>/dev/null || true

echo -e "${CYAN}═══ 1: Create PVC with storageClassName: standard ═══${NC}"
k apply --validate=false -f - <<YAML
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: $PVC_NAME
  namespace: $NS
spec:
  accessModes: [ReadWriteOnce]
  resources: { requests: { storage: 10Mi } }
  storageClassName: standard
YAML

for i in $(seq 1 20); do
  phase=$(k get pvc "$PVC_NAME" -n "$NS" -o jsonpath='{.status.phase}' 2>/dev/null || echo "")
  [ "$phase" = "Bound" ] && break
  sleep 1
done
check "PVC Bound" test "$(k get pvc "$PVC_NAME" -n "$NS" -o jsonpath='{.status.phase}' 2>/dev/null)" = "Bound"
pv_name=$(k get pvc "$PVC_NAME" -n "$NS" -o jsonpath='{.spec.volumeName}' 2>/dev/null)
check "volumeName set" test -n "$pv_name"
check "PV status Bound" test "$(k get pv "$pv_name" -o jsonpath='{.status.phase}' 2>/dev/null)" = "Bound"
HOST_PATH="${PV_BASE}/${pv_name}"
check "hostPath exists" test -d "$HOST_PATH"
echo "  PV=$pv_name  hostPath=$HOST_PATH"

echo -e "${CYAN}═══ 2: Create deployment mounting the PVC ═══${NC}"
k apply --validate=false -f - <<YAML
apiVersion: apps/v1
kind: Deployment
metadata:
  name: storage-test
  namespace: $NS
spec:
  replicas: 1
  selector:
    matchLabels:
      app: storage-test
  template:
    metadata:
      labels:
        app: storage-test
    spec:
      containers:
      - name: app
        image: alpine:latest
        command: ["/bin/sh", "-c"]
        args:
          - echo 'ready' && exec sleep infinity
        volumeMounts:
        - name: data
          mountPath: /mnt/data
      volumes:
      - name: data
        persistentVolumeClaim:
          claimName: $PVC_NAME
YAML

for i in $(seq 1 30); do
  r=$(k get deployment storage-test -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo 0)
  [ "${r:-0}" -ge 1 ] && break
  sleep 2
done
POD=$(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/||p' | grep storage-test | head -1)
check "Pod ready" test -n "$POD"

echo -e "${CYAN}═══ 3: Write file from pod, verify on host ═══${NC}"
k exec -n "$NS" "$POD" -- sh -c 'echo "hello-from-pod" > /mnt/data/test.txt'
sleep 1
check "file exists on host" test -f "$HOST_PATH/test.txt"
check "content matches" grep -q "hello-from-pod" "$HOST_PATH/test.txt"

echo -e "${CYAN}═══ 4: Write file from host, verify in pod ═══${NC}"
echo "written-from-host" | tee "$HOST_PATH/host-file.txt" >/dev/null
check "host-file visible in pod" test "$(k exec -n "$NS" "$POD" -- cat /mnt/data/host-file.txt 2>/dev/null)" = "written-from-host"

echo -e "${CYAN}═══ 5: Delete file from pod, verify gone on host ═══${NC}"
k exec -n "$NS" "$POD" -- rm /mnt/data/host-file.txt
sleep 1
check "host-file gone from host" test ! -f "$HOST_PATH/host-file.txt"

echo -e "${CYAN}═══ 6: Scale to 2 replicas, verify persistence ═══${NC}"
k scale deployment storage-test -n "$NS" --replicas=2 >/dev/null
for i in $(seq 1 20); do
  r=$(k get pods -n "$NS" 2>/dev/null | grep 'storage-test-' | grep -c 'Running' || true)
  [ "${r:-0}" -ge 2 ] && break
  sleep 2
done
POD2=$(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/||p' | grep storage-test | tail -1)
for i in 1 2 3 4 5; do k exec -n "$NS" "$POD2" -- true 2>/dev/null && break || sleep 2; done
check "test.txt in new pod" test "$(k exec -n "$NS" "$POD2" -- cat /mnt/data/test.txt 2>/dev/null)" = "hello-from-pod"

echo -e "${CYAN}═══ 7: Write from new pod, verify on host ═══${NC}"
k exec -n "$NS" "$POD2" -- sh -c 'echo "written-from-pod2" > /mnt/data/pod2-file.txt'
sleep 1
check "pod2-file on host" test -f "$HOST_PATH/pod2-file.txt"
check "pod2-file content" grep -q "written-from-pod2" "$HOST_PATH/pod2-file.txt"

echo -e "${CYAN}═══ 8: Delete file from host, verify gone in pod ═══${NC}"
rm "$HOST_PATH/pod2-file.txt"
sleep 1
check "pod2-file gone from pods" test ! -f "$HOST_PATH/pod2-file.txt"

echo -e "${CYAN}═══ 9: Scale to 0, then back to 1 ═══${NC}"
k scale deployment storage-test -n "$NS" --replicas=0 >/dev/null
for i in $(seq 1 20); do
  r=$(k get pods -n "$NS" 2>/dev/null | grep -c 'storage-test-' || true)
  [ "${r:-0}" -eq 0 ] && break
  sleep 2
done
check "all pods gone" test "$(k get pods -n "$NS" 2>/dev/null | grep -c 'storage-test-' || true)" -eq 0

k scale deployment storage-test -n "$NS" --replicas=1 >/dev/null
for i in $(seq 1 30); do
  POD3=$(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/||p' | grep storage-test | head -1)
  [ -n "$POD3" ] && k exec -n "$NS" "$POD3" -- true 2>/dev/null && break
  sleep 2
done
check "new pod ready" test -n "$POD3"

echo -e "${CYAN}═══ 10: Verify files persist across full scale-down ═══${NC}"
check "test.txt after rescale" test "$(k exec -n "$NS" "$POD3" -- cat /mnt/data/test.txt 2>/dev/null)" = "hello-from-pod"
check "host-file gone (was deleted)" test ! -f "$HOST_PATH/host-file.txt"

echo -e "${CYAN}═══ 11: Write from pod after rescale, check host ═══${NC}"
k exec -n "$NS" "$POD3" -- sh -c 'echo "after-rescale" > /mnt/data/rescale.txt'
sleep 1
check "rescale.txt on host" test -f "$HOST_PATH/rescale.txt"
check "rescale.txt content" grep -q "after-rescale" "$HOST_PATH/rescale.txt"

echo -e "${CYAN}═══ 12: Verify mount point exists ═══${NC}"
check "mount point present" mountpoint -q "$HOST_PATH"

echo -e "${GREEN}════════════════════════════════════════════${NC}"
echo -e "${GREEN}  STORAGE TEST PASSED${NC}"
echo -e "${GREEN}════════════════════════════════════════════${NC}"
kubectl delete ns "$NS" --ignore-not-found --wait=false 2>/dev/null || true
