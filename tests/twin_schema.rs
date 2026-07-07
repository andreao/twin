//! Schema inference — the twin understanding its own data (§8: schema is
//! event-sourced data).  Pass one is statistical and model-free: the moment a
//! source mounts, a profiler claims types, keys, enums, gaps, duplicate columns,
//! cross-source references, and string patterns on the `schema` stream.  Pass two
//! is semantic: the agent's `annotate` tool claims what fields MEAN, overriding
//! the statistics on fold.  Both surface in perception (`fields`) and in the UI.

use std::io::Write;
use twin_runtime::JsGraph;

fn temp_csv(tag: &str, body: &str) -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "twin_schema_{tag}_{}_{}.csv",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn profiling_claims_types_keys_enums_and_gaps() {
    let path = temp_csv(
        "sensors",
        "id,unit,status,note\n\
         1,degC,ok,\n\
         2,degC,ok,x\n\
         3,barg,ok,\n\
         4,degC,ok,\n\
         5,barg,ok,\n\
         6,degC,ok,\n",
    );
    let mut g = JsGraph::new_twin();
    g.twin_read_source("sensors", &path, "mounted");
    let seen = g.twin_perceive();
    // id: every value distinct and present → a unique key
    assert!(seen.contains("id: number · unique key"), "no key claim: {seen}");
    // unit: few distinct values, many rows → an enum with its values listed
    assert!(seen.contains("one of:") && seen.contains("degC") && seen.contains("barg"), "no enum claim: {seen}");
    // status: one value everywhere → a constant, not an enum
    assert!(seen.contains("always “ok”"), "no constant claim: {seen}");
    // note: mostly missing → a gap percentage
    assert!(seen.contains("% empty"), "no empty claim: {seen}");
}

#[test]
fn cross_source_references_are_detected() {
    let a = temp_csv(
        "assets",
        "id,parentId,name\n\
         10,,Platform\n\
         11,10,Compressor\n\
         12,10,Pump\n\
         13,11,Gearbox\n\
         14,11,Seal\n\
         15,12,Impeller\n",
    );
    let t = temp_csv(
        "ts",
        "id,assetId,name\n\
         90,11,temp-a\n\
         91,11,temp-b\n\
         92,12,flow\n\
         93,13,vib\n\
         94,13,vib2\n",
    );
    let e = temp_csv(
        "ev",
        "id,assetIds,type\n\
         1,11;12,WO\n\
         2,13,WO\n\
         3,11;13,WO\n\
         4,12,WO\n\
         5,14;10,WO\n",
    );
    let mut g = JsGraph::new_twin();
    g.twin_read_source("assets", &a, "mounted");
    g.twin_read_source("timeseries", &t, "mounted");
    g.twin_read_source("events", &e, "mounted");
    let seen = g.twin_perceive();
    // the sensor→equipment link, found from value containment alone
    assert!(seen.contains("assetId: number · references assets.id"), "no FK claim: {seen}");
    // the self-referencing hierarchy: parentId points back into the same table
    assert!(seen.contains("parentId: number · references assets.id"), "no self-ref claim: {seen}");
    // ';'-joined ids are split and claimed as a multi-reference
    assert!(seen.contains("multi-references assets.id"), "no multi-ref claim: {seen}");
}

#[test]
fn string_patterns_and_duplicate_columns_are_mined() {
    let t = temp_csv(
        "ts",
        "id,externalId,name\n\
         1,VAL_23-YA-96118-02:Z.X.Value,VAL_23-YA-96118-02:Z.X.Value\n\
         2,VAL_23-TT-96115-01:Z.X.Value,VAL_23-TT-96115-01:Z.X.Value\n\
         3,VAL_23-PT-96186-03:Z.X.Value,VAL_23-PT-96186-03:Z.X.Value\n\
         4,VAL_23-FT-92537-02:Z.X.Value,VAL_23-FT-92537-02:Z.X.Value\n\
         5,VAL_23-TT-96114-03:Z.X.Value,VAL_23-TT-96114-03:Z.X.Value\n\
         6,VAL_23-YA-96120-01:Z.X.Value,VAL_23-YA-96120-01:Z.X.Value\n",
    );
    let mut g = JsGraph::new_twin();
    g.twin_read_source("timeseries", &t, "mounted");
    let seen = g.twin_perceive();
    // the tag grammar: site/digits masked, the stable channel suffix kept
    assert!(seen.contains("pattern") && seen.contains(":Z.X.Value"), "no mined pattern: {seen}");
    assert!(seen.contains("##"), "digits not generalized in the pattern: {seen}");
    // name is a verbatim copy of externalId — say so instead of repeating the stats
    assert!(seen.contains("duplicates externalId"), "no duplicate-column claim: {seen}");
}

