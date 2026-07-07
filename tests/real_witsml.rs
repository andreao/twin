//! The readers vs the REAL Volve corpus (obtain-volve `github`: the original
//! Statoil WITSML tree for wells 15/9-F-4/-7/-9).  Skips quietly when the
//! corpus hasn't been pulled — run ./skills/obtain-volve/pull_volve.sh github.

const ROOT: &str = "data/volve/real/witsml";

#[test]
fn every_real_witsml_file_parses() {
    if !std::path::Path::new(ROOT).exists() {
        eprintln!("skip: real corpus not pulled (obtain-volve github)");
        return;
    }
    let mut files = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(ROOT)];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().map(|x| x == "xml").unwrap_or(false) {
                files.push(p);
            }
        }
    }
    let mut ok = 0usize;
    let mut rows_total = 0usize;
    let mut first_fail = String::new();
    for p in &files {
        match twin_runtime::source::read_file(p.to_str().unwrap()) {
            Ok(rows) => {
                ok += 1;
                rows_total += rows.len();
            }
            Err(e) if first_fail.is_empty() => first_fail = format!("{}: {e}", p.display()),
            Err(_) => {}
        }
    }
    eprintln!("real WITSML: {ok}/{} files parsed, {rows_total} rows", files.len());
    assert!(
        ok * 10 >= files.len() * 9,
        "under 90% of real WITSML parsed; first failure: {first_fail}"
    );
}

#[test]
fn pulled_data_appears_in_perception_unmounted() {
    // the point of the scan: a fresh pull is VISIBLE to the agent, and mounting
    // it makes the offer disappear — no human message required in between
    let mut g = twin_runtime::JsGraph::new_twin();
    let seen = g.twin_perceive();
    assert!(seen.contains("\"unmounted\""), "no unmounted section: {seen}");
    assert!(seen.contains("data/volve/witsml"), "sample witsml dir not offered: {seen}");
    g.twin_read_source("witsml-samples", "data/volve/witsml", "mounted");
    let seen2 = g.twin_perceive();
    assert!(!seen2.contains("\"path\":\"data/volve/witsml\""), "mounted dir still offered: {seen2}");
}

#[test]
fn a_whole_well_mounts_as_one_source() {
    let well = format!("{ROOT}/Norway-Statoil-NO 15_$47$_9-F-4");
    if !std::path::Path::new(&well).exists() {
        eprintln!("skip: real corpus not pulled (obtain-volve github)");
        return;
    }
    let mut g = twin_runtime::JsGraph::new_twin();
    let status = g.twin_read_source("well-f4", &well, "mounted");
    assert!(status.starts_with("mounted"), "{status}");
    let seen = g.twin_perceive();
    // rows arrive kind-tagged so extraction lenses can carve by object type,
    // and file-tagged so every row keeps its lineage
    assert!(seen.contains("\"kind\""), "no kind column in perception: {seen}");
    assert!(seen.contains("\"file\""), "no file column");
    // and a lens extracts one object type from the mixed mount
    g.twin_agent_tool(
        r#"{"tool":"make_lens","args":{"name":"F-4 trajectory stations","description":"Survey stations extracted from the raw WITSML tree.","source":"well-f4","code":"return rows.filter(r => r.kind === 'trajectory')"}}"#,
    );
    let seen2 = g.twin_perceive();
    assert!(seen2.contains("F-4 trajectory stations"), "extraction lens missing: {seen2}");
}
