//! The agent runs skills itself (§9.8 governed effect): the user points the
//! direction, the SYSTEM fetches.  Uses obtain-volve's fast `status` command —
//! no network, just a directory listing.

use twin_runtime::{server::dispatch_tool, JsGraph};

fn store() -> twin_runtime::embed::EmbedStore {
    let p = std::env::temp_dir().join(format!("twin_runskill_{}.jsonl", std::process::id()));
    twin_runtime::embed::EmbedStore::open(p.to_str().unwrap())
}

#[test]
fn run_skill_executes_and_reports_into_activity() {
    let mut g = JsGraph::new_twin();
    let mut e = store();
    dispatch_tool(&mut g, &mut e, r#"{"tool":"run_skill","args":{"skill":"obtain-volve","command":"status"}}"#, false);
    let muts = g.twin_from(0);
    assert!(muts.contains("obtain-volve status"), "no run step logged: {muts}");
    assert!(muts.contains("volve data under"), "script output not captured: {muts}");
}

#[test]
fn replay_never_reexecutes_a_skill_run() {
    let mut g = JsGraph::new_twin();
    let mut e = store();
    dispatch_tool(&mut g, &mut e, r#"{"tool":"run_skill","args":{"skill":"obtain-volve","command":"github"}}"#, true);
    let muts = g.twin_from(0);
    assert!(muts.contains("not re-executed on replay"), "replay guard missing: {muts}");
    assert!(!muts.contains("cloning"), "replay actually ran the script");
}

#[test]
fn unknown_skills_and_bad_commands_fail_readably() {
    let mut g = JsGraph::new_twin();
    let mut e = store();
    dispatch_tool(&mut g, &mut e, r#"{"tool":"run_skill","args":{"skill":"no-such-skill","command":"pull"}}"#, false);
    assert!(g.twin_from(0).contains("no skill by that name"));
    dispatch_tool(&mut g, &mut e, r#"{"tool":"run_skill","args":{"skill":"obtain-volve","command":"x; rm -rf /"}}"#, false);
    assert!(g.twin_from(0).contains("does not accept"), "shell metacharacters must be rejected");
}
