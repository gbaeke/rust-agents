# rust-agents

A "deep agent" in Rust with [rig](https://github.com/0xPlaygrounds/rig) 0.46. It has a web UI, an MCP server for tools, and agentic RAG over a sample knowledge base.

```
browser (agent/static/index.html)
   │  POST /api/chat → SSE stream of text, tool calls, results, subagent steps, todos and files
   ▼
agent  (axum + rig, Claude via ANTHROPIC_API_KEY)
   ├── local tools: write_todos · ls/read_file/write_file/edit_file · search_knowledge_base/read_document/list_documents · task (subagent)
   └── MCP client (rmcp, streamable HTTP)
          ▼
mcp-server  (rmcp + axum on :8081/mcp)
   └── calculate · current_time · get_weather (Open-Meteo) · fetch_url
```

## Run it

```sh
cp .env.example .env        # then put your key in .env (it is gitignored)
cargo run -p mcp-server     # terminal 1
cargo run -p agent          # terminal 2
open http://127.0.0.1:3000
```

Both binaries read `.env` from the working directory or any parent directory. Variables already set in the environment take precedence over the file.

The agent still starts if the MCP server is down. It runs without the MCP tools and logs a warning.

| Variable | Default |
|---|---|
| `ANTHROPIC_API_KEY` | required |
| `AGENT_MODEL` | `claude-sonnet-5-5` |
| `AGENT_ADDR` | `127.0.0.1:3000` |
| `MCP_URL` | `http://127.0.0.1:8081/mcp` |
| `MCP_ADDR` (mcp-server) | `127.0.0.1:8081` |
| `KB_DIR` | `./knowledge-base` |
| `DATABASE_URL` | unset: sessions in memory |
| `SESSION_TTL_DAYS` | `7`; `0` keeps sessions forever |

## Run it in containers

```sh
cp .env.example .env        # put your key in it
docker compose up -d --build
open http://localhost:3000
```

Both binaries are built as fully static musl executables. TLS uses rustls with aws-lc, so there is no OpenSSL. Each binary runs on `gcr.io/distroless/static-debian12:nonroot`, which has no shell and no libc and runs as a non-root user. Each image is about 35 MB, almost all of it the binary. Only the agent's port is published. The MCP server is reachable only from inside the compose network.

The MCP server is **stateless**: `legacy_session_mode(false)` means it gives out no `Mcp-Session-Id`, so you can scale it to several replicas behind a load balancer. `MCP_ALLOWED_HOSTS` lists the hostnames it accepts, as protection against DNS rebinding. It defaults to loopback only, so compose sets it to `mcp-server`.

## Run it on Kubernetes (kind)

`kind/` holds a local cluster that any app can use. It knows nothing about this repo (see `kind/README.md`). `deploy/` deploys this repo's two services to it.

```sh
kind/cluster.sh install     # once: kind, kubectl, helm
kind/cluster.sh up          # one control-plane node and two workers; host ports 30000-30009 reach NodePorts
kind/agentgateway.sh install   # once per cluster: the agentgateway controller
deploy/deploy.sh            # build, load into kind, apply deploy/k8s, wait for the rollout
open http://omarchy:30000
```

- `deploy/deploy.sh` can be run again after code changes. `deploy/deploy.sh delete` removes the app.
- `kind/cluster.sh stop` and `start` pause the cluster and resume it, keeping its state.
- Everything runs in the namespace `rust-agents`, which enforces the `restricted` Pod Security Standard.
- The agent is **stateless** and runs as 3 replicas. History, plan and files are in Postgres (a StatefulSet with a 1 Gi volume), so any pod can take any message. A turn stays on one pod because it is one HTTP request. The MCP server runs as 2 replicas. Disruption budgets keep at least one of each running.
- **All model and tool traffic goes through agentgateway** (`deploy/k8s/gateway.yaml`). The app has its own proxy, 2 replicas, reachable only inside the cluster. The agent pods get `ANTHROPIC_BASE_URL=http://agentgateway:8080` and `MCP_URL=http://agentgateway:8080/mcp`.
  - The real API key is only in the gateway's Secret `anthropic`. The agent sends a placeholder, and the gateway replaces it.
  - Claude is an `AgentgatewayModel` (`claude-sonnet-5-5`) on the gateway's listener. The gateway serves Anthropic's own `/v1/messages` and lists its models at `/v1/models`. **Only declared models are served**, so to change `AGENT_MODEL`, add an `AgentgatewayModel` for the new model too. The MCP server is an `AgentgatewayBackend` behind an HTTPRoute on `/mcp` of the same listener.
  - MCP uses `sessionRouting: Stateless`, because the MCP server keeps no sessions either.
  - NetworkPolicies enforce the gateway: agent pods may only reach the gateway, Postgres and DNS, and the MCP server only accepts requests from the gateway.
  - The gateway's log (`kubectl -n rust-agents logs deploy/agentgateway`) has one line per model call, with model and token counts, and one per MCP method.
- The database password is the Secret `postgres`. `deploy.sh` makes it once and then keeps it.
- The API key goes into the Secret `agent-secrets`, taken from `.env`. Other settings are in the generated ConfigMap `agent-config` in `deploy/k8s/kustomization.yaml`.
- On SIGTERM, both servers stop accepting requests and let running ones finish. A rollout or pod delete waits for the agent turn in progress, for up to 120s.
- Health endpoints: `/healthz` on both servers, and `/readyz` on the agent.

## What the agent can do

The design follows LangChain's Deep Agents:

- **Planning:** `write_todos` keeps a todo list. The UI shows it live in the Plan tab.
- **Scratch filesystem:** `ls`, `read_file`, `write_file` and `edit_file` work on files kept in the session's memory. Reports and drafts appear in the Files tab.
- **Agentic RAG:** the knowledge base in `knowledge-base/` (a fictional robotics company) is split into sections (`agent/src/kb.rs`) and indexed with the [`bm25`](https://crates.io/crates/bm25) crate (Snowball stemming, stop words, no embedding service needed). The agent decides itself what to search for, rephrases queries, and reads full documents when it needs whole tables.
- **Subagents:** `task` starts a research subagent with its own context and the knowledge-base and MCP tools, and returns only its report. Several `task` calls in one turn give parallel research. Subagent steps stream to the Activity tab.
- **MCP tools:** the agent is told to use `calculate` for all arithmetic. The other MCP tools give it live data (weather, time, web pages).

Try the example prompts in the UI. Answering the SLA-credit questions takes the postmortem, the SLA document and the calculator together. For example, Bluefin's 4h12m outage gives 99.42% uptime, which earns a 15% credit on 140 × EUR 179.

## Sessions

`agent/src/store.rs` keeps a chat session in three parts:

| Part | With `DATABASE_URL` | Without |
|---|---|---|
| history | rig's `PostgresSessionStore` (tables `rig_memory_*`) through the agent's `memory`/`conversation` | rig's `MemoryStore` |
| plan and files | table `agent_workspaces`, saved after every tool result | a map in memory |
| one turn at a time | `pg_try_advisory_lock` on its own connection, so a crash releases it | a set in memory |

- rig loads the history before the first model call and saves it only when the turn succeeds. A failed, cancelled or crashed turn adds nothing, and resending works.
- A second message to a chat that is still busy gets `409 Conflict`, also when it reaches another replica.
- `/readyz` fails while the database is unreachable.
- **Expiry:** every turn updates its session's `updated_at`. At startup and then every hour, one replica deletes the sessions idle for longer than `SESSION_TTL_DAYS`, together with their history. A Postgres lock picks the replica. In memory nothing expires.
- **Cleaning up by hand:** the UI's reset button deletes one chat. To delete all of them, run `kubectl -n rust-agents exec postgres-0 -- psql -U agent -d agent -c "TRUNCATE rig_memory_logs, rig_memory_log_chunks, agent_workspaces"`. `deploy/deploy.sh delete` also removes the database volume.

## Code map

- `mcp-server/src/main.rs`: rmcp `#[tool_router]` server.
- `agent/src/main.rs`: axum app, agent assembly, and the SSE translation of rig's `MultiTurnStreamItem`.
- `agent/src/store.rs`: where sessions are kept (Postgres or memory) and the per-session turn lock.
- `agent/src/tools/`: rig `Tool` implementations. Argument schemas are generated from Rust types with `schemars`.
- `agent/src/kb.rs`: chunking by section, plus BM25 search through the `bm25` crate.

## Notes

- Without `DATABASE_URL`, sessions live in memory and are lost on restart.
- The UI starts a new session on every page load. The server keeps the old ones, but the UI does not reopen them.
- To use another provider, change the one `AnthropicConfig` line in `main.rs`. rig supports OpenAI, Gemini, Azure, Ollama and others.
- `cargo test --workspace` tests the retrieval and the calculator.
