# z8s cluster kubectl config — run once to set up contexts
#
# Usage:
#   source tests/cluster/kc.sh          # set up contexts
#   kubectl config use-context z8s-a    # switch to node A
#   kubectl config use-context z8s-b    # switch to node B  
#   kubectl config use-context z8s-c    # switch to node C

kubectl config set-cluster z8s-a --server=http://localhost:6443 --insecure-skip-tls-verify=true 2>/dev/null
kubectl config set-cluster z8s-b --server=http://localhost:7443 --insecure-skip-tls-verify=true 2>/dev/null
kubectl config set-cluster z8s-c --server=http://localhost:8443 --insecure-skip-tls-verify=true 2>/dev/null

kubectl config set-context z8s-a --cluster=z8s-a 2>/dev/null
kubectl config set-context z8s-b --cluster=z8s-b 2>/dev/null
kubectl config set-context z8s-c --cluster=z8s-c 2>/dev/null

echo "Contexts created. Usage:"
echo "  kubectl config use-context z8s-a"
echo "  kubectl config use-context z8s-b"
echo "  kubectl config use-context z8s-c"
