// Workspace component-lenses (design_doc §11.6, §11.14) — the agent's surface.
//
// Like <Table> (§11.7), each is a component-lens that turns appends into the minimal
// §11.3 mutation IR against stable logical keys.  These render the agent workspace:
// a vertical Feed (agent messages / thoughts / question cards / user replies) and a
// ProfilePanel (the growing "what the twin knows about you").  Both are append-mostly,
// so their forward output is a small batch of create/setText mutations.

'use strict';

// One global, append-only mutation log shared by every workspace component.  Its
// insertion order IS the wire order (§11.3), so a fresh client replays [0..n) to
// rebuild the DOM exactly (§11.5) — no per-component merge, no sort.
const TWIN_MUT = [];

class Feed {
  constructor(root = 'feed-root') {
    this.root = root;
    this.items = [];
  }
  _emit(m) { TWIN_MUT.push(m); }

  append(item) {
    const idx = this.items.length;
    this.items.push(item);
    const key = `item:${item.seq}`;
    this._emit({ op: 'create', key, tag: 'div', parent: this.root, index: idx });
    this._emit({ op: 'setAttr', key, name: 'class', value: `feed-item ${item.kind}` });

    if (item.kind === 'view') {
      // a rich view the agent renders into the conversation: a titled card with an
      // empty body the app fills with a table / hierarchy / document lens (§11.16).
      const hk = `${key}:h`;
      this._emit({ op: 'create', key: hk, tag: 'div', parent: key, index: 0 });
      this._emit({ op: 'setAttr', key: hk, name: 'class', value: 'view-title' });
      this._emit({ op: 'setText', key: hk, text: item.text || '' });
      const bk = `${key}:body`;
      this._emit({ op: 'create', key: bk, tag: 'div', parent: key, index: 1 });
      this._emit({ op: 'setAttr', key: bk, name: 'class', value: 'view-body' });
    } else if (item.kind === 'question') {
      const qk = `${key}:q`;
      this._emit({ op: 'create', key: qk, tag: 'div', parent: key, index: 0 });
      this._emit({ op: 'setAttr', key: qk, name: 'class', value: 'q-text' });
      this._emit({ op: 'setText', key: qk, text: item.text || '' });
      (item.options || []).forEach((opt, i) => {
        const ok = `option:${item.seq}:${i}`;
        this._emit({ op: 'create', key: ok, tag: 'button', parent: key, index: i + 1 });
        this._emit({ op: 'setAttr', key: ok, name: 'class', value: 'q-option' });
        this._emit({ op: 'setText', key: ok, text: `${opt}` });
        this._emit({ op: 'listen', key: ok, type: 'click' });
      });
    } else {
      const tk = `${key}:t`;
      this._emit({ op: 'create', key: tk, tag: 'div', parent: key, index: 0 });
      this._emit({ op: 'setAttr', key: tk, name: 'class', value: 'text' });
      this._emit({ op: 'setText', key: tk, text: item.text || '' });
    }
  }
}

class ProfilePanel {
  constructor(root = 'profile-root') {
    this.root = root;
    this.fields = new Map();
  }
  _emit(m) { TWIN_MUT.push(m); }

  set(field, value) {
    if (this.fields.has(field)) {
      this.fields.set(field, value);
      this._emit({ op: 'setText', key: `pf:${field}:v`, text: `${value}` });
      return;
    }
    const i = this.fields.size;
    this.fields.set(field, value);
    const rk = `pf:${field}`;
    this._emit({ op: 'create', key: rk, tag: 'div', parent: this.root, index: i });
    this._emit({ op: 'setAttr', key: rk, name: 'class', value: 'pf-row' });
    this._emit({ op: 'create', key: `${rk}:k`, tag: 'div', parent: rk, index: 0 });
    this._emit({ op: 'setAttr', key: `${rk}:k`, name: 'class', value: 'pf-k' });
    this._emit({ op: 'setText', key: `${rk}:k`, text: `${field}` });
    this._emit({ op: 'create', key: `${rk}:v`, tag: 'div', parent: rk, index: 1 });
    this._emit({ op: 'setAttr', key: `${rk}:v`, name: 'class', value: 'pf-v' });
    this._emit({ op: 'setText', key: `${rk}:v`, text: `${value}` });
  }

  asObject() {
    const o = {};
    for (const [k, v] of this.fields) o[k] = v;
    return o;
  }
}

// The mounted/materialized data sources (the residence spectrum, §7/§9.9).  Each row
// shows a source's name, its residence badge, and its size — the twin growing.
class SourcesPanel {
  constructor(root = 'sources-root') {
    this.root = root;
    this.names = new Map();
  }
  _emit(m) { TWIN_MUT.push(m); }

