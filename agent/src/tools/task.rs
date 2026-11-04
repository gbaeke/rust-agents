//! The `task` tool starts a subagent with a fresh context window. It does
//! the digging (searches, reads, calculations) and returns only a concise
//! report, so the main agent's context stays small. Several `task` calls in
//! one turn give the main agent parallel researchers.

use std::sync::Arc;

use futures::StreamExt;
use rig::DynModel;
use rig::agent::MultiTurnStreamItem;
use rig::operation::Completion;
use rig::prelude::*;
use rig::tool::{DynamicTool, Tool, ToolContext};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{ListDocuments, ReadDocument, SearchKnowledgeBase, ToolError, schema};
use crate::events::{EventSender, UiEvent};
use crate::kb::KnowledgeBase;

const RESEARCHER_PREAMBLE: &str = "You are a research subagent. You get one self-contained research task. \
Use search_knowledge_base (several times, with different keywords), read_document for full context, and \
the calculate tool for any arithmetic. Do not guess: every fact in your answer must come from a document \
or a tool result. Finish with a concise report: the answer, the key facts with their source documents, \
and anything you could not find.";

#[derive(Deserialize, JsonSchema)]
pub struct TaskArgs {
    /// A complete, self-contained description of the research task. The subagent
    /// cannot see the conversation, so include every detail it needs.
    pub description: String,
}

pub struct Task {
    pub model: DynModel<Completion>,
    pub kb: Arc<KnowledgeBase>,
    pub mcp_tools: Vec<DynamicTool>,
    pub events: EventSender,
}

impl Tool for Task {
    const NAME: &'static str = "task";
    type Error = ToolError;
    type Args = TaskArgs;
    type Output = String;

    fn description(&self) -> String {
        "Delegate a research task to a subagent with its own context window. It can search and read \
         the knowledge base and use the calculator, weather, time and web tools, and returns a short \
         report. Use it for independent sub-questions; call it several times in the same turn to \
         research in parallel."
            .to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        schema::<TaskArgs>()
    }

    async fn call(&self, _: &mut ToolContext, args: TaskArgs) -> Result<String, ToolError> {
        let label: String = args.description.chars().take(70).collect();
        let notify = |kind: &str, detail: String| {
            let _ = self.events.send(UiEvent::Subagent { task: label.clone(), kind: kind.to_owned(), detail });
        };
        notify("started", args.description.clone());

        let researcher = AgentBuilder::new(self.model.clone())
            .preamble(RESEARCHER_PREAMBLE)
            .max_tokens(4096)
            .tool(SearchKnowledgeBase { kb: self.kb.clone() })
            .tool(ReadDocument { kb: self.kb.clone() })
            .tool(ListDocuments { kb: self.kb.clone() })
            .dynamic_tools(self.mcp_tools.clone())
            .build();

        let mut stream = researcher.prompt(args.description.as_str()).max_turns(15).stream();
        let mut report = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::ToolCall { tool_call }) => notify(
                    "tool_call",
                    format!(
                        "{}({})",
                        tool_call.function.name,
                        serde_json::Value::Object(tool_call.function.arguments.clone())
                    ),
                ),
                Ok(MultiTurnStreamItem::FinalResponse(response)) => report = Some(response.output()),
                Ok(_) => {}
                Err(error) => {
                    notify("error", error.to_string());
                    return Err(ToolError(format!("the subagent failed: {error}")));
                }
            }
        }
        let report = report.ok_or_else(|| ToolError("the subagent finished without a report".to_owned()))?;
        notify("done", report.clone());
        Ok(report)
    }
}
