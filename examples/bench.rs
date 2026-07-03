//! Reproduce the §17 validated invariants on the Rust + raw-V8 stack.
//!
//!   cargo run --example bench --release
//!
//! Not the exact prototype numbers, but the same invariants: edit cost = blast
//! radius; 0 downstream on no-op output; millions of 0-copy branches; hot-swap in
//! microseconds; governed determinism; table mutations = output delta.

use std::time::Instant;

use twin_runtime::engine::{Engine, Value};
use twin_runtime::jsgraph::JsGraph;
use twin_runtime::layers::{LayeredStore, BRANCH};
use twin_runtime::runtime::Runtime;

// a tiny deterministic PRNG (no rand dep; avoids ungoverned entropy anyway)
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }
}

fn bench_a1() {
    println!("\nA1  edit cost = blast radius  (claim: ~3% recompute; 0 on no-op output)");
    let e = Engine::new();
    let inputs: Vec<_> = (0..60).map(|i| e.input(Value::Num((i % 97 + 1) as f64))).collect();
    let mut rng = Lcg(1);
    let mut derived = Vec::new();
    for _ in 0..2000 {
        let a = inputs[rng.next(60)].clone();
        let b = inputs[rng.next(60)].clone();
        let c = inputs[rng.next(60)].clone();
        derived.push(e.derived(move |eng| {
            let s = [&a, &b, &c].iter().map(|n| match eng.get(n) { Value::Num(x) => x, _ => 0.0 }).sum();
            Value::Num(s)
        }));
    }
    for d in &derived {
        e.get(d);
    }
    let total = inputs.len() + derived.len();
    e.recompute_count.set(0);
    let cur = match e.get(&inputs[0]) { Value::Num(x) => x, _ => 0.0 };
    e.set(&inputs[0], Value::Num(cur + 1.0));
    for d in &derived {
        e.get(d);
    }
    let rc = e.recompute_count.get();
    println!("    graph {total} nodes; one input edited -> {rc} recomputed ({:.1}%)",
             100.0 * rc as f64 / total as f64);

    // no-op-output edit
    let e2 = Engine::new();
    let a = e2.input(Value::Num(5.0));
    let a2 = a.clone();
    let sign = e2.derived(move |eng| Value::Text(if matches!(eng.get(&a2), Value::Num(x) if x > 0.0) { "pos".into() } else { "neg".into() }));
    let mut chain = sign;
    for _ in 0..20 {
        let prev = chain.clone();
        chain = e2.derived(move |eng| eng.get(&prev));
    }
    e2.get(&chain);
    e2.recompute_count.set(0);
    e2.set(&a, Value::Num(9999.0)); // still positive -> sign unchanged
    e2.get(&chain);
    println!("    no-op-output edit (5 -> 9999): {} nodes downstream recomputed (early cutoff)",
             e2.recompute_count.get() - 1);
}

fn bench_overlay() {
    println!("\noverlay  millions of 0-copy branches  (claim: 1M @ ~2.8 KB, 0.7 us fork)");
    let mut store = LayeredStore::new();
    for i in 0..1000 {
        store.write_shared(&format!("asset:{i}"), i);
    }
    let base = store.branch("base");
    let n = 200_000;
    let t = Instant::now();
    let mut branches = Vec::with_capacity(n);
    for i in 0..n {
        let mut b = base.fork(&format!("b{i}"));
        b.write(&format!("asset:{}", i % 1000), 999_999, BRANCH); // one divergent write
        branches.push(b);
    }
    let per = t.elapsed().as_nanos() as f64 / n as f64 / 1000.0;
    println!("    forked {n} branches over a shared {}-row base @ {per:.3} us/fork", store.shared_len());
    // isolation: each branch sees its own write; base unchanged
    assert_eq!(branches[5].get("asset:5"), Some(999_999));
    assert_eq!(base.get("asset:5"), Some(5));
    println!("    isolation verified: base row unchanged; each branch diverges by one write");
}

fn bench_hot_swap() {
    println!("\nhot-swap  rebind -> observe  (claim: 42 us)");
    let mut rt = Runtime::new();
    let br = rt.branch("main", false);
    rt.bind(&br, "f", "x => x + 1").unwrap();
    let _ = rt.eval(&br, "f(10)");
    let t = Instant::now();
    rt.bind(&br, "f", "x => x * 100").unwrap();
    let after = rt.eval(&br, "f(10)").unwrap();
    println!("    edit -> observe f(10)={after} in {:.1} us", t.elapsed().as_nanos() as f64 / 1000.0);
}

fn bench_effects() {
    println!("\neffects  governed determinism  (claim: identical across branches; replay exact)");
    let mut rt = Runtime::new();
    let g1 = rt.branch("g1", true);
    let g2 = rt.branch("g2", true);
    rt.seed(&g1, 42);
    rt.seed(&g2, 42);
    let prog = "[ctx.random(), ctx.now(), ctx.uuid()].join('|')";
    let o1 = rt.eval(&g1, prog).unwrap();
    let o2 = rt.eval(&g2, prog).unwrap();
    println!("    same seed across two branches identical: {}", o1 == o2);
}

fn bench_table() {
    println!("\nB1/C2  table mutations = output delta  (claim: median 0/edit; 1 per visible change)");
    let mut g = JsGraph::new_dataflow();
    // build a table + source in JS, insert N rows, then do random edits and measure
    let js = r#"
      const G = new Graph();
      const s = G.source('s');
      const t = new Table(['id','v'], {idField:'id', sortField:'id', window:[0,50]});
      G.observe(s, d => { G.hostMutations += t.forward(d); });
      const N = 5000;
      const rows = [];
      for (let i=0;i<N;i++) rows.push(rec({id:'r'+String(i).padStart(4,'0'), v:i}));
      G.submit(s, ZSet.insert(...rows), {origin:'local',author:'b'});
      let s0 = 6364136223846793005n, counts = [];
      const rnd = (n)=>{ s0=(s0*6364136223846793005n+1n)&((1n<<64n)-1n); return Number((s0>>33n)%BigInt(n)); };
      for (let k=0;k<1000;k++){ const i=rnd(N); const old=rows[i]; const nw=old.with_({v:old.get('v')+1}); rows[i]=nw;
        const before=G.hostMutations; G.submit(s, ZSet.update(old,nw), {origin:'local',author:'b'}); counts.push(G.hostMutations-before); }
      counts.sort((a,b)=>a-b);
      const median = counts[counts.length>>1];
      const before=G.hostMutations; const v=rows[0]; G.submit(s, ZSet.update(v, v.with_({v:v.get('v')+1})), {origin:'local',author:'b'});
      const visible = G.hostMutations-before;
      JSON.stringify({median, visible})
    "#;
    let out = g.call(js);
    let m: serde_json::Value = serde_json::from_str(&out).unwrap();
    println!("    5000 rows, 50-row window, 1000 random edits: median {} mutations/edit", m["median"]);
    println!("    one visible change -> {} setText mutation", m["visible"]);
}

fn main() {
    println!("========================================================================");
    println!("BENCHMARKS — §17 invariants on Rust + raw V8");
    println!("========================================================================");
    bench_a1();
    bench_overlay();
    bench_hot_swap();
    bench_effects();
    bench_table();
    println!();
}
