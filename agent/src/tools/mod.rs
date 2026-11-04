//! The agent's local tools. Together with the MCP server's tools they give
//! the agent the "deep agent" toolkit: a plan (`write_todos`), a scratch
//! filesystem, a searchable knowledge base and subagents (`task`).

mod files;
mod knowledge;
mod planning;
mod task;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;

pub use files::{EditFile, ListFiles, ReadFile, WriteFile};
pub use knowledge::{ListDocuments, ReadDocument, SearchKnowledgeBase};
pub use planning::{PlanGuard, Todo, WriteTodos};
pub use task::Task;

/// The state an agent builds up during a conversation: its plan and the
/// files it wrote. It lives as long as the chat session.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Workspace {
    pub todos: Vec<Todo>,
    pub files: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
pub struct SharedWorkspace(Arc<Mutex<Workspace>>);

impl SharedWorkspace {
    pub fn lock(&self) -> MutexGuard<'_, Workspace> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn snapshot(&self) -> Workspace {
        self.lock().clone()
    }
}

/// A tool failure, shown to the model so it can correct itself.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ToolError(pub String);

/// The JSON Schema of a tool's arguments, derived from its Rust type.
fn schema<T: schemars::JsonSchema>() -> serde_json::Value {
    let mut value = serde_json::to_value(schemars::schema_for!(T)).unwrap_or_default();
    if let Some(object) = value.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    value
}
