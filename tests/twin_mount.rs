//! The mount pipeline, end to end, model-free: reading a file as a source (§9.9)
//! makes it appear in the agent's perception (§12.3) and renders it as a source
//! (§11.3), while the file stays put (federation — the residence model).

use std::io::Write;
use twin_runtime::JsGraph;

fn write_temp_csv() -> String {
    // unique per call: tests run in parallel, and a shared path can be observed
    // mid-truncation by another test (seen as a phantom 0-row source)
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "twin_test_turbines_{}_{}.csv",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(
        b"turbine_id,site,gearbox_temp,vibration\n\
          WT-01,North,62.4,2.1\n\
          WT-02,North,71.9,4.8\n\
          WT-03,South,83.2,7.3\n",
    )
    .unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn mounting_a_file_appears_in_perception_and_renders() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();

    let status = g.twin_read_source("turbines", &path, "mounted");
    assert!(status.contains("3 rows"), "status: {status}");

    // (1) the agent now perceives the source with its schema + sample (§12.3)
    let seen = g.twin_perceive();
    assert!(seen.contains("turbines"), "perceive missing source: {seen}");
    assert!(seen.contains("gearbox_temp"), "perceive missing schema: {seen}");
    assert!(seen.contains("WT-01"), "perceive missing sample rows: {seen}");
    assert!(seen.contains("\"rowcount\":3"), "perceive missing rowcount: {seen}");

    // (2) it renders: a board tile + a "Mounted …" ACTION CARD in the chat (the
    // history reads as work that was done, openable like any card)
    let muts = g.twin_from(0);
    assert!(muts.contains("sr:turbines"), "no source tile rendered: {muts}");
    assert!(muts.contains("Mounted turbines"), "no mount card in the chat: {muts}");
    assert!(muts.contains("card-title openable"), "mount card is not openable");

    // (3) the file is untouched — federation, not export
    assert!(std::fs::metadata(&path).is_ok(), "source file must remain");
}

#[test]
fn opening_a_source_renders_a_browsable_table() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    // the user clicks the source in the UI → backward event
    g.twin_event(r#"{"type":"open_source","name":"turbines"}"#);
    let muts = g.twin_from(0);
    assert!(muts.contains("panel:0:body"), "table not rendered into stack panel 0");
    assert!(muts.contains("exp:tbl"));
    assert!(muts.contains("gearbox_temp"), "column header missing");
    assert!(muts.contains("WT-01"), "row data missing");
}

#[test]
fn charting_a_sensor_renders_an_svg_line() {
    let dir = std::path::Path::new("data/cognite/datapoints");
    if !dir.exists() {
        eprintln!("skip: no datapoints pulled");
        return;
    }
    // pick the largest series (most data) so we don't land on an empty sensor
    let mut best: Option<(u64, String)> = None;
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().map(|x| x == "csv").unwrap_or(false) {
            if let (Ok(md), Some(stem)) = (e.metadata(), p.file_stem().and_then(|s| s.to_str())) {
                if best.as_ref().map(|(b, _)| md.len() > *b).unwrap_or(true) {
                    best = Some((md.len(), stem.to_string()));
                }
            }
        }
    }
    let id = match best {
        Some((_, i)) => i,
        None => return,
    };
    let mut g = JsGraph::new_twin();
    let n = g.twin_fetch("cognite-datapoints", &id, "sensor", 0);
    assert!(n > 0, "no points read for {id}");
    let muts = g.twin_from(0);
    assert!(muts.contains("\"tag\":\"svg\""), "no svg element rendered");
    assert!(muts.contains("chart-line") && muts.contains("exp:path"), "no chart line path");
}

