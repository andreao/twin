//! The drilling slice, end to end and model-free (§8.1, §9.9): every format the
//! industry actually stores wells in mounts as flat rows, and the linking lenses
//! join them deterministically — WITSML daily reports, an EDM engineering export,
//! a LAS log, fixed-width formation picks, and a PDF final well report, all for
//! Volve well NO 15/9-F-14 (the bundled obtain-volve samples).

use twin_runtime::JsGraph;

fn mount_volve(g: &mut JsGraph) {
    for (name, path) in [
        ("drillReports", "data/volve/witsml/drillReports.xml"),
        ("trajectory", "data/volve/witsml/trajectory.xml"),
        ("export", "data/volve/edm/export.xml"),
        ("log-15-9-F-14", "data/volve/logs/15_9-F-14.las"),
        ("wellpicks", "data/volve/picks/wellpicks.txt"),
    ] {
        let status = g.twin_read_source(name, path, "mounted");
        assert!(status.starts_with("mounted"), "{path}: {status}");
    }
}

#[test]
fn every_drilling_format_mounts_as_rows() {
    let mut g = JsGraph::new_twin();
    mount_volve(&mut g);
    let seen = g.twin_perceive();
    // WITSML activities exploded: 3 reports × their activities = 8 rows
    assert!(seen.contains("\"drillReports\""), "drill reports not perceived");
    assert!(seen.contains("proprietaryCode"), "activity fields not in schema: {seen}");
    // the trajectory's stations, the EDM families, the LAS curves, the picks
    assert!(seen.contains("incl"), "trajectory station fields missing");
    assert!(seen.contains("family"), "EDM family column missing");
    assert!(seen.contains("RHOB"), "LAS curve columns missing");
    assert!(seen.contains("Wellbore"), "picks columns missing");
}

#[test]
fn witsml_activities_carry_their_report_context() {
    let rows = twin_runtime::source::read_file("data/volve/witsml/drillReports.xml").unwrap();
    assert_eq!(rows.len(), 8, "3 daily reports should explode to 8 activity rows");
    assert!(rows.iter().all(|r| r["nameWell"] == "NO 15/9-F-14"));
    assert!(rows.iter().any(|r| r["proprietaryCode"] == "TRIP"));
    // the report's own scalar context rides on every activity row
    assert!(rows.iter().all(|r| r.get("mdReport").is_some()));
}

#[test]
fn a_linking_lens_joins_log_depths_to_picked_formation_tops() {
    let mut g = JsGraph::new_twin();
    mount_volve(&mut g);
    // The §8.1 move: the LAS well is "15/9-F-14", the picks say "NO 15/9-F-14" —
    // normalizeWell reconciles the naming, inInterval does the depth join.
    g.twin_agent_tool(
        r#"{"tool":"make_lens","args":{"name":"Hugin reservoir stations","description":"Log stations inside the Hugin formation, located by the picks.","source":"log-15-9-F-14","code":"const picks = table('wellpicks').filter(p => normalizeWell(p.Wellbore) === normalizeWell(rows[0].well)); const top = picks.find(p => String(p.Surface).includes('Hugin Fm. Top')); const base = picks.find(p => String(p.Surface).includes('Hugin Fm. Base')); return rows.filter(r => inInterval(r.DEPT, top.MD, base.MD))"}}"#,
    );
    let muts = g.twin_from(0);
    assert!(muts.contains("ln:hugin-reservoir-stations"), "linking lens not built: {muts}");
    let seen = g.twin_perceive();
    // stations at 2115..2190 lie inside [2113, 2195]; 2100 and 2205 do not
    assert!(seen.contains("\"rowcount\":6"), "depth-interval join wrong: {seen}");
}

#[test]
fn reports_link_to_engineering_export_by_normalized_well_name() {
    let mut g = JsGraph::new_twin();
    mount_volve(&mut g);
    // wells in the EDM export that have drilling activity reported — a cross-format
    // join on the reconciled well name (both say NO 15/9-F-14 here, but through
    // normalizeWell so either side's convention works)
    g.twin_agent_tool(
        r#"{"tool":"make_lens","args":{"name":"Wells with reported activity","description":"EDM wells that appear in the daily drill reports.","source":"export","code":"const reps = table('drillReports'); return rows.filter(r => r.family === 'CD_WELL' && reps.some(x => normalizeWell(x.nameWell) === normalizeWell(r.well_legal_name)))"}}"#,
    );
    let seen = g.twin_perceive();
    assert!(seen.contains("Wells with reported activity"), "join lens missing");
    // F-14 reports exist, F-15 has none → exactly one linked well
    assert!(seen.contains("\"rowcount\":1"), "normalized name join wrong: {seen}");
}

