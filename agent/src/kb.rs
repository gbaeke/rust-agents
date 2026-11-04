//! An in-memory knowledge base over markdown files, searched with BM25.
//!
//! Documents are split into sections at `#`/`##`/`###` headings. Each section
//! is one searchable chunk, so a hit points the agent at a specific part of a
//! document, which it can then read in full with `read_document`. Tokenizing
//! (stemming, stop words) and BM25 scoring come from the `bm25` crate.

use std::path::Path;

use bm25::{Language, SearchEngine, SearchEngineBuilder};
use serde::Serialize;

pub struct Document {
    pub path: String,
    pub title: String,
    pub content: String,
}

struct Chunk {
    doc: usize,
    heading: String,
    text: String,
}

pub struct KnowledgeBase {
    pub documents: Vec<Document>,
    chunks: Vec<Chunk>,
    /// Indexes the chunks; a search result's id is the chunk's position.
    engine: SearchEngine<usize>,
}

#[derive(Serialize)]
pub struct Hit {
    pub document: String,
    pub section: String,
    pub score: f32,
    pub snippet: String,
}

fn sections(content: &str) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    let mut heading = String::new();
    let mut body = String::new();
    for line in content.lines() {
        if let Some(title) = line.strip_prefix("#").map(|rest| rest.trim_start_matches('#').trim()) {
            if !body.trim().is_empty() {
                sections.push((heading.clone(), body.trim().to_owned()));
            }
            heading = title.to_owned();
            body.clear();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if !body.trim().is_empty() {
        sections.push((heading, body.trim().to_owned()));
    }
    sections
}

impl KnowledgeBase {
    /// Loads every `.md` file in `dir` (not recursive).
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let mut paths: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
            .collect();
        paths.sort();

        let mut documents = Vec::new();
        for path in paths {
            let content = std::fs::read_to_string(&path)?;
            let title = content
                .lines()
                .find_map(|line| line.strip_prefix("# "))
                .unwrap_or_default()
                .to_owned();
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            documents.push(Document { path: name, title, content });
        }
        Ok(Self::index(documents))
    }

    fn index(documents: Vec<Document>) -> Self {
        let chunks: Vec<Chunk> = documents
            .iter()
            .enumerate()
            .flat_map(|(doc, document)| {
                sections(&document.content)
                    .into_iter()
                    .map(move |(heading, text)| Chunk { doc, heading, text })
            })
            .collect();
        // The title and heading are indexed too: "SLA credits" should find
        // the section headed "Service credits" in the SLA document.
        let corpus = chunks.iter().enumerate().map(|(id, chunk)| bm25::Document {
            id,
            contents: format!("{} {} {}", documents[chunk.doc].title, chunk.heading, chunk.text),
        });
        let engine = SearchEngineBuilder::with_documents(Language::English, corpus).build();
        Self { documents, chunks, engine }
    }

    pub fn search(&self, query: &str, top_k: usize) -> Vec<Hit> {
        self.engine
            .search(query, top_k)
            .into_iter()
            .map(|result| {
                let chunk = &self.chunks[result.document.id];
                Hit {
                    document: self.documents[chunk.doc].path.clone(),
                    section: chunk.heading.clone(),
                    score: (result.score * 100.0).round() / 100.0,
                    snippet: snippet(&chunk.text, 600),
                }
            })
            .collect()
    }

    pub fn document(&self, path: &str) -> Option<&Document> {
        self.documents.iter().find(|doc| doc.path == path)
    }

    /// Each document with its section headings, as an outline for the agent.
    pub fn outline(&self) -> Vec<(String, String, Vec<String>)> {
        self.documents
            .iter()
            .enumerate()
            .map(|(index, doc)| {
                let headings = self
                    .chunks
                    .iter()
                    .filter(|chunk| chunk.doc == index && !chunk.heading.is_empty() && chunk.heading != doc.title)
                    .map(|chunk| chunk.heading.clone())
                    .collect();
                (doc.path.clone(), doc.title.clone(), headings)
            })
            .collect()
    }
}

fn snippet(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb() -> KnowledgeBase {
        KnowledgeBase::index(vec![
            Document {
                path: "sla.md".into(),
                title: "Plans and SLA".into(),
                content: "# Plans and SLA\n\n## Service credits\n\nCredits are paid when uptime drops.\n\n## Plans\n\nStarter, Pro.".into(),
            },
            Document {
                path: "expenses.md".into(),
                title: "Expense policy".into(),
                content: "# Expense policy\n\n## Meals\n\nDinner limit is EUR 35.".into(),
            },
        ])
    }

    #[test]
    fn finds_the_matching_section() {
        let hits = kb().search("how do service credits work", 3);
        assert_eq!(hits[0].document, "sla.md");
        assert_eq!(hits[0].section, "Service credits");
    }

    #[test]
    fn searches_the_sample_knowledge_base() {
        let kb = KnowledgeBase::load(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../knowledge-base"))).unwrap();
        for (query, document) in [
            ("SLA service credit enterprise", "pricing-and-sla.md"),
            ("which customers were affected by the outage", "incident-2026-03-18-postmortem.md"),
            ("customer dinner per person limit", "expense-policy.md"),
            ("RFID range thermal camera", "firmware-release-notes.md"),
        ] {
            assert_eq!(kb.search(query, 1)[0].document, document, "query: {query}");
        }
    }

    #[test]
    fn unrelated_query_finds_nothing() {
        assert!(kb().search("quantum entanglement", 3).is_empty());
    }
}
