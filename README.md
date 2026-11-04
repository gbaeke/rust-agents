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

## Run it in containers

```sh
cp .env.example .env        # put your key in it
docker compose up -d --build
open http://localhost:3000
```

Both binaries are built as fully static musl executables. TLS uses rustls with aws-lc, so there is no OpenSSL. Each binary runs on `gcr.io/distroless/static-debian12:nonroot`, which has no shell and no libc and runs as a non-root user. Each image is about 35 MB, almost all of it the binary. Only the agent's port is published. The MCP server is reachable only from inside the compose network.

The MCP server is **stateless**: `legacy_session_mode(false)` means it gives out no `Mcp-Session-Id`, so you can scale it to several replicas behind a load balancer. `MCP_ALLOWED_HOSTS` lists the hostnames it accepts, as protection against DNS rebinding. It defaults to loopback only, so compose sets it to `mcp-server`.

## What the agent can do

The design follows LangChain's Deep Agents:

- **Planning:** `write_todos` keeps a todo list. The UI shows it live in the Plan tab.
- **Scratch filesystem:** `ls`, `read_file`, `write_file` and `edit_file` work on files kept in the session's memory. Reports and drafts appear in the Files tab.
- **Agentic RAG:** the knowledge base in `knowledge-base/` (a fictional robotics company) is split into sections and indexed with BM25 (`agent/src/kb.rs`, no embedding service needed). The agent decides itself what to search for, rephrases queries, and reads full documents when it needs whole tables.
- **Subagents:** `task` starts a research subagent with its own context and the knowledge-base and MCP tools, and returns only its report. Several `task` calls in one turn give parallel research. Subagent steps stream to the Activity tab.
- **MCP tools:** the agent is told to use `calculate` for all arithmetic. The other MCP tools give it live data (weather, time, web pages).

Try the example prompts in the UI. Answering the SLA-credit questions takes the postmortem, the SLA document and the calculator together. For example, Bluefin's 4h12m outage gives 99.42% uptime, which earns a 15% credit on 140 × EUR 179.

## Code map

- `mcp-server/src/main.rs`: rmcp `#[tool_router]` server.
- `agent/src/main.rs`: axum app, sessions, agent assembly, and the SSE translation of rig's `MultiTurnStreamItem`.
- `agent/src/tools/`: rig `Tool` implementations. Argument schemas are generated from Rust types with `schemars`.
- `agent/src/kb.rs`: chunking and BM25.

## Notes

- Sessions live in memory and are lost on restart.
- To use another provider, change the one `AnthropicConfig` line in `main.rs`. rig supports OpenAI, Gemini, Azure, Ollama and others.
- `cargo test --workspace` tests the retrieval and the calculator.
