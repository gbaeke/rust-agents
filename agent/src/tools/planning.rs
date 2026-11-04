use std::sync::atomic::{AtomicUsize, Ordering};

use rig::agent::{AgentHook, HookContext, ModelTurnAction, ModelTurnFinished};
use rig::message::AssistantContent;
use rig::tool::{Tool, ToolContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{SharedWorkspace, ToolError, schema};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct Todo {
    /// What needs doing, as a short imperative sentence.
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteTodosArgs {
    /// The complete, updated todo list. It replaces the previous list.
    pub todos: Vec<Todo>,
}

pub struct WriteTodos {
    pub workspace: SharedWorkspace,
}

impl Tool for WriteTodos {
    const NAME: &'static str = "write_todos";
    type Error = ToolError;
    type Args = WriteTodosArgs;
    type Output = String;

    fn description(&self) -> String {
        "Create or update your plan as a todo list. Use it for any task with three or more steps: \
         write the plan first, mark exactly one item in_progress while you work on it, and call this \
         tool again to mark each item completed as soon as it is done. Before your final answer every \
         item must be completed (remove items you decided to skip). Always send the whole list."
            .to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        schema::<WriteTodosArgs>()
    }

    async fn call(&self, _: &mut ToolContext, args: Self::Args) -> Result<String, ToolError> {
        let in_progress = args.todos.iter().filter(|todo| todo.status == TodoStatus::InProgress).count();
        if in_progress > 1 {
            return Err(ToolError(format!(
                "{in_progress} todos are in_progress; keep exactly one in_progress at a time"
            )));
        }
        let done = args.todos.iter().filter(|todo| todo.status == TodoStatus::Completed).count();
        let total = args.todos.len();
        self.workspace.lock().todos = args.todos;
        Ok(if done == total {
            format!("Plan updated: all {total} items completed.")
        } else {
            format!(
                "Plan updated: {done}/{total} completed. Call write_todos again as you finish each item; \
                 the plan must be fully completed before your final answer."
            )
        })
    }
}

/// How many times one run may be sent back to finish its plan.
const MAX_PLAN_NUDGES: usize = 2;

/// Models often write a plan and then forget to tick it off. This hook
/// rejects a final answer (a turn without tool calls) while the plan still
/// has open items, and asks the model to update the plan first. The rejected
/// draft is discarded by the UI, so the model is told to answer again in full.
pub struct PlanGuard {
    workspace: SharedWorkspace,
    nudges: AtomicUsize,
}

impl PlanGuard {
    pub fn new(workspace: SharedWorkspace) -> Self {
        Self { workspace, nudges: AtomicUsize::new(0) }
    }
}

impl AgentHook for PlanGuard {
    async fn on_model_turn_finished(&self, _: &HookContext, event: ModelTurnFinished<'_>) -> ModelTurnAction {
        let calls_tools = event.content.iter().any(|content| matches!(content, AssistantContent::ToolCall(_)));
        if calls_tools {
            return ModelTurnAction::continue_run();
        }
        let open: Vec<String> = self
            .workspace
            .lock()
            .todos
            .iter()
            .filter(|todo| todo.status != TodoStatus::Completed)
            .map(|todo| format!("- {} ({:?})", todo.content, todo.status))
            .collect();
        if open.is_empty() || self.nudges.fetch_add(1, Ordering::Relaxed) >= MAX_PLAN_NUDGES {
            return ModelTurnAction::continue_run();
        }
        ModelTurnAction::retry_with_feedback(format!(
            "<system-reminder>Your plan still has open items:\n{}\n\nBefore finishing, call write_todos so \
             the plan reflects what actually happened: mark finished items completed, finish any item you \
             still need to do, and remove items you decided to skip. Then write your complete final answer \
             again; the previous draft was not shown to the user.</system-reminder>",
            open.join("\n")
        ))
    }
}
