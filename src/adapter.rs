//! Boundary adapter: mock external DB + poll/diff (design_doc §9.9).
//!
//! The adapter manufactures a reliable edit stream from an external system that does
//! not provide one: it snapshots the source, diffs against last-known state by
//! stable key + content hash, and emits Z-set deltas.  It advances a durable cursor
//! on write-through so its own writes are not re-ingested on the next poll (echo
//! suppression, §9.7).  This is the Rust side of the boundary; deltas cross into the
//! V8 graph as JSON (the one place serialization is correct — the external seam).

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::hashing::content_hash;
use crate::jsgraph::JsGraph;

/// A keyed external table.  An independent writer may mutate it, which is exactly
/// what forces the poll+diff adapter to reconcile rather than assume write-through.
pub struct MockDb {
    pub key_field: String,
    rows: HashMap<String, Value>,
}

impl MockDb {
    pub fn new(key_field: &str) -> Self {
        MockDb { key_field: key_field.to_string(), rows: HashMap::new() }
    }
    fn key_of(&self, row: &Value) -> String {
        match &row[&self.key_field] {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
    pub fn insert(&mut self, row: Value) {
        self.rows.insert(self.key_of(&row), row);
    }
    pub fn update(&mut self, key: &str, field: &str, value: Value) {
        if let Some(row) = self.rows.get_mut(key) {
            row[field] = value;
        }
    }
    pub fn upsert(&mut self, row: Value) {
        self.rows.insert(self.key_of(&row), row);
    }
    pub fn delete(&mut self, key: &str) {
        self.rows.remove(key);
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.rows.get(key)
    }
    fn snapshot(&self) -> HashMap<String, Value> {
        self.rows.clone()
    }
}

fn row_hash(row: &Value) -> String {
    // serde_json::Value objects serialize with sorted keys (BTreeMap) -> stable.
    content_hash(row.to_string().as_bytes())
}

pub struct PollDiffAdapter {
    pub stream: String,
    pub author: String,
    cursor: HashMap<String, String>, // key -> content hash (durable cursor, §9.9)
    known: HashMap<String, Value>,
    pub poll_epoch: u64,
}

impl PollDiffAdapter {
    pub fn new(stream: &str, author: &str) -> Self {
        PollDiffAdapter {
            stream: stream.to_string(),
            author: author.to_string(),
            cursor: HashMap::new(),
            known: HashMap::new(),
            poll_epoch: 0,
        }
    }

    /// One poll cycle: snapshot, diff, submit the net delta into the V8 graph.
    /// Returns the number of changed rows in the delta.
    pub fn poll(&mut self, db: &MockDb, graph: &mut JsGraph) -> usize {
        self.poll_epoch += 1;
        let snap = db.snapshot();
        let mut delta: Vec<(Value, i64)> = Vec::new();

        for (key, row) in &snap {
            let h = row_hash(row);
            if self.cursor.get(key) == Some(&h) {
                continue; // unchanged (content-hash skip)
            }
            if let Some(old) = self.known.get(key) {
                delta.push((old.clone(), -1)); // retract old
            }
            delta.push((row.clone(), 1)); // assert new
        }
        for (key, old) in &self.known {
            if !snap.contains_key(key) {
                delta.push((old.clone(), -1)); // delete detection
            }
        }

        self.known = snap;
        self.cursor = self.known.iter().map(|(k, r)| (k.clone(), row_hash(r))).collect();

        let n = delta.len();
        if !delta.is_empty() {
            let delta_json = serde_json::to_string(&delta).unwrap();
            let prov = json!({"origin": "upstream", "author": self.author,
                              "note": format!("poll#{}", self.poll_epoch)});
            graph.submit(&self.stream, &delta_json, &prov.to_string());
        }
        n
    }

    /// Apply a backward edit (from the graph's outbox) to the external DB and reflect
    /// it optimistically.  The cursor advances to the written value, so the next poll
    /// diff sees no self-induced change (echo suppression, §9.7).
    pub fn write_through(&mut self, edit: &[(Value, i64)], author: &str,
                         db: &mut MockDb, graph: &mut JsGraph) {
        let asserted: std::collections::HashSet<String> =
            edit.iter().filter(|(_, w)| *w > 0).map(|(r, _)| db.key_of(r)).collect();
        for (row, w) in edit {
            let key = db.key_of(row);
            if *w > 0 {
                db.upsert(row.clone());
                self.known.insert(key.clone(), row.clone());
                self.cursor.insert(key, row_hash(row)); // cursor advances past our write
            } else if *w < 0 && !asserted.contains(&key) {
                db.delete(&key);
                self.known.remove(&key);
                self.cursor.remove(&key);
            }
        }
        // optimistic local reflection: the table updates now; truth lives in the DB.
        let edit_json = serde_json::to_string(edit).unwrap();
        let prov = json!({"origin": "local", "author": author, "note": "write-through (optimistic)"});
        graph.submit(&self.stream, &edit_json, &prov.to_string());
    }
}