#[test]
fn documents_source_renders_a_gallery_and_embed() {
    let mut g = JsGraph::new_twin();
    // documents are just a mounted source now (§7 uniform residence) — the core has no
    // special "documents" path; the app-lens renders the gallery.
    let d = std::env::temp_dir().join("twin_docs.json");
    std::fs::write(&d, r#"[{"name":"PID-1.pdf","bytes":1024},{"name":"draw.svg","bytes":2048}]"#).unwrap();
    g.twin_read_source("documents", d.to_str().unwrap(), "mounted");
    g.twin_event(r#"{"type":"open_source","name":"documents"}"#);
    let gallery = g.twin_from(0);
    assert!(gallery.contains("doc-gallery") && gallery.contains("doc:PID-1.pdf"), "no gallery");
    g.twin_event(r#"{"type":"open_document","name":"PID-1.pdf"}"#);
    let embed = g.twin_from(0);
    assert!(embed.contains("/file/PID-1.pdf"), "no document embed src");
}

#[test]
fn agent_show_renders_a_table_inline_in_the_conversation() {
    let mut g = JsGraph::new_twin();
    let a = std::env::temp_dir().join("show_assets.csv");
    std::fs::write(&a, "id,name,description\n1,Compressor,1st stage\n2,Pump,water inj\n3,Valve,relief\n").unwrap();
    g.twin_read_source("assets", a.to_str().unwrap(), "mounted");
    // the AGENT shows data as a real component — not markdown text
    g.twin_agent_tool(r#"{"tool":"show","args":{"view":"table","source":"assets","limit":2}}"#);
    let d = g.twin_from(0);
    assert!(d.contains("feed-item view") || d.contains("view-body"), "no inline view card rendered");
    assert!(d.contains("Compressor") && d.contains("Pump"), "table rows missing");
    assert!(!d.contains("Valve"), "limit not applied (should show only 2 of 3)");
}

#[test]
fn missing_file_reports_a_readable_error() {
    let mut g = JsGraph::new_twin();
    let status = g.twin_read_source("nope", "/no/such/file.csv", "mounted");
    assert!(status.starts_with("read_source error"), "status: {status}");
    let muts = g.twin_from(0);
    assert!(muts.contains("Couldn't read"), "no error surfaced to user: {muts}");
}

#[test]
fn parametrized_datapoints_lens_fetches_on_demand() {
    if !std::path::Path::new("/tmp/cognite_token.json").exists() {
        eprintln!("skip: no Cognite token");
        return;
    }
    let mut g = JsGraph::new_twin();
    // 1009048440794092: empty in our 30-day window, but has data back in 2023
    let _ = std::fs::remove_file("data/cognite/datapoints/1009048440794092.csv");
    let n = g.twin_fetch("cognite-datapoints", "1009048440794092", "VAL-test", 0);
    eprintln!("on-demand chart: {n} points");
    let muts = g.twin_from(0);
    if n > 0 {
        assert!(muts.contains("\"tag\":\"svg\""), "should render a chart");
        assert!(muts.contains("fetched live on demand"), "should note the on-demand fetch");
        assert!(std::path::Path::new("data/cognite/datapoints/1009048440794092.csv").exists(), "should cache locally");
    }
}

#[test]
fn asset_tree_and_events_timeline_render() {
    let mut g = JsGraph::new_twin();
    // assets → hierarchy tree
    let apath = std::env::temp_dir().join("twin_assets.csv");
    std::fs::write(&apath, "id,parentId,name,description\n1,,Platform,root\n2,1,Compressor,stage1\n3,2,Seal,gas\n").unwrap();
    g.twin_read_source("assets", apath.to_str().unwrap(), "mounted");
    g.twin_event(r#"{"type":"open_source","name":"assets","mode":"tree"}"#);
    let t = g.twin_from(0);
    assert!(t.contains("exp:tree") && t.contains("tn:2") && t.contains("Compressor"), "no asset tree");
    assert!(t.contains("mode:assets:tree"), "no mode switcher");

    // events → timeline bar chart
    let epath = std::env::temp_dir().join("twin_events.csv");
    std::fs::write(&epath, "id,type,startTime\n1,x,1674000000000\n2,x,1676700000000\n3,y,1674000500000\n").unwrap();
    g.twin_read_source("events", epath.to_str().unwrap(), "mounted");
    g.twin_event(r#"{"type":"open_source","name":"events","mode":"timeline"}"#);
    let tl = g.twin_from(0);
    assert!(tl.contains("chart-bar") && tl.contains("bar:0"), "no timeline bars");
}

#[test]
fn asset_dashboard_composes_sensors_and_events() {
    let mut g = JsGraph::new_twin();
    let a = std::env::temp_dir().join("ad_assets.csv");
    std::fs::write(&a, "id,parentId,name,description\n10,,Compressor,1st stage\n").unwrap();
    let t = std::env::temp_dir().join("ad_ts.csv");
    std::fs::write(&t, "id,externalId,name,unit,assetId,description\n99,VAL-TT-1,temp,degC,10,bearing\n").unwrap();
    let e = std::env::temp_dir().join("ad_ev.csv");
    std::fs::write(&e, "id,type,subtype,startTime,description,assetIds\n5,WO,repair,1674000000000,seal swap,10;77\n").unwrap();
    g.twin_read_source("assets", a.to_str().unwrap(), "mounted");
    g.twin_read_source("timeseries", t.to_str().unwrap(), "mounted");
    g.twin_read_source("events", e.to_str().unwrap(), "mounted");
    // a P&ID that references asset 10 and an unrelated one that does not
    let dj = std::env::temp_dir().join("ad_docs.json");
    std::fs::write(&dj, r#"[{"name":"PID-10.pdf","bytes":10,"assetIds":[10]},{"name":"other.pdf","bytes":20,"assetIds":[77]}]"#).unwrap();
    g.twin_read_source("documents", dj.to_str().unwrap(), "mounted");
    g.twin_event(r#"{"type":"open_asset","id":"10"}"#);
    let d = g.twin_from(0);
    assert!(d.contains("Compressor"), "no asset header");
    assert!(d.contains("sens:99") && d.contains("VAL-TT-1"), "sensor not linked by assetId");
    assert!(d.contains("seal swap"), "event not linked by assetIds");
    assert!(d.contains("doc:PID-10.pdf"), "P&ID referencing this asset missing from dashboard");
    assert!(!d.contains("doc:other.pdf"), "unrelated drawing should not appear on this asset");
}

#[test]
fn search_is_a_parametrized_lens() {
    let mut g = JsGraph::new_twin();
    let a = std::env::temp_dir().join("se_assets.csv");
    std::fs::write(&a, "id,parentId,name,description\n10,,Compressor,first stage seal\n11,,Pump,water\n").unwrap();
    g.twin_read_source("assets", a.to_str().unwrap(), "mounted");
    // parametrized by the query "seal"
    g.twin_event(r#"{"type":"search","query":"seal"}"#);
    let r = g.twin_from(0);
    assert!(r.contains("1 match for") || r.contains("matches for"), "no result header: {}", &r[..r.len().min(120)]);
    assert!(r.contains("tn:10") && r.contains("Compressor"), "seal-matching asset missing");
    assert!(!r.contains("tn:11"), "non-matching asset should be excluded");
}

#[test]
fn hierarchy_rolls_up_links_and_agent_can_inspect() {
    let mut g = JsGraph::new_twin();
    let a = std::env::temp_dir().join("h_assets.csv");
    std::fs::write(&a, "id,parentId,name\n1,,Platform\n2,1,Compressor\n").unwrap();
    let t = std::env::temp_dir().join("h_ts.csv");
    std::fs::write(&t, "id,name,unit,assetId\n9,temp,degC,2\n").unwrap();
    g.twin_read_source("assets", a.to_str().unwrap(), "mounted");
    g.twin_read_source("timeseries", t.to_str().unwrap(), "mounted");
    // tree rolls the sensor up to the Platform subtree
    g.twin_event(r#"{"type":"open_source","name":"assets","mode":"tree"}"#);
    let tree = g.twin_from(0);
    assert!(tree.contains("tn:1:dot") && tree.contains("tn:1:b"), "no rollup badge on parent");
    assert!(tree.contains("1 sensors"), "sensor not rolled up: check badge text");
    // agent inspect tool computes stats — the result lands in its activity log
    g.twin_agent_tool(r#"{"tool":"inspect","args":{"source":"timeseries"}}"#);
    let log = g.twin_from(0);
    assert!(log.contains("checked") && log.contains("timeseries"), "inspect did not run");
    assert!(log.contains("agent-act"), "inspect result not on the now-strip");
}

#[test]
fn agent_shows_a_chart_inline_in_the_conversation() {
    let dir = std::path::Path::new("data/cognite/datapoints");
    if !dir.exists() {
        eprintln!("skip: no datapoints pulled");
        return;
    }
    let id = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().map(|x| x == "csv").unwrap_or(false) && e.metadata().map(|m| m.len() > 200).unwrap_or(false))
                .then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
        })
        .next();
    let id = match id {
        Some(i) => i,
        None => return,
    };
    let mut g = JsGraph::new_twin();
    let n = g.twin_show_chart("cognite-datapoints", &id, "bearing temp");
    assert!(n > 0, "no points for {id}");
    let muts = g.twin_from(0);
    // the chart is a view CARD in the feed, not the explorer pane
    assert!(muts.contains("feed-item view"), "no view card in the conversation: {}", &muts[..muts.len().min(300)]);
    assert!(muts.contains(":svg") && muts.contains("chart-line"), "no inline svg chart");
    assert!(!muts.contains("panel:0"), "inline chart leaked into the detail stack");
}

#[test]
fn user_input_is_captured_raw_and_derived_with_lineage() {
    let mut g = JsGraph::new_twin();
    // pure event sourcing (§8): the user's actions land verbatim in the `input`
    // stream (ts stamped at the boundary); feed items etc. are DERIVED from them.
    g.twin_event(r#"{"type":"search","query":"pump","ts":1700000000000}"#);
    g.twin_event(r#"{"type":"open_source","name":"assets","ts":1700000000001}"#);
    g.twin_event(r#"{"type":"user_message","text":"hello twin","ts":1700000000002}"#);
    // the derived feed item renders…
    let muts = g.twin_from(0);
    assert!(muts.contains("hello twin"), "derived feed item missing: {muts}");
    // …and the agent perceives the user's raw behavior, to derive goals from
    let seen = g.twin_perceive();
    assert!(seen.contains("userActions"), "no raw-action stream in perception");
    assert!(seen.contains("pump"), "search action not perceived: {seen}");
    assert!(seen.contains("sent a message"), "message action not perceived: {seen}");
}

#[test]
fn agent_keeps_an_agenda_and_activity_log() {
    let mut g = JsGraph::new_twin();
    g.twin_agent_tool(r#"{"tool":"plan","args":{"items":["Profile the turbines source","Look for data gaps"]}}"#);
    g.twin_agent_tool(r#"{"tool":"work","args":{"task":"profile","text":"profiling turbines: checking ranges"}}"#);
    // the agenda + activity fold to the one-line now-strip above the board
    let muts = g.twin_from(0);
    assert!(muts.contains("agent-plan") && muts.contains("Profile the turbines source"), "plan line missing: {muts}");
    assert!(muts.contains("agent-act") && muts.contains("profiling turbines"), "activity line missing");
    // the ACTIVE item feeds the in-progress card's title
    assert!(muts.contains(r#""key":"agent-doing","text":"Profile the turbines source""#), "active item not exported: {muts}");
    // status changes are new EVENTS; when the active item is done, the fold moves on
    g.twin_agent_tool(r#"{"tool":"done","args":{"task":"profile"}}"#);
    assert!(g.twin_from(0).contains("next: Look for data gaps"), "plan line did not advance");
    let seen = g.twin_perceive();
    assert!(seen.contains("Look for data gaps"), "agenda not perceived: {seen}");
    assert!(seen.contains("\"agendaDone\":1"), "done count not perceived: {seen}");
}

#[test]
fn agent_records_findings_on_the_board() {
    let mut g = JsGraph::new_twin();
    g.twin_agent_tool(r#"{"tool":"finding","args":{"severity":"warn","text":"3 turbines have no vibration sensor","source":"turbines"}}"#);
    let muts = g.twin_from(0);
    assert!(muts.contains("tile:findings"), "findings tile did not appear on the board");
    assert!(muts.contains("Findings · 1"), "tile title has no count");
    assert!(muts.contains("fnd:1") && muts.contains("sev-warn"), "finding card missing: {muts}");
    assert!(muts.contains("no vibration sensor"), "finding text missing");
    // the chat is the user's space: filing a finding posts NOTHING there
    assert!(!muts.contains("feed-item card"), "finding leaked into the chat: {muts}");
    // filing the same finding twice is a no-op
    g.twin_agent_tool(r#"{"tool":"finding","args":{"severity":"warn","text":"3 turbines have no vibration sensor"}}"#);
    assert!(!g.twin_from(0).contains("fnd:2"), "duplicate finding filed twice");
    assert!(g.twin_perceive().contains("no vibration sensor"), "finding not perceived");
}

#[test]
fn inspect_handles_a_documents_source() {
    let mut g = JsGraph::new_twin();
    let d = std::env::temp_dir().join("insp_docs.json");
    std::fs::write(&d, r#"[{"name":"PID-1.pdf","bytes":1},{"name":"train.mp4","bytes":2}]"#).unwrap();
    g.twin_read_source("documents", d.to_str().unwrap(), "mounted");
    g.twin_agent_tool(r#"{"tool":"inspect","args":{"source":"documents"}}"#);
    let log = g.twin_from(0);
    assert!(log.contains("2 files") && log.contains("1 pdf"), "documents inspect failed: {log}");
}

#[test]
fn agent_authors_a_lens_with_lineage() {
    let path = write_temp_csv(); // WT-01 62.4 / WT-02 71.9 / WT-03 83.2 gearbox_temp
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_agent_tool(
        r#"{"tool":"make_lens","args":{"name":"Hot gearboxes","description":"Turbines whose gearbox runs above 70 degrees.","source":"turbines","code":"return rows.filter(r => Number(r.gearbox_temp) > 70)"}}"#,
    );
    let muts = g.twin_from(0);
    // the board tile shows a HUMAN title + description — never code, never "lens:" slugs
    assert!(muts.contains("ln:hot-gearboxes"), "no lens tile on the board: {muts}");
    assert!(muts.contains("Hot gearboxes"), "human title missing from the tile");
    assert!(muts.contains("above 70 degrees"), "description missing from the tile");
    assert!(muts.contains("a lens over turbines"), "lineage-in-words missing from the tile");
    assert!(!muts.contains("return rows.filter"), "code leaked onto the board tile");
    // authoring is QUIET: nothing lands in the chat unless the agent shows it
    assert!(!muts.contains("feed-item view"), "lens auto-posted a card into the chat");
    // deep inspection: expanding the lens shows the rows, the chain AND the code
    g.twin_event(r#"{"type":"open_source","name":"lens:hot-gearboxes"}"#);
    let deep = g.twin_from(0);
    assert!(deep.contains("exp:tbl"), "lens not browsable as a table");
    assert!(deep.contains("WT-02") && deep.contains("WT-03"), "derived rows missing from the deep view");
    assert!(!deep.contains("WT-01"), "lens filter not applied");
    assert!(deep.contains("chain-part"), "no derivation breadcrumb in the expanded view");
    assert!(deep.contains("code-toggle"), "no code toggle in the expanded view");
    assert!(deep.contains("return rows.filter"), "code not present behind the toggle");
    assert!(deep.contains(r#""name":"hidden""#), "code block should start hidden");
    let seen = g.twin_perceive();
    assert!(seen.contains("lens:hot-gearboxes"), "lens not perceived: {seen}");
}

#[test]
fn empty_lenses_are_rejected_not_rendered() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_agent_tool(
        r#"{"tool":"make_lens","args":{"name":"Nothing here","description":"Filters to a temp no turbine reaches.","source":"turbines","code":"return rows.filter(r => Number(r.gearbox_temp) > 9000)"}}"#,
    );
    let muts = g.twin_from(0);
    assert!(!muts.contains("ln:nothing-here"), "0-row lens landed on the board");
    assert!(!muts.contains("0 rows derived"), "0-row card landed in the chat");
    assert!(muts.contains("EMPTY (0 rows)"), "agent not told the lens was empty");
    assert!(!g.twin_perceive().contains("lens:nothing-here"), "empty lens perceived as real");
}

#[test]
fn lenses_compose_and_the_chain_is_shown() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_agent_tool(
        r#"{"tool":"make_lens","args":{"name":"Hot gearboxes","description":"Gearboxes above 70.","source":"turbines","code":"return rows.filter(r => Number(r.gearbox_temp) > 70)"}}"#,
    );
    // a lens over a lens — composition
    g.twin_agent_tool(
        r#"{"tool":"make_lens","args":{"name":"Critical gearboxes","description":"The hot ones above 80.","source":"lens:hot-gearboxes","code":"return rows.filter(r => Number(r.gearbox_temp) > 80)"}}"#,
    );
    g.twin_event(r#"{"type":"open_source","name":"lens:critical-gearboxes"}"#);
    let deep = g.twin_from(0);
    // the chain reads root → hop → this, in human titles
    // the breadcrumb walks root → hop → this, in human titles, this-hop emphasized
    assert!(deep.contains("turbines"), "chain root missing");
    assert!(deep.contains("Hot gearboxes"), "chain middle hop missing");
    assert!(deep.contains("chain-part here") && deep.contains("Critical gearboxes"), "chain endpoint missing: {deep}");
    assert!(deep.contains("WT-03") , "composed lens rows wrong");
}

#[test]
fn the_history_tells_the_origin_story() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    // chapter 1: a capability exists
    g.twin_install_skill("obtain-oid", "Obtain open industrial data", "Pulls real oil-&-gas data from Cognite's public Valhall dataset.", &[]);
    // chapter 2: data arrives through it, with provenance on the mount card
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_agent_tool(r#"{"tool":"describe","args":{"source":"turbines","title":"Turbine fleet","description":"All turbines.","origin":"Cognite CDF (Valhall) · via skill obtain-oid"}}"#);
    let muts = g.twin_from(0);
    assert!(muts.contains("Installed skill: Obtain open industrial data"), "no skill card: {muts}");
    assert!(muts.contains("from Cognite CDF (Valhall)"), "mount card carries no provenance");
}

#[test]
fn describe_gives_sources_human_titles() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_agent_tool(r#"{"tool":"describe","args":{"source":"turbines","title":"Turbine fleet","description":"All wind turbines with gearbox and vibration readings."}}"#);
    let muts = g.twin_from(0);
    assert!(muts.contains("Turbine fleet"), "title not applied to the tile");
    assert!(muts.contains("All wind turbines"), "description not applied to the tile");
    assert!(g.twin_perceive().contains("Turbine fleet"), "title not perceived");
}

#[test]
fn a_finding_opens_with_evidence_link_and_actions() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_agent_tool(r#"{"tool":"finding","args":{"severity":"warn","text":"2 of 3 turbines run gearboxes above 70°C","source":"turbines"}}"#);
    // opening the finding shows the ISSUE (not the raw table) + calls to action
    g.twin_event(r#"{"type":"open_finding","id":1,"panel":0}"#);
    let d = g.twin_from(0);
    assert!(d.contains("fd-text") && d.contains("above 70"), "finding text missing: {d}");
    assert!(d.contains("in turbines"), "no evidence link to the source");
    assert!(d.contains("fd:investigate") && d.contains("fd:fix") && d.contains("fd:resolve"), "calls to action missing");
    // resolving is an event: the board card dims and the count reflects it
    g.twin_event(r#"{"type":"resolve_finding","id":1}"#);
    let r = g.twin_from(0);
    assert!(r.contains("fnd sev-warn resolved"), "board card not marked resolved");
    assert!(r.contains("(1 resolved)"), "tile count not updated: {r}");
}

#[test]
fn detail_stack_panels_are_independent_columns() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    // column 0: the table; column 1: something else — both alive at once
    g.twin_event(r#"{"type":"open_source","name":"turbines","panel":0}"#);
    g.twin_event(r#"{"type":"search","query":"WT-01","panel":1}"#);
    let muts = g.twin_from(0);
    assert!(muts.contains("p0:exp:tbl"), "no table in column 0: {muts}");
    assert!(muts.contains("p1:exp:note"), "no search in column 1");
    // each column is stamped with what it shows, so a replaying client can rebuild
    // the panel chrome from the log — open columns survive reloads
    assert!(muts.contains(r#""key":"panel:0:body","name":"data-title""#), "no restore marker on column 0: {muts}");
    // re-opening at column 0 invalidates column 1 (everything to the right)
    let before = g.twin_total();
    g.twin_event(r#"{"type":"open_source","name":"turbines","panel":0}"#);
    let tail = g.twin_from(before);
    assert!(tail.contains(r#""op":"remove","key":"p1:"#), "column 1 not cleared on re-open at 0: {tail}");
}

#[test]
fn agent_page_shows_agenda_and_activity_tidily() {
    let mut g = JsGraph::new_twin();
    g.twin_agent_tool(r#"{"tool":"plan","args":{"items":["Profile the turbines","Chart the hottest sensor"]}}"#);
    g.twin_agent_tool(r#"{"tool":"work","args":{"task":"profile","text":"profiling turbines: ranges look sane"}}"#);
    g.twin_event(r#"{"type":"open_agent","panel":0}"#);
    let d = g.twin_from(0);
    assert!(d.contains("Plan"), "no plan section: {d}");
    assert!(d.contains("task:1") && d.contains("task:2"), "task rows are not clickable things");
    assert!(d.contains(r#""text":"now""#), "active task not labelled now");
    assert!(d.contains("Recent steps") && d.contains("ranges look sane"), "activity log missing");

    // a task is a THING: it opens to its own page with status, its story, actions
    g.twin_event(r#"{"type":"open_task","id":1,"panel":1}"#);
    let t = g.twin_from(0);
    assert!(t.contains("task-active") && t.contains("in progress"), "no status chip: {t}");
    assert!(t.contains(r#""value":"story""#) && t.contains("ranges look sane"), "task-scoped story missing");
    // a RUNNING task never offers "start it" — only its way to done; and the page
    // says what the agent is doing right now
    assert!(!t.contains("ta:work"), "running task still offers a start button: {t}");
    assert!(t.contains("ta:done") && t.contains("Mark done"), "no done action: {t}");
    assert!(t.contains("ta:now") && t.contains("ranges look sane"), "no live now-line: {t}");
    // marking done is an event; the fold updates everywhere
    g.twin_event(r#"{"type":"set_task","id":1,"status":"done"}"#);
    assert!(g.twin_perceive().contains("\"agendaDone\":1"), "done not folded");
}

#[test]
fn closing_a_column_is_an_event_and_replays() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_event(r#"{"type":"open_source","name":"turbines","panel":0}"#);
    g.twin_event(r#"{"type":"search","query":"WT","panel":1}"#);
    let before = g.twin_total();
    g.twin_event(r#"{"type":"close_panel","panel":1}"#);
    let tail = g.twin_from(before);
    assert!(tail.contains(r#""key":"p1:"#), "column 1 content not cleared: {tail}");
    assert!(tail.contains(r#""name":"data-closed","value":"true""#), "no close marker for replay");
}

#[test]
fn recents_and_stars_are_lenses_over_raw_input() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    // visiting pages derives recents chips…
    g.twin_event(r#"{"type":"open_source","name":"turbines","panel":0,"title":"Turbine fleet"}"#);
    let muts = g.twin_from(0);
    assert!(muts.contains("rc:0") && muts.contains("Turbine fleet"), "no recents chip: {muts}");
    // …and starring derives a starred chip; starring again removes it (a toggle fold)
    g.twin_event(r#"{"type":"star","title":"Turbine fleet","target":{"type":"open_source","name":"turbines"}}"#);
    let starred = g.twin_from(0);
    assert!(starred.contains("st:0") && starred.contains("★ Turbine fleet"), "no starred chip");
    let before = g.twin_total();
    g.twin_event(r#"{"type":"star","title":"Turbine fleet","target":{"type":"open_source","name":"turbines"}}"#);
    assert!(g.twin_from(before).contains(r#""op":"remove","key":"st:0"#), "unstar did not remove the chip");
}

#[test]
fn watch_derives_blind_spots_and_hotspots() {
    let mut g = JsGraph::new_twin();
    let a = std::env::temp_dir().join("w_assets.csv");
    std::fs::write(&a, "id,parentId,name\n1,,Compressor\n2,,Pump\n").unwrap();
    let t = std::env::temp_dir().join("w_ts.csv");
    std::fs::write(&t, "id,name,assetId\n9,temp,1\n").unwrap(); // asset 1 monitored, asset 2 not
    let e = std::env::temp_dir().join("w_ev.csv");
    // asset 2 has events but no sensors → a blind spot
    std::fs::write(&e, "id,type,startTime,assetIds\n1,WO,1,2\n2,WO,2,2\n3,WO,3,1\n").unwrap();
    g.twin_read_source("assets", a.to_str().unwrap(), "mounted");
    g.twin_read_source("timeseries", t.to_str().unwrap(), "mounted");
    g.twin_read_source("events", e.to_str().unwrap(), "mounted");
    g.twin_event(r#"{"type":"watch"}"#);
    let w = g.twin_from(0);
    assert!(w.contains("Blind spots"), "no blind-spots section");
    assert!(w.contains("iss:2:1:0") && w.contains("Pump"), "Pump (events, no sensors) not flagged as blind spot");
    assert!(w.contains("Maintenance hotspots"), "no hotspots section");
}

#[test]
fn task_page_folds_retries_into_a_story() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_agent_tool(r#"{"tool":"plan","args":{"items":["Find hot equipment"]}}"#);
    g.twin_agent_tool(r#"{"tool":"work","args":{"task":"hot","text":"scanning the registry for hot gearboxes"}}"#);
    // two failed attempts (bad code, then a bad source), then the one that works
    g.twin_agent_tool(r#"{"tool":"make_lens","args":{"name":"Hot equipment","source":"turbines","code":"return rows.filter(r => r.nope.toLowerCase())"}}"#);
    g.twin_agent_tool(r#"{"tool":"make_lens","args":{"name":"Hot equipment","source":"turbines","code":"return rows.filter(r =>"}}"#);
    g.twin_agent_tool(r#"{"tool":"make_lens","args":{"name":"Hot equipment","description":"Gearboxes above 70.","source":"turbines","code":"return rows.filter(r => Number(r.gearbox_temp) > 70)"}}"#);

    g.twin_event(r#"{"type":"open_task","id":1,"panel":0}"#);
    let t = g.twin_from(0);
    // the retries are folded INTO the success — one created card, no failure lines left
    assert!(t.contains("Created Hot equipment"), "no created card in the telling: {t}");
    assert!(!t.contains("Couldn't build Hot equipment"), "resolved failures still shown in the telling: {t}");
    // the card carries the retry story and opens the lens itself
    assert!(t.contains("worked after 2 failed attempts"), "card lost the retry story: {t}");
    assert!(t.contains("open_source") && t.contains("lens:hot-equipment"), "result is not openable: {t}");
    // the work note reads as the agent narrating, with the chat's own component
    assert!(t.contains(r#""value":"feed-item thought""#) && t.contains("scanning the registry"), "work note missing: {t}");

    // the created step's page tells the whole story: facts, retries, and the way in
    g.twin_event(r#"{"type":"open_step","seq":4,"n":2,"panel":1}"#);
    let sp = g.twin_from(0);
    assert!(sp.contains("succeeded after 2 failed attempts"), "step page lost the retry story: {sp}");
    assert!(sp.contains("2 rows from turbines"), "step page has no facts: {sp}");
    assert!(sp.contains("part of: Find hot equipment"), "step page not linked to its task: {sp}");
    assert!(sp.contains("Open the view"), "step page cannot open the produced lens: {sp}");
}

#[test]
fn unresolved_failures_collapse_with_an_attempt_count() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_agent_tool(r#"{"tool":"plan","args":{"items":["Chase a broken idea"]}}"#);
    g.twin_agent_tool(r#"{"tool":"work","args":{"task":"broken","text":"trying a derivation"}}"#);
    g.twin_agent_tool(r#"{"tool":"make_lens","args":{"name":"Doomed","source":"turbines","code":"return rows.map(r => r.x.y)"}}"#);
    g.twin_agent_tool(r#"{"tool":"make_lens","args":{"name":"Doomed","source":"turbines","code":"return rows.map(r => r.x.z)"}}"#);
    g.twin_event(r#"{"type":"open_task","id":1,"panel":0}"#);
    let t = g.twin_from(0);
    // failures are NOT part of the telling — they fold into the machine log
    assert!(!t.contains("Couldn't build"), "failure leaked into the telling: {t}");
    assert!(t.contains("ta:allbtn") && t.contains("every step"), "no way into the full step log: {t}");
    assert!(t.contains("(2 attempts)") && t.contains("t-error"), "failures not collapsed with a count in the log: {t}");
}

#[test]
fn inspect_reads_like_a_person_wrote_it() {
    let mut g = JsGraph::new_twin();
    let e = std::env::temp_dir().join(format!("insp_h_{}.csv", std::process::id()));
    // id-ish columns, epoch-ms timestamps, an always-empty column, a constant
    std::fs::write(&e, "id,assetIds,startTime,count,description\n\
        1,87732307364972,1552521600000,1,\n\
        2,87732307364973,1698883200000,1,\n\
        3,87732307364974,1600000000000,1,\n").unwrap();
    g.twin_read_source("events", e.to_str().unwrap(), "mounted");
    g.twin_agent_tool(r#"{"tool":"inspect","args":{"source":"events"}}"#);
    let log = g.twin_from(0);
    // identifiers are never ranged; timestamps read as dates; constants are skipped
    assert!(!log.contains("assetIds 8773"), "id column ranged as a measurement: {log}");
    assert!(log.contains("startTime 2019-03-14 → 2023-11-02"), "timestamps not read as dates: {log}");
    assert!(!log.contains("count 1–1"), "constant column shown as a range: {log}");
    assert!(log.contains("description always empty"), "empty column not summarized: {log}");
}

#[test]
fn lens_columns_inherit_upstream_field_semantics() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_agent_tool(r#"{"tool":"annotate","args":{"source":"turbines","field":"gearbox_temp","title":"Gearbox temperature","description":"Degrees C at the gearbox bearing."}}"#);
    g.twin_agent_tool(r#"{"tool":"make_lens","args":{"name":"Hot gearboxes","description":"Above 70.","source":"turbines","code":"return rows.filter(r => Number(r.gearbox_temp) > 70)"}}"#);
    // the derived table's column header carries the upstream human title —
    // semantics flow along the from-chain, statistics stay per-derivation
    g.twin_event(r#"{"type":"open_source","name":"lens:hot-gearboxes","panel":0}"#);
    let t = g.twin_from(0);
    assert!(t.contains("Gearbox temperature"), "lens column did not inherit the upstream title: {t}");
    assert!(t.contains("Degrees C at the gearbox bearing."), "lens field guide did not inherit the description: {t}");
}

#[test]
fn undocumented_schema_is_a_finding_that_resolves_itself() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    // the gap is an ISSUE from the moment the source lands — no model involved
    let muts = g.twin_from(0);
    assert!(muts.contains("4 of 4 fields have no documented meaning"), "no schema-gap finding: {muts}");
    assert!(muts.contains("noticed by the twin"), "schema finding misattributed: {muts}");
    // its page offers documentation as the fix (id = 900000 + hash of the name)
    g.twin_event(r#"{"type":"open_finding","id":932836,"panel":0}"#);
    let all = g.twin_from(0);
    assert!(all.contains("fd:document") && all.contains("Document the fields"), "no document CTA: {all}");
    // annotating every field resolves the finding by itself
    for f in ["turbine_id", "site", "gearbox_temp", "vibration"] {
        g.twin_agent_tool(&format!(
            r#"{{"tool":"annotate","args":{{"source":"turbines","field":"{f}","title":"T","description":"d"}}}}"#
        ));
    }
    let after = g.twin_from(0);
    assert!(after.contains("documented every field of turbines"), "no finished step: {after}");
    assert!(after.contains(r#""value":"fnd sev-info resolved""#), "schema finding did not resolve: {after}");
}

#[test]
fn agent_lens_references_resolve_by_title() {
    let path = write_temp_csv();
    let mut g = JsGraph::new_twin();
    g.twin_read_source("turbines", &path, "mounted");
    g.twin_agent_tool(r#"{"tool":"make_lens","args":{"name":"Hot gearboxes","description":"Above 70.","source":"turbines","code":"return rows.filter(r => Number(r.gearbox_temp) > 70)"}}"#);
    // the agent refers to its lens by TITLE, not slug — meet it halfway
    g.twin_agent_tool(r#"{"tool":"make_lens","args":{"name":"Critical gearboxes","description":"Above 80.","source":"lens:Hot Gearboxes","code":"return rows.filter(r => Number(r.gearbox_temp) > 80)"}}"#);
    let muts = g.twin_from(0);
    assert!(muts.contains("ln:critical-gearboxes"), "title-cased lens source did not resolve: {muts}");
}

#[test]
fn done_tasks_offer_reopen_and_open_tasks_offer_work() {
    let mut g = JsGraph::new_twin();
    g.twin_agent_tool(r#"{"tool":"plan","args":{"items":["Profile the fleet"]}}"#);
    g.twin_event(r#"{"type":"set_task","id":1,"status":"done"}"#);
    g.twin_event(r#"{"type":"open_task","id":1,"panel":0}"#);
    let t = g.twin_from(0);
    assert!(t.contains(r#""text":"Reopen""#), "done task has no reopen action: {t}");
    assert!(t.contains(r#""value":"open""#), "reopen does not carry the open status: {t}");
    // reopening folds the status back and the page offers work again
    g.twin_event(r#"{"type":"set_task","id":1,"status":"open"}"#);
    g.twin_event(r#"{"type":"open_task","id":1,"panel":0}"#);
    let r = g.twin_from(0);
    assert!(r.contains("Start now"), "reopened task has no start action: {r}");
}

#[test]
fn lens_code_joins_other_sources_via_table() {
    let mut g = JsGraph::new_twin();
    let a = std::env::temp_dir().join(format!("join_assets_{}.csv", std::process::id()));
    std::fs::write(&a, "id,name\n1,Compressor\n2,Pump\n").unwrap();
    let e = std::env::temp_dir().join(format!("join_events_{}.csv", std::process::id()));
    std::fs::write(&e, "id,assetIds,type\n10,1,WO\n11,1,WO\n").unwrap();
    g.twin_read_source("assets", a.to_str().unwrap(), "mounted");
    g.twin_read_source("events", e.to_str().unwrap(), "mounted");
    // a cross-reference: assets that have events — impossible without table()
    g.twin_agent_tool(r#"{"tool":"make_lens","args":{"name":"Assets with work orders","description":"Assets that appear in maintenance events.","source":"assets","code":"const ev = table('events'); return rows.filter(r => ev.some(x => String(x.assetIds) === String(r.id)))"}}"#);
    let muts = g.twin_from(0);
    assert!(muts.contains("ln:assets-with-work-orders"), "join lens not built: {muts}");
    assert!(muts.contains("1 rows"), "join row count wrong: {muts}");
    let seen = g.twin_perceive();
    assert!(seen.contains("Compressor"), "joined rows wrong: {seen}");
    // an unknown table fails with a pointed message, not a bare ReferenceError
    g.twin_agent_tool(r#"{"tool":"make_lens","args":{"name":"Broken join","source":"assets","code":"return table('nope')"}}"#);
    assert!(g.twin_from(0).contains("no such source"), "unknown table not explained");
}

#[test]
fn an_open_task_page_follows_the_work_live() {
    let mut g = JsGraph::new_twin();
    g.twin_agent_tool(r#"{"tool":"plan","args":{"items":["Profile the fleet"]}}"#);
    g.twin_agent_tool(r#"{"tool":"work","args":{"task":"fleet","text":"starting with the registry"}}"#);
    g.twin_event(r#"{"type":"open_task","id":1,"panel":0}"#);
    let before = g.twin_total();
    // the agent keeps working while the page is open — the now-line follows
    g.twin_agent_tool(r#"{"tool":"work","args":{"text":"now checking sensor coverage"}}"#);
    let tail = g.twin_from(before);
    assert!(tail.contains(r#""key":"p0:ta:now:t""#) && tail.contains("now checking sensor coverage"),
        "open task page did not follow the work: {tail}");
    // …and marking it done flips the chip live, and hides the now-line
    g.twin_event(r#"{"type":"set_task","id":1,"status":"done"}"#);
    let t2 = g.twin_from(before);
    assert!(t2.contains(r#""key":"p0:ta:st""#) && t2.contains(r#""text":"done""#), "chip did not follow the status: {t2}");
    assert!(t2.contains(r#""key":"p0:ta:now","name":"hidden""#), "now-line not hidden when done: {t2}");
}

#[test]
fn pausing_is_captured_and_confirmed_quietly() {
    let mut g = JsGraph::new_twin();
    g.twin_event(r#"{"type":"pause"}"#);
    let muts = g.twin_from(0);
    assert!(muts.contains("Twin paused"), "no confirmation line for pause: {muts}");
    g.twin_event(r#"{"type":"resume"}"#);
    assert!(g.twin_from(0).contains("Twin resumed"), "no confirmation line for resume");
    // the raw events are in the input stream, so the agent perceives what happened
    assert!(g.twin_perceive().contains("paused the twin"), "pause not perceived as a user action");
}
