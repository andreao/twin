// The dataflow graph: scheduler + lens catalogue (design_doc §9, C6).
//
// Runs inside V8.  The scheduler advances one revision per submit and recomputes
// the observed blast-radius in topological (insertion) order with early cutoff; an
// empty output delta stops propagation (§9.6).  Backward edits are re-entrant: at a
// plain source they re-inject as forward input; at an EXTERNAL source (a boundary
// adapter, §9.9) they land in an outbox the Rust host drains and writes through —
// the transactional-outbox pattern, avoiding reentrant host callbacks.

'use strict';

const EMPTY = new ZSet();

// ---- lens catalogue (§9.5, §9.11) -------------------------------------------
// Each lens: { arity, backwardKind, config(), forward(deltas,states), backward(edit,states) }

function calibrateLens(field, offset) {
  const fwd = (r) => r.with_({ [field]: r.get(field, 0) + offset });
  const bwd = (r) => r.with_({ [field]: r.get(field, 0) - offset });
  return {
    arity: 1, backwardKind: 'k1',
    config: () => ({ lens: 'calibrate', field, offset }),
    forward: (d) => d[0].map(fwd),
    backward: (e) => [e.map(bwd)],
  };
}

function filterLens(pred) {
  return {
    arity: 1, backwardKind: 'k1',
    config: () => ({ lens: 'filter' }),
    forward: (d) => d[0].filter(pred),
    backward: (e) => [e], // pass-through
  };
}

function mapLens(name, fwd, bwd) {
  return {
    arity: 1, backwardKind: bwd ? 'k1' : 'k4',
    config: () => ({ lens: 'map', name }),
    forward: (d) => d[0].map(fwd),
    backward: (e) => { if (!bwd) throw new Error(`${name}: no backward (k4)`); return [e.map(bwd)]; },
  };
}

// Inner join on a key with multi-port backward routing (k3, §9.5).  Forward uses
// the differential rule with the −dA⋈dB correction so a diamond (both inputs change
// in one round) does not double-count the bilinear term.
function joinLens(leftKey, rightKey, leftFields, rightFields, prefixRight = '') {
  const index = (z, key) => {
    const m = new Map();
    for (const [row, w] of z.entries()) {
      const k = row.get(key);
      if (!m.has(k)) m.set(k, []);
      m.get(k).push([row, w]);
    }
    return m;
  };
  const combine = (l, r) => {
    const o = {};
    for (const f of leftFields) o[f] = l.get(f);
    o[leftKey] = l.get(leftKey);
    for (const f of rightFields) o[prefixRight + f] = r.get(f);
    return rec(o);
  };
  const join = (left, right) => {
    const ridx = index(right, rightKey);
    const out = new ZSet();
    for (const [lrow, lw] of left.entries()) {
      const matches = ridx.get(lrow.get(leftKey)) || [];
      for (const [rrow, rw] of matches) out._add(combine(lrow, rrow), lw * rw);
    }
    return out;
  };
  return {
    arity: 2, backwardKind: 'k3',
    config: () => ({ lens: 'join', leftKey, rightKey, leftFields, rightFields }),
    forward: (deltas, states) => {
      const [dA, dB] = deltas, [A, B] = states;
      let out = EMPTY;
      if (!dA.isEmpty()) out = out.plus(join(dA, B));
      if (!dB.isEmpty()) out = out.plus(join(A, dB));
      if (!dA.isEmpty() && !dB.isEmpty()) out = out.minus(join(dA, dB));
      return out;
    },
    backward: (edit) => {
      const leftDelta = new ZSet(), rightDelta = new ZSet();
      for (const [row, w] of edit.entries()) {
        const l = {}; for (const f of leftFields) l[f] = row.get(f); l[leftKey] = row.get(leftKey);
        leftDelta._add(rec(l), w);
        const r = {}; for (const f of rightFields) r[f] = row.get(prefixRight + f); r[rightKey] = row.get(leftKey);
        rightDelta._add(rec(r), w);
      }
      return [leftDelta, rightDelta];
    },
  };
}

// Linear aggregates (§3): maintained per signed delta, O(groups).
function countLens(groupField, countField = 'count') {
  const counts = new Map();
  return {
    arity: 1, backwardKind: 'k4',
    config: () => ({ lens: 'count', groupField }),
    forward: (d) => {
      const out = new ZSet(), touched = new Map();
      for (const [row, w] of d[0].entries()) {
        const g = row.get(groupField);
        if (!touched.has(g)) touched.set(g, counts.get(g) || 0);
        counts.set(g, (counts.get(g) || 0) + w);
      }
      for (const [g, old] of touched) {
        const now = counts.get(g) || 0;
        if (old === now) continue;
        if (old > 0) out._add(rec({ [groupField]: g, [countField]: old }), -1);
        if (now > 0) out._add(rec({ [groupField]: g, [countField]: now }), 1);
      }
      return out;
    },
  };
}

