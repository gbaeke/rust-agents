# CLAUDE.md

## agentgateway admin UI (Kubernetes mode)

In Kubernetes mode the admin UI's menu only shows Models and Client Setup under LLM (the UI hard-codes this for its read-only `dumpMode`). Two more pages work but are not linked, so open them directly:

- LLM analytics (spend, tokens, calls over time): `http://localhost:15000/ui/llm/analytics`
- LLM request logs (one row per call): `http://localhost:15000/ui/llm/logs`

They need the request-log database (`config.database.url` in `rawConfig` of `AgentgatewayParameters`, see `deploy/k8s/gateway.yaml`). To reach the UI, port-forward a proxy pod:

```sh
kubectl -n rust-agents port-forward deploy/agentgateway 15000:15000
# from another machine: add --address 0.0.0.0 and use http://omarchy:15000/...
```
