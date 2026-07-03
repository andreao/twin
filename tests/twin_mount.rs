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
    let n = g.twin_chart_series(&id, "sensor");
    assert!(n > 0, "no points read for {id}");
    let muts = g.twin_from(0);
    assert!(muts.contains("\"tag\":\"svg\""), "no svg element rendered");
    assert!(muts.contains("chart-line") && muts.contains("exp:path"), "no chart line path");
}

#[test]
fn documents_source_renders_a_gallery_and_embed() {
    let mut g = JsGraph::new_twin();
    g.twin_register_documents(r#"[{"name":"PID-1.pdf","bytes":1024},{"name":"draw.svg","bytes":2048}]"#);
    g.twin_event(r#"{"type":"open_source","name":"documents"}"#);
    let gallery = g.twin_from(0);
    assert!(gallery.contains("doc-gallery") && gallery.contains("doc:PID-1.pdf"), "no gallery");
    g.twin_event(r#"{"type":"open_document","name":"PID-1.pdf"}"#);
    let embed = g.twin_from(0);
    assert!(embed.contains("/file/PID-1.pdf"), "no document embed src");
}

#[test]
fn missing_file_reports_a_readable_error() {
    let mut g = JsGraph::new_twin();
    let status = g.twin_read_source("nope", "/no/such/file.csv", "mounted");
    assert!(status.starts_with("read_source error"), "status: {status}");
    let muts = g.twin_from(0);
    assert!(muts.contains("Couldn't read"), "no error surfaced to user: {muts}");
}
