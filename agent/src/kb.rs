//! An in-memory knowledge base over markdown files, searched with BM25.
//!
//! Documents are split into sections at `#`/`##`/`###` headings. Each section
//! is one searchable chunk, so a hit points the agent at a specific part of a
//! document, which it can then read in full with `read_document`.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

const K1: f64 = 1.2;
const B: f64 = 0.75;

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "have", "how", "i", "in", "is",
    "it", "its", "of", "on", "or", "our", "that", "the", "their", "this", "to", "was", "we", "what", "when",
    "which", "who", "will", "with", "you", "your", "do", "does", "can",
];

pub struct Document {
    pub path: String,
    pub title: String,
    pub content: String,
}

struct Chunk {
    doc: usize,
    heading: String,
    text: String,
    terms: HashMap<String, u32>,
    len: u32,
}

pub struct KnowledgeBase {
    pub documents: Vec<Document>,
    chunks: Vec<Chunk>,
    doc_freq: HashMap<String, u32>,
    avg_len: f64,
}

#[derive(Serialize)]
pub struct Hit {
    pub document: String,
    pub section: String,
    pub score: f64,
    pub snippet: String,
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .filter(|word| !STOPWORDS.contains(&word.as_str()))
        // A crude stemmer: "credits" and "credit" should match.
        .map(|word| match word.strip_suffix('s') {
            Some(stem) if stem.len() > 3 && !stem.ends_with('s') => stem.to_owned(),
            _ => word,
        })
        .collect()
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
        let mut chunks = Vec::new();
        let mut doc_freq: HashMap<String, u32> = HashMap::new();
        for (doc, document) in documents.iter().enumerate() {
            for (heading, text) in sections(&document.content) {
                // The title and heading are indexed too: "SLA credits" should
                // find the section headed "Service credits" in the SLA document.
                let tokens = tokenize(&format!("{} {heading} {text}", document.title));
                let mut terms = HashMap::new();
                for token in &tokens {
                    *terms.entry(token.clone()).or_insert(0) += 1;
                }
                for term in terms.keys() {
                    *doc_freq.entry(term.clone()).or_insert(0) += 1;
                }
                chunks.push(Chunk { doc, heading, text, terms, len: tokens.len() as u32 });
            }
        }
        let avg_len = chunks.iter().map(|chunk| chunk.len as f64).sum::<f64>() / chunks.len().max(1) as f64;
        Self { documents, chunks, doc_freq, avg_len }
    }

    pub fn search(&self, query: &str, top_k: usize) -> Vec<Hit> {
        let query = tokenize(query);
        let n = self.chunks.len() as f64;
        let mut scored: Vec<(f64, &Chunk)> = self
            .chunks
            .iter()
            .map(|chunk| {
                let score = query
                    .iter()
                    .filter_map(|term| {
                        let tf = *chunk.terms.get(term)? as f64;
                        let df = *self.doc_freq.get(term)? as f64;
                        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                        let norm = K1 * (1.0 - B + B * chunk.len as f64 / self.avg_len);
                        Some(idf * tf * (K1 + 1.0) / (tf + norm))
                    })
                    .sum::<f64>();
                (score, chunk)
            })
            .filter(|(score, _)| *score > 0.0)
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
            .into_iter()
            .take(top_k)
            .map(|(score, chunk)| Hit {
                document: self.documents[chunk.doc].path.clone(),
                section: chunk.heading.clone(),
                score: (score * 100.0).round() / 100.0,
                snippet: snippet(&chunk.text, 600),
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
