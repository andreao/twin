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

    if (item.kind === 'question') {
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
