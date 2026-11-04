#!/usr/bin/env bash
# Installs agentgateway (https://agentgateway.dev) in Kubernetes mode on the
# kind cluster: the Gateway API CRDs, agentgateway's CRDs and its controller.
# Shared by every app; each app then creates its own Gateway (and so its own
# proxy), backends and routes in its own namespace.
#
#   kind/agentgateway.sh install     install or upgrade
#   kind/agentgateway.sh status      controller and the Gateways it serves
#   kind/agentgateway.sh uninstall   remove the controller and agentgateway's CRDs
set -euo pipefail

CLUSTER="${KIND_CLUSTER:-dev}"
GATEWAY_API_VERSION="${GATEWAY_API_VERSION:-v1.6.3}"
AGENTGATEWAY_VERSION="${AGENTGATEWAY_VERSION:-v1.7.0}"
CHARTS=oci://cr.agentgateway.dev/charts
NAMESPACE=agentgateway-system
k() { kubectl --context "kind-$CLUSTER" "$@"; }
h() { helm --kube-context "kind-$CLUSTER" "$@"; }

install() {
  k apply --server-side -f \
    "https://github.com/kubernetes-sigs/gateway-api/releases/download/$GATEWAY_API_VERSION/standard-install.yaml"
  h upgrade -i --create-namespace -n "$NAMESPACE" --version "$AGENTGATEWAY_VERSION" \
    agentgateway-crds "$CHARTS/agentgateway-crds"
  h upgrade -i -n "$NAMESPACE" --version "$AGENTGATEWAY_VERSION" \
    agentgateway "$CHARTS/agentgateway" --wait
  status
}

status() {
  k -n "$NAMESPACE" get pods
  echo
  k get gatewayclass agentgateway
  echo
  k get gateways --all-namespaces
}

uninstall() {
  h uninstall -n "$NAMESPACE" agentgateway --ignore-not-found
  h uninstall -n "$NAMESPACE" agentgateway-crds --ignore-not-found
  # The Gateway API CRDs stay: other controllers may use them.
}

case "${1:-}" in
  install|status|uninstall) "$1" ;;
  *) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