#[test]
fn the_final_well_report_reads_and_mounts_as_text() {
    // clear the cache so this exercises the real extraction path
    let _ = std::fs::remove_file("data/models/docs/final-well-report-15-9-F-14.pdf.json");
    let d = twin_runtime::doc::read("final-well-report-15-9-F-14.pdf").unwrap();
    assert_eq!(d.ocr_pages, 0, "the sample report has a text layer — no OCR needed");
    assert_eq!(d.rows.len(), 2, "two pages of text expected");
    let all: String = d.rows.iter().map(|r| r["text"].as_str().unwrap_or("")).collect();
    assert!(all.contains("Hugin formation was penetrated at 2113 m"), "report text wrong: {all}");
    assert!(all.contains("Recommendation"), "second page missing");

    // and the second read comes from the write-through cache
    let d2 = twin_runtime::doc::read("final-well-report-15-9-F-14.pdf").unwrap();
    assert!(d2.cached, "second read should hit the cache");

    // mounted, the text is a source like any other — lenses can reach it
    let mut g = JsGraph::new_twin();
    g.twin_mount_rows("doc:final-well-report", &d.rows, "document final-well-report-15-9-F-14.pdf");
    let seen = g.twin_perceive();
    assert!(seen.contains("doc:final-well-report"), "doc source not perceived");
    assert!(seen.contains("Hugin"), "doc text not in perception sample");
}

#[test]
fn show_survives_the_models_loose_column_names() {
    let mut g = JsGraph::new_twin();
    mount_volve(&mut g);
    // lowercase, stray whitespace, and one invented column — the cells must still
    // render (a header whose rows are all blank is worse than no table)
    g.twin_agent_tool(
        r#"{"tool":"show","args":{"view":"table","source":"wellpicks","columns":["wellbore"," Surface","md","Depth (TVD)","nonsense"],"limit":10,"title":"Formation Picks Overview"}}"#,
    );
    let muts = g.twin_from(0);
    assert!(muts.contains("NO 15/9-F-14"), "wellbore cells empty: {muts}");
    assert!(muts.contains("Hugin Fm. Top"), "surface cells empty");
    assert!(muts.contains("2113"), "md cells empty");
    assert!(!muts.contains("nonsense"), "an invented column should drop, not render empty");
}

#[test]
fn host_effects_report_into_the_activity_log() {
    let mut g = JsGraph::new_twin();
    g.twin_log_step("read", "final-well-report.pdf", "2 text blocks from the text layer", "doc:final-well-report", "");
    let muts = g.twin_from(0);
    assert!(muts.contains("final-well-report.pdf"), "host step not logged: {muts}");
    assert!(muts.contains("agent-act"), "host step not on the now-strip");
}

#[test]
fn document_text_chunks_for_embedding() {
    let d = twin_runtime::doc::read("final-well-report-15-9-F-14.pdf").unwrap();
    let blocks: Vec<String> = d.rows.iter().filter_map(|r| r["text"].as_str().map(String::from)).collect();
    let chunks: Vec<String> = blocks.iter().flat_map(|b| twin_runtime::embed::chunk(b)).collect();
    assert!(!chunks.is_empty());
    assert!(chunks.iter().any(|c| c.contains("Hugin")));
}

#[test]
fn semantic_search_answers_from_the_report_when_the_model_host_is_up() {
    // a REAL model round-trip — embedding the report and asking a question — but
    // only when the local host is reachable; CI without Ollama skips quietly.
    if twin_runtime::ollama::post("/api/tags", "{}").is_err() {
        eprintln!("skip: no local model host");
        return;
    }
    let path = std::env::temp_dir().join(format!("twin_drill_embed_{}.jsonl", std::process::id()));
    let mut store = twin_runtime::embed::EmbedStore::open(path.to_str().unwrap());
    let d = twin_runtime::doc::read("final-well-report-15-9-F-14.pdf").unwrap();
    let blocks: Vec<String> = d.rows.iter().filter_map(|r| r["text"].as_str().map(String::from)).collect();
    match store.add_document("doc:final-well-report", &blocks) {
        Ok(n) if n > 0 => {}
        Ok(_) | Err(_) => {
            eprintln!("skip: embedding model not available");
            let _ = std::fs::remove_file(&path);
            return;
        }
    }
    let hits = store.search("at what depth was the reservoir found?", 3).unwrap();
    assert!(!hits.is_empty());
    let joined: String = hits.iter().map(|h| h.text.as_str()).collect();
    assert!(joined.contains("2113"), "top hits should mention the Hugin depth: {joined}");
    let _ = std::fs::remove_file(&path);
}
