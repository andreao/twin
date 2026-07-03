//! End-to-end §18 milestone invariants on the Rust + raw-V8 substrate.

use serde_json::{json, Value};
use twin_runtime::adapter::{MockDb, PollDiffAdapter};
use twin_runtime::jsgraph::JsGraph;

fn drain(readings_ad: &mut PollDiffAdapter, readings_db: &mut MockDb,
         graph: &mut JsGraph, outbox_json: &str) {
    let outbox: Vec<Value> = serde_json::from_str(outbox_json).unwrap();
    for entry in outbox {
        let edit: Vec<(Value, i64)> = serde_json::from_value(entry["edit"].clone()).unwrap();
        if entry["stream"] == "readings" {
            readings_ad.write_through(&edit, "operator", readings_db, graph);
        }
    }
}

fn temp_of(graph: &mut JsGraph, id: &str) -> f64 {
    let rows = graph.visible_rows();
    for r in rows.as_array().unwrap() {
        if r["id"] == id {
            return r["temp"].as_f64().unwrap();
        }
    }
    panic!("row {id} not visible");
}

#[test]
fn milestone_end_to_end() {
    let mut assets_db = MockDb::new("id");
    assets_db.insert(json!({"id": "T07", "name": "Turbine 07", "site": "Kelmarsh"}));
    assets_db.insert(json!({"id": "T11", "name": "Turbine 11", "site": "Kelmarsh"}));
    let mut readings_db = MockDb::new("asset");
    readings_db.insert(json!({"asset": "T07", "temp": 58.0}));
    readings_db.insert(json!({"asset": "T11", "temp": 44.0}));

    let mut graph = JsGraph::new_milestone();
    let mut assets_ad = PollDiffAdapter::new("assets", "assets-db");
    let mut readings_ad = PollDiffAdapter::new("readings", "readings-db");

    assets_ad.poll(&assets_db, &mut graph);
    readings_ad.poll(&readings_db, &mut graph);

    // calibrated display: raw 58 -> 60, raw 44 -> 46
    assert_eq!(temp_of(&mut graph, "T07"), 60.0);
    assert_eq!(temp_of(&mut graph, "T11"), 46.0);

    // local cell edit -> write-through with calibration inverted
    let outbox = graph.cell_edit("T07", "temp", "75.0",
        &json!({"origin": "local", "author": "operator"}).to_string());
    drain(&mut readings_ad, &mut readings_db, &mut graph, &outbox);

    // DB stores RAW (75 - 2.0); table shows calibrated 75
    assert_eq!(readings_db.get("T07").unwrap()["temp"].as_f64().unwrap(), 73.0);
    assert_eq!(temp_of(&mut graph, "T07"), 75.0);

    // echo suppressed: next poll produces zero changes, no double-apply
    let before = graph.mutation_count();
    let changed = readings_ad.poll(&readings_db, &mut graph);
    assert_eq!(changed, 0, "own write must be echo-suppressed");
    assert_eq!(graph.mutation_count(), before, "no mutations from the echo poll");
    assert_eq!(temp_of(&mut graph, "T07"), 75.0);
}

#[test]
fn external_change_is_ingested() {
    let mut db = MockDb::new("asset");
    db.insert(json!({"asset": "T07", "temp": 58.0}));
    db.insert(json!({"asset": "T11", "temp": 44.0}));
    let mut graph = JsGraph::new_milestone();
    let mut ad = PollDiffAdapter::new("readings", "db");
    // assets so the join produces rows
    let mut adb = MockDb::new("id");
    adb.insert(json!({"id": "T07", "name": "T7", "site": "S"}));
    adb.insert(json!({"id": "T11", "name": "T11", "site": "S"}));
    PollDiffAdapter::new("assets", "adb").poll(&adb, &mut graph);
    ad.poll(&db, &mut graph);
    assert_eq!(temp_of(&mut graph, "T11"), 46.0);
    db.update("T11", "temp", json!(61.0));
    ad.poll(&db, &mut graph);
    assert_eq!(temp_of(&mut graph, "T11"), 63.0);
    // an unchanged poll is empty
    assert_eq!(ad.poll(&db, &mut graph), 0);
}
