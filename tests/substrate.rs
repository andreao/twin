//! Integration tests for the V8 substrate kernel (design_doc §4, §6, §14).
//!
//! Each test owns its own Runtime (its own Isolate); V8 platform init is guarded by
//! a process-global Once, so parallel test threads are fine.

use twin_runtime::Runtime;

#[test]
fn v2_cross_context_heap_sharing() {
    // §14 (V2): a pure object created in one branch is readable by reference from a
    // DIFFERENT branch in the same Isolate — pure structure shared, not copied.
    let mut rt = Runtime::new();
    let a = rt.branch("a", false);
    let b = rt.branch("b", false);
    let v = rt
        .read_shared_field(&a, "({ value: 41 })", &b, "value")
        .unwrap();
    assert_eq!(v, "41");
}

#[test]
fn v3_hot_swap_rebind_observe() {
    // §14 (V3) / §5.1: rebinding a definition changes observed behaviour live.
    let mut rt = Runtime::new();
    let br = rt.branch("main", false);
    rt.bind(&br, "f", "x => x + 1").unwrap();
    assert_eq!(rt.eval(&br, "f(10)").unwrap(), "11");
    rt.bind(&br, "f", "x => x * 100").unwrap(); // hot-swap
    assert_eq!(rt.eval(&br, "f(10)").unwrap(), "1000");
}

#[test]
fn branches_are_isolated_at_the_code_level() {
    // Two branches bind the same name to different code without interfering.
    let mut rt = Runtime::new();
    let prod = rt.branch("prod", false);
    let exp = rt.branch("experiment", false);
    rt.bind(&prod, "threshold", "0.8").unwrap();
    rt.bind(&exp, "threshold", "0.45").unwrap();
    assert_eq!(rt.eval(&prod, "threshold").unwrap(), "0.8");
    assert_eq!(rt.eval(&exp, "threshold").unwrap(), "0.45");
}

#[test]
fn governed_effects_are_deterministic_and_replayable() {
    // §6: same seed across branches -> identical; the recorded log reproduces it.
    let mut rt = Runtime::new();
    let g1 = rt.branch("g1", true);
    let g2 = rt.branch("g2", true);
    rt.seed(&g1, 123);
    rt.seed(&g2, 123);
    let prog = "[ctx.random(), ctx.uuid(), ctx.now()].join('|')";
    let o1 = rt.eval(&g1, prog).unwrap();
    let o2 = rt.eval(&g2, prog).unwrap();
    assert_eq!(o1, o2, "same seed must give byte-identical output (sound sharing)");

    let log1 = rt.effect_log(&g1);
    assert!(log1.contains("id-"), "effect log records governed values: {log1}");
}

#[test]
fn different_seed_diverges() {
    let mut rt = Runtime::new();
    let a = rt.branch("a", true);
    let b = rt.branch("b", true);
    rt.seed(&a, 1);
    rt.seed(&b, 2);
    let prog = "ctx.random().toFixed(9)";
    assert_ne!(rt.eval(&a, prog).unwrap(), rt.eval(&b, prog).unwrap());
}

#[test]
fn ambient_nondeterminism_is_governed_away() {
    // §6.1: in a governed branch, ambient entropy/clock are deleted; only ctx works.
    let mut rt = Runtime::new();
    let g = rt.branch("g", true);
    assert!(rt.eval(&g, "Math.random()").is_err(), "ungoverned entropy must throw");
    assert_eq!(rt.eval(&g, "typeof Date").unwrap(), "undefined", "ambient clock deleted");
    rt.seed(&g, 5);
    assert!(rt.eval(&g, "typeof ctx.random()").unwrap() == "number");
}
