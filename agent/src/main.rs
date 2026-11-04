//! A "deep agent" built with rig: it plans with a todo list, keeps notes in
//! a scratch filesystem, searches a knowledge base (agentic RAG), delegates
//! research to subagents and uses the tools of an MCP server. A small web UI
//! talks to it over server-sent events.
//!
//! Run the MCP server first (`cargo run -p mcp-server`), then this one with
//! `ANTHROPIC_API_KEY` set, and open http://127.0.0.1:3000.

mod events;
mod kb;
mod store;
mod tools;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use rig::agent::{Agent, MultiTurnStreamItem};
use rig::message::ToolResultContent;
use rig::operation::Completion;
use rig::prelude::*;
use rig::providers::anthropic::{self, AnthropicConfig};
use rig::streaming::{Item, StreamEvent};
use rig::tool::DynamicTool;
use rig::tool::rmcp::McpConnection;
use rig::DynModel;
use rmcp::ServiceExt;
use rmcp::model::{ClientCapabilities, ClientConfig, Implementation};
use rmcp::transport::StreamableHttpClientTransport;
use serde::Deserialize;
use serde_json::json;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::mpsc;

use events::{EventSender, UiEvent};
use kb::KnowledgeBase;
use store::{History, Store};
use tools::*;

const MAX_TOOL_RESULT_CHARS: usize = 4_000;

const SYSTEM_PROMPT: &str = "You are Quokka Assistant, an expert assistant for the employees of Quokka Robotics.

You have these capabilities:
- write_todos: your plan. For any request that needs three or more steps, write the plan first. Call write_todos again each time you finish an item; every item must be completed before your final answer.
- search_knowledge_base, read_document, list_documents: the internal knowledge base (products, pricing and SLA, incidents, policies, API docs, release notes). Search with several phrasings, and read full documents when you need tables or exact figures.
- task: delegate an independent research question to a subagent with its own context. Launch several in the same turn when a request has independent parts.
- ls, read_file, write_file, edit_file: a scratch workspace the user can see. Save longer deliverables (reports, emails, comparisons) as markdown files there.
- calculate, current_time, get_weather, fetch_url: tools from an MCP server. Never do arithmetic in your head; always use calculate.

Rules:
- Ground every company fact in the knowledge base and cite the document name, e.g. (pricing-and-sla.md).
- If the knowledge base does not answer the question, say so instead of guessing.
- Keep the chat answer concise; put long material in a workspace file and mention it.";

struct AppState {
    model: DynModel<Completion>,
    model_name: String,
    kb: Arc<KnowledgeBase>,
    mcp_tools: Vec<DynamicTool>,
    store: Store,
}

impl AppState {
    /// The main agent for one run, its tools bound to the session's workspace.
    fn agent(&self, workspace: &SharedWorkspace, events: EventSender) -> Agent {
        let ws = || workspace.clone();
        AgentBuilder::new(self.model.clone())
            .preamble(SYSTEM_PROMPT)
            .max_tokens(8192)
            .tool(WriteTodos { workspace: ws() })
            .tool(ListFiles { workspace: ws() })
            .tool(ReadFile { workspace: ws() })
            .tool(WriteFile { workspace: ws() })
            .tool(EditFile { workspace: ws() })
            .tool(SearchKnowledgeBase { kb: self.kb.clone() })
            .tool(ReadDocument { kb: self.kb.clone() })
            .tool(ListDocuments { kb: self.kb.clone() })
            .tool(Task {
                model: self.model.clone(),
                kb: self.kb.clone(),
                mcp_tools: self.mcp_tools.clone(),
                events,
            })
            .dynamic_tools(self.mcp_tools.clone())
            .build()
    }
}

async fn connect_mcp(url: &str) -> anyhow::Result<Vec<DynamicTool>> {
    let config = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("rust-agents", env!("CARGO_PKG_VERSION")),
    );
    let transport = StreamableHttpClientTransport::from_uri(url);
    // The tools hold the connection open for as long as they live.
    let connection = McpConnection::new(config.serve(transport).await?);
    connection
        .tools()
        .await?
        .into_iter()
        .map(|tool| DynamicTool::try_from(tool).map_err(|error| anyhow::anyhow!("{error}")))
        .collect()
}

#[derive(Deserialize)]
struct ChatRequest {
    session_id: String,
    message: String,
}

#[derive(Deserialize)]
struct SessionRequest {
    session_id: String,
}

