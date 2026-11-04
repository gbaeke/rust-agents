//! Agentic RAG: instead of stuffing retrieved chunks into the prompt, the
//! agent gets search and read tools and decides itself what to look up,
//! how to rephrase a query that found nothing, and which documents to read
//! in full.

use std::sync::Arc;

use rig::tool::{Tool, ToolContext};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{ToolError, schema};
use crate::kb::KnowledgeBase;

#[derive(Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Keywords to search for. Short, specific queries work best.
    pub query: String,
    /// Number of results, 1 to 10. Defaults to 5.
    pub top_k: Option<usize>,
}

pub struct SearchKnowledgeBase {
    pub kb: Arc<KnowledgeBase>,
}

impl Tool for SearchKnowledgeBase {
    const NAME: &'static str = "search_knowledge_base";
    type Error = ToolError;
    type Args = SearchArgs;
    type Output = String;

    fn description(&self) -> String {
        "Keyword search (BM25) over the company knowledge base: product catalog, pricing and SLA, \
         incident postmortems, policies, API docs and release notes. Returns matching sections with \
         snippets. Search several times with different keywords when a question has several parts, \
         and read the full document when a snippet is cut off."
            .to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        schema::<SearchArgs>()
    }

    async fn call(&self, _: &mut ToolContext, args: SearchArgs) -> Result<String, ToolError> {
        let hits = self.kb.search(&args.query, args.top_k.unwrap_or(5).clamp(1, 10));
        if hits.is_empty() {
            return Ok(format!(
                "No results for `{}`. Try other keywords or call list_documents.",
                args.query
            ));
        }
        serde_json::to_string_pretty(&hits).map_err(|error| ToolError(error.to_string()))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadDocumentArgs {
    /// The document's file name as returned by search or list_documents, e.g. `pricing-and-sla.md`.
    pub document: String,
}

pub struct ReadDocument {
    pub kb: Arc<KnowledgeBase>,
}

impl Tool for ReadDocument {
    const NAME: &'static str = "read_document";
    type Error = ToolError;
    type Args = ReadDocumentArgs;
    type Output = String;

    fn description(&self) -> String {
        "Read a whole knowledge-base document. Use it when search snippets are not enough, for \
         example to read a full table."
            .to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        schema::<ReadDocumentArgs>()
    }

    async fn call(&self, _: &mut ToolContext, args: ReadDocumentArgs) -> Result<String, ToolError> {
        self.kb
            .document(args.document.trim())
            .map(|doc| doc.content.clone())
            .ok_or_else(|| ToolError(format!("no document `{}`; call list_documents", args.document)))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct NoArgs {}

pub struct ListDocuments {
    pub kb: Arc<KnowledgeBase>,
}

impl Tool for ListDocuments {
    const NAME: &'static str = "list_documents";
    type Error = ToolError;
    type Args = NoArgs;
    type Output = String;

    fn description(&self) -> String {
        "List the knowledge-base documents with their titles and section headings.".to_owned()
    }

    fn parameters(&self) -> serde_json::Value {
        schema::<NoArgs>()
    }

    async fn call(&self, _: &mut ToolContext, _: NoArgs) -> Result<String, ToolError> {
        Ok(self
            .kb
            .outline()
            .into_iter()
            .map(|(path, title, headings)| format!("- {path}: {title}\n  sections: {}", headings.join("; ")))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}
