//! The mount pipeline, end to end, model-free: reading a file as a source (§9.9)
//! makes it appear in the agent's perception (§12.3) and renders it as a source
//! (§11.3), while the file stays put (federation — the residence model).

use std::io::Write;
use twin_runtime::JsGraph;

fn write_temp_csv() -> String {
    let path = std::env::temp_dir().join("twin_test_turbines.csv");
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

    // (2) it renders: a Sources-panel row + a "Mounted …" system feed item (§11.3)
    let muts = g.twin_from(0);
    assert!(muts.contains("sr:turbines"), "no source row rendered: {muts}");
    assert!(muts.contains("Mounted"), "no system feed note: {muts}");

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
    assert!(muts.contains("explorer-root"), "explorer table not rendered");
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
    let n = g.twin_fetch("cognite-datapoints", &id, "sensor");
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
    let n = g.twin_fetch("cognite-datapoints", "1009048440794092", "VAL-test");
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
    assert!(log.contains("inspected") && log.contains("timeseries"), "inspect did not run");
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
    assert!(!muts.contains("explorer-root"), "inline chart leaked into the explorer pane");
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
    assert!(muts.contains("Findings — 1"), "tile title has no count");
    assert!(muts.contains("fnd:1") && muts.contains("sev-warn"), "finding card missing: {muts}");
    assert!(muts.contains("no vibration sensor"), "finding text missing");
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
    // the derived rows render inline as a view card; the filtered-out row does not
    assert!(muts.contains("WT-02") && muts.contains("WT-03"), "derived rows missing from inline view");
    assert!(!muts.contains("WT-01"), "lens filter not applied");
    // deep inspection: expanding the lens shows the derivation chain AND the code
    g.twin_event(r#"{"type":"open_source","name":"lens:hot-gearboxes"}"#);
    let deep = g.twin_from(0);
    assert!(deep.contains("exp:tbl"), "lens not browsable as a table");
    assert!(deep.contains("how this data is derived"), "no derivation chain in the expanded view");
    assert!(deep.contains("return rows.filter"), "code not shown on deep inspection");
    let seen = g.twin_perceive();
    assert!(seen.contains("lens:hot-gearboxes"), "lens not perceived: {seen}");
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
    assert!(
        deep.contains("turbines  →  Hot gearboxes  →  Critical gearboxes"),
        "derivation chain missing or wrong: {deep}"
    );
    assert!(deep.contains("WT-03") , "composed lens rows wrong");
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
