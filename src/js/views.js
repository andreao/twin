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
    this._emit({ op: 'setAttr', key, name: 'class', value: `feed-item ${item.kind}${item.tone ? ' ' + item.tone : ''}` });

    if (item.kind === 'card') {
      // a compact action card: work the agent (or the system) DID, as chat history —
      // title (openable when it points somewhere) + one quiet detail line.
      const hk = `${key}:h`;
      this._emit({ op: 'create', key: hk, tag: 'div', parent: key, index: 0 });
      this._emit({ op: 'setAttr', key: hk, name: 'class', value: 'card-title' });
      this._emit({ op: 'setText', key: hk, text: item.text || '' });
      const dk = `${key}:d`;
      this._emit({ op: 'create', key: dk, tag: 'div', parent: key, index: 1 });
      this._emit({ op: 'setAttr', key: dk, name: 'class', value: 'card-sub' });
      this._emit({ op: 'setText', key: dk, text: item.sub || '' });
      const mk = `${key}:m`;
      this._emit({ op: 'create', key: mk, tag: 'div', parent: key, index: 2 });
      this._emit({ op: 'setAttr', key: mk, name: 'class', value: 'card-meta' });
      this._emit({ op: 'setText', key: mk, text: item.meta || '' });
    } else if (item.kind === 'view') {
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

// "About you" is a TILE on the twin board like everything else — it does not exist
// until the agent learns the first fact, then it grows.  The twin starts empty.
class ProfilePanel {
  constructor(root = 'board-root') {
    this.root = root;
    this.fields = new Map();
    this.made = false;
  }
  _emit(m) { TWIN_MUT.push(m); }

  _tile() {
    if (this.made) return;
    this.made = true;
    this._emit({ op: 'create', key: 'tile:you', tag: 'div', parent: this.root, index: 0 });
    this._emit({ op: 'setAttr', key: 'tile:you', name: 'class', value: 'tile you-tile' });
    this._emit({ op: 'create', key: 'tile:you:h', tag: 'div', parent: 'tile:you', index: 0 });
    this._emit({ op: 'setAttr', key: 'tile:you:h', name: 'class', value: 'tile-h' });
    this._emit({ op: 'setText', key: 'tile:you:h', text: 'About you' });
    this._emit({ op: 'create', key: 'tile:you:b', tag: 'div', parent: 'tile:you', index: 1 });
  }

  set(field, value) {
    this._tile();
    if (this.fields.has(field)) {
      this.fields.set(field, value);
      this._emit({ op: 'setText', key: `pf:${field}:v`, text: `${value}` });
      return;
    }
    const i = this.fields.size;
    this.fields.set(field, value);
    const rk = `pf:${field}`;
    this._emit({ op: 'create', key: rk, tag: 'div', parent: 'tile:you:b', index: i });
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
  constructor(root = 'board-root') {
    this.root = root;
    this.names = new Map();
  }
  _emit(m) { TWIN_MUT.push(m); }

  set(name, meta) {
    const resid = meta.kind === 'documents' ? `${meta.rowcount} documents`
      : meta.residence === 'mounted' ? `federated · live · ${meta.rowcount} rows`
      : meta.residence === 'mounted-partial' ? `federated · ${meta.materialized} of ${meta.rowcount} local`
      : `${meta.residence} · ${meta.rowcount} rows`;
    const title = meta.title || name;
    const desc = meta.description || '';
    const rk = `sr:${name}`;
    if (this.names.has(name)) {
      this.names.set(name, meta);
      this._emit({ op: 'setAttr', key: rk, name: 'data-title', value: title });
      this._emit({ op: 'setText', key: `${rk}:n`, text: title });
      this._emit({ op: 'setText', key: `${rk}:desc`, text: desc });
      this._emit({ op: 'setText', key: `${rk}:d`, text: resid });
      return;
    }
    const i = this.names.size;
    this.names.set(name, meta);
    this._emit({ op: 'create', key: rk, tag: 'div', parent: this.root, index: i });
    this._emit({ op: 'setAttr', key: rk, name: 'class', value: 'src-row' });
    this._emit({ op: 'setAttr', key: rk, name: 'data-title', value: title });
    this._emit({ op: 'create', key: `${rk}:n`, tag: 'div', parent: rk, index: 0 });
    this._emit({ op: 'setAttr', key: `${rk}:n`, name: 'class', value: 'src-n' });
    this._emit({ op: 'setText', key: `${rk}:n`, text: title });
    this._emit({ op: 'create', key: `${rk}:desc`, tag: 'div', parent: rk, index: 1 });
    this._emit({ op: 'setAttr', key: `${rk}:desc`, name: 'class', value: 'src-desc' });
    this._emit({ op: 'setText', key: `${rk}:desc`, text: desc });
    this._emit({ op: 'create', key: `${rk}:d`, tag: 'div', parent: rk, index: 2 });
    this._emit({ op: 'setAttr', key: `${rk}:d`, name: 'class', value: 'src-d' });
    this._emit({ op: 'setText', key: `${rk}:d`, text: resid });
  }
}

// ---- the agent-at-work surfaces ---------------------------------------------
// Agenda, activity, findings, and agent-authored lenses are all PURE EVENT SOURCING
// (§8): append-only streams in the one graph, folded here for the human.  The UI has
// exactly TWO concepts — the conversation and the twin (a board of lenses that grows
// as the agent works) — so these folds render either as board tiles or as the single
// one-line "now" strip above the board.  Full history stays in the event log.

// The agenda folds to ONE line in the now-strip: the active (or next open) item.
class AgendaPanel {
  constructor(key = 'agent-plan') {
    this.key = key;
    this.items = new Map();
  }

  set(id, text, status) {
    const prev = this.items.get(id) || {};
    this.items.set(id, { text: text || prev.text || '', status });
    const all = [...this.items.values()];
    const open = all.filter((i) => i.status !== 'done');
    const active = all.find((i) => i.status === 'active');
    // the strip's activity slot already says what's happening NOW, so this line
    // shows what comes NEXT (the open items beyond the active one)
    const next = open.filter((i) => i !== active);
    const line = next.length
      ? `next: ${next[0].text}${next.length > 1 ? `  (+${next.length - 1})` : ''}`
      : (all.length && !open.length ? 'plan: all done' : '');
    TWIN_MUT.push({ op: 'setText', key: this.key, text: line });
  }
}

// The work log folds to the LATEST note in the now-strip (history is in the graph).
class ActivityPanel {
  constructor(key = 'agent-act') {
    this.key = key;
  }
  add(seq, text) {
    TWIN_MUT.push({ op: 'setText', key: this.key, text });
  }
}

// Findings are a TILE on the board — it appears with the first discovery and its
// title carries the count; cards inside are severity-tinted, newest first.
class FindingsPanel {
  constructor(root = 'board-root') {
    this.root = root;
    this.ids = new Set();
    this.made = false;
  }
  _emit(m) { TWIN_MUT.push(m); }

  _tile() {
    if (this.made) return;
    this.made = true;
    this._emit({ op: 'create', key: 'tile:findings', tag: 'div', parent: this.root, index: 0 });
    this._emit({ op: 'setAttr', key: 'tile:findings', name: 'class', value: 'tile findings-tile' });
    this._emit({ op: 'create', key: 'tile:findings:h', tag: 'div', parent: 'tile:findings', index: 0 });
    this._emit({ op: 'setAttr', key: 'tile:findings:h', name: 'class', value: 'tile-h' });
    this._emit({ op: 'create', key: 'tile:findings:b', tag: 'div', parent: 'tile:findings', index: 1 });
    this._emit({ op: 'setAttr', key: 'tile:findings:b', name: 'class', value: 'fnd-list' });
  }

  set(id, severity, text, source) {
    this._tile();
    const rk = `fnd:${id}`;
    if (this.ids.has(id)) {
      this._emit({ op: 'setText', key: `${rk}:t`, text });
      return;
    }
    this.ids.add(id);
    this._emit({ op: 'setText', key: 'tile:findings:h', text: `Findings — ${this.ids.size}` });
    this._emit({ op: 'create', key: rk, tag: 'div', parent: 'tile:findings:b', index: 0 });
    this._emit({ op: 'setAttr', key: rk, name: 'class', value: `fnd sev-${severity}` });
    this._emit({ op: 'create', key: `${rk}:t`, tag: 'div', parent: rk, index: 0 });
    this._emit({ op: 'setText', key: `${rk}:t`, text });
    this._emit({ op: 'create', key: `${rk}:m`, tag: 'div', parent: rk, index: 1 });
    this._emit({ op: 'setAttr', key: `${rk}:m`, name: 'class', value: 'fnd-m' });
    this._emit({ op: 'setText', key: `${rk}:m`, text: source ? `${severity} · in ${source} · found by the agent` : `${severity} · found by the agent` });
  }
}

// Agent-authored lenses (§4.1: lenses are data) — tiles on the twin board.  The
// tile shows the lens's human TITLE and its lineage in words; the code is part of
// the deeper inspection (the expanded view), not the board.
class LensPanel {
  constructor(root = 'board-root') {
    this.root = root;
    this.names = new Map();
  }
  _emit(m) { TWIN_MUT.push(m); }

  set(name, title, description, sourceTitle, rowcount) {
    const rk = `ln:${name}`;
    const meta = `a lens over ${sourceTitle} · ${rowcount} rows · by the agent`;
    if (this.names.has(name)) {
      this._emit({ op: 'setAttr', key: rk, name: 'data-title', value: title });
      this._emit({ op: 'setText', key: `${rk}:n`, text: title });
      this._emit({ op: 'setText', key: `${rk}:desc`, text: description });
      this._emit({ op: 'setText', key: `${rk}:d`, text: meta });
      return;
    }
    const i = this.names.size;
    this.names.set(name, true);
    this._emit({ op: 'create', key: rk, tag: 'div', parent: this.root, index: i });
    this._emit({ op: 'setAttr', key: rk, name: 'class', value: 'src-row lens-card' });
    this._emit({ op: 'setAttr', key: rk, name: 'data-title', value: title });
    this._emit({ op: 'create', key: `${rk}:n`, tag: 'div', parent: rk, index: 0 });
    this._emit({ op: 'setAttr', key: `${rk}:n`, name: 'class', value: 'src-n' });
    this._emit({ op: 'setText', key: `${rk}:n`, text: title });
    this._emit({ op: 'create', key: `${rk}:desc`, tag: 'div', parent: rk, index: 1 });
    this._emit({ op: 'setAttr', key: `${rk}:desc`, name: 'class', value: 'src-desc' });
    this._emit({ op: 'setText', key: `${rk}:desc`, text: description });
    this._emit({ op: 'create', key: `${rk}:d`, tag: 'div', parent: rk, index: 2 });
    this._emit({ op: 'setAttr', key: `${rk}:d`, name: 'class', value: 'src-d' });
    this._emit({ op: 'setText', key: `${rk}:d`, text: meta });
  }
}

// A row of clickable page-chips in the header — used for RECENTS (a fold of the
// user's raw open-events) and STARRED (a fold of their star-events).  Both are
// lenses over the input stream: no client-side state, fully replayable.
class ChipRow {
  constructor(root, pfx, starred) {
    this.root = root;
    this.pfx = pfx;
    this.starred = !!starred;
    this.keys = [];
  }
  _emit(m) { TWIN_MUT.push(m); }

  render(items) {
    for (const k of this.keys) this._emit({ op: 'remove', key: k });
    this.keys = [];
    items.forEach((it, i) => {
      const k = `${this.pfx}:${i}`;
      this._emit({ op: 'create', key: k, tag: 'button', parent: this.root, index: i });
      this._emit({ op: 'setAttr', key: k, name: 'class', value: 'chip' + (this.starred ? ' starred' : '') });
      this._emit({ op: 'setAttr', key: k, name: 'data-ev', value: JSON.stringify(it.ev) });
      this._emit({ op: 'setAttr', key: k, name: 'data-title', value: it.title });
      this._emit({ op: 'setText', key: k, text: (this.starred ? '★ ' : '') + it.title });
      this.keys.push(k);
    });
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