fn internal_error(error: anyhow::Error) -> Response {
    tracing::error!("{error:#}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": error.to_string() }))).into_response()
}

/// Runs the agent on one message and streams what happens as SSE events.
/// The history is loaded before the first model call and saved when the turn
/// succeeds; the workspace is saved after every tool result.
async fn chat(State(state): State<Arc<AppState>>, Json(request): Json<ChatRequest>) -> Response {
    let session_id = request.session_id.clone();
    // One turn per session at a time, also across replicas: two turns would
    // edit the same plan and files, and the second history save would be refused.
    let lock = match state.store.lock(&session_id).await {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            let error = "This chat is still working on a previous message.";
            return (StatusCode::CONFLICT, Json(json!({ "error": error }))).into_response();
        }
        Err(error) => return internal_error(error),
    };
    if let Err(error) = state.store.touch(&session_id).await {
        return internal_error(error);
    }
    let workspace = match state.store.load_workspace(&session_id).await {
        Ok(workspace) => SharedWorkspace::new(workspace),
        Err(error) => return internal_error(error),
    };
    let (sender, mut subagent_events) = mpsc::unbounded_channel();
    let agent = state.agent(&workspace, sender);

    let events = async_stream::stream! {
        let _lock = lock;
        yield UiEvent::Workspace(workspace.snapshot());
        let runner = agent
            .prompt(request.message.clone())
            .conversation(session_id.clone())
            .max_turns(40)
            .add_hook(PlanGuard::new(workspace.clone()));
        let runner = match &state.store.history {
            History::Postgres(store) => runner.memory(store.clone()),
            History::Memory(store) => runner.memory(store.clone()),
        };
        let mut run = runner.stream();
        loop {
            // Subagents report through the channel while the main run waits on
            // their `task` calls, so both sources are polled together.
            let next = tokio::select! {
                Some(event) = subagent_events.recv() => Err(event),
                item = run.next() => Ok(item),
            };
            let item = match next {
                Err(subagent_event) => { yield subagent_event; continue; }
                Ok(Some(item)) => item,
                Ok(None) => break,
            };
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text { text, .. }))) => {
                    yield UiEvent::Text { delta: text };
                }
                Ok(MultiTurnStreamItem::ToolCall { tool_call }) => {
                    yield UiEvent::ToolCall {
                        id: tool_call.id.to_string(),
                        name: tool_call.function.name.to_string(),
                        args: serde_json::Value::Object(tool_call.function.arguments),
                    };
                }
                Ok(MultiTurnStreamItem::ToolResult { tool_result }) => {
                    yield UiEvent::ToolResult {
                        id: tool_result.call.to_string(),
                        name: tool_result.name.to_string(),
                        content: clip(&result_text(&tool_result.content), MAX_TOOL_RESULT_CHARS),
                        is_error: tool_result.is_error,
                    };
                    let snapshot = workspace.snapshot();
                    // A pod that dies mid-turn then loses at most the last tool's change.
                    if let Err(error) = state.store.save_workspace(&session_id, &snapshot).await {
                        tracing::warn!("could not save the workspace of {session_id}: {error:#}");
                    }
                    yield UiEvent::Workspace(snapshot);
                }
                Ok(MultiTurnStreamItem::ModelTurnRetried { .. }) => {
                    yield UiEvent::DiscardDraft;
                }
                Ok(MultiTurnStreamItem::FinalResponse(response)) => {
                    let usage = response.usage();
                    yield UiEvent::Done {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        requests: response.requests(),
                    };
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!("agent run failed: {error}");
                    yield UiEvent::Error { message: error.to_string() };
                    break;
                }
            }
        }
        while let Ok(event) = subagent_events.try_recv() {
            yield event;
        }
    };

    let events = events.map(|event| Ok::<_, std::convert::Infallible>(Event::default().json_data(event).unwrap_or_default()));
    Sse::new(events).keep_alive(KeepAlive::default()).into_response()
}

async fn reset(State(state): State<Arc<AppState>>, Json(request): Json<SessionRequest>) -> Response {
    match state.store.reset(&request.session_id).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(error) => internal_error(error),
    }
}

/// Ready once the knowledge base is loaded (before the server starts) and
/// while the session store answers. The MCP tools are optional.
async fn ready(State(state): State<Arc<AppState>>) -> StatusCode {
    if state.store.ready().await { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE }
}

