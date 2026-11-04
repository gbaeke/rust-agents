//! The events the server streams to the browser over SSE.

use serde::Serialize;

use crate::tools::Workspace;

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    /// A fragment of the assistant's answer.
    Text { delta: String },
    /// The model decided to call a tool.
    ToolCall { id: String, name: String, args: serde_json::Value },
    /// A tool answered.
    ToolResult { id: String, name: String, content: String, is_error: bool },
    /// The model's last answer draft was rejected (see `PlanGuard`); drop it.
    DiscardDraft,
    /// Something a subagent started by the `task` tool did.
    Subagent { task: String, kind: String, detail: String },
    /// The session's todos and files after a tool ran.
    Workspace(Workspace),
    /// The run finished.
    Done { input_tokens: Option<u64>, output_tokens: Option<u64>, requests: usize },
    Error { message: String },
}

pub type EventSender = tokio::sync::mpsc::UnboundedSender<UiEvent>;
