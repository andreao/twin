//! Proof: the protobuf `datapoints.decode` lens is pure JS, runs in V8 with zero new
//! deps, and its output AGREES with the JSON ingestion path (the "prove they agree"
//! demo). Reads the OID `.pb` files we pulled; soft-skips if that data isn't present.

use std::collections::HashMap;
use std::path::Path;
use twin_runtime::Runtime;

const PBJS: &str = include_str!("../src/js/pbdatapoints.js");
const SENSOR: &str = "6190956317771"; // VAL_23-PDT-92501 (numeric), pulled in both formats

#[test]
fn protobuf_decode_in_js_agrees_with_json() {
    let pb_dir = format!("data/cognite/datapoints_pb/{SENSOR}");
    let csv = format!("data/cognite/datapoints/{SENSOR}.csv");
    if !Path::new(&pb_dir).exists() || !Path::new(&csv).exists() {
        eprintln!("skip: OID data not present (run skills/obtain-oid/pull_oid.sh)");
        return;
    }

    // JSON path (ground truth): ts -> value
    let text = std::fs::read_to_string(&csv).unwrap();
    let json: HashMap<i64, f64> = text
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(','))
        .filter_map(|(t, v)| Some((t.parse().ok()?, v.parse().ok()?)))
        .collect();
    // the protobuf demo was pulled later than the JSON path, so it has fresher points;
    // compare only where the two windows overlap.
    let json_max = *json.keys().max().unwrap();

    // Decode every daily .pb through the pure-JS lens, in V8.
    let mut rt = Runtime::new();
    let br = rt.branch("decode", false);
    rt.eval(&br, PBJS).unwrap();

    let mut files: Vec<_> = std::fs::read_dir(&pb_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "pb").unwrap_or(false))
        .collect();
    files.sort();

    let mut decoded = 0usize;
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        if bytes.is_empty() {
            continue;
        }
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let out = rt
            .eval(&br, &format!("JSON.stringify(decodeDatapoints(hexToBytes('{hex}')))"))
            .unwrap();
        let rows: Vec<(f64, f64)> = serde_json::from_str(&out).unwrap();
        for (ts, val) in rows {
            let ts = ts as i64;
            if ts > json_max {
                continue; // fresher point the earlier JSON pull never saw
            }
            let jv = json
                .get(&ts)
                .unwrap_or_else(|| panic!("pb ts {ts} absent from JSON path"));
            assert!(
                (jv - val).abs() <= 1e-6 * (1.0 + jv.abs()),
                "value mismatch at {ts}: json {jv} vs pb {val}"
            );
            decoded += 1;
        }
    }

    assert!(decoded > 1000, "decoded too few points ({decoded})");
    eprintln!("OK: {decoded} protobuf-decoded points, all agree with the JSON path");
}
