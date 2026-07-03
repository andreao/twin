// Z-sets and records: the delta algebra (design_doc §9.7, C5).
//
// This runs INSIDE V8 as a content-addressed definition (§4.1).  Deltas are V8
// objects here, so lens transforms manipulate them with zero marshalling — the
// whole reason the runtime is V8-native and not a Rust engine that serializes
// values across an FFI boundary (§14 rationale).
//
// A Z-set is a multiset whose rows carry signed integer weights: insert +1,
// delete -1, update = retract+assert.  Signed weights keep aggregation, distinct,
// and deletion correct (non-monotone operators).  Zero-weight rows are dropped, so
// a delta that nets to nothing IS empty — the §3 early-cutoff signal in delta form.

'use strict';

// ---- Record: an immutable row with a cached, value-based key ----------------
class Record {
  constructor(fields) {
    // fields: plain object; store sorted entries for a stable key
    this._f = fields;
    this._key = undefined;
  }
  static of(fields) { return new Record(fields); }
  get(k, dflt = undefined) { return k in this._f ? this._f[k] : dflt; }
  with_(changes) { return new Record(Object.assign({}, this._f, changes)); }
  without(...keys) {
    const o = {};
    for (const k of Object.keys(this._f)) if (!keys.includes(k)) o[k] = this._f[k];
    return new Record(o);
  }
  fields() { return Object.keys(this._f); }
  asObject() { return Object.assign({}, this._f); }
  // stable canonical key: sorted key:JSON(value) pairs
  zkey() {
    if (this._key === undefined) {
      const ks = Object.keys(this._f).sort();
      let s = 'R';
      for (const k of ks) s += '|' + k + '=' + JSON.stringify(this._f[k]);
      this._key = s;
    }
    return this._key;
  }
  toString() {
    return '{' + Object.keys(this._f).sort().map(k => `${k}=${this._f[k]}`).join(', ') + '}';
  }
}

function rec(fields) { return new Record(fields); }

function zkey(row) {
  if (row instanceof Record) return row.zkey();
  return typeof row + ':' + JSON.stringify(row);
}

// ---- ZSet: Map<key, [row, weight]>, weight !== 0 invariant ------------------
class ZSet {
  constructor() { this._w = new Map(); }

  _add(row, weight) {
    if (weight === 0) return;
    const k = zkey(row);
    const cur = this._w.get(k);
    if (cur === undefined) { this._w.set(k, [row, weight]); }
    else {
      const nw = cur[1] + weight;
      if (nw === 0) this._w.delete(k);
      else cur[1] = nw;
    }
  }

  static fromRows(rows, weight = 1) {
    const z = new ZSet();
    for (const r of rows) z._add(r, weight);
    return z;
  }
  static insert(...rows) { return ZSet.fromRows(rows, 1); }
  static remove(...rows) { return ZSet.fromRows(rows, -1); }
  static update(oldRow, newRow) {
    const z = new ZSet(); z._add(oldRow, -1); z._add(newRow, 1); return z;
  }

  plus(other) {
    const out = new ZSet();
    for (const [, [row, w]] of this._w) out._add(row, w);
    for (const [, [row, w]] of other._w) out._add(row, w);
    return out;
  }
  neg() {
    const out = new ZSet();
    for (const [k, [row, w]] of this._w) out._w.set(k, [row, -w]);
    return out;
  }
  minus(other) { return this.plus(other.neg()); }

  filter(pred) {
    const out = new ZSet();
    for (const [k, [row, w]] of this._w) if (pred(row)) out._w.set(k, [row, w]);
    return out;
  }
  map(fn) {
    const out = new ZSet();
    for (const [, [row, w]] of this._w) out._add(fn(row), w);
    return out;
  }
  distinct() {
    const out = new ZSet();
    for (const [k, [row, w]] of this._w) if (w > 0) out._w.set(k, [row, 1]);
    return out;
  }

  weight(row) { const c = this._w.get(zkey(row)); return c === undefined ? 0 : c[1]; }
  support() { const o = []; for (const [, [row, w]] of this._w) if (w > 0) o.push(row); return o; }
  entries() { return [...this._w.values()].map(([row, w]) => [row, w]); }
  isEmpty() { return this._w.size === 0; }
  get size() { return this._w.size; }
  equals(other) {
    if (this._w.size !== other._w.size) return false;
    for (const [k, [, w]] of this._w) { const o = other._w.get(k); if (!o || o[1] !== w) return false; }
    return true;
  }
  toString() {
    return 'ZSet({' + [...this._w.values()].map(([r, w]) => `${r}:${w > 0 ? '+' : ''}${w}`).sort().join(', ') + '})';
  }

  // JSON <-> ZSet at the Rust boundary (adapter/host serialization, §9.9).
  toJSON() {
    return [...this._w.values()].map(([row, w]) => [row instanceof Record ? row.asObject() : row, w]);
  }
  static fromJSON(arr) {
    const z = new ZSet();
    for (const [obj, w] of arr) z._add(obj && typeof obj === 'object' ? new Record(obj) : obj, w);
    return z;
  }
}
