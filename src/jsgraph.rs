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
    /// File-backed mounts being WATCHED for growth (§9.9 boundary, made live):
    /// the server polls these between commands; appended rows stream into the
    /// graph as ordinary +deltas, so open views update like any other change.
    watches: Vec<MountWatch>,
}

/// Rows evaluated into V8 per call — bounds the eval string, not the mount.
const MOUNT_CHUNK: usize = 4000;

/// The in-heap materialization bound (§15.1): how many rows of a mounted file
/// live in the graph.  The DOM no longer cares (views are windowed), so this is
/// purely a heap/boot-time policy — override with TWIN_MATERIALIZE_CAP.
fn materialize_cap() -> usize {
    std::env::var("TWIN_MATERIALIZE_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000)
}

/// One watched mount: where the file is, how far into it the graph reaches.
struct MountWatch {
    name: String,
    path: String,
    /// rows already IN the graph (== the file's row count when fully materialized)
    rows_seen: usize,
    /// byte length at last look — the cheap change signal (a stat, not a read)
    len: u64,
    /// mounted-partial: the graph holds a bounded prefix; growth only moves the
    /// counts, it does not stream rows (residence policy stays the user's call)
    partial: bool,
    /// the boundary lens's REFRESH POLICY (§9.9): how often this mount may be
    /// looked at.  0 = live (every beat), u64::MAX = manual (a remount refreshes),
    /// else a floor in ms between polls.
    refresh: u64,
    /// when it was last looked at (boundary wall-clock, ms)
    last_poll: u64,
}

/// Parse a refresh policy: "live" (default) · "30s"/"5m"/"2h" · "manual"/"open".
/// Unknown text reads as live — a bad policy must never silence a source.
fn parse_refresh(s: &str) -> u64 {
    let t = s.trim().to_lowercase();
    if t.is_empty() || t == "live" {
        return 0;
    }
    if ["manual", "open", "off", "once"].contains(&t.as_str()) {
        return u64::MAX;
    }
    for (suffix, unit) in [("ms", 1u64), ("s", 1000), ("m", 60_000), ("h", 3_600_000)] {
        if let Some(n) = t.strip_suffix(suffix).and_then(|v| v.trim().parse::<u64>().ok()) {
            return n.saturating_mul(unit);
        }
    }
    0
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

        JsGraph { rt, branch, defs, watches: Vec::new() }
    }

    /// Load only the dataflow runtime (Z-sets, lenses, scheduler, table) — for
    /// exercising the lens catalogue directly, without the milestone app.
    pub fn new_dataflow() -> Self {
        let mut rt = Runtime::new();
        let branch = rt.branch("main", false);
        let combined = format!("{}\n{}\n{}", JS_ZSET, JS_GRAPH, JS_TABLE);
        rt.eval(&branch, &combined).expect("load JS dataflow runtime");
        JsGraph { rt, branch, defs: DefinitionStore::new(), watches: Vec::new() }
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
        JsGraph { rt, branch, defs: DefinitionStore::new(), watches: Vec::new() }
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

    /// The structured projection the agent perceives (§12.3), as JSON — augmented
    /// at the host seam with one boundary fact (§9.9): what data sits on disk that
    /// is NOT yet mounted, so a fresh pull never waits for a human to name it.
    pub fn twin_perceive(&mut self) -> String {
        let raw = self.call("T.perceive()");
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return raw;
        };
        let mounted: Vec<String> = v["sources"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s["locator"].as_str())
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let offers = crate::source::scan_available("data", &mounted);
        if !offers.is_empty() {
            v["unmounted"] = serde_json::Value::Array(offers);
        }
        v.to_string()
    }

    /// A generic boundary fetch (§9.9): invoke a named host capability by `(adapter, id)`
    /// and land the result back in JS to render.  The CORE knows only adapter *keys* —
    /// each adapter owns its own domain specifics (endpoints, on-disk layout), so the
    /// core carries no notion of "asset", "sensor", or CDF.  Returns a row count.
    /// `panel` picks the column of the detail stack the chart renders into.
    pub fn twin_fetch(&mut self, adapter: &str, id: &str, label: &str, panel: usize) -> usize {
        self.fetch_to(adapter, id, label, Some(panel))
    }

    /// The same boundary fetch, rendered as a card INLINE in the conversation — the
    /// agent's `show {view:"chart"}` tool.  Same adapters, same lineage; only the
    /// render target differs.
    pub fn twin_show_chart(&mut self, adapter: &str, id: &str, label: &str) -> usize {
        self.fetch_to(adapter, id, label, None)
    }

    /// `panel: Some(i)` renders into stack column i; `None` renders an inline card.
    fn fetch_to(&mut self, adapter: &str, id: &str, label: &str, panel: Option<usize>) -> usize {
        match adapter {
            // materialize-on-demand time-series lens (§9.11) backed by the Cognite adapter.
            "cognite-datapoints" => self.fetch_datapoints(id, label, panel),
            other => {
                let msg = format!("no adapter “{other}”");
                match panel {
                    Some(p) => self.call(&format!("T.chartMessage({label:?}, {msg:?}, {p})")),
                    None => self.call(&format!("T.chartInlineMessage({label:?}, {msg:?})")),
                };
                0
            }
        }
    }

    /// The Cognite datapoints adapter body: if the series isn't materialized locally,
    /// fetch it on demand — anchored to where the data actually is — write-through-cache
    /// it (§7), then hand the points to the JS chart lens (explorer or inline card).
    /// Only this method (and the `cognite` module) knows the CDF layout; the core
    /// dispatch above does not.
    fn fetch_datapoints(&mut self, id: &str, label: &str, panel: Option<usize>) -> usize {
        let msg = |m: &str| match panel {
            Some(p) => format!("T.chartMessage({label:?}, {m:?}, {p})"),
            None => format!("T.chartInlineMessage({label:?}, {m:?})"),
        };
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
                    let call = msg("no datapoints exist for this series");
                    self.call(&call);
                    return 0;
                }
                Err(e) => {
                    let call = msg(&format!("couldn't fetch on demand — {e}"));
                    self.call(&call);
                    return 0;
                }
            }
        }

        let n = pts.len();
        let json = serde_json::to_string(&pts).unwrap_or_else(|_| "[]".into());
        let call = match panel {
            Some(p) => format!("T.chartSeries({id:?}, {label:?}, {json}, {provenance:?}, {p})"),
            None => format!("T.chartInline({id:?}, {label:?}, {json}, {provenance:?})"),
        };
        self.call(&call);
        n
    }

    /// Mount rows manufactured by a host effect (document text, a fetch) as a
    /// derived source — same entry as a file mount, different residence.
    pub fn twin_mount_rows(&mut self, name: &str, rows: &[serde_json::Value], locator: &str) {
        let rows_json = serde_json::to_string(rows).unwrap_or_else(|_| "[]".into());
        let meta = serde_json::json!({
            "locator": locator, "residence": "derived",
            "rowcount": rows.len(), "materialized": rows.len(),
        })
        .to_string();
        self.call(&format!("T.mountSource({name:?}, {meta}, {rows_json})"));
    }

    /// One beat of boundary time into the graph (§9.8: the wall clock crosses at
    /// the boundary, never computed in-graph): due interval-refresh views redraw.
    pub fn twin_tick(&mut self, now: u64) {
        self.call(&format!("T.tick({now})"));
    }

    /// Log one step into the agent's activity log (§12.3) from a host-side effect,
    /// so slow boundary work (OCR, search) reports back the same way JS tools do.
    pub fn twin_log_step(&mut self, kind: &str, text: &str, detail: &str, subject: &str, tone: &str) {
        self.call(&format!("T.log({kind:?}, {text:?}, {detail:?}, {subject:?}, {tone:?})"));
    }

    /// Install a skill (§4.1) into the twin — used by the core skills-loader (§11.13).
    pub fn twin_install_skill(&mut self, name: &str, title: &str, description: &str, files: &[String]) {
        let meta = serde_json::json!({ "title": title, "description": description, "files": files }).to_string();
        self.call(&format!("T.installSkill({name:?}, {meta})"));
    }

    /// Mount a local file as a source (§9.9) — federation, not export.  We materialize
    /// only a bounded subset in-heap (§15.1 disposable view); if the file is larger,
    /// it stays `mounted-partial` (the selective-sync point of the residence model).
    /// The file remains the source of truth — and stays WATCHED: growth streams in
    /// (see `twin_poll_mounts`).  Returns a short status for dev logs.
    pub fn twin_read_source(&mut self, name: &str, path: &str, mode: &str) -> String {
        self.twin_read_source_with(name, path, mode, "")
    }

    /// The full form: `refresh` is the mount's boundary refresh policy ("live",
    /// "30s", "manual", …) — how often the watch may look at the file.
    pub fn twin_read_source_with(&mut self, name: &str, path: &str, mode: &str, refresh: &str) -> String {
        let cap = materialize_cap();
        match crate::source::read_file(path) {
            Ok(rows) => {
                let total = rows.len();
                let take = total.min(cap);
                let residence = if total > take { "mounted-partial" } else { mode };
                // mount the first slice, stream the rest in bounded chunks — one
                // giant eval would spike the heap for nothing
                let first = take.min(MOUNT_CHUNK);
                let rows_json = serde_json::to_string(&rows[..first]).unwrap_or_else(|_| "[]".into());
                let meta = serde_json::json!({
                    "locator": path, "residence": residence,
                    // the first chunk's rows — appendRows accumulates the rest
                    "rowcount": total, "materialized": first,
                })
                .to_string();
                self.call(&format!("T.mountSource({name:?}, {meta}, {rows_json})"));
                for chunk in rows[first..take].chunks(MOUNT_CHUNK) {
                    let cj = serde_json::to_string(chunk).unwrap_or_else(|_| "[]".into());
                    self.call(&format!("T.appendRows({name:?}, {{}}, {cj})"));
                }
                if std::path::Path::new(path).is_file() {
                    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                    self.watches.retain(|w| w.name != name);
                    self.watches.push(MountWatch {
                        name: name.to_string(),
                        path: path.to_string(),
                        rows_seen: total,
                        len,
                        partial: total > take,
                        refresh: parse_refresh(refresh),
                        last_poll: 0,
                    });
                }
                format!("mounted {name}: {total} rows ({residence})")
            }
            Err(e) => {
                self.call(&format!("T.sourceError({name:?}, {e:?})"));
                format!("read_source error: {e}")
            }
        }
    }

    /// Look at every watched mount; if a file GREW, stream its appended rows into
    /// the graph as a +delta (the same path any adapter delta takes — open views
    /// patch incrementally).  A stat per file when nothing changed; a re-read only
    /// on growth.  Partial mounts move their counts, not their rows (§15.1).
    /// Returns one status line per source that changed.  `now` is boundary
    /// wall-clock (ms) — each watch is looked at no more often than its own
    /// refresh policy allows.
    pub fn twin_poll_mounts(&mut self, now: u64) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..self.watches.len() {
            let (name, path, rows_seen, len, partial) = {
                let w = &self.watches[i];
                if w.refresh == u64::MAX || now.saturating_sub(w.last_poll) < w.refresh {
                    continue; // this boundary's policy says: not yet
                }
                (w.name.clone(), w.path.clone(), w.rows_seen, w.len, w.partial)
            };
            self.watches[i].last_poll = now;
            let Ok(md) = std::fs::metadata(&path) else { continue };
            if md.len() == len {
                continue;
            }
            if md.len() < len {
                // rewritten/truncated: not an append — leave the mounted view as-is
                // (a remount is an explicit act), just stop re-reading every poll
                self.watches[i].len = md.len();
                continue;
            }
            let Ok(rows) = crate::source::read_file(&path) else {
                self.watches[i].len = md.len();
                continue;
            };
            let total = rows.len();
            if total > rows_seen {
                if partial {
                    self.call(&format!("T.appendRows({name:?}, {{\"rowcount\":{total}}}, [])"));
                    out.push(format!("{name}: {total} rows in the file now (bounded view unchanged)"));
                } else {
                    for chunk in rows[rows_seen..].chunks(MOUNT_CHUNK) {
                        let cj = serde_json::to_string(chunk).unwrap_or_else(|_| "[]".into());
                        self.call(&format!("T.appendRows({name:?}, {{\"rowcount\":{total}}}, {cj})"));
                    }
                    out.push(format!("{name}: +{} row(s) streamed in", total - rows_seen));
                }
                self.watches[i].rows_seen = total;
            }
            self.watches[i].len = md.len();
        }
        out
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

#[cfg(test)]
mod tests {
    use super::parse_refresh;

    #[test]
    fn refresh_policies_parse() {
        assert_eq!(parse_refresh(""), 0);
        assert_eq!(parse_refresh("live"), 0);
        assert_eq!(parse_refresh("300ms"), 300);
        assert_eq!(parse_refresh("30s"), 30_000);
        assert_eq!(parse_refresh("5m"), 300_000);
        assert_eq!(parse_refresh("2h"), 7_200_000);
        assert_eq!(parse_refresh("manual"), u64::MAX);
        assert_eq!(parse_refresh("OPEN"), u64::MAX);
        // a bad policy must never silence a source: unknown reads as live
        assert_eq!(parse_refresh("whenever"), 0);
    }
}
