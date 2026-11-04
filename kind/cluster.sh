#!/usr/bin/env bash
# Lifecycle of a local kind cluster that any app can be deployed to.
# Knows nothing about the apps: they bring their own images and manifests.
#
#   kind/cluster.sh install   install kind, kubectl and helm (with mise, no sudo)
#   kind/cluster.sh up        create the cluster, or start it if it is stopped
#   kind/cluster.sh stop      stop the node containers; state is kept
#   kind/cluster.sh start     start them again
#   kind/cluster.sh status    nodes, namespaces and published ports
#   kind/cluster.sh down      delete the cluster and everything in it
#
# KIND_CLUSTER picks the cluster name (default: dev); kubectl's context is kind-$KIND_CLUSTER.
set -euo pipefail

CLUSTER="${KIND_CLUSTER:-dev}"
CONFIG="$(dirname "$(realpath "$0")")/cluster.yaml"

nodes() { docker ps -a --filter "label=io.x-k8s.kind.cluster=$CLUSTER" --format '{{.Names}}'; }
exists() { kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; }
need() { command -v "$1" >/dev/null || { echo "$1 is not installed; run: $0 install" >&2; exit 1; }; }

wait_ready() {
  kubectl config use-context "kind-$CLUSTER" >/dev/null
  echo "waiting for nodes to be Ready..."
  kubectl wait --for=condition=Ready nodes --all --timeout=180s
}

install() {
  if command -v mise >/dev/null; then
    mise use -g kind@latest kubectl@latest helm@latest
  elif command -v pacman >/dev/null; then
    sudo pacman -S --needed kind kubectl helm
  else
    echo "install kind, kubectl and helm with your package manager" >&2
    exit 1
  fi
  kind version && kubectl version --client && helm version --short
}

up() {
  need kind; need kubectl
  if exists; then
    start
  else
    kind create cluster --name "$CLUSTER" --config "$CONFIG"
    wait_ready
  fi
}

start() {
  need kind; need kubectl
  exists || { echo "no cluster $CLUSTER; run: $0 up" >&2; exit 1; }
  # The control plane first, so the workers find the API server when they come up.
  docker start "$CLUSTER-control-plane" >/dev/null
  nodes | grep -vx "$CLUSTER-control-plane" | xargs -r docker start >/dev/null
  wait_ready
}

stop() {
  nodes | xargs -r docker stop >/dev/null
  echo "cluster $CLUSTER stopped"
}

status() {
  need kubectl
  if ! exists; then echo "no cluster $CLUSTER"; return; fi
  docker ps -a --filter "label=io.x-k8s.kind.cluster=$CLUSTER" --format 'table {{.Names}}\t{{.Status}}'
  if docker ps --format '{{.Names}}' | grep -qx "$CLUSTER-control-plane"; then
    echo
    kubectl --context "kind-$CLUSTER" get nodes
    echo
    kubectl --context "kind-$CLUSTER" get services --all-namespaces --field-selector spec.type=NodePort \
      -o custom-columns='NAMESPACE:.metadata.namespace,SERVICE:.metadata.name,NODE PORT:.spec.ports[*].nodePort'
  fi
}

down() {
  need kind
  kind delete cluster --name "$CLUSTER"
}

case "${1:-}" in
  install|up|start|stop|status|down) "$1" ;;
  *) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
