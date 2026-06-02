# z8s container image — for nested PID1 test (z8s inside a z8s pod).
# Build from repo root:
#   cargo build --release
#   docker build -t z8s:dev -f deploy/z8s/Dockerfile .
#
# Run on host cluster: tests/nested/z8s-pid1-pod.yaml

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        iptables \
        iproute2 \
    && rm -rf /var/lib/apt/lists/*

COPY target/release/z8s /z8s

# PID 1 in pod: use run --daemon (foreground server). See docs/plan/02-cri.md §7b.
ENTRYPOINT ["/z8s"]
CMD ["run", "--daemon", "--port", "6443", "--pod-cidr", "10.201.0.0/16", "--service-cidr", "10.111.0.0/16", "--data-dir", "/var/lib/z8s"]
