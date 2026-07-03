//! Lens-catalogue and scheduler invariants, exercised directly in the V8 runtime
//! (design_doc §9).  Each test runs a JS scenario that returns "true"/"false".

use twin_runtime::jsgraph::JsGraph;

fn check(scenario: &str) -> String {
    let mut g = JsGraph::new_dataflow();
    g.call(scenario)
}

#[test]
fn calibrate_roundtrip_k1() {
    // §9.10 round-trip property: backward(forward(x)) == x for an invertible lens.
    let out = check(r#"
        const l = calibrateLens('temp', 3.0);
        const r = rec({id:'t1', temp: 10});
        const fwd = l.forward([ZSet.insert(r)], [new ZSet()]);
        const bwd = l.backward(fwd, [new ZSet()]);
        String(bwd[0].equals(ZSet.insert(r)))
    "#);
    assert_eq!(out, "true");
}

#[test]
fn join_diamond_no_double_count() {
    // A diamond: one source feeds BOTH join inputs, so a single edit changes both
    // inputs in one round.  The join must equal the brute-force result (the −dA⋈dB
    // correction prevents double-counting the bilinear term).
    let out = check(r#"
        const G = new Graph();
        const s = G.source('s');
        const L = G.apply(mapLens('L', r => rec({k:r.get('k'), lname:r.get('v')})), [s]);
        const R = G.apply(mapLens('R', r => rec({k:r.get('k'), rval:r.get('v')*10})), [s]);
        const j = G.apply(joinLens('k','k',['lname'],['rval'],''), [L, R]);
        const prov = {origin:'local', author:'t'};
        const brute = () => {
          const out = new ZSet(); const ridx = new Map();
          for (const [row,w] of G.stateOf(R).entries()) { const k=row.get('k'); if(!ridx.has(k))ridx.set(k,[]); ridx.get(k).push([row,w]); }
          for (const [lrow,lw] of G.stateOf(L).entries()) for (const [rrow,rw] of (ridx.get(lrow.get('k'))||[]))
            out._add(rec({k:lrow.get('k'), lname:lrow.get('lname'), rval:rrow.get('rval')}), lw*rw);
          return out;
        };
        G.submit(s, ZSet.insert(rec({id:'a', k:1, v:7})), prov);
        const ok1 = G.stateOf(j).equals(brute());
        G.submit(s, ZSet.update(rec({id:'a',k:1,v:7}), rec({id:'a',k:1,v:9})), prov);
        const ok2 = G.stateOf(j).equals(brute());
        const val = G.stateOf(j).support()[0].get('rval');
        String(ok1 && ok2 && val === 90)
    "#);
    assert_eq!(out, "true");
}

#[test]
fn count_aggregate_signed() {
    let out = check(r#"
        const l = countLens('site');
        const o1 = l.forward([ZSet.insert(rec({id:'a',site:'N'}), rec({id:'b',site:'N'}))], [new ZSet()]);
        const a = o1.weight(rec({site:'N', count:2})) === 1;
        const o2 = l.forward([ZSet.remove(rec({id:'a',site:'N'}))], [new ZSet()]);
        const b = o2.weight(rec({site:'N', count:2})) === -1 && o2.weight(rec({site:'N', count:1})) === 1;
        String(a && b)
    "#);
    assert_eq!(out, "true");
}

#[test]
fn topk_holistic_redderives_next() {
    let out = check(r#"
        const l = topKLens('score', 2);
        const d1 = l.forward([ZSet.insert(rec({id:'a',score:1}), rec({id:'b',score:5}), rec({id:'c',score:3}))], [new ZSet()]);
        // top-2 by score desc is {b,c}
        const top1 = d1.weight(rec({id:'b',score:5})) === 1 && d1.weight(rec({id:'c',score:3})) === 1;
        // retract the current top (b) -> the next one (a) is re-derived into the window
        const d2 = l.forward([ZSet.remove(rec({id:'b',score:5}))], [new ZSet()]);
        const rederive = d2.weight(rec({id:'a',score:1})) === 1 && d2.weight(rec({id:'b',score:5})) === -1;
        String(top1 && rederive)
    "#);
    assert_eq!(out, "true");
}

#[test]
fn scheduler_early_cutoff_empty_delta_stops() {
    // A filtered-out edit produces an empty delta downstream of the filter -> the
    // observer is not called (early cutoff, §3/§9.6).
    let out = check(r#"
        const G = new Graph();
        const s = G.source('s');
        const cal = G.apply(calibrateLens('temp', 2.0), [s]);
        const hot = G.apply(filterLens(r => r.get('temp') > 50), [cal]);
        let seen = 0;
        G.observe(hot, () => { seen++; });
        G.submit(s, ZSet.insert(rec({id:'t1', temp:60})), {origin:'local',author:'t'}); // 62 passes
        const a = seen === 1;
        G.submit(s, ZSet.insert(rec({id:'t2', temp:10})), {origin:'local',author:'t'}); // 12 filtered
        const b = seen === 1; // no new observer call (early cutoff below the filter)
        String(a && b)
    "#);
    assert_eq!(out, "true");
}

#[test]
fn join_backward_routes_to_owning_upstream_k3() {
    let out = check(r#"
        const G = new Graph();
        const assets = G.source('assets');
        const readings = G.source('readings');
        const j = G.apply(joinLens('id','asset',['name'],['temp'],''), [assets, readings]);
        const prov = {origin:'local', author:'t'};
        G.submit(assets, ZSet.insert(rec({id:'p1', name:'Pump 1'})), prov);
        G.submit(readings, ZSet.insert(rec({asset:'p1', temp:70})), prov);
        const joined = G.stateOf(j).support()[0];
        G.pushBackward(j, ZSet.update(joined, joined.with_({temp:99})), prov);
        const after = G.stateOf(readings).support();
        String(after.some(r => r.get('temp') === 99))
    "#);
    assert_eq!(out, "true");
}