  set(name, meta) {
    const resid = meta.kind === 'documents' ? `${meta.rowcount} documents`
      : meta.residence === 'mounted' ? `federated · live · ${meta.rowcount} rows`
      : meta.residence === 'mounted-partial' ? `federated · ${meta.materialized} of ${meta.rowcount} local`
      : `${meta.residence} · ${meta.rowcount} rows`;
    const detail = resid;
    if (this.names.has(name)) {
      this.names.set(name, meta);
      this._emit({ op: 'setText', key: `sr:${name}:d`, text: detail });
      return;
    }
    const i = this.names.size;
    this.names.set(name, meta);
    const rk = `sr:${name}`;
    this._emit({ op: 'create', key: rk, tag: 'div', parent: this.root, index: i });
    this._emit({ op: 'setAttr', key: rk, name: 'class', value: 'src-row' });
    this._emit({ op: 'create', key: `${rk}:n`, tag: 'div', parent: rk, index: 0 });
    this._emit({ op: 'setAttr', key: `${rk}:n`, name: 'class', value: 'src-n' });
    this._emit({ op: 'setText', key: `${rk}:n`, text: name });
    this._emit({ op: 'create', key: `${rk}:d`, tag: 'div', parent: rk, index: 1 });
    this._emit({ op: 'setAttr', key: `${rk}:d`, name: 'class', value: 'src-d' });
    this._emit({ op: 'setText', key: `${rk}:d`, text: detail });
  }
}

// ---- the agent-at-work surfaces ---------------------------------------------
// Agenda, activity, findings, and agent-authored lenses are all PURE EVENT SOURCING
// (§8): each is an append-only stream in the one graph, and these panels are folds
// over those streams (last event per key wins in the DOM; the full history stays in
// the event log).  They exist so the human can see what the agent is doing.

// The agent's own to-do list.  Events: { id, text, status } — a status change is a
// new event for the same id, folded here into the row's chip.
class AgendaPanel {
  constructor(root = 'agenda-root') {
    this.root = root;
    this.ids = new Map();
  }
  _emit(m) { TWIN_MUT.push(m); }

  set(id, text, status) {
    const rk = `ag:${id}`;
    if (this.ids.has(id)) {
      this.ids.set(id, status);
      this._emit({ op: 'setAttr', key: rk, name: 'class', value: `ag-row ${status}` });
      this._emit({ op: 'setText', key: `${rk}:s`, text: status });
      if (text) this._emit({ op: 'setText', key: `${rk}:t`, text });
      return;
    }
    const i = this.ids.size;
    this.ids.set(id, status);
    this._emit({ op: 'create', key: rk, tag: 'div', parent: this.root, index: i });
    this._emit({ op: 'setAttr', key: rk, name: 'class', value: `ag-row ${status}` });
    this._emit({ op: 'create', key: `${rk}:t`, tag: 'div', parent: rk, index: 0 });
    this._emit({ op: 'setAttr', key: `${rk}:t`, name: 'class', value: 'ag-t' });
    this._emit({ op: 'setText', key: `${rk}:t`, text: text || '' });
    this._emit({ op: 'create', key: `${rk}:s`, tag: 'span', parent: rk, index: 1 });
    this._emit({ op: 'setAttr', key: `${rk}:s`, name: 'class', value: 'ag-s' });
    this._emit({ op: 'setText', key: `${rk}:s`, text: status });
  }
}

// The agent's background work log — newest note on top, display bounded (the full
// log stays in the graph's event log).
class ActivityPanel {
  constructor(root = 'activity-root', cap = 30) {
    this.root = root;
    this.cap = cap;
    this.keys = [];
  }
  _emit(m) { TWIN_MUT.push(m); }

  add(seq, text) {
    const k = `act:${seq}`;
    this._emit({ op: 'create', key: k, tag: 'div', parent: this.root, index: 0 });
    this._emit({ op: 'setAttr', key: k, name: 'class', value: 'act-row' });
    this._emit({ op: 'setText', key: k, text });
    this.keys.push(k);
    if (this.keys.length > this.cap) this._emit({ op: 'remove', key: this.keys.shift() });
  }
}

// Data issues / insights the agent discovered, as severity-tinted cards.
class FindingsPanel {
  constructor(root = 'findings-root') {
    this.root = root;
    this.ids = new Set();
  }
  _emit(m) { TWIN_MUT.push(m); }