async fn info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mcp_tools: Vec<String> = state.mcp_tools.iter().map(|tool| tool.name().to_string()).collect();
    let documents: Vec<_> = state
        .kb
        .documents
        .iter()
        .map(|doc| json!({ "path": doc.path, "title": doc.title }))
        .collect();
    Json(json!({
        "model": state.model_name,
        "mcp_tools": mcp_tools,
        "documents": documents,
        "store": state.store.kind(),
        "pod": std::env::var("POD_NAME").ok(),
    }))
}

fn result_text(content: &[ToolResultContent]) -> String {
    content
        .iter()
        .map(|part| match part {
            ToolResultContent::Text(text) => text.text.clone(),
            ToolResultContent::Json { value } => value.to_string(),
            ToolResultContent::Image(_) => "<image>".to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn clip(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Settings and secrets from a gitignored .env (searched upward from the working directory).
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        // No colour codes when the output is not a terminal, e.g. `kubectl logs`.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();

    let addr = std::env::var("AGENT_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());

    // As PID 1 in a container the process ignores SIGTERM until it has a
    // handler, so install one before the slow startup: a stop during startup
    // then exits at once instead of waiting out the whole grace period.
    let mut terminate = signal(SignalKind::terminate())?;
    let state = tokio::select! {
        state = start() => Arc::new(state?),
        _ = terminate.recv() => return Ok(()),
        _ = tokio::signal::ctrl_c() => return Ok(()),
    };

    // Sessions idle for longer than SESSION_TTL_DAYS (default 7; 0 keeps them) are deleted hourly.
    let ttl_days: f64 = std::env::var("SESSION_TTL_DAYS").ok().and_then(|days| days.parse().ok()).unwrap_or(7.0);
    if ttl_days > 0.0 {
        let state = state.clone();
        let idle = std::time::Duration::from_secs_f64(ttl_days * 86_400.0);
        tokio::spawn(async move {
            let mut hourly = tokio::time::interval(std::time::Duration::from_secs(3_600));
            loop {
                hourly.tick().await;
                match state.store.expire(idle).await {
                    Ok(0) => {}
                    Ok(gone) => tracing::info!("deleted {gone} conversations idle for over {ttl_days} days"),
                    Err(error) => tracing::warn!("session expiry failed: {error:#}"),
                }
            }
        });
    }

    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../static/index.html")) }))
        .route("/api/info", get(info))
        .route("/api/chat", post(chat))
        .route("/api/reset", post(reset))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("agent UI on http://{addr}");
    // Running agent turns are open requests, so they finish before the process exits.
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal(terminate)).await?;
    Ok(())
}

/// The model, knowledge base, MCP tools and session store.
async fn start() -> anyhow::Result<AppState> {
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let model_name = env("AGENT_MODEL", anthropic::completion::CLAUDE_SONNET_5_5);
    let kb_dir = PathBuf::from(env("KB_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/../knowledge-base")));
    let mcp_url = env("MCP_URL", "http://127.0.0.1:8081/mcp");

    let model = AnthropicConfig::from_env()?.client().completion(model_name.as_str()).erase();

    let kb = KnowledgeBase::load(&kb_dir)?;
    tracing::info!("loaded {} documents from {}", kb.documents.len(), kb_dir.display());

    // Retry for a few seconds: in containers both servers start at the same time.
    let mut mcp_tools = Vec::new();
    // Each attempt is capped: a connect to a pod that is going away can otherwise hang for 30s.
    for attempt in 1..=10 {
        let connect = tokio::time::timeout(std::time::Duration::from_secs(2), connect_mcp(&mcp_url));
        match connect.await.unwrap_or_else(|_| Err(anyhow::anyhow!("timed out"))) {
            Ok(tools) => {
                tracing::info!("connected to MCP server at {mcp_url} with {} tools", tools.len());
                mcp_tools = tools;
                break;
            }
            Err(error) if attempt == 10 => tracing::warn!("no MCP tools: could not connect to {mcp_url}: {error}"),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
        }
    }

    let store = Store::open(std::env::var("DATABASE_URL").ok().filter(|url| !url.is_empty())).await?;
    tracing::info!("sessions are kept in {}", store.kind());

    Ok(AppState {
        model,
        model_name,
        kb: Arc::new(kb),
        mcp_tools,
        store,
    })

}

/// Resolves on Ctrl-C or SIGTERM (what Kubernetes and Docker send on stop).
async fn shutdown_signal(mut terminate: Signal) {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
    tracing::info!("shutting down; waiting for running requests to finish");
}
