# z8s — Definition of Done (DoD)

> This document is the **mandatory quality gate** for every task, PR, or AI-generated code change in the `z8s` project.
> Every item below must pass before any code is considered done. No exceptions.

***

## 1. Dependency Governance

- **No new dependency may be added without explicit human approval.** Before adding any crate to `[dependencies]` or `[dev-dependencies]`, the agent must stop, state the crate name, its purpose, its binary size impact, and wait for approval.
- **Dev dependencies** follow the same rule — even test helpers and build tools require approval.
- **Transitive dependency count must not silently grow.** Run `cargo tree` and report the delta before and after.
- **Prefer crates from the existing stack** (`tokio`, `serde`, `axum`, `nix`, `anyhow`, `tracing`, `k8s-openapi`) before reaching for a new one.
- **Justify every dependency**: if a feature can be implemented in <50 lines of Rust without a new crate, do not add the crate.

***

## 2. Memory Safety

- **No `unsafe` blocks** unless there is no safe alternative AND the block is:
  - Wrapped in a clearly named safe abstraction function.
  - Documented with a `// SAFETY:` comment explaining every invariant.
  - Reviewed and approved before merge.
- **No raw pointer arithmetic** outside of verified FFI boundaries.
- **No `std::mem::transmute`** unless it is provably sound and documented.
- **Prefer borrowing over cloning.** Every `.clone()` call must be justified in a comment. If a value can be passed by reference, it must be.
- **No unnecessary heap allocation.** Prefer stack allocation, slices, and references when the lifetime allows it.
- **No unbounded buffers.** Every read, recv, or collect must have an explicit size limit or a documented rationale for why it is safe.

***

## 3. Performance

- **No blocking calls on the async runtime.** All blocking I/O or CPU-heavy work must be dispatched via `tokio::task::spawn_blocking` or `tokio::task::block_in_place`.
- **Use `io_uring` via `tokio-uring` or `monoio` for file and socket I/O** wherever applicable and the kernel version supports it (Linux >= 5.10). This is preferred over standard epoll-backed I/O for hot paths.
- **Utilize multi-threading.** The Tokio runtime must be initialized with `#[tokio::main]` using the multi-thread scheduler. Worker count should match available cores unless a specific reason exists to limit it.
- **No busy-wait loops.** All polling must use async-aware mechanisms (`tokio::select!`, `tokio::time::sleep`, channel receivers).
- **No unnecessary serialization/deserialization cycles.** Do not re-parse a value that is already in memory in its typed form.
- **Avoid lock contention.** Shared state must use:
  - Lock-free structures (`Arc<AtomicXxx>`, `DashMap`, channels) as the first option.
  - `RwLock` over `Mutex` when reads vastly outnumber writes.
  - `Mutex` only when mutable access is genuinely exclusive and brief.
  - **`std::sync::Mutex` is forbidden in async code.** Use `tokio::sync::Mutex` only when truly needed; prefer channels.
- **Measure before optimizing.** Any non-trivial optimization must reference a benchmark result or a profiling finding.

***

## 4. Concurrency and Locking

- **Prefer message passing over shared state.** Use `tokio::sync::mpsc`, `broadcast`, or `watch` channels to communicate between tasks.
- **Avoid holding locks across `.await` points.** This is a hard rule — it causes deadlocks and runtime stalls.
- **Every shared data structure must document its concurrency model** in a `// CONCURRENCY:` comment at the definition site.
- **No `Arc<Mutex<Vec<T>>>` patterns** for hot paths. Use a dedicated worker task with a channel inbox instead.
- **Watch channels** (`tokio::sync::watch`) are preferred for broadcasting state to many readers (e.g., pod status updates).

***

## 5. Code Quality and Minimalism

- **Minimum viable code.** Write the least code that correctly solves the problem. Clever abstractions are only introduced when the same pattern appears three or more times.
- **No dead code.** All unused functions, types, and imports must be removed before a task is done. The compiler warning `#[allow(dead_code)]` is forbidden as a permanent annotation.
- **No `unwrap()` or `expect()` in production paths.** All errors must be propagated via `?` or handled explicitly. `unwrap()` is allowed only in tests and with a comment explaining why it is safe.
- **No `println!` in production code.** All output must go through `tracing` (`tracing::info!`, `tracing::debug!`, `tracing::warn!`, `tracing::error!`).
- **Functions must be short.** A function body exceeding 60 lines is a signal to refactor. There is no hard limit, but every function must do one thing.
- **No magic numbers.** All constants must be named with `const` or `static` with a descriptive name and a unit comment where relevant.
- **All public types and functions must have doc comments (`///`).** Internal helpers that are non-obvious must also be documented.

***

## 6. Error Handling

- **Use `anyhow::Result` for application-level code** and `thiserror`-derived enums for library-level errors that callers must match on.
- **Never silently swallow errors.** Every `Err` that is not propagated must be logged at `tracing::warn!` or above with context.
- **Add `.context("...")` or `.with_context(|| ...)` to every `?` operator** at the point where the error crosses a module boundary, so stack traces are meaningful.
- **No `panic!` in production paths.** Set `panic = "abort"` in release (already done in `Cargo.toml`) and never rely on catch_unwind for control flow.

