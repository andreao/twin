//! Semantic search over document text (design_doc §9.8, §9.14 note) — chunk →
//! embed via the local model host → cosine top-k.  The vector store is a delegated
//! index in the §9.14 sense: derived, rebuildable, persisted as JSONL next to the
//! other materialized views; the source text's lineage is the (source, seq) key.
//! Brute-force cosine is the right call at document scale — thousands of chunks,
//! not billions; no ANN structure to keep incremental.

use crate::ollama;
use serde_json::Value;
use std::io::Write;

pub struct EmbedStore {
    path: String,
    items: Vec<Item>,
}

struct Item {
    source: String,
    text: String,
    vec: Vec<f32>,
}

/// One search hit: where it came from, the chunk itself, and the cosine score.
pub struct Hit {
    pub source: String,
    pub text: String,
    pub score: f32,
}

impl EmbedStore {
    /// Open (or start) the store persisted at `path` (JSONL, one chunk per line).
    pub fn open(path: &str) -> Self {
        let mut items = Vec::new();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                let vec: Vec<f32> = v["vec"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
                    .unwrap_or_default();
                if vec.is_empty() {
                    continue;
                }
                items.push(Item {
                    source: v["source"].as_str().unwrap_or("").to_string(),
                    text: v["text"].as_str().unwrap_or("").to_string(),
                    vec,
                });
            }
        }
        EmbedStore { path: path.to_string(), items }
    }

    /// Is this source already embedded?  (Replay and re-reads skip the model.)
    pub fn has(&self, source: &str) -> bool {
        self.items.iter().any(|i| i.source == source)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Chunk + embed a document's text blocks and persist them under `source`.
    pub fn add_document(&mut self, source: &str, blocks: &[String]) -> Result<usize, String> {
        let chunks: Vec<String> = blocks.iter().flat_map(|b| chunk(b)).collect();
        if chunks.is_empty() {
            return Ok(0);
        }
        let vecs = ollama::embed(&ollama::embed_model(), &chunks)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("{}: {e}", self.path))?;
        let mut added = 0;
        for (text, vec) in chunks.into_iter().zip(vecs) {
            if vec.is_empty() {
                continue;
            }
            let line = serde_json::json!({ "source": source, "text": text, "vec": vec });
            let _ = writeln!(file, "{line}");
            self.items.push(Item { source: source.to_string(), text, vec });
            added += 1;
        }
        Ok(added)
    }

    /// Embed the query and return the best `k` chunks across all sources.
    pub fn search(&self, query: &str, k: usize) -> Result<Vec<Hit>, String> {
        if self.items.is_empty() {
            return Ok(Vec::new());
        }
        let qv = ollama::embed(&ollama::embed_model(), &[query.to_string()])?
            .into_iter()
            .next()
            .filter(|v| !v.is_empty())
            .ok_or("the embedding model returned nothing for the query")?;
        let mut scored: Vec<(f32, &Item)> = self
            .items
            .iter()
            .map(|i| (cosine(&qv, &i.vec), i))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored
            .into_iter()
            .take(k)
            .map(|(score, i)| Hit { source: i.source.clone(), text: i.text.clone(), score })
            .collect())
    }
}

/// Split text into ~700-char chunks on sentence-ish boundaries, with a floor so
/// title-block fragments don't become one-word chunks.
pub fn chunk(text: &str) -> Vec<String> {
    const TARGET: usize = 700;
    let t = text.trim();
    if t.is_empty() {
        return Vec::new();
    }
    if t.len() <= TARGET {
        return vec![t.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for piece in t.split_inclusive(['.', '\n', ';']) {
        if cur.len() + piece.len() > TARGET && cur.len() > 100 {
            out.push(std::mem::take(&mut cur).trim().to_string());
        }
        // a single run longer than the target still becomes its own chunk(s)
        if piece.len() > TARGET {
            let mut s = piece;
            while s.len() > TARGET {
                let cut = (0..=TARGET).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
                out.push(s[..cut].trim().to_string());
                s = &s[cut..];
            }
            cur.push_str(s);
        } else {
            cur.push_str(piece);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out.retain(|c| !c.is_empty());
    out
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_respect_target_and_floor() {
        let text = "One sentence. ".repeat(200); // ~2800 chars
        let cs = chunk(&text);
        assert!(cs.len() >= 3);
        assert!(cs.iter().all(|c| c.len() <= 720), "chunk sizes: {:?}", cs.iter().map(|c| c.len()).collect::<Vec<_>>());
        assert!(cs.iter().all(|c| c.len() > 50));
    }

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk("Hugin Fm. top at 2145 m MD."), vec!["Hugin Fm. top at 2145 m MD.".to_string()]);
    }

    #[test]
    fn cosine_orders_similarity() {
        let a = [1.0, 0.0, 0.0];
        assert!(cosine(&a, &[1.0, 0.0, 0.0]) > 0.99);
        assert!(cosine(&a, &[0.0, 1.0, 0.0]).abs() < 1e-6);
        assert!(cosine(&a, &[0.7, 0.7, 0.0]) > 0.5);
    }

    #[test]
    fn store_roundtrips_through_its_file() {
        let path = std::env::temp_dir().join("twin_embed_test.jsonl");
        let p = path.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        {
            let mut s = EmbedStore::open(p);
            // bypass the model: persist a hand-made item through the same format
            let line = serde_json::json!({ "source": "doc:x", "text": "hello", "vec": [0.6, 0.8] });
            std::fs::write(p, format!("{line}\n")).unwrap();
            s.items.push(Item { source: "doc:x".into(), text: "hello".into(), vec: vec![0.6, 0.8] });
            assert!(s.has("doc:x"));
        }
        let s = EmbedStore::open(p);
        assert_eq!(s.len(), 1);
        assert!(s.has("doc:x"));
        let _ = std::fs::remove_file(p);
    }
}