#[test]
fn agent_annotates_a_field_and_the_ui_follows() {
    let t = temp_csv(
        "ts",
        "id,assetId,unit\n\
         90,11,degC\n\
         91,11,degC\n\
         92,12,barg\n",
    );
    let mut g = JsGraph::new_twin();
    g.twin_read_source("timeseries", &t, "mounted");
    g.twin_agent_tool(
        r#"{"tool":"annotate","args":{"source":"timeseries","field":"assetId","title":"Measured equipment","description":"The equipment this instrument is mounted on.","ref":"assets"}}"#,
    );
    // the semantic claim folds over the statistical one and reaches perception
    let seen = g.twin_perceive();
    assert!(seen.contains("“Measured equipment”"), "annotation not perceived: {seen}");
    assert!(seen.contains("mounted on"), "field description not perceived: {seen}");
    // the table renders the human title as the column header, and the field guide
    // explains the annotated column
    g.twin_event(r#"{"type":"open_source","name":"timeseries"}"#);
    let muts = g.twin_from(0);
    assert!(muts.contains("Measured equipment"), "header not humanized: {muts}");
    assert!(muts.contains("field-guide"), "no field guide on the source page: {muts}");
    // annotating a field that does not exist fails with the real field list
    g.twin_agent_tool(r#"{"tool":"annotate","args":{"source":"timeseries","field":"nope","title":"X"}}"#);
    let m = g.twin_from(0);
    assert!(m.contains("no field “nope”") && m.contains("fields are:"), "bad field not explained: {m}");
}

/// The claims the profiler makes on the REAL Valhall data — the table that
/// motivated this feature.  Skips when the open industrial data isn't pulled.
#[test]
fn valhall_data_profiles_end_to_end() {
    if !std::path::Path::new("data/cognite/assets.csv").exists() {
        eprintln!("skip: no Valhall data pulled");
        return;
    }
    let mut g = JsGraph::new_twin();
    g.twin_read_source("assets", "data/cognite/assets.csv", "mounted");
    g.twin_read_source("timeseries", "data/cognite/timeseries.csv", "mounted");
    let seen = g.twin_perceive();
    let fields: Vec<&str> = seen.split("\"fields\":").collect();
    eprintln!("valhall fields: {}", fields.get(2).map(|s| &s[..s.len().min(900)]).unwrap_or(&""));
    // the instrument→equipment link is found from the data alone
    assert!(seen.contains("assetId: number · references assets.id"), "no FK on real data: {seen}");
    // the plant hierarchy self-reference
    assert!(seen.contains("parentId: number · references assets.id"), "no hierarchy ref on real data");
    // the VAL_ tag grammar surfaces as a mined pattern
    assert!(seen.contains("externalId: string") && seen.contains("pattern"), "no externalId pattern on real data");
    // name mostly duplicates externalId — with the honest overlap, because the
    // rows where it diverges (junk/test series) are the mapping gaps
    assert!(seen.contains("duplicates externalId on"), "duplicate name column not claimed on real data");
    assert!(seen.contains("isString: boolean"), "isString not typed");
}

#[test]
fn lenses_are_profiled_like_mounts() {
    let t = temp_csv(
        "ts",
        "id,unit\n1,degC\n2,degC\n3,barg\n4,degC\n5,barg\n6,degC\n",
    );
    let mut g = JsGraph::new_twin();
    g.twin_read_source("timeseries", &t, "mounted");
    g.twin_agent_tool(
        r#"{"tool":"make_lens","args":{"name":"Temperature sensors","description":"Sensors measured in degC.","source":"timeseries","code":"return rows.filter(r => r.unit === 'degC')"}}"#,
    );
    let seen = g.twin_perceive();
    // the derived source got the same first-pass inference as a mount
    assert!(seen.contains("lens:temperature-sensors"), "lens missing: {seen}");
    assert!(seen.contains("always “degC”"), "lens rows not profiled (unit is constant after the filter): {seen}");
}
