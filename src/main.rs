//! Substrate demo (design_doc §4, §6, §14): content-addressed code + branching +
//! governed effects on raw V8, driven from Rust.
//!
//!   cargo run --release

use std::time::Instant;
use twin_runtime::{DefinitionStore, Runtime};

fn main() {
    println!("========================================================================");
    println!("SUBSTRATE (§14): content-addressed code + branching + governed effects on V8");
    println!("========================================================================\n");

    // ---- §4 content-addressed definitions + namespace -----------------------
    let mut defs = DefinitionStore::new();
    let h1 = defs.publish("inc", "lens", "x => x + 1", &[]);
    let h2 = defs.publish("plusOne", "lens", "x => x + 1   // different name, same meaning", &[]);
    println!("[§4] content-addressed definitions");
    println!("     inc      -> {h1}");
    println!("     plusOne  -> {h2}");
    println!("     identical content shared automatically: {}  (objects stored: {})",
             h1 == h2, defs.object_count());

    let mut rt = Runtime::new();

    // ---- §14 (V2) cross-Context heap sharing --------------------------------
    let branch_a = rt.branch("production", false);
    let branch_b = rt.branch("what-if", false);
    let shared = rt
        .read_shared_field(&branch_a, "({ kind: 'pure-node', value: 41 })", &branch_b, "value")
        .unwrap();
    println!("\n[§14 V2] cross-Context heap sharing");
    println!("     branch 'what-if' reads a pure node created in 'production': value = {shared}");
    println!("     (shared by reference across branches in one Isolate, not copied)");

    // ---- §14 (V3) / §5.1 hot-swap (rebind -> observe) -----------------------
    rt.bind(&branch_a, "health", "s => s > 0.8 ? 'alarm' : 'ok'").unwrap();
    let before = rt.eval(&branch_a, "health(0.6)").unwrap();
    let t = Instant::now();
    rt.bind(&branch_a, "health", "s => s > 0.5 ? 'alarm' : 'ok'").unwrap(); // edit the threshold
    let after = rt.eval(&branch_a, "health(0.6)").unwrap();
    let dt = t.elapsed();
    println!("\n[§14 V3 / §5.1] hot-swap (rebind -> observe)");
    println!("     health(0.6): {before}  --edit threshold 0.8->0.5-->  {after}   in {:.1} us (no rebuild)",
             dt.as_nanos() as f64 / 1000.0);

    // ---- §6 governed effects: determinism + replay --------------------------
    let g1 = rt.branch("g1", true);
    let g2 = rt.branch("g2", true);
    rt.seed(&g1, 42);
    rt.seed(&g2, 42);
    let lens = "[ctx.random(), ctx.random(), ctx.random()].map(x=>x.toFixed(6)).join(',')";
    let o1 = rt.eval(&g1, lens).unwrap();
    let o2 = rt.eval(&g2, lens).unwrap();
    let g3 = rt.branch("g3", true);
    rt.seed(&g3, 7);
    let o3 = rt.eval(&g3, lens).unwrap();
    println!("\n[§6] governed effects (the only entropy door is ctx)");
    println!("     same seed across two branches identical: {}", o1 == o2);
    println!("     different seed diverges:                 {}", o1 != o3);
    println!("     recorded effect log (for replay, §8): {}", rt.effect_log(&g1));

    // ambient nondeterminism is deleted in a governed branch (§6.1)
    let ungoverned = rt.eval(&g1, "Math.random()");
    let no_date = rt.eval(&g1, "typeof Date").unwrap();
    println!("     ambient Math.random() rejected: {}", ungoverned.is_err());
    println!("     ambient Date deleted (typeof Date = {no_date})");

    println!("\nDONE — the trusted kernel (§4.1) runs on raw V8: content-addressed,");
    println!("branchable, hot-swappable, governed. This is the substrate Node cannot be.\n");
}
