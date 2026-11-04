#!/usr/bin/env bash
# Builds the agent and MCP server images and deploys them to the kind cluster
# (create that first with kind/cluster.sh up). Safe to run again after changes.
#
#   deploy/deploy.sh            build, load, apply, restart
#   deploy/deploy.sh delete     remove the app (its namespace) from the cluster
set -euo pipefail

cd "$(dirname "$(realpath "$0")")/.."
CLUSTER="${KIND_CLUSTER:-dev}"
NAMESPACE=rust-agents
k() { kubectl --context "kind-$CLUSTER" "$@"; }

if [[ "${1:-}" == delete ]]; then
  k delete namespace "$NAMESPACE" --ignore-not-found
  exit
fi

kind get clusters 2>/dev/null | grep -qx "$CLUSTER" || { echo "no kind cluster $CLUSTER; run: kind/cluster.sh up" >&2; exit 1; }
key_line="$(grep -E '^ANTHROPIC_API_KEY=.+' .env 2>/dev/null || true)"
[[ -n "$key_line" ]] || { echo "ANTHROPIC_API_KEY is missing from .env" >&2; exit 1; }

for target in agent mcp-server; do
  docker build --target "$target" -t "rust-agents/$target:dev" .
done
kind load docker-image rust-agents/agent:dev rust-agents/mcp-server:dev --name "$CLUSTER"

first_install=$(k -n "$NAMESPACE" get deployment agent >/dev/null 2>&1 && echo no || echo yes)
k apply -f deploy/k8s/namespace.yaml
# Only the key goes into the Secret, read from stdin so it never shows up in `ps`.
k -n "$NAMESPACE" create secret generic agent-secrets --from-env-file=/dev/stdin \
  --dry-run=client -o yaml <<<"$key_line" | k apply -f -
k apply -k deploy/k8s

# Same tag after a rebuild, so restart to pick up the newly loaded images.
# Not on a first install: the pods were just created from them.
[[ "$first_install" == yes ]] || k -n "$NAMESPACE" rollout restart deployment/mcp-server deployment/agent
k -n "$NAMESPACE" rollout status deployment/mcp-server --timeout=120s
k -n "$NAMESPACE" rollout status deployment/agent --timeout=180s
echo "agent UI: http://$(hostname):30000"
