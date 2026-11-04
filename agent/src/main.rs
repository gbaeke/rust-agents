//! A "deep agent" built with rig: it plans with a todo list, keeps notes in
//! a scratch filesystem, searches a knowledge base (agentic RAG), delegates
//! research to subagents and uses the tools of an MCP server. A small web UI
//! talks to it over server-sent events.
//!
//! Run the MCP server first (`cargo run -p mcp-server`), then this one with
//! `ANTHROPIC_API_KEY` set, and open http://127.0.0.1:3000.

mod events;
mod kb;
mod tools;

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{Stream, StreamExt};
use rig::agent::{Agent, MultiTurnStreamItem};
use rig::message::{Message, ToolResultContent};
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
use tokio::sync::mpsc;

use events::{EventSender, UiEvent};
use kb::KnowledgeBase;
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
    sessions: Mutex<HashMap<String, Session>>,
}

#[derive(Default)]
struct Session {
    history: Vec<Message>,
    workspace: SharedWorkspace,
}

impl AppState {
    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

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

/// Runs the agent on one message and streams what happens as SSE events.
async fn chat(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ChatRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (history, workspace) = {
        let mut sessions = state.sessions();
        let session = sessions.entry(request.session_id.clone()).or_default();
        (session.history.clone(), session.workspace.clone())
    };
    let (sender, mut subagent_events) = mpsc::unbounded_channel();
    let agent = state.agent(&workspace, sender);

    let events = async_stream::stream! {
        let mut run = agent
            .prompt(request.message.clone())
            .history(history)
            .max_turns(40)
            .add_hook(PlanGuard::new(workspace.clone()))
            .stream();
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
                    yield UiEvent::Workspace(workspace.snapshot());
                }
                Ok(MultiTurnStreamItem::ModelTurnRetried { .. }) => {
                    yield UiEvent::DiscardDraft;
                }
                Ok(MultiTurnStreamItem::FinalResponse(response)) => {
                    if let Some(session) = state.sessions().get_mut(&request.session_id) {
                        session.history.extend(response.messages().iter().cloned());
                    }
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

    Sse::new(events.map(|event| Ok(Event::default().json_data(event).unwrap_or_default())))
        .keep_alive(KeepAlive::default())
}

async fn reset(State(state): State<Arc<AppState>>, Json(request): Json<SessionRequest>) -> impl IntoResponse {
    state.sessions().remove(&request.session_id);
    Json(json!({ "ok": true }))
}

async fn info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mcp_tools: Vec<String> = state.mcp_tools.iter().map(|tool| tool.name().to_string()).collect();
    let documents: Vec<_> = state
        .kb
        .documents
        .iter()
        .map(|doc| json!({ "path": doc.path, "title": doc.title }))
        .collect();
    Json(json!({ "model": state.model_name, "mcp_tools": mcp_tools, "documents": documents }))
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
        .init();

    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let model_name = env("AGENT_MODEL", anthropic::completion::CLAUDE_SONNET_5_5);
    let kb_dir = PathBuf::from(env("KB_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/../knowledge-base")));
    let mcp_url = env("MCP_URL", "http://127.0.0.1:8081/mcp");
    let addr = env("AGENT_ADDR", "127.0.0.1:3000");

    let model = AnthropicConfig::from_env()?.client().completion(model_name.as_str()).erase();

    let kb = KnowledgeBase::load(&kb_dir)?;
    tracing::info!("loaded {} documents from {}", kb.documents.len(), kb_dir.display());

    // Retry for a few seconds: in containers both servers start at the same time.
    let mut mcp_tools = Vec::new();
    for attempt in 1..=10 {
        match connect_mcp(&mcp_url).await {
            Ok(tools) => {
                tracing::info!("connected to MCP server at {mcp_url} with {} tools", tools.len());
                mcp_tools = tools;
                break;
            }
            Err(error) if attempt == 10 => tracing::warn!("no MCP tools: could not connect to {mcp_url}: {error}"),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
        }
    }

    let state = Arc::new(AppState {
        model,
        model_name,
        kb: Arc::new(kb),
        mcp_tools,
        sessions: Mutex::default(),
    });

    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../static/index.html")) }))
        .route("/api/info", get(info))
        .route("/api/chat", post(chat))
        .route("/api/reset", post(reset))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("agent UI on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
