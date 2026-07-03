// The incremental <Table> (design_doc §11.7, §11.14) — a component-lens in V8.
//
// An incremental materialized index keyed by (sortKey, id) plus a window of the top
// K rows.  A data update re-evaluates one record, updates the index, recomputes the
// window, and emits only the implied mutations against stable logical keys (§11.5) —
// cost in mutations independent of total rows (B1/C2).

'use strict';

class Table {
  constructor(columns, { idField = 'id', sortField = null, descending = false,
    window = [0, 50], root = 'table' } = {}) {
    this.columns = columns;
    this.idField = idField;
    this.sortField = sortField || idField;
    this.descending = descending;
    [this.winOffset, this.winLimit] = window;
    this.root = root;
    this.rows = new Map();      // id -> Record
    this.order = [];            // sorted [sortKey, id]
    this.mounted = new Map();   // id -> window index
    this.text = new Map();      // cell key -> last text (to suppress no-op setText)
    this.mutations = [];        // recorded mutation stream
  }

  _sk(r) { return [r.get(this.sortField), r.get(this.idField)]; }
  _cmp(a, b) { return a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : (a[1] < b[1] ? -1 : a[1] > b[1] ? 1 : 0); }
  _pos(sk) {
    let lo = 0, hi = this.order.length;
    while (lo < hi) { const m = (lo + hi) >> 1; if (this._cmp(this.order[m], sk) < 0) lo = m + 1; else hi = m; }
    return lo;
  }
  _windowIds() {
    const lo = this.winOffset, hi = this.winOffset + this.winLimit;
    let view = this.descending ? [...this.order].reverse() : this.order;
    return view.slice(lo, hi).map(([, id]) => id);
  }

  // forward: a row-set delta -> minimal mutations
  forward(delta) {
    const entries = delta.entries().sort((a, b) => a[1] - b[1]); // deletes (-1) first
    for (const [row, w] of entries) {
      const id = row.get(this.idField);
      if (w < 0) {
        const old = this.rows.get(id);
        if (old) { const i = this._pos(this._sk(old)); if (i < this.order.length && this._cmp(this.order[i], this._sk(old)) === 0) this.order.splice(i, 1); this.rows.delete(id); }
      } else {
        const old = this.rows.get(id);
        if (old) { const i = this._pos(this._sk(old)); if (i < this.order.length && this._cmp(this.order[i], this._sk(old)) === 0) this.order.splice(i, 1); }
        this.rows.set(id, row);
        const sk = this._sk(row); this.order.splice(this._pos(sk), 0, sk);
      }
    }
    return this._reconcile();
  }

  _rowKey(id) { return `row:${id}`; }
  _cellKey(id, col) { return `cell:${id}:${col}`; }
  _fmt(v) { return v === null || v === undefined ? '' : String(v); }

  _emit(m) {
    if (m.op === 'setText') { if (this.text.get(m.key) === m.text) return; this.text.set(m.key, m.text); }
    else if (m.op === 'remove') this.text.delete(m.key);
    this.mutations.push(m);
  }

  _reconcile() {
    const before = this.mutations.length;
    const newIds = this._windowIds(), newSet = new Set(newIds);
    for (const id of [...this.mounted.keys()]) if (!newSet.has(id)) { this._emit({ op: 'remove', key: this._rowKey(id) }); this.mounted.delete(id); }
    newIds.forEach((id, idx) => {
      const row = this.rows.get(id), rkey = this._rowKey(id);
      if (!this.mounted.has(id)) {
        this._emit({ op: 'create', key: rkey, tag: 'tr', parent: this.root, index: idx });
        this.columns.forEach((col, ci) => { const ck = this._cellKey(id, col); this._emit({ op: 'create', key: ck, tag: 'td', parent: rkey, index: ci }); this._emit({ op: 'setText', key: ck, text: this._fmt(row.get(col)) }); });
      } else {
        if (this.mounted.get(id) !== idx) this._emit({ op: 'move', key: rkey, parent: this.root, index: idx });
        for (const col of this.columns) this._emit({ op: 'setText', key: this._cellKey(id, col), text: this._fmt(row.get(col)) });
      }
      this.mounted.set(id, idx);
    });
    return this.mutations.length - before; // number of mutations emitted this round
  }

  // backward: a cell-edit -> a Z-set update on the owning row (§11.7)
  cellEditToDelta(id, col, value) {
    const old = this.rows.get(id);
    return ZSet.update(old, old.with_({ [col]: value }));
  }

  render() {
    return this._windowIds().map(id => {
      const r = this.rows.get(id);
      return this.columns.map(c => this._fmt(r.get(c))).join(' | ');
    });
  }
  visibleRows() { return this._windowIds().map(id => this.rows.get(id).asObject()); }
  rowCount() { return this.rows.size; }
}
