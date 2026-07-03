//! The §18 first milestone, end to end on the Rust + raw-V8 substrate.
//!
//!   cargo run --example milestone --release
//!
//! DB adapter (Rust poll+diff) -> calibrate/filter lenses -> join -> <Table> (all in
//! V8), with a UI cell edit round-tripping backward through the join into the DB via
//! the outbox, the calibration inverted, and the write's echo suppressed.

use serde_json::{json, Value};
use twin_runtime::adapter::{MockDb, PollDiffAdapter};
use twin_runtime::jsgraph::JsGraph;

fn drain_and_write(readings_ad: &mut PollDiffAdapter, readings_db: &mut MockDb,
                   graph: &mut JsGraph, outbox_json: &str) {
    let outbox: Vec<Value> = serde_json::from_str(outbox_json).unwrap();
    for entry in outbox {
        let stream = entry["stream"].as_str().unwrap();
        let edit: Vec<(Value, i64)> = serde_json::from_value(entry["edit"].clone()).unwrap();
        let author = entry["prov"]["author"].as_str().unwrap_or("operator");
        if stream == "readings" {
            readings_ad.write_through(&edit, author, readings_db, graph);
        }
    }
}

fn main() {
    println!("========================================================================");
    println!("FIRST MILESTONE (§18) on Rust + raw V8: DB adapter -> lenses -> join -> <Table>");
    println!("========================================================================");

    // external tables (the system of record outside the twin)
    let mut assets_db = MockDb::new("id");
    assets_db.insert(json!({"id": "T07", "name": "Turbine 07", "site": "Kelmarsh"}));
    assets_db.insert(json!({"id": "T11", "name": "Turbine 11", "site": "Kelmarsh"}));
    let mut readings_db = MockDb::new("asset");
    readings_db.insert(json!({"asset": "T07", "temp": 58.0}));
    readings_db.insert(json!({"asset": "T11", "temp": 44.0}));

    // the V8-resident graph + the Rust boundary adapters
    let mut graph = JsGraph::new_milestone();
    let mut assets_ad = PollDiffAdapter::new("assets", "assets-db");
    let mut readings_ad = PollDiffAdapter::new("readings", "readings-db");

    println!("\n[1] Initial poll of both external tables (snapshot -> stream)");
    assets_ad.poll(&assets_db, &mut graph);
    readings_ad.poll(&readings_db, &mut graph);
    println!("    host mutations applied: {}", graph.mutation_count());
    println!("    table (temp desc, calibrated = raw+2.0):");
    for line in graph.render() { println!("      {line}"); }

    println!("\n[2] External change: T11 heats to 61.0 in the DB (independent writer)");
    readings_db.update("T11", "temp", json!(61.0));
    let before = graph.mutation_count();
    readings_ad.poll(&readings_db, &mut graph);
    println!("    host mutations this cycle: {}", graph.mutation_count() - before);
    for line in graph.render() { println!("      {line}"); }

    println!("\n[3] Local cell edit: set T07 temp -> 75 (backward through join, k3)");
    println!("    -> calibrate inverts (-2.0) -> outbox -> write-through to the DB");
    let outbox = graph.cell_edit("T07", "temp", "75.0",
        &json!({"origin": "local", "author": "operator"}).to_string());
    drain_and_write(&mut readings_ad, &mut readings_db, &mut graph, &outbox);
    println!("    DB row for T07 after write-through: {}", readings_db.get("T07").unwrap());
    println!("    (DB stores RAW 73.0 = displayed 75.0 - 2.0 calibration)");

    println!("\n[4] Next poll: the write-through echo must NOT double-apply");
    let before = graph.mutation_count();
    let changed = readings_ad.poll(&readings_db, &mut graph);
    println!("    poll delta rows: {changed} (0 = echo suppressed)");
    println!("    host mutations from echo poll: {}", graph.mutation_count() - before);
    for line in graph.render() { println!("      {line}"); }

    println!("\n[5] Lineage (§8): every edit on the readings stream");
    let blame: Vec<Value> = serde_json::from_str(&graph.blame("readings")).unwrap();
    for e in blame {
        println!("      seq={} ts={} by={:<12} origin={:<8} {}",
            e["seq"], e["ts"], e["author"].as_str().unwrap(),
            e["origin"].as_str().unwrap(), e["note"].as_str().unwrap_or(""));
    }

    println!("\nDONE — the milestone loop ran on raw V8: external feed in, calibrated,");
    println!("joined, rendered incrementally; a UI edit round-tripped into the DB with the");
    println!("calibration inverted, echo suppressed. The dataflow graph is V8-native JS.\n");
}