***

## 7. Security

- **No hardcoded credentials, tokens, or secrets** anywhere in source code or config files committed to the repo.
- **Validate all external input.** Every field from a kubectl request, manifest file, or OCI registry response must be validated for length, type, and content before use.
- **Enforce resource limits** on deserialization: set maximum YAML/JSON size before parsing to prevent memory exhaustion attacks.
- **Privilege separation.** The API server and the container runtime components must request only the capabilities they need. Avoid `CAP_SYS_ADMIN` where a narrower capability suffices.
- **No `setuid` binaries in the project itself.** External setuid helpers (`newuidmap`) are documented requirements, not embedded.
- **TLS everywhere.** All API server endpoints exposed over the network must be served over TLS. Plaintext is only allowed on loopback for testing.
- **Run containers as non-root by default.** `runAsNonRoot: true` is the default behavior for all managed pods unless explicitly overridden by the manifest.
- **Static binary target is `x86_64-unknown-linux-musl`** for the init/runtime binary. Musl avoids glibc dynamic linking risks in minimal environments.

***

## 8. Kubernetes Compatibility

- **Every API endpoint response must be byte-for-byte valid Kubernetes JSON.** Use `k8s-openapi` types for all resource serialization — never hand-craft Kubernetes JSON.
- **Discovery endpoints must always be correct.** `/version`, `/api`, `/api/v1`, `/apis`, and `/openapi/v2` must return accurate and consistent metadata reflecting the actual supported resource set.
- **`resourceVersion` must increment** on every mutating operation. Watches depend on this.
- **Watch streams must emit `ADDED`, `MODIFIED`, `DELETED`, and `BOOKMARK` events** in correct order. No event may be dropped silently.
- **`metadata.uid`** must be a valid v4 UUID generated at creation time and never changed.
- **`metadata.creationTimestamp`** must be set at object creation and never changed.
- **Patch types** — JSON Patch, Merge Patch, Strategic Merge Patch, and Server-Side Apply — must all be handled by content type, not guessed.

***

## 9. Linux and Runtime

- **Use `nix` crate bindings** instead of raw libc calls wherever available.
- **Every `fork`/`exec` path must handle `SIGCHLD`** and reap all child processes to avoid zombie accumulation.
- **Mount namespaces must be cleaned up** on container exit. Leaked mounts are a hard bug.
- **`/proc/<pid>/fd` and `/proc/<pid>/ns`** handles must be closed after use. FD leaks are a hard bug.
- **`emptyDir` backing directories must be created before container start and deleted on pod termination.** No orphaned directories.
- **`cgroups v2` is the default target.** cgroups v1 support is optional and must be explicitly feature-flagged.

***

## 10. Testing

- **Every new public function must have at least one unit test.**
- **Every Kubernetes API endpoint must have an integration test** that sends a raw HTTP request and validates the response shape matches the Kubernetes API spec.
- **No `sleep` in tests.** Use channels, watches, or `tokio::sync::Notify` to synchronize async test assertions.
- **Tests must not require root** unless they are explicitly tagged `#[ignore]` with a comment explaining the privilege requirement.
- **No network access in unit tests.** Mock all external calls.

***

## 11. Observability

- **All significant operations must emit a `tracing` span** with relevant fields (pod name, namespace, UID, operation type).
- **Error paths must log at `warn` or `error` level** with enough context to diagnose the issue without a debugger.
- **Startup and shutdown must log** the version, configuration summary, and any detected capability gaps (e.g., missing `newuidmap`).

***

## 12. Release Profile Compliance

The `Cargo.toml` release profile is the law for production builds:

```toml
[profile.release]
opt-level = "z"       # binary size first
lto = true            # required
strip = true          # required
codegen-units = 1     # required
panic = "abort"       # required — no unwinding machinery
```

- **Never submit code that compiles only in debug mode.** All code must compile and pass tests in `--release` as well.
- **Binary size must not regress without approval.** Track with `ls -lh target/release/z8s` and report the delta.

***

## 13. AI Agent Checklist (Run Before Every Commit)

Before submitting any code, the agent must self-verify:

- [ ] No new crate added without human approval
- [ ] No `unsafe` without `// SAFETY:` comment and approval
- [ ] No `.clone()` without a justification comment
- [ ] No `unwrap()` in non-test code
- [ ] No `println!` — only `tracing::*`
- [ ] No blocking call on the async executor
- [ ] No lock held across `.await`
- [ ] No dead code or unused imports
- [ ] All public items have doc comments
- [ ] All errors propagated or logged with context
- [ ] All new API endpoints tested against Kubernetes spec
- [ ] Binary size delta reported if dependencies changed
- [ ] Release build compiles and passes tests

***

*This document is the single source of truth for code quality in z8s. It overrides any AI default behavior, style preference, or convenience shortcut.*