# Plan: from Docker Compose to Kubernetes, then a stateless agent

Status: **phases 1 and 2 implemented.** See the results for phase 2 below the phase 1 notes.

Phase 1 as built differs from the draft below in these ways:
- The cluster is generic and lives in `kind/` (`kind/cluster.sh install|up|start|stop|status|down`, cluster `dev`). It knows nothing about this app. Host ports 30000–30009 map 1:1 to NodePorts, and the agent uses 30000 (`http://omarchy:30000`). ufw-docker needs `sudo ufw route allow proto tcp from any to any port 30000:30009`.
- The app is deployed by `deploy/deploy.sh` (and removed with `deploy/deploy.sh delete`) into namespace `rust-agents`, from `deploy/k8s/`.
- Tools were installed with mise (no sudo).
- 1.4.3 MCP reconnect is **not needed**. After `rollout restart deploy/mcp-server` the agent's tools kept working, because the stateless MCP transport makes a new HTTP request for each call. D9 is settled.
- Extra: each MCP connect attempt at startup is capped at 2s, and the agent has a `startupProbe`. Without these, a connect to an MCP pod that was going away hung for 30s, the liveness probe failed, and a container that was already terminating got restarted and stayed stuck in `Terminating` for minutes.

Phase 2 as built:
- Decisions taken as proposed: D3 in-memory fallback without `DATABASE_URL`, D7 own table `agent_workspaces` (the workspace as one JSONB column), D8 plain StatefulSet.
- History goes through the agent's `.memory(store).conversation(session_id)`, which loads before the first model call and saves only when the turn succeeds. The log is named after the session id, so `/api/reset` deletes its `rig_memory_*` rows directly (rig's store has no delete).
- Turn lock: `pg_try_advisory_lock(hashtext('agent_turn'), hashtext(session_id))` on a dedicated connection, not a pooled one, because a pooled connection would go back to the pool still holding the lock.
- No HPA: CPU is a weak signal for this agent (see 2.6), and metrics-server is not installed.
- Startup fix: as PID 1 in a container the agent ignored SIGTERM until its handler was installed, and that only happened after startup (DB and MCP connects). A pod stopped during startup then waited out the 120s grace period. Both servers now install the handler first. This was also behind the slow `Terminating` pods seen in phase 1.
- Verified on kind:
  - 3 agent pods over both workers.
  - Message 1 to pod A and message 2 to pod B: B knew the history and the files.
  - All agent pods deleted: the chat went on.
  - A second message while busy: 409.
  - SIGKILL of the pod mid-turn: no history saved, the plan saved up to the last tool, the lock released, and a resend worked.
  - Reset: no rows left.
  - Local run without a database: in memory.
- Session expiry (added after review): each turn touches its `agent_workspaces` row first. Hourly, under a Postgres lock, one replica deletes rows idle for longer than `SESSION_TTL_DAYS` (default 7) and every `rig_memory_*` log that has no row. Verified with a chat backdated by 8 days.
- Memory, idle: agent ~10 MiB per pod, mcp-server ~3 MiB, postgres ~37 MiB; the kind cluster itself ~1.3 GiB.

## Goal

