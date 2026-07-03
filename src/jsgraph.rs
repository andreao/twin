//! The Rust host that drives the V8-resident dataflow graph (design_doc §14, §9).
//!
//! The running graph (Z-sets, lenses, scheduler, table) lives inside V8 as
//! content-addressed JS definitions (§4.1); this Rust host loads them, then drives
//! the graph coarsely — submitting deltas the Rust boundary adapter manufactures,
//! pushing UI edits backward, draining the write-through outbox — crossing the
//! Rust<->V8 boundary only at those points (never per node), so deltas stay V8
//! objects on the hot path.

use crate::definitions::DefinitionStore;
use crate::runtime::{Branch, Runtime};

// The content-addressed JS runtime, loaded as one script so the class/function
// declarations share scope; `milestone.js` publishes `globalThis.M`.
const JS_ZSET: &str = include_str!("js/zset.js");
const JS_GRAPH: &str = include_str!("js/graph.js");
const JS_TABLE: &str = include_str!("js/table.js");
const JS_MILESTONE: &str = include_str!("js/milestone.js");

pub struct JsGraph {
    rt: Runtime,
    branch: Branch,
    #[allow(dead_code)]
    defs: DefinitionStore,
}

impl JsGraph {
    /// Load the runtime + the §18 milestone graph into a fresh V8 branch.
    pub fn new_milestone() -> Self {
        let mut rt = Runtime::new();
        let branch = rt.branch("main", false);
        let mut defs = DefinitionStore::new();

        // register each unit as a content-addressed definition (§4.1) ...
        for (name, src) in [
            ("rt/zset", JS_ZSET),
            ("rt/graph", JS_GRAPH),
            ("rt/table", JS_TABLE),
            ("app/milestone", JS_MILESTONE),
        ] {
            defs.publish(name, "runtime-js", src, &[]);
        }
        // ... and load them as one script so top-level classes share scope.
        let combined = format!("{}\n{}\n{}\n{}", JS_ZSET, JS_GRAPH, JS_TABLE, JS_MILESTONE);
        rt.eval(&branch, &combined).expect("load JS runtime");

        JsGraph { rt, branch, defs }
    }

    /// Load only the dataflow runtime (Z-sets, lenses, scheduler, table) — for
    /// exercising the lens catalogue directly, without the milestone app.
    pub fn new_dataflow() -> Self {
        let mut rt = Runtime::new();
        let branch = rt.branch("main", false);
        let combined = format!("{}\n{}\n{}", JS_ZSET, JS_GRAPH, JS_TABLE);
        rt.eval(&branch, &combined).expect("load JS dataflow runtime");
        JsGraph { rt, branch, defs: DefinitionStore::new() }
    }

    /// Evaluate an expression against the graph branch (returns the JS result).
    pub fn call(&mut self, expr: &str) -> String {
        self.rt.eval(&self.branch, expr).unwrap_or_else(|e| panic!("JS error: {e}\n  in: {expr}"))
    }

    /// Submit a delta (JSON `[[rowObj, weight], ...]`) manufactured by the adapter.
    pub fn submit(&mut self, stream: &str, delta_json: &str, prov_json: &str) -> String {
        self.call(&format!("M.submit({:?}, {}, {})", stream, delta_json, prov_json))
    }

    /// Push a UI cell edit backward through the join; returns the drained outbox.
    pub fn cell_edit(&mut self, id: &str, col: &str, value_json: &str, prov_json: &str) -> String {
        self.call(&format!("M.cellEdit({:?}, {:?}, {}, {})", id, col, value_json, prov_json))
    }

    pub fn drain_outbox(&mut self) -> String {
        self.call("M.drainOutbox()")
    }
    pub fn render(&mut self) -> Vec<String> {
        serde_json::from_str(&self.call("M.render()")).unwrap_or_default()
    }
    pub fn mutation_count(&mut self) -> u64 {
        self.call("M.mutationCount()").parse().unwrap_or(0)
    }
    pub fn blame(&mut self, stream: &str) -> String {
        self.call(&format!("M.blame({:?})", stream))
    }
    pub fn visible_rows(&mut self) -> serde_json::Value {
        serde_json::from_str(&self.call("M.visibleRows()")).unwrap_or(serde_json::Value::Null)
    }
}
