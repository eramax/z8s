# Z8S Multi-Node Runtime Stabilization Fixes

This document details the issues, root causes, and architectural fixes implemented to stabilize pod lifecycle, gossip synchronization, and multi-node network assignment in the Z8S cluster.

## 1. Subnet IP Pool Exhaustion on Worker Nodes

### Symptoms
Worker nodes (`node-7443`) failed to attach pods with the error:
`WARN ThreadId(14) z8s::cri::runtime: NetMux: failed to attach pod Pod/hub-spokes/dep-spoke2-pod-0efd469e: No IPs available in subnet 'spoke-2-sub'`

### Root Cause
Subnet IP pools are held in memory by the `NetMux` component. Previously, a subnet pool was only initialized when `register_subnet_cidr` was invoked by the API server's `on_apply` hook. 
Because `Subnet` resources were created on the hub node (`6443`) and replicated to the worker node (`7443`) via the Gossip protocol, the API `on_apply` hook was never triggered on the worker. The worker node received the subnet data but never initialized the corresponding IP address pool in memory, leading to an immediate "No IPs available" error upon pod assignment.

### Fix
1. **Idempotency**: Modified `NetMux::register_subnet_cidr` in `src/netmux/mod.rs` to be idempotent. It now safely checks if an `IpPool` for the subnet already exists before attempting to allocate a new one, preventing accidental pool resets.
2. **Reconciliation Loop Integration**: Moved the `register_subnet_cidr` invocation out of the `on_apply` hook and into `SubnetResource::reconcile` in `src/components/network/subnet.rs`. Since the reconciliation loop runs continuously on all nodes against their local datastores, any worker node receiving a new subnet via gossip will now correctly initialize the pool memory.

---

## 2. Pods Stuck in "Pending" State (State Sync Gap)

### Symptoms
When a pod was successfully scheduled and started by a remote worker node, the hub node (and `kubectl`) continued to report the pod's status as `Pending`, even though the container was physically running.

### Root Cause
`ProcessTracker` handles starting, stopping, and crashing pods. Whenever it started a pod, it would update the local node's REDB database to set the pod's state to `Running`. 
However, `ProcessTracker` was disconnected from the network layer. It only executed local state changes and never triggered a broadcast event. Consequently, the hub node was completely unaware of the worker node's progress and retained the last known `Pending` state.

### Fix
1. **Broadcast Channel in ProcessTracker**: Introduced an asynchronous multi-producer, single-consumer (MPSC) channel (`broadcast_tx`) directly into the `ProcessTracker` struct (`src/scheduler/process.rs`).
2. **State Emission**: Updated all lifecycle methods (`start_pod`, `stop_pod`, and container crash/reap handlers) to push the updated `AnyResource` onto this broadcast channel immediately after committing the state change to the local database.
3. **Gossip Protocol Wiring**: Updated the node initialization sequence in `src/node.rs`. The receiving end of the MPSC channel is now wired directly to the active `GossipState`. Any pod state changes detected by the `ProcessTracker` are actively pushed to the network layer via `broadcast_write()`, ensuring real-time synchronization back to the hub node.

---

## 3. Storage Layer Initialization Failures

### Symptoms
The scheduler and heartbeat systems intermittently panicked or dropped data upon restart due to "missing directories" or missing state logic.

### Root Cause
The database persistent volume directory (`data_dir`) defaulted to an uninitialized temporary path, which meant components relying on REDB (like heartbeats) failed to bind correctly upon startup. 

### Fix
Standardized the default `data_dir` configuration in `src/config.rs` to point to `/var/lib/z8s`, ensuring robust directory initialization across all nodes.

---

## 4. Scheduling Dead-Node Loop

### Symptoms
The hub node's scheduler would constantly re-assign pods from worker nodes that it briefly considered "dead", resulting in infinite reassignment loops.

### Root Cause
The scheduler loop was assigning pods locally but never broadcasting the updated assignment to the rest of the cluster. Worker nodes never received their assignments, and the hub re-triggered the process.

### Fix
Injected `GossipState` directly into `src/scheduler/scheduler.rs`. Pod assignments are now coupled with an immediate `broadcast_write` to inform the peer nodes the exact millisecond a task is distributed.