| Phase | Outcome |
|---|---|
| **1. Kubernetes on this machine** | The agent and the MCP server run in a local [kind](https://kind.sigs.k8s.io/) cluster on omarchy. The UI is still reachable from the Mac at `http://omarchy:3000`. The Docker Compose containers are stopped. |
| **2. Stateless agent** | No chat state lives in an agent pod. History, plan and files are kept in Postgres, so the agent runs as several replicas and any pod can serve any message. |

Each phase ends with a checkpoint where you review the result before the next one starts.

## Where we are now

- Two binaries, `agent` and `mcp-server`, built into distroless images by one `Dockerfile` and run with `compose.yaml`.
- The **MCP server is already stateless**: `legacy_session_mode(false)`, no `Mcp-Session-Id`. It can scale as it is.
- The **agent is stateful**. Each pod holds, in memory:

| State | Where it lives today | Survives a restart? | Phase 2 |
|---|---|---|---|
| Chat history | `HashMap<String, Session>` in `AppState` | no | Postgres, through rig's `PostgresSessionStore` |
| Plan (todos) and scratch files | `SharedWorkspace` in the same map | no | Postgres, our own table |
| Running agent turn (SSE stream, subagent events) | the HTTP request in progress | no | stays in the pod; a turn is one HTTP request |
| MCP tool connection | opened once at startup | reconnects only on pod restart | reconnect logic (see 1.6) |
| Knowledge-base index | built in memory from `/knowledge-base` | rebuilt at start | stays per pod; it's read-only |

- Machine: 16 cores, 27 GB RAM. Docker 29.9 with Compose 5.6. **kind, kubectl and helm are not installed.**
- ufw-docker is active. Port 3000 is already allowed with `ufw route allow`.

---

## Phase 1: run on kind

### 1.0 Install the tools (you, with sudo)

```sh
sudo pacman -S kubectl helm
sudo pacman -S kind            # if it isn't in the repos: yay -S kind-bin
kind version && kubectl version --client && helm version
```

helm is only needed for metrics-server in phase 2, and could be skipped by applying its manifest instead.

### 1.1 Cluster definition

New file `deploy/kind/cluster.yaml`:

```yaml
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
name: rust-agents
nodes:
  - role: control-plane
    extraPortMappings:
      - containerPort: 30080      # NodePort of the agent Service
        hostPort: 3000            # what the Mac opens: http://omarchy:3000
        listenAddress: "0.0.0.0"
        protocol: TCP
  - role: worker
  - role: worker
```

- **Two workers** so that phase 2 can show replicas spread over nodes. A single node would be enough for phase 1. Each kind node is a Docker container, about 500 MB RAM each.
- **Access:** kind maps host port 3000 to NodePort 30080 on the control-plane node. That is a Docker-published port, so the existing `ufw route allow ... port 3000` rule covers it.
- **No ingress controller** in phase 1. A NodePort plus a port mapping is the simplest path. An ingress such as ingress-nginx is listed under open decisions.

### 1.2 Images into the cluster

kind can't see local Docker images until they're loaded into its nodes. No registry is needed:

```sh
docker build --target agent      -t rust-agents/agent:dev .
docker build --target mcp-server -t rust-agents/mcp-server:dev .
kind load docker-image rust-agents/agent:dev rust-agents/mcp-server:dev --name rust-agents
```

- Tag `:dev`, not `:latest`. With `:latest` Kubernetes defaults to `imagePullPolicy: Always`, tries Docker Hub and fails. Manifests set `imagePullPolicy: IfNotPresent`.
- After a rebuild, run `kind load` again and then `kubectl rollout restart`. A script handles this (1.5).

### 1.3 Manifests (Kustomize)

New folder `deploy/k8s/`, applied with `kubectl apply -k deploy/k8s`. Kustomize is built into kubectl, so no extra tool is needed for phase 1.

```
deploy/k8s/
├── kustomization.yaml
├── namespace.yaml              namespace "agents"
├── mcp-server.yaml             Deployment + Service (ClusterIP :8081)
├── agent.yaml                  Deployment + Service (NodePort 30080 -> 3000)
└── agent-config.yaml           ConfigMap: AGENT_MODEL, MCP_URL, ...
```

**mcp-server**
- Deployment, 2 replicas from the start. It's stateless, so this exercises that right away.
- Service `mcp-server`, ClusterIP, port 8081.
- `MCP_ALLOWED_HOSTS=mcp-server,mcp-server:8081,mcp-server.agents.svc.cluster.local,mcp-server.agents.svc.cluster.local:8081`. Without the cluster DNS names, the server rejects the agent's requests (DNS-rebinding protection).

**agent**
- Deployment, **1 replica in phase 1**. With 2 replicas the in-memory sessions would break: a follow-up message could land on the other pod.
- Service `agent`, type NodePort, `nodePort: 30080`, port 3000.
- `MCP_URL=http://mcp-server:8081/mcp`, `AGENT_ADDR=0.0.0.0:3000`, `KB_DIR=/knowledge-base` (already baked into the image).

**The API key** goes into a Secret created from `.env` by the deploy script, never by a file in git:

```sh
kubectl -n agents create secret generic agent-secrets \
  --from-env-file=.env --dry-run=client -o yaml | kubectl apply -f -
```

The agent gets it through `envFrom: secretRef: agent-secrets`. Kustomize's `secretGenerator` could read `.env` too, but then the generated Secret is easy to leak with `kubectl kustomize > file`; the script is more explicit.

**Pod security**, the same for both. Distroless `nonroot` fits the strict "restricted" profile:

```yaml
securityContext:
  runAsNonRoot: true
  runAsUser: 65532
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities: { drop: ["ALL"] }
  seccompProfile: { type: RuntimeDefault }
```

The namespace gets the label `pod-security.kubernetes.io/enforce: restricted`, so the cluster refuses anything weaker.

**Resources**, starting values to be adjusted after measuring:

| | requests | limits |
|---|---|---|
| mcp-server | cpu 10m, memory 16Mi | memory 64Mi |
| agent | cpu 50m, memory 64Mi | memory 256Mi |

No CPU limits, which avoids throttling during streaming; memory limits catch leaks.

### 1.4 Code changes needed for Kubernetes

Small changes, all of them also useful outside Kubernetes:

1. **Health endpoints.** Distroless has no shell, so probes must be HTTP.
   - `GET /healthz` on both servers: returns 200 while the process runs (liveness).
   - `GET /readyz` on the agent: 200 once the knowledge base is loaded. Should it also require the MCP tools to be present? See open decision D5.
   - The MCP server needs `/healthz` as a separate axum route next to `/mcp`.
2. **Graceful shutdown.** On SIGTERM, stop accepting new requests and let running agent turns finish (`axum::serve(...).with_graceful_shutdown(...)`). The agent Deployment gets `terminationGracePeriodSeconds: 120`, because a deep-agent turn can take a minute or more. Today a rollout kills a running turn.
3. **MCP reconnect.** Today the agent lists the MCP tools once at startup. If every MCP pod restarts, for example in a rollout, the agent's MCP connection may stay broken until the agent itself restarts. Earlier logs showed `Transport channel closed` when the server was gone. Options:
   - **(a)** connect and list the tools per agent turn: simple, one extra round trip per message (~ms in-cluster);
   - **(b)** keep one connection and reconnect when a call fails, or use rig's `McpClientHandler`.

   Proposal: first test what happens in 1.6; if it breaks, implement (a).
4. **Logs** stay as they are. tracing already writes to stdout. ANSI colours should be switched off when not on a terminal, so `kubectl logs` stays readable.

### 1.5 Scripts

New folder `scripts/`, so every step is repeatable:

| Script | Does |
|---|---|
| `scripts/kind-up.sh` | creates the cluster from `deploy/kind/cluster.yaml` (skipped if it exists) |
| `scripts/kind-deploy.sh` | builds both images, `kind load`s them, creates or updates the Secret from `.env`, `kubectl apply -k deploy/k8s`, restarts the Deployments, waits for rollout |
| `scripts/kind-down.sh` | deletes the cluster |

A `justfile` or `Makefile` could wrap these; that's open decision D1.

### 1.6 Switching over and verifying

1. `docker compose down`, which frees port 3000.
2. `scripts/kind-up.sh && scripts/kind-deploy.sh`.
3. Checks:
   - [ ] `kubectl -n agents get pods`: 1 agent and 2 mcp-server pods `Running`, `Ready`.
   - [ ] `kubectl -n agents logs deploy/agent` shows `connected to MCP server ... with 4 tools`.
   - [ ] `http://omarchy:3000` loads on the Mac; the header lists the 4 MCP tools.
   - [ ] The Bluefin question returns €3,759 using `calculate`, which proves agent → Service → MCP pod works.
   - [ ] `kubectl -n agents rollout restart deploy/mcp-server`, then ask again: do the MCP tools still work? This decides 1.4.3.
   - [ ] `kubectl -n agents delete pod -l app=agent` while a turn is running: with graceful shutdown the turn finishes first.
   - [ ] The namespace rejects a pod that runs as root (restricted profile active).
4. Update the README: a Kubernetes section. Compose stays available for quick local runs unless you decide otherwise (D2).

**Checkpoint 1: you review phase 1 before phase 2 starts.**

---

## Phase 2: make the agent stateless

### 2.1 Target picture

```
             ┌─────────── namespace agents ───────────┐
Mac ──3000──▶│ Service agent (NodePort)                │
             │   ├── agent pod 1 ─┐                    │
             │   ├── agent pod 2 ─┼──▶ Service mcp-server ──▶ mcp pods (2)
             │   └── agent pod 3 ─┘                    │
             │          │                              │
             │          ▼                              │
             │   postgres (StatefulSet + volume)       │
             └─────────────────────────────────────────┘
```

Any agent pod can take any message. A message's turn runs on one pod from start to end, because it's one HTTP request with an SSE response, so no sticky sessions are needed.

### 2.2 Postgres

- **Local:** a single-instance `postgres:17` StatefulSet with a 1 Gi PersistentVolumeClaim (kind's default storage class keeps it on the node's disk), a Service `postgres`, and the password in a Secret generated by the deploy script.
- **Later, in a real cluster:** a managed database such as Azure Database for PostgreSQL, or an operator such as CloudNativePG. Only `DATABASE_URL` changes.
- The agent gets `DATABASE_URL` from the Secret.

### 2.3 Chat history: rig's `PostgresSessionStore`

- Add `rig-postgres` with feature `memory` (version 0.46, matching rig) and `sqlx` (Postgres, tokio, rustls; no OpenSSL, so the static musl build still works, which needs checking).
- At startup: `PgPool::connect(DATABASE_URL)` and `PostgresSessionStore::open(pool)`. The store creates its own three `rig_memory_*` tables.
- Per turn: build the agent with `AgentBuilder::memory(store)` and name the conversation by session id (`conversation(session_id)`). rig then loads the history before the first model call and appends the new messages when the turn ends. This replaces `.history(history)` and the manual `session.history.extend(...)`.
- A failed or cancelled turn saves nothing in rig's model. The user can resend the message without getting a half turn in the history.
- `/api/reset` deletes the conversation.

### 2.4 Plan and files: our own table

rig's store covers messages only. The plan and scratch files get a small table of their own:

```sql
CREATE TABLE IF NOT EXISTS agent_workspaces (
    session_id  TEXT PRIMARY KEY,
    todos       JSONB NOT NULL DEFAULT '[]',
    files       JSONB NOT NULL DEFAULT '{}',
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

- At the start of a turn: load the row into the in-memory `SharedWorkspace`. The tools keep working on memory exactly as now.
- After **each** tool result, at the same point where the UI already gets its `workspace` event: write the row back. A pod that dies mid-turn then loses at most the last tool's change.
- Created at startup with `CREATE TABLE IF NOT EXISTS` under a Postgres advisory lock, the same pattern rig uses. For one table, no migration tool is needed; `sqlx migrate` could come later.
- Alternative to consider: keep the plan inside the conversation through rig's run entries. That's more elegant, but needs investigation; see D7.

### 2.5 Concurrency: one turn per session at a time

Two messages for the same session at the same time, for example a double click or two tabs, could land on two pods. Then both would edit the same plan and files, and rig would refuse the second history save (`SessionError::Changed`) after the model had already done the work.

Proposal: a **per-session lock in Postgres** for the duration of a turn:
- At the start: `pg_try_advisory_lock(hashtext(session_id))` on a connection held for the whole turn.
- If the lock is taken: respond `409 Conflict` with "this chat is still working on a previous message". The UI shows that message.
- The lock is released at the end of the turn, or automatically when the pod dies, because the connection closes.

This makes 2.3 and 2.4 safe without any merge logic.

### 2.6 Scaling and availability

- Agent Deployment: `replicas: 3`, topology spread over the two workers.
- `PodDisruptionBudget` `minAvailable: 1` for agent and mcp-server.
- **Autoscaling:** a HorizontalPodAutoscaler on CPU (2–6 replicas) needs metrics-server. In kind that's `helm install metrics-server ... --set args={--kubelet-insecure-tls}`. CPU is a weak signal for an agent that mostly waits on Claude; a better signal is "turns in progress per pod". That would mean exposing a metric and using KEDA or a custom metric, so it's listed under later.
- Readiness of the agent additionally requires the database to be reachable.

### 2.7 Local development without Kubernetes

Proposal: if `DATABASE_URL` is not set, the agent falls back to rig's `MemoryStore` and an in-memory workspace. That's today's behaviour, so `cargo run` and Compose keep working without Postgres. Alternatively Compose gets a Postgres service too; see D3.

### 2.8 Verifying phase 2

- [ ] 3 agent pods running; `kubectl -n agents get pods -o wide` shows them on both workers.
- [ ] **Continuity across pods:** send message 1 to pod A and message 2 to pod B (with `kubectl port-forward pod/...` to target pods directly); message 2 knows about message 1, and the Plan and Files tabs show the same content.
- [ ] **Restart:** delete all agent pods; after they come back, the chat continues with its history, plan and files.
- [ ] **Concurrent messages** on one session return 409 for the second; no corrupted history.
- [ ] **Pod killed mid-turn:** the user sees an error; nothing half-written in the history; resending works.
- [ ] `kubectl scale deploy/agent --replicas=1` and back to 3 work during use.
- [ ] Costs: these tests use real model calls. Proposal: run them with `AGENT_MODEL=claude-haiku-5-5` to keep token costs low.

**Checkpoint 2: review.**

---

## Open decisions for your review

| # | Question | Proposal |
|---|---|---|
| D1 | Plain shell scripts, or a `justfile`/`Makefile` on top? | scripts + a small `justfile` |
| D2 | Keep `compose.yaml` after phase 1? | keep it for quick local runs |
| D3 | Phase 2 without `DATABASE_URL`: in-memory fallback, or require Postgres everywhere (Compose gets Postgres too)? | in-memory fallback |
| D4 | Kustomize or a Helm chart? | Kustomize now; Helm only if this goes to other clusters |
| D5 | Should the agent be "not ready" while the MCP tools are missing? | no: it works without them and logs a warning |
| D6 | NodePort + port mapping, or an ingress controller (ingress-nginx) with a host name? | NodePort for now |
| D7 | Plan and files in our own table, or inside the conversation via rig's run entries? | own table: simple and predictable |
| D8 | Postgres as a plain StatefulSet, or the CloudNativePG operator already locally? | plain StatefulSet locally |
| D9 | MCP reconnect: wait for the 1.6 test, or implement per-turn connect straight away? | wait for the test |

## Out of scope for now

- TLS and authentication on the UI. Still needed before anything leaves this machine.
- A container registry and CI that builds and pushes images.
- Autoscaling on a "turns in progress" metric (KEDA).
- Observability beyond logs: metrics, traces, dashboards.
- Deploying to a cloud cluster (AKS); the manifests are written so that this is mostly a matter of changing the image source, `DATABASE_URL` and the Service type.

## Files that will be added or changed

| Phase | File | Change |
|---|---|---|
| 1 | `deploy/kind/cluster.yaml` | new |
| 1 | `deploy/k8s/*.yaml`, `kustomization.yaml` | new |
| 1 | `scripts/kind-up.sh`, `kind-deploy.sh`, `kind-down.sh` | new |
| 1 | `agent/src/main.rs`, `mcp-server/src/main.rs` | health endpoints, graceful shutdown, possibly MCP reconnect |
| 1 | `README.md` | Kubernetes section |
| 2 | `deploy/k8s/postgres.yaml`, PDBs, HPA | new |
| 2 | `agent/Cargo.toml` | `rig-postgres` (memory), `sqlx` |
| 2 | `agent/src/main.rs` + new `agent/src/store.rs` | session store, workspace table, session lock, 409 handling |
| 2 | `agent/static/index.html` | show the "still working" message on 409 |