// Holistic aggregate: top-k, O(rows) state, no backward (k4, §9.5).
function topKLens(sortField, k, descending = true) {
  let rows = new Map(), last = [];
  return {
    arity: 1, backwardKind: 'k4',
    config: () => ({ lens: 'topk', sortField, k }),
    forward: (d) => {
      for (const [row, w] of d[0].entries()) {
        const key = zkey(row), cur = (rows.get(key)?.[1] || 0) + w;
        if (cur <= 0) rows.delete(key); else rows.set(key, [row, cur]);
      }
      const ranked = [...rows.values()].map(([r]) => r)
        .sort((a, b) => (a.get(sortField, 0) - b.get(sortField, 0)) * (descending ? -1 : 1))
        .slice(0, k);
      const out = ZSet.fromRows(ranked).minus(ZSet.fromRows(last));
      last = ranked;
      return out;
    },
  };
}

// ---- the graph / scheduler --------------------------------------------------
class Graph {
  constructor(branch = 'main') {
    this.branch = branch;
    this.nodes = [];       // insertion order == a topological order
    this.revision = 0;
    this.log = [];         // committed events (mirrored to the Rust EventLog)
    this.outbox = [];      // external write-throughs for the host to drain (§9.9)
    this.suppress = new Map();
    this.hostMutations = 0; // count of mutations pushed to hosts (B1 instrumentation)
    this.forwardEvals = 0;
  }

  source(stream, external = false) {
    const n = { id: this.nodes.length, lens: null, inputs: [], stream, state: EMPTY,
      subscribers: [], observers: [], external };
    this.nodes.push(n);
    return n.id;
  }
  apply(lens, inputs, stream) {
    if (inputs.length !== lens.arity) throw new Error(`arity: ${stream}`);
    const n = { id: this.nodes.length, lens, inputs, stream: stream || lens.config().lens,
      state: EMPTY, subscribers: [], observers: [], external: false };
    for (const i of inputs) this.nodes[i].subscribers.push(n.id);
    this.nodes.push(n);
    return n.id;
  }
  observe(id, cb) { this.nodes[id].observers.push(cb); }
  expectEcho(key, count = 1) { this.suppress.set(key, (this.suppress.get(key) || 0) + count); }

  _isEcho(prov) {
    const k = prov.idempotencyKey;
    if (k && (this.suppress.get(k) || 0) > 0) { this.suppress.set(k, this.suppress.get(k) - 1); return true; }
    return false;
  }

  submit(srcId, delta, prov) {
    if (delta.isEmpty()) return [];
    if (prov.origin === 'echo' || this._isEcho(prov)) return [];
    this.revision += 1;
    const ts = [this.revision, 0];
    const src = this.nodes[srcId];
    const ev = { seq: this.log.length + 1, ts, stream: src.stream, provenance: prov,
      note: prov.note || '', delta: delta.toJSON() };
    this.log.push(ev);

    const round = new Map([[srcId, delta]]);
    src.state = src.state.plus(delta);
    for (const cb of src.observers) cb(delta);

    for (const node of this.nodes) {
      if (node.lens === null) continue;
      const inDeltas = node.inputs.map(i => round.get(i) || EMPTY);
      if (inDeltas.every(d => d.isEmpty())) continue;
      const inStates = node.inputs.map(i => this.nodes[i].state);
      this.forwardEvals += 1;
      const out = node.lens.forward(inDeltas, inStates);
      if (out.isEmpty()) continue; // early cutoff (§3)
      node.state = node.state.plus(out);
      round.set(node.id, out);
      for (const cb of node.observers) cb(out);
    }
    return [ev];
  }

  pushBackward(nodeId, edit, prov) {
    const node = this.nodes[nodeId];
    if (node.lens === null) {
      if (node.external) { this.outbox.push({ stream: node.stream, edit: edit.toJSON(), prov }); return; }
      this.submit(nodeId, edit, prov); return;
    }
    const inDeltas = node.lens.backward(edit, node.inputs.map(i => this.nodes[i].state));
    node.inputs.forEach((inp, k) => { if (!inDeltas[k].isEmpty()) this.pushBackward(inp, inDeltas[k], prov); });
  }

  drainOutbox() { const o = this.outbox; this.outbox = []; return o; }
  stateOf(id) { return this.nodes[id].state; }
}