  set(id, severity, text, source) {
    const rk = `fnd:${id}`;
    if (this.ids.has(id)) {
      this._emit({ op: 'setText', key: `${rk}:t`, text });
      return;
    }
    this.ids.add(id);
    this._emit({ op: 'create', key: rk, tag: 'div', parent: this.root, index: 0 });
    this._emit({ op: 'setAttr', key: rk, name: 'class', value: `fnd sev-${severity}` });
    this._emit({ op: 'create', key: `${rk}:t`, tag: 'div', parent: rk, index: 0 });
    this._emit({ op: 'setText', key: `${rk}:t`, text });
    this._emit({ op: 'create', key: `${rk}:m`, tag: 'div', parent: rk, index: 1 });
    this._emit({ op: 'setAttr', key: `${rk}:m`, name: 'class', value: 'fnd-m' });
    this._emit({ op: 'setText', key: `${rk}:m`, text: source ? `${severity} · in ${source} · found by the agent` : `${severity} · found by the agent` });
  }
}

// Agent-authored lenses (§4.1: lenses are data) — the Artifacts page.  Each card
// carries the lens's full lineage: which source, what code, who authored it.
class LensPanel {
  constructor(root = 'lenses-root') {
    this.root = root;
    this.names = new Map();
  }
  _emit(m) { TWIN_MUT.push(m); }

  set(name, source, code, rowcount) {
    const rk = `ln:${name}`;
    const meta = `derived from ${source} · ${rowcount} rows · authored by the agent`;
    if (this.names.has(name)) {
      this._emit({ op: 'setText', key: `${rk}:d`, text: meta });
      this._emit({ op: 'setText', key: `${rk}:c`, text: code });
      return;
    }
    const i = this.names.size;
    this.names.set(name, true);
    this._emit({ op: 'create', key: rk, tag: 'div', parent: this.root, index: i });
    this._emit({ op: 'setAttr', key: rk, name: 'class', value: 'src-row lens-card' });
    this._emit({ op: 'create', key: `${rk}:n`, tag: 'div', parent: rk, index: 0 });
    this._emit({ op: 'setAttr', key: `${rk}:n`, name: 'class', value: 'src-n' });
    this._emit({ op: 'setText', key: `${rk}:n`, text: `lens:${name}` });
    this._emit({ op: 'create', key: `${rk}:d`, tag: 'div', parent: rk, index: 1 });
    this._emit({ op: 'setAttr', key: `${rk}:d`, name: 'class', value: 'src-d' });
    this._emit({ op: 'setText', key: `${rk}:d`, text: meta });
    this._emit({ op: 'create', key: `${rk}:c`, tag: 'div', parent: rk, index: 2 });
    this._emit({ op: 'setAttr', key: `${rk}:c`, name: 'class', value: 'lens-code' });
    this._emit({ op: 'setText', key: `${rk}:c`, text: code });
  }
}

// The installed skills (capabilities the agent can draw on), seeded by the core
// skills-loader (§4.1) from the static skills/ directory.
class SkillsPanel {
  constructor(root = 'skills-root') {
    this.root = root;
    this.names = new Map();
  }
  _emit(m) { TWIN_MUT.push(m); }

  set(name, title, description, files) {
    const filesText = files && files.length ? `tooling: ${files.join(', ')}` : '';
    if (this.names.has(name)) {
      this._emit({ op: 'setText', key: `sk:${name}:n`, text: title });
      this._emit({ op: 'setText', key: `sk:${name}:d`, text: description });
      this._emit({ op: 'setText', key: `sk:${name}:f`, text: filesText });
      return;
    }
    const i = this.names.size;
    this.names.set(name, title);
    const rk = `sk:${name}`;
    this._emit({ op: 'create', key: rk, tag: 'div', parent: this.root, index: i });
    this._emit({ op: 'setAttr', key: rk, name: 'class', value: 'src-row skill-card' });
    this._emit({ op: 'create', key: `${rk}:n`, tag: 'div', parent: rk, index: 0 });
    this._emit({ op: 'setAttr', key: `${rk}:n`, name: 'class', value: 'src-n' });
    this._emit({ op: 'setText', key: `${rk}:n`, text: title });
    this._emit({ op: 'create', key: `${rk}:d`, tag: 'div', parent: rk, index: 1 });
    this._emit({ op: 'setAttr', key: `${rk}:d`, name: 'class', value: 'src-d' });
    this._emit({ op: 'setText', key: `${rk}:d`, text: description });
    this._emit({ op: 'create', key: `${rk}:f`, tag: 'div', parent: rk, index: 2 });
    this._emit({ op: 'setAttr', key: `${rk}:f`, name: 'class', value: 'skill-files' });
    this._emit({ op: 'setText', key: `${rk}:f`, text: filesText });
  }
}
