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
const JS_VIEWS: &str = include_str!("js/views.js");
const JS_MILESTONE: &str = include_str!("js/milestone.js");
const JS_TWIN: &str = include_str!("js/twin.js");

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

    /// Load the runtime + the twin app graph (§9, §11) into a fresh V8 branch and
    /// seed demo data — the Phase 0 live-spine graph the server drives.
    pub fn new_twin() -> Self {
        let mut rt = Runtime::new();
        let branch = rt.branch("main", false);
        let combined = format!(
            "{}\n{}\n{}\n{}\n{}",
            JS_ZSET, JS_GRAPH, JS_TABLE, JS_VIEWS, JS_TWIN
        );
        rt.eval(&branch, &combined).expect("load twin graph");
        JsGraph { rt, branch, defs: DefinitionStore::new() }
    }

    /// Total length of the append-only mutation stream (§11.3).
    pub fn twin_total(&mut self) -> usize {
        self.call("T.total()").parse().unwrap_or(0)
    }

    /// The mutation stream slice `[n..)` as a JSON array (the §11.3 forward IR).
    pub fn twin_from(&mut self, n: usize) -> String {
        self.call(&format!("T.from({n})"))
    }

    /// Push a backward UI event (§11.4) as JSON into the graph.
    pub fn twin_event(&mut self, json: &str) {
        self.call(&format!("T.event({json:?})"));
    }

    /// Apply an agent tool-call (§12.1) — `{"tool":..,"args":..}` — as a graph edit.
    pub fn twin_agent_tool(&mut self, json: &str) {
        self.call(&format!("T.agentTool({json:?})"));
    }

    /// The structured projection the agent perceives (§12.3), as JSON.
    pub fn twin_perceive(&mut self) -> String {
        self.call("T.perceive()")
    }

    /// A generic boundary fetch (§9.9): invoke a named host capability by `(adapter, id)`
    /// and land the result back in JS to render.  The CORE knows only adapter *keys* —
    /// each adapter owns its own domain specifics (endpoints, on-disk layout), so the
    /// core carries no notion of "asset", "sensor", or CDF.  Returns a row count.
    pub fn twin_fetch(&mut self, adapter: &str, id: &str, label: &str) -> usize {
        self.fetch_to(adapter, id, label, false)
    }

    /// The same boundary fetch, rendered as a card INLINE in the conversation — the
    /// agent's `show {view:"chart"}` tool.  Same adapters, same lineage; only the
    /// render target differs.
    pub fn twin_show_chart(&mut self, adapter: &str, id: &str, label: &str) -> usize {
        self.fetch_to(adapter, id, label, true)
    }

    fn fetch_to(&mut self, adapter: &str, id: &str, label: &str, inline: bool) -> usize {
        match adapter {
            // materialize-on-demand time-series lens (§9.11) backed by the Cognite adapter.
            "cognite-datapoints" => self.fetch_datapoints(id, label, inline),
            other => {
                let msg = format!("no adapter “{other}”");
                let f = if inline { "T.chartInlineMessage" } else { "T.chartMessage" };
                self.call(&format!("{f}({label:?}, {msg:?})"));
                0
            }
        }
    }

    /// The Cognite datapoints adapter body: if the series isn't materialized locally,
    /// fetch it on demand — anchored to where the data actually is — write-through-cache
    /// it (§7), then hand the points to the JS chart lens (explorer or inline card).
    /// Only this method (and the `cognite` module) knows the CDF layout; the core
    /// dispatch above does not.
    fn fetch_datapoints(&mut self, id: &str, label: &str, inline: bool) -> usize {
        let msg_fn = if inline { "T.chartInlineMessage" } else { "T.chartMessage" };
        let path = format!("data/cognite/datapoints/{id}.csv");
        let mut pts = crate::source::read_series_downsampled(&path, 700);
        let mut provenance = "materialized locally";

        if pts.is_empty() {
            match crate::cognite::fetch_series(id, 180) {
                Ok(raw) if !raw.is_empty() => {
                    let _ = crate::source::write_series(&path, &raw); // sync local
                    provenance = "fetched live on demand (180d)";
                    pts = crate::source::downsample(raw, 700);
                }
                Ok(_) => {
                    self.call(&format!("{msg_fn}({label:?}, {:?})", "no datapoints exist for this series"));
                    return 0;
                }
                Err(e) => {
                    self.call(&format!("{msg_fn}({label:?}, {:?})", format!("couldn't fetch on demand — {e}")));
                    return 0;
                }
            }
        }

        let n = pts.len();
        let json = serde_json::to_string(&pts).unwrap_or_else(|_| "[]".into());
        let render = if inline { "T.chartInline" } else { "T.chartSeries" };
        self.call(&format!("{render}({id:?}, {label:?}, {json}, {provenance:?})"));
        n
    }

    /// Install a skill (§4.1) into the twin — used by the core skills-loader (§11.13).
    pub fn twin_install_skill(&mut self, name: &str, title: &str, description: &str, files: &[String]) {
        let meta = serde_json::json!({ "title": title, "description": description, "files": files }).to_string();
        self.call(&format!("T.installSkill({name:?}, {meta})"));
    }

    /// Mount a local file as a source (§9.9) — federation, not export.  We materialize
    /// only a bounded subset in-heap (§15.1 disposable view); if the file is larger,
    /// it stays `mounted-partial` (the selective-sync point of the residence model).
    /// The file remains the source of truth.  Returns a short status for dev logs.
    pub fn twin_read_source(&mut self, name: &str, path: &str, mode: &str) -> String {
        const MATERIALIZE_CAP: usize = 5000;
        match crate::source::read_file(path) {
            Ok(rows) => {
                let total = rows.len();
                let take = total.min(MATERIALIZE_CAP);
                let residence = if total > take { "mounted-partial" } else { mode };
                let rows_json = serde_json::to_string(&rows[..take]).unwrap_or_else(|_| "[]".into());
                let meta = serde_json::json!({
                    "locator": path, "residence": residence,
                    "rowcount": total, "materialized": take,
                })
                .to_string();
                self.call(&format!("T.mountSource({name:?}, {meta}, {rows_json})"));
                format!("mounted {name}: {total} rows ({residence})")
            }
            Err(e) => {
                self.call(&format!("T.sourceError({name:?}, {e:?})"));
                format!("read_source error: {e}")
            }
        }
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
