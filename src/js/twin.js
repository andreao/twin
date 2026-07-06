// The twin app graph — Phase 1 + 2: agent workspace + data sources (§9, §11, §12).
//
// The front surface is the agent at work: you open nearly blind and grow the twin
// organically (à la Claude Code).  Everything here is a graph edit in the event log
// (§8) — the chat log, the agent's thoughts, the growing user profile, and now the
// mounted/materialized data sources (the residence spectrum, §7/§9.9).  The client is
// a dumb host applying the §11.3 mutation IR; backward §11.4 events flow in via `event`.

'use strict';

const T = (() => {
  const G = new Graph('twin');

  // Recorded streams — PURE EVENT SOURCING (§8): every one is an append-only stream
  // in the ONE graph event log.  `input` is the system of record for the user: every
  // raw UI event lands there verbatim, and everything else about the user (feed items,
  // inferred goals, renders) is DERIVED from it with `input#<seq>` lineage.
  //   input    — the user's raw actions, exactly as they arrived (stamped at the boundary)
  //   feed     — the conversation (user items are derived from input)
  //   profile  — durable facts about the user (recorded by the agent, incl. inferred goals)
  //   agenda   — the agent's own to-do list ({id,text,status}; status changes are new events)
  //   activity — the agent's background work log
  //   findings — data issues / insights the agent discovered
  //   lenses   — lenses the agent authored (name + source + code = full lineage)
  //   src:*    — mounted/materialized data sources (created on demand)
  const inputSrc = G.source('input', false);
  const feedSrc = G.source('feed', false);
  const profileSrc = G.source('profile', false);
  const skillsSrc = G.source('skills', false);
  const agendaSrc = G.source('agenda', false);
  const activitySrc = G.source('activity', false);
  const findingsSrc = G.source('findings', false);
  const lensesSrc = G.source('lenses', false);
  const describeSrc = G.source('describe', false); // human titles/descriptions for any lens or source

  // The UI is TWO concepts: the conversation (feed) and the twin — one board of
  // lens tiles that grows as the agent works.  Sources are the rawest lenses;
  // agent-authored lenses, findings, and the user profile are tiles like any other.
  // The agenda/activity folds render as the one-line "now" strip above the board.
  const feed = new Feed('feed-root');
  const profile = new ProfilePanel();      // → tile:you on the board
  const sourcesPanel = new SourcesPanel(); // → sr:* tiles on the board
  const skillsPanel = new SkillsPanel('skills-root'); // capability registry (not on the board)
  const agendaPanel = new AgendaPanel();   // → the now-strip plan line
  const activityPanel = new ActivityPanel(); // → the now-strip last-action line
  const findingsPanel = new FindingsPanel(); // → tile:findings on the board
  const lensPanel = new LensPanel();       // → ln:* tiles on the board

  G.observe(feedSrc, (delta) => {
    for (const [row] of delta.entries()) feed.append(row.asObject());
  });
  G.observe(profileSrc, (delta) => {
    for (const [row] of delta.entries()) profile.set(row.get('field'), row.get('value'));
  });
  G.observe(skillsSrc, (delta) => {
    for (const [row] of delta.entries()) {
      const s = skills.get(row.get('name'));
      skillsPanel.set(row.get('name'), row.get('title'), row.get('description'), s ? s.files : []);
    }
  });

  // Folds over the event streams (the panels above render them; these Maps hold the
  // same folded state for perception + tool logic — last event per key wins).
  const agenda = new Map();    // id -> { text, status }
  const findings = new Map();  // id -> { severity, text, source }
  const lenses = new Map();    // name -> { source, code, rowcount, ver }
  const activityTail = [];     // recent activity notes (perception window)
  const userActions = [];      // recent raw user actions, described (perception window)

  G.observe(agendaSrc, (delta) => {
    for (const [row] of delta.entries()) {
      const r = row.asObject();
      const text = r.text || (agenda.get(r.id) || {}).text || '';
      agenda.set(r.id, { text, status: r.status });
      agendaPanel.set(r.id, text, r.status);
    }
  });
  G.observe(activitySrc, (delta) => {
    for (const [row] of delta.entries()) {
      const r = row.asObject();
      activityTail.push(r.text);
      if (activityTail.length > 12) activityTail.shift();
      activityPanel.add(r.seq, r.text);
    }
  });
  G.observe(findingsSrc, (delta) => {
    for (const [row] of delta.entries()) {
      const r = row.asObject();
      findings.set(r.id, { severity: r.severity, text: r.text, source: r.source });
      findingsPanel.set(r.id, r.severity, r.text, r.source);
    }
  });
  const srcTitle = (n) => (sources.get(n) || {}).title || n;
  G.observe(lensesSrc, (delta) => {
    for (const [row] of delta.entries()) {
      const r = row.asObject();
      lenses.set(r.name, { title: r.title, description: r.description, source: r.source, code: r.code, rowcount: r.rowcount, ver: r.ver });
      lensPanel.set(r.name, r.title || r.name, r.description || '', srcTitle(r.source), r.rowcount);
    }
  });
  // Descriptions are events too: a re-description folds onto the tile; history stays.
  G.observe(describeSrc, (delta) => {
    for (const [row] of delta.entries()) {
      const r = row.asObject();
      const s = sources.get(r.name);
      if (!s) continue;
      if (r.title) s.title = r.title;
      if (r.description) s.description = r.description;
      if (r.name.startsWith('lens:')) {
        const n = r.name.slice(5);
        const l = lenses.get(n);
        if (l) {
          if (r.title) l.title = r.title;
          if (r.description) l.description = r.description;
          lensPanel.set(n, l.title || n, l.description || '', srcTitle(l.source), l.rowcount);
        }
      } else {
        sourcesPanel.set(r.name, s);
        // the mount card in the chat picks up the human title/description too —
        // a projection update; the original mount event is untouched in the log.
        if (s.cardSeq) {
          TWIN_MUT.push({ op: 'setText', key: `item:${s.cardSeq}:h`, text: `Mounted ${s.title}` });
          TWIN_MUT.push({ op: 'setAttr', key: `item:${s.cardSeq}:h`, name: 'data-title', value: s.title });
          TWIN_MUT.push({ op: 'setText', key: `item:${s.cardSeq}:d`, text: mountCardSub(s) });
        }
      }
    }
  });

  const sourceIds = new Map(); // name -> graph source node id
  const sources = new Map();   // name -> { residence, locator, rowcount, schema, sample }
  const skills = new Map();    // name -> { description, files } — capabilities the agent has

  let seq = 0;
  const prov = (author) => ({ author, origin: 'agent', note: '' });

  // `note` marks a DERIVED item and names the raw event it came from (e.g. "input#7"),
  // so every feed item traces back to the event that caused it.
  function append(kind, payload, note) {
    seq += 1;
    const s = seq;
    G.submit(feedSrc, ZSet.fromRows([rec(Object.assign({ seq: s, kind }, payload))]),
      { author: kind === 'user' ? 'user' : 'agent', origin: note ? 'derived' : 'agent', note: note || '' });
    return s; // the feed seq — used to address a rendered view's body card
  }
  function recordProfile(field, value) {
    if (!field) return;
    G.submit(profileSrc, ZSet.fromRows([rec({ field: String(field), value: String(value) })]), prov('agent'));
  }

  // ---- data sources (§9.9 boundary; residence per §7) ------------------------
  function inferSchema(rows) {
    const cols = {};
    for (const r of rows.slice(0, 100)) {
      for (const k in r) {
        if (!(k in cols)) cols[k] = typeof r[k] === 'number' ? 'number' : typeof r[k];
      }
    }
    return cols;
  }
  // Called from the Rust boundary once a file has been read (mount = federate, §9.9).
  function mountSource(name, meta, rows) {
    // A documents corpus is an app-lens concern (not core): a mounted source whose rows
    // are files (name + optional assetIds).  The residence model is uniform — it's still
    // just a federated source — but we render it as a gallery, not a table.
    if (name === 'documents') {
      const docs = rows.map((r) => ({ name: r.name, bytes: r.bytes || 0, assetIds: r.assetIds || [] }));
      const info = { kind: 'documents', residence: meta.residence, rowcount: meta.rowcount, docs,
        title: meta.title || name, description: meta.description || '' };
      sources.set('documents', info);
      sourcesPanel.set('documents', info);
      info.cardSeq = workCard(`Mounted ${info.title}`, mountCardSub(info), { type: 'open_source', name: 'documents' });
      return;
    }
    let id = sourceIds.get(name);
    if (id === undefined) { id = G.source('src:' + name, true); sourceIds.set(name, id); }
    // materialize the rebuildable in-heap view (§15.1); truth stays in the file.
    G.submit(id, ZSet.fromRows(rows.map((r) => rec(r))), prov('upstream'));
    const schema = inferSchema(rows);
    const info = {
      kind: 'table', residence: meta.residence, locator: meta.locator,
      title: meta.title || name, description: meta.description || '',
      rowcount: meta.rowcount, materialized: meta.materialized || rows.length,
      schema, sample: rows.slice(0, 3),
    };
    sources.set(name, info);
    sourcesPanel.set(name, info);
    info.cardSeq = workCard(`Mounted ${info.title}`, mountCardSub(info), { type: 'open_source', name });
  }

  // What a mount card says under its title: the description (once known) plus the
  // honest residence facts — the same line the tile shows.
  function mountCardSub(s) {
    const meta = s.kind === 'documents'
      ? `${(s.docs || []).length} files · ${residenceLabel(s)}`
      : `${s.rowcount} rows · ${Object.keys(s.schema || {}).length} columns · ${residenceLabel(s)}`;
    return s.description ? `${s.description}  ·  ${meta}` : meta;
  }

  // A compact ACTION CARD in the chat: work that was done (by the agent or the
  // system on its behalf), titled like a human would say it, openable when it
  // points at something.  This is how background work reads as history from 0.
  function workCard(title, sub, openEv, tone) {
    const sq = append('card', { text: title, sub: sub || '', tone: tone || '' });
    if (openEv) {
      TWIN_MUT.push({ op: 'setAttr', key: `item:${sq}:h`, name: 'class', value: 'card-title openable' });
      TWIN_MUT.push({ op: 'setAttr', key: `item:${sq}:h`, name: 'data-title', value: String(title) });
      TWIN_MUT.push({ op: 'setAttr', key: `item:${sq}:h`, name: 'data-ev', value: JSON.stringify(openEv) });
    }
    return sq;
  }
  function sourceError(name, msg) {
    append('system', { text: `Couldn't read “${name}”: ${msg}` });
  }

  // ---- viewer lenses (§11.15): every visualization is a lens rendering into a
  // PANEL of the horizontal detail stack (Miller columns, Finder-style).  Opening at
  // panel i clears i and everything to its right; keys are prefixed `p<i>:` so the
  // same lens can be open in two columns without colliding.  The client owns the
  // panel chrome (title/star/close) and registers `panel:<i>:body` as the root. ----
  const MAX_EXPLORE_ROWS = 200;
  const panelKeys = new Map(); // panel index -> keys rendered into it
  const fmtNum = (n) => (Math.abs(n) >= 1000 || Number.isInteger(n)) ? n.toFixed(0) : Number(n.toPrecision(4)).toString();

  function viewer(panel) {
    const p = Number(panel) || 0;
    // opening at p invalidates p and every column to its right
    for (const [pi, ks] of [...panelKeys]) {
      if (pi >= p) {
        for (const k of ks) TWIN_MUT.push({ op: 'remove', key: k });
        panelKeys.delete(pi);
      }
    }
    const fresh = [];
    panelKeys.set(p, fresh);
    const root = `panel:${p}:body`;
    const K = (k) => `p${p}:${k}`;
    return {
      panel: p,
      key: K,
      add(tag, k, parent, index) {
        const kk = K(k);
        TWIN_MUT.push({ op: 'create', key: kk, tag, parent: parent == null ? root : K(parent), index });
        fresh.push(kk);
      },
      set(k, name, value) { TWIN_MUT.push({ op: 'setAttr', key: K(k), name, value: String(value) }); },
      text(k, t) { TWIN_MUT.push({ op: 'setText', key: K(k), text: String(t) }); },
    };
  }

  function openSource(name, mode, panel) {
    const s = sources.get(name);
    if (!s) return;
    const V = viewer(panel);
    if (s.kind === 'documents') return renderDocuments(V, s);
    if (mode === 'tree' && name === 'assets') return renderTree(V, name, s);
    if (mode === 'timeline' && name === 'events') return renderTimeline(V, name, s);
    renderTable(V, name, s);
  }

  // per-source view modes (§11.15: several lenses over the same data)
  const MODES = { assets: [['table', 'Table'], ['tree', 'Hierarchy']], events: [['table', 'Table'], ['timeline', 'Timeline']] };
  function renderModes(V, name, current) {
    const modes = MODES[name];
    if (!modes) return;
    V.add('div', 'exp:modes', null, 0); V.set('exp:modes', 'class', 'view-modes');
    modes.forEach(([m, lbl], i) => { const k = `mode:${name}:${m}`; V.add('button', k, 'exp:modes', i); V.set(k, 'class', 'mode-btn' + (m === current ? ' active' : '')); V.text(k, lbl); });
  }

  function renderTable(V, name, s) {
    const id = sourceIds.get(name);
    if (id === undefined) return;
    const rows = G.stateOf(id).support().slice(0, MAX_EXPLORE_ROWS).map((r) => r.asObject());
    const cols = Object.keys(s.schema);
    const chartable = name === 'timeseries'; // a sensor row → chart its datapoints
    renderModes(V, name, 'table');
    let i = 1;
    // deep inspection of a derived lens: its description, then the derivation chain
    // as a quiet breadcrumb (walked over the from-links), with the code of this hop
    // behind a subtle {…} toggle — never a slab of code by default.
    if (s.code) {
      if (s.description) {
        V.add('div', 'exp:desc', null, i++); V.set('exp:desc', 'class', 'src-desc');
        V.text('exp:desc', s.description);
      }
      V.add('div', 'exp:lin', null, i++); V.set('exp:lin', 'class', 'lens-chain');
      const parts = chainOf(name);
      parts.forEach((p, pi) => {
        if (pi) { V.add('span', `exp:lsep${pi}`, 'exp:lin', pi * 2 - 1); V.set(`exp:lsep${pi}`, 'class', 'chain-sep'); V.text(`exp:lsep${pi}`, '→'); }
        V.add('span', `exp:lp${pi}`, 'exp:lin', pi * 2); V.set(`exp:lp${pi}`, 'class', 'chain-part' + (pi === parts.length - 1 ? ' here' : '')); V.text(`exp:lp${pi}`, p);
      });
      V.add('button', 'exp:codebtn', 'exp:lin', parts.length * 2); V.set('exp:codebtn', 'class', 'code-toggle'); V.text('exp:codebtn', '{…} code');
      V.add('div', 'exp:code', null, i++); V.set('exp:code', 'class', 'lens-code'); V.set('exp:code', 'hidden', 'true');
      V.text('exp:code', s.code);
    }
    V.add('div', 'exp:note', null, i++); V.set('exp:note', 'class', 'explorer-note');
    V.text('exp:note', `${residenceLabel(s)} · ${cols.length} columns · showing ${rows.length} of ${s.rowcount} rows${chartable ? ' · click a sensor to chart it' : ''}`);
    V.add('table', 'exp:tbl', null, i);
    V.add('thead', 'exp:thead', 'exp:tbl', 0);
    V.add('tr', 'exp:htr', 'exp:thead', 0);
    cols.forEach((c, ci) => { V.add('th', `exp:th:${ci}`, 'exp:htr', ci); V.text(`exp:th:${ci}`, c); });
    V.add('tbody', 'exp:tb', 'exp:tbl', 1);
    rows.forEach((row, ri) => {
      const rk = `exp:r:${ri}`;
      V.add('tr', rk, 'exp:tb', ri);
      if (chartable) { V.set(rk, 'class', 'chartable'); V.set(rk, 'data-series', row.id); V.set(rk, 'data-label', row.externalId || row.name || row.id); }
      cols.forEach((c, ci) => { const ck = `exp:c:${ri}:${ci}`; V.add('td', ck, rk, ci); V.text(ck, row[c] == null ? '' : String(row[c])); });
    });
  }

  // asset hierarchy tree (§11.15) — the equipment structure, enriched at a glance with
  // per-subtree sensor + maintenance-event counts (rolled up over the links).
  function renderTree(V, name, s) {
    const id = sourceIds.get(name);
    if (id === undefined) return;
    const rows = G.stateOf(id).support().map((r) => r.asObject());
    renderModes(V, name, 'tree');
    V.add('div', 'exp:note', null, 1); V.set('exp:note', 'class', 'explorer-note');
    V.text('exp:note', `asset hierarchy · ${rows.length} equipment items · • = instrumented · counts roll up the subtree`);
    const byId = new Map(rows.map((r) => [String(r.id), r]));
    const kids = new Map(); const roots = [];
    rows.forEach((r) => {
      const p = r.parentId != null ? String(r.parentId) : null;
      if (p && byId.has(p)) { (kids.get(p) || kids.set(p, []).get(p)).push(r); } else { roots.push(r); }
    });
    // link indexes (sensors by assetId, events by assetIds), rolled up per subtree
    const sByA = new Map(); rowsOf('timeseries').forEach((t) => { const a = String(t.assetId); sByA.set(a, (sByA.get(a) || 0) + 1); });
    const eByA = new Map(); rowsOf('events').forEach((e) => String(e.assetIds || '').split(';').forEach((a) => { if (a) eByA.set(a, (eByA.get(a) || 0) + 1); }));
    const sub = new Map();
    const roll = (r) => {
      let sc = sByA.get(String(r.id)) || 0, ec = eByA.get(String(r.id)) || 0;
      (kids.get(String(r.id)) || []).forEach((c) => { const cv = roll(c); sc += cv.s; ec += cv.e; });
      const v = { s: sc, e: ec }; sub.set(String(r.id), v); return v;
    };
    roots.forEach(roll);
    V.add('div', 'exp:tree', null, 2); V.set('exp:tree', 'class', 'tree');
    let idx = 0;
    const emit = (r, depth) => {
      const k = `tn:${r.id}`, v = sub.get(String(r.id)) || { s: 0, e: 0 };
      V.add('div', k, 'exp:tree', idx++); V.set(k, 'class', 'tree-node'); V.set(k, 'style', `padding-left:${depth * 20 + 4}px`);
      V.add('span', `${k}:dot`, k, 0); V.set(`${k}:dot`, 'class', 'tree-dot' + (v.s ? ' on' : ''));
      V.add('span', `${k}:nm`, k, 1); V.set(`${k}:nm`, 'class', 'tree-name'); V.text(`${k}:nm`, r.name || String(r.id));
      if (v.s || v.e) { V.add('span', `${k}:b`, k, 2); V.set(`${k}:b`, 'class', 'tree-badge'); V.text(`${k}:b`, `  ${v.s} sensors · ${v.e} events`); }
      (kids.get(String(r.id)) || []).forEach((c) => emit(c, depth + 1));
    };
    roots.forEach((r) => emit(r, 0));
  }

  // events timeline (§11.15) — a bar chart of maintenance activity over time
  function renderTimeline(V, name, s) {
    const id = sourceIds.get(name);
    if (id === undefined) return;
    const rows = G.stateOf(id).support().map((r) => r.asObject());
    renderModes(V, name, 'timeline');
    const buckets = new Map(); let lo = null, hi = null;
    rows.forEach((r) => {
      const t = Number(r.startTime); if (!t) return;
      const d = new Date(t), m = d.getUTCFullYear() * 12 + d.getUTCMonth();
      buckets.set(m, (buckets.get(m) || 0) + 1);
      if (lo === null || m < lo) lo = m; if (hi === null || m > hi) hi = m;
    });
    V.add('div', 'exp:note', null, 1); V.set('exp:note', 'class', 'explorer-note');
    if (lo === null) { V.text('exp:note', `${rows.length} events · none are dated`); return; }
    const months = []; for (let m = lo; m <= hi; m++) months.push(m);
    const counts = months.map((m) => buckets.get(m) || 0);
    const maxC = Math.max(...counts, 1);
    const mstr = (m) => `${Math.floor(m / 12)}-${String((m % 12) + 1).padStart(2, '0')}`;
    V.text('exp:note', `${rows.length} events (sample) · ${months.length} months · ${mstr(lo)} → ${mstr(hi)} · peak ${maxC}/mo`);
    const W = 1000, H = 340, pad = 44, bw = (W - 2 * pad) / months.length;
    V.add('svg', 'exp:svg', null, 2); V.set('exp:svg', 'viewBox', `0 0 ${W} ${H}`); V.set('exp:svg', 'class', 'chart');
    V.add('line', 'exp:ax', 'exp:svg', 0); V.set('exp:ax', 'x1', pad); V.set('exp:ax', 'y1', H - pad); V.set('exp:ax', 'x2', W - pad); V.set('exp:ax', 'y2', H - pad); V.set('exp:ax', 'class', 'chart-axis');
    months.forEach((m, i) => {
      const c = counts[i], h = (c / maxC) * (H - 2 * pad), x = pad + i * bw, y = (H - pad) - h, k = `bar:${i}`;
      V.add('rect', k, 'exp:svg', i + 1); V.set(k, 'x', x + 1); V.set(k, 'y', y); V.set(k, 'width', Math.max(bw - 2, 1)); V.set(k, 'height', h); V.set(k, 'class', 'chart-bar');
    });
    V.add('text', 'exp:lmin', 'exp:svg', months.length + 1); V.set('exp:lmin', 'x', pad); V.set('exp:lmin', 'y', H - pad + 16); V.set('exp:lmin', 'class', 'chart-xlbl'); V.text('exp:lmin', mstr(lo));
    V.add('text', 'exp:lmax', 'exp:svg', months.length + 2); V.set('exp:lmax', 'x', W - pad); V.set('exp:lmax', 'y', H - pad + 16); V.set('exp:lmax', 'class', 'chart-xlbl endlbl'); V.text('exp:lmax', mstr(hi));
  }

  function chartMessage(label, msg, panel) {
    const V = viewer(panel);
    V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
    V.text('exp:note', `${label} — ${msg}.`);
  }

  // chart lens: raw datapoints (from Rust, downsampled) → an SVG line chart.
  function chartSeries(id, label, points, provenance, panel) {
    const V = viewer(panel);
    if (!points || !points.length) {
      V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
      V.text('exp:note', `${label} — no datapoints.`);
      return;
    }
    const W = 1000, H = 360, pad = 46;
    const ts = points.map((p) => p[0]), vs = points.map((p) => p[1]);
    const tmin = Math.min(...ts), tmax = Math.max(...ts), vmin = Math.min(...vs), vmax = Math.max(...vs);
    const sx = (t) => pad + (tmax > tmin ? (t - tmin) / (tmax - tmin) : 0) * (W - 2 * pad);
    const sy = (v) => (H - pad) - (vmax > vmin ? (v - vmin) / (vmax - vmin) : 0) * (H - 2 * pad);
    let d = '';
    points.forEach((p, i) => { d += (i ? 'L' : 'M') + sx(p[0]).toFixed(1) + ' ' + sy(p[1]).toFixed(1) + ' '; });
    V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
    V.text('exp:note', `${label} · ${points.length} points · min ${fmtNum(vmin)} · max ${fmtNum(vmax)}${provenance ? ' · ' + provenance : ''}`);
    V.add('svg', 'exp:svg', null, 1); V.set('exp:svg', 'viewBox', `0 0 ${W} ${H}`); V.set('exp:svg', 'class', 'chart');
    V.add('line', 'exp:ax', 'exp:svg', 0); V.set('exp:ax', 'x1', pad); V.set('exp:ax', 'y1', H - pad); V.set('exp:ax', 'x2', W - pad); V.set('exp:ax', 'y2', H - pad); V.set('exp:ax', 'class', 'chart-axis');
    V.add('line', 'exp:ay', 'exp:svg', 1); V.set('exp:ay', 'x1', pad); V.set('exp:ay', 'y1', pad); V.set('exp:ay', 'x2', pad); V.set('exp:ay', 'y2', H - pad); V.set('exp:ay', 'class', 'chart-axis');
    V.add('path', 'exp:path', 'exp:svg', 2); V.set('exp:path', 'd', d.trim()); V.set('exp:path', 'class', 'chart-line');
    V.add('text', 'exp:ymax', 'exp:svg', 3); V.set('exp:ymax', 'x', pad - 8); V.set('exp:ymax', 'y', pad + 4); V.set('exp:ymax', 'class', 'chart-lbl'); V.text('exp:ymax', fmtNum(vmax));
    V.add('text', 'exp:ymin', 'exp:svg', 4); V.set('exp:ymin', 'x', pad - 8); V.set('exp:ymin', 'y', H - pad); V.set('exp:ymin', 'class', 'chart-lbl'); V.text('exp:ymin', fmtNum(vmin));
  }

  // Asset dashboard (§11.16 an application is a lens) — the core twin use-case:
  // "everything about this equipment." Composes assets × timeseries (assetId) ×
  // events (assetIds) so a click on a compressor shows its sensors + maintenance.
  const fmtDate = (ms) => { const t = Number(ms); if (!t) return ''; const d = new Date(t); return `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, '0')}-${String(d.getUTCDate()).padStart(2, '0')}`; };
  function rowsOf(name) { const id = sourceIds.get(name); return id === undefined ? [] : G.stateOf(id).support().map((r) => r.asObject()); }
  function openAsset(assetId, panel) {
    const aid = String(assetId);
    const asset = rowsOf('assets').find((r) => String(r.id) === aid);
    if (!asset) return;
    const V = viewer(panel);
    const sensors = rowsOf('timeseries').filter((r) => String(r.assetId) === aid);
    const events = rowsOf('events')
      .filter((r) => String(r.assetIds || '').split(';').includes(aid))
      .sort((a, b) => Number(b.startTime) - Number(a.startTime));
    V.add('div', 'ad:hn', null, 0); V.set('ad:hn', 'class', 'ad-name'); V.text('ad:hn', asset.name || aid);
    V.add('div', 'ad:hd', null, 1); V.set('ad:hd', 'class', 'ad-desc'); V.text('ad:hd', `${asset.description || ''}  ·  id ${aid}`);

    V.add('div', 'ad:st', null, 2); V.set('ad:st', 'class', 'ad-section'); V.text('ad:st', `Sensors — ${sensors.length}`);
    V.add('div', 'ad:sl', null, 3); V.set('ad:sl', 'class', 'sens-list');
    if (!sensors.length) { V.add('div', 'ad:sn', 'ad:sl', 0); V.set('ad:sn', 'class', 'ad-empty'); V.text('ad:sn', 'no sensors linked to this asset'); }
    sensors.forEach((s, i) => {
      const k = `sens:${s.id}`;
      V.add('div', k, 'ad:sl', i); V.set(k, 'class', 'sens-row'); V.set(k, 'data-series', s.id); V.set(k, 'data-label', s.externalId || s.name || s.id);
      V.add('span', `${k}:n`, k, 0); V.set(`${k}:n`, 'class', 'sens-n'); V.text(`${k}:n`, s.externalId || s.name || String(s.id));
      if (s.unit) { V.add('span', `${k}:u`, k, 1); V.set(`${k}:u`, 'class', 'sens-u'); V.text(`${k}:u`, ` [${s.unit}]`); }
    });

    V.add('div', 'ad:et', null, 4); V.set('ad:et', 'class', 'ad-section'); V.text('ad:et', `Maintenance events — ${events.length}`);
    V.add('div', 'ad:el', null, 5);
    if (!events.length) { V.add('div', 'ad:en', 'ad:el', 0); V.set('ad:en', 'class', 'ad-empty'); V.text('ad:en', 'no events linked (in the sampled events)'); }
    else {
      V.add('table', 'ad:etbl', 'ad:el', 0); V.add('tbody', 'ad:etb', 'ad:etbl', 0);
      events.slice(0, 60).forEach((e, i) => {
        const rk = `ade:${i}`;
        V.add('tr', rk, 'ad:etb', i);
        [fmtDate(e.startTime), e.type || '', e.subtype || '', e.description || ''].forEach((c, ci) => {
          const ck = `${rk}:${ci}`; V.add('td', ck, rk, ci); V.text(ck, String(c));
        });
      });
    }

    // Documents — the P&IDs/drawings that reference this asset (they carry assetIds,
    // so each drawing lands on the dashboard of the equipment it depicts).
    const docs = ((sources.get('documents') || {}).docs || [])
      .filter((d) => (d.assetIds || []).map(String).includes(aid));
    V.add('div', 'ad:dt', null, 6); V.set('ad:dt', 'class', 'ad-section'); V.text('ad:dt', `Documents — ${docs.length}`);
    V.add('div', 'ad:dl', null, 7); V.set('ad:dl', 'class', 'sens-list');
    if (!docs.length) { V.add('div', 'ad:dn', 'ad:dl', 0); V.set('ad:dn', 'class', 'ad-empty'); V.text('ad:dn', 'no drawings reference this asset'); }
    docs.forEach((d, i) => {
      const k = `doc:${d.name}`;
      V.add('div', k, 'ad:dl', i); V.set(k, 'class', 'sens-row');
      V.add('span', `${k}:n`, k, 0); V.set(`${k}:n`, 'class', 'sens-n'); V.text(`${k}:n`, d.name);
    });
  }

  // Search — a PARAMETRIZED lens (§9.11): parametrized by the query, it derives
  // matches across the twin's entities and renders them, reusing the existing
  // action keys (tn:/sens:/doc:) so results are click-through to their views.
  function search(query, panel) {
    const q = String(query || '').trim().toLowerCase();
    const V = viewer(panel);
    V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
    if (!q) { V.text('exp:note', 'Search assets, sensors, events, and documents…'); return; }
    const m = (v) => v != null && String(v).toLowerCase().includes(q);
    const assets = rowsOf('assets').filter((r) => m(r.name) || m(r.description)).slice(0, 40);
    const sensors = rowsOf('timeseries').filter((r) => m(r.externalId) || m(r.name) || m(r.description)).slice(0, 40);
    const events = rowsOf('events').filter((r) => m(r.description) || m(r.type)).slice(0, 30);
    const docs = ((sources.get('documents') || {}).docs || []).filter((d) => m(d.name)).slice(0, 40);
    const total = assets.length + sensors.length + events.length + docs.length;
    V.text('exp:note', `${total} match${total === 1 ? '' : 'es'} for “${query}”`);
    let idx = 1;
    const sec = (title, n) => {
      V.add('div', `se:h${idx}`, null, idx); V.set(`se:h${idx}`, 'class', 'ad-section'); V.text(`se:h${idx}`, `${title} — ${n}`); idx++;
      const lk = `se:l${idx}`; V.add('div', lk, null, idx); V.set(lk, 'class', 'sens-list'); idx++; return lk;
    };
    const card = (key, parent, i, name, sub) => {
      V.add('div', key, parent, i); V.set(key, 'class', 'sens-row');
      V.add('span', `${key}:n`, key, 0); V.set(`${key}:n`, 'class', 'sens-n'); V.text(`${key}:n`, name);
      if (sub) { V.add('span', `${key}:s`, key, 1); V.set(`${key}:s`, 'class', 'sens-u'); V.text(`${key}:s`, ' ' + sub); }
    };
    if (assets.length) { const lk = sec('Assets', assets.length); assets.forEach((a, i) => card(`tn:${a.id}`, lk, i, a.name || String(a.id), a.description)); }
    if (sensors.length) { const lk = sec('Sensors', sensors.length); sensors.forEach((s, i) => { const k = `sens:${s.id}`; card(k, lk, i, s.externalId || s.name || String(s.id), s.unit ? `[${s.unit}]` : ''); V.set(k, 'data-series', s.id); V.set(k, 'data-label', s.externalId || s.name || s.id); }); }
    if (docs.length) { const lk = sec('Documents', docs.length); docs.forEach((d, i) => card(`doc:${d.name}`, lk, i, d.name, '')); }
    if (events.length) { const lk = sec('Events', events.length); events.forEach((e, i) => card(`se:ev${i}`, lk, i, `${fmtDate(e.startTime)} · ${e.type || ''}`.trim(), e.description || '')); }
  }

  // Watch (§12.4) — the twin watching itself: derived issues from the asset ↔ sensor
  // ↔ event links, surfaced as a queue you browse and click through to the asset.
  function watch(panel) {
    const V = viewer(panel);
    const assets = rowsOf('assets'); const byId = new Map(assets.map((a) => [String(a.id), a]));
    const eByA = new Map(); rowsOf('events').forEach((e) => String(e.assetIds || '').split(';').forEach((a) => { if (a) eByA.set(a, (eByA.get(a) || 0) + 1); }));
    const sByA = new Map(); rowsOf('timeseries').forEach((t) => { const a = String(t.assetId); sByA.set(a, (sByA.get(a) || 0) + 1); });
    V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
    V.text('exp:note', 'Derived issues — the twin watching itself over the asset ↔ sensor ↔ event links.');
    let idx = 1;
    const section = (title, hint) => {
      V.add('div', `w:h${idx}`, null, idx); V.set(`w:h${idx}`, 'class', 'ad-section'); V.text(`w:h${idx}`, title); idx++;
      if (hint) { V.add('div', `w:hh${idx}`, null, idx); V.set(`w:hh${idx}`, 'class', 'ad-empty'); V.text(`w:hh${idx}`, hint); idx++; }
      const lk = `w:l${idx}`; V.add('div', lk, null, idx); V.set(lk, 'class', 'sens-list'); idx++; return lk;
    };
    const card = (lk, sec, i, id, name, detail, sev) => {
      const k = `iss:${id}:${sec}:${i}`;
      V.add('div', k, lk, i); V.set(k, 'class', `issue-card ${sev}`);
      V.add('span', `${k}:n`, k, 0); V.set(`${k}:n`, 'class', 'sens-n'); V.text(`${k}:n`, name);
      V.add('span', `${k}:d`, k, 1); V.set(`${k}:d`, 'class', 'sens-u'); V.text(`${k}:d`, ' ' + detail);
    };

    // 1) blind spots — maintenance activity but no monitoring
    const blind = [...eByA.entries()].filter(([a, c]) => c >= 2 && !sByA.get(a) && byId.has(a)).sort((x, y) => y[1] - x[1]).slice(0, 15);
    const lk1 = section(`Blind spots — ${blind.length}`, 'equipment with maintenance events but no sensors monitoring it');
    if (!blind.length) { V.add('div', 'w:e1', lk1, 0); V.set('w:e1', 'class', 'ad-empty'); V.text('w:e1', 'none found'); }
    blind.forEach(([a, c], i) => card(lk1, 1, i, a, byId.get(a).name || a, `${c} events · 0 sensors`, 'sev-amber'));

    // 2) maintenance hotspots — where the work orders concentrate
    const hot = [...eByA.entries()].filter(([a]) => byId.has(a)).sort((x, y) => y[1] - x[1]).slice(0, 10);
    const lk2 = section(`Maintenance hotspots — top ${hot.length}`, 'assets with the most work orders');
    hot.forEach(([a, c], i) => card(lk2, 2, i, a, byId.get(a).name || a, `${c} events · ${sByA.get(a) || 0} sensors`, 'sev-blue'));
  }

  // document viewer: a gallery panel; a chosen file opens in the NEXT column
  // (served at /file/<name>) — Finder-style.
  function renderDocuments(V, s) {
    V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
    V.text('exp:note', `${s.docs.length} documents (P&IDs, drawings, training) — click to view`);
    V.add('div', 'exp:gal', null, 1); V.set('exp:gal', 'class', 'doc-gallery');
    s.docs.forEach((doc, i) => {
      const k = `doc:${doc.name}`;
      V.add('div', k, 'exp:gal', i); V.set(k, 'class', 'doc-item');
      V.add('div', `${k}:n`, k, 0); V.set(`${k}:n`, 'class', 'doc-name'); V.text(`${k}:n`, doc.name);
      V.add('div', `${k}:t`, k, 1); V.set(`${k}:t`, 'class', 'doc-type'); V.text(`${k}:t`, `${(doc.bytes / 1024).toFixed(0)} KB`);
    });
  }
  function openDocument(name, panel) {
    const V = viewer(panel);
    const ext = String(name).split('.').pop().toLowerCase();
    const src = '/file/' + encodeURIComponent(name);
    if (ext === 'pdf') { V.add('iframe', 'exp:doc', null, 0); V.set('exp:doc', 'src', src); V.set('exp:doc', 'class', 'doc-frame'); }
    else if (ext === 'mp4') { V.add('video', 'exp:doc', null, 0); V.set('exp:doc', 'src', src); V.set('exp:doc', 'controls', 'true'); V.set('exp:doc', 'class', 'doc-frame'); }
    else { V.add('img', 'exp:doc', null, 0); V.set('exp:doc', 'src', src); V.set('exp:doc', 'class', 'doc-img'); }
  }

  function residenceLabel(s) {
    if (s.residence === 'mounted') return 'federated · live (read-through, not copied)';
    if (s.residence === 'mounted-partial') return `federated · ${s.materialized} of ${s.rowcount} synced local`;
    return s.residence;
  }

  // Install a skill (§4.1) — called by the core skills-loader from the static dir.
  function installSkill(name, meta) {
    if (skills.has(name)) return;
    const title = meta.title || name;
    skills.set(name, { title, description: meta.description || '', files: meta.files || [] });
    G.submit(skillsSrc, ZSet.fromRows([rec({ name, title, description: meta.description || '' })]), prov('core'));
  }

  // ---- the agent's working state (agenda / activity / findings) ---------------
  // All pure event sourcing: each call appends an event; the observers above fold it
  // into the panels and the perception state.  Nothing is mutated in place.
  let agendaSeq = 0;
  function addAgenda(text) {
    agendaSeq += 1;
    G.submit(agendaSrc, ZSet.fromRows([rec({ id: agendaSeq, text: String(text), status: 'open' })]), prov('agent'));
  }
  // Resolve a task by id or text fragment and append a status-change event for it.
  function setAgenda(ref, status) {
    const q = String(ref).trim().toLowerCase();
    if (!q) return false;
    let hit = null;
    for (const [id, a] of agenda) {
      if (String(id) === q) { hit = id; break; }
      if (hit === null && a.status !== 'done' && a.text.toLowerCase().includes(q)) hit = id;
    }
    if (hit === null) return false;
    G.submit(agendaSrc, ZSet.fromRows([rec({ id: hit, text: agenda.get(hit).text, status })]), prov('agent'));
    return true;
  }
  let actSeq = 0;
  function logActivity(text) {
    actSeq += 1;
    G.submit(activitySrc, ZSet.fromRows([rec({ seq: actSeq, text: String(text) })]), prov('agent'));
  }
  let findingSeq = 0;
  function addFinding(a) {
    const text = String(a.text || a.description || '').trim();
    if (!text) return;
    for (const f of findings.values()) if (f.text === text) return; // already on the board
    findingSeq += 1;
    const sev = ['info', 'warn', 'critical'].includes(a.severity) ? a.severity : 'info';
    const src = String(a.source || '');
    G.submit(findingsSrc, ZSet.fromRows([rec({ id: findingSeq, severity: sev, text, source: src })]), prov('agent'));
    // findings are DONE work — they read as a card in the chat history too
    const sevTitle = sev === 'critical' ? 'Critical issue' : sev === 'warn' ? 'Data issue' : 'Noted';
    workCard(sevTitle, text + (src ? `  ·  in ${srcTitle(src)}` : ''),
      src && sources.has(src) ? { type: 'open_source', name: src } : null, `sev-${sev}`);
    logActivity(`finding (${sev}): ${text}`);
  }

  // The agent AUTHORS a lens (§4.1: lenses are data): pure JavaScript over a source's
  // rows, evaluated in-graph.  Purity is enforced by capability-absence — this isolate
  // has no IO of any kind, so lens code can compute but never effect.  The result
  // becomes a derived source `lens:<name>` (browsable/showable like any source) whose
  // full lineage — source, code, author — is recorded and shown on its card.
  function makeLens(a) {
    const srcName = String(a.source || '');
    // Names must read like a human named them.  Strip any "lens" the model prefixed
    // (the tile IS a lens — saying so is noise), slug for the stable id, and titleize
    // for display unless the agent gave an explicit title.
    const cleaned = String(a.name || '').replace(/^(the\s+)?lens[\s:_-]*/i, '').trim();
    const name = cleaned.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '') || 'unnamed';
    const words = name.replace(/-/g, ' ');
    const title = String(a.title || '').trim() || (words.charAt(0).toUpperCase() + words.slice(1));
    const description = String(a.description || '').trim();
    const id = sourceIds.get(srcName);
    if (id === undefined) { logActivity(`lens “${title}”: no source “${srcName}” — mount it first`); return; }
    const code = String(a.code || '');
    let out;
    try { out = new Function('rows', code)(G.stateOf(id).support().map((r) => r.asObject())); }
    catch (err) { logActivity(`lens “${title}” failed: ${err.message}`); return; }
    if (!Array.isArray(out)) { logActivity(`lens “${title}”: code must return an array of rows`); return; }
    out = out.slice(0, 5000).map((r) => (r && typeof r === 'object' ? r : { value: r }));
    const lname = 'lens:' + name;
    // re-authoring the same name = a NEW version; old versions stay in the event log
    const ver = ((lenses.get(name) || {}).ver || 0) + 1;
    const lid = G.source(`src:${lname}#${ver}`, true);
    sourceIds.set(lname, lid);
    G.submit(lid, ZSet.fromRows(out.map((r) => rec(r))), { author: 'agent', origin: 'lens', note: `from ${srcName} v${ver}` });
    const schema = inferSchema(out);
    sources.set(lname, {
      kind: 'table', residence: 'derived', title, description, code, from: srcName,
      rowcount: out.length, materialized: out.length, schema, sample: out.slice(0, 3),
    });
    G.submit(lensesSrc, ZSet.fromRows([rec({ name, title, description, source: srcName, code, rowcount: out.length, ver })]), prov('agent'));
    logActivity(`authored lens “${title}” — ${out.length} rows from ${srcTitle(srcName)}`);
    const sq = append('view', { text: `${title} — ${out.length} rows derived from ${srcName}`, view: 'table' });
    renderTableInto(`item:${sq}:body`, `v${sq}`, out.slice(0, 8), Object.keys(schema));
    cardOpen(sq, title, { type: 'open_source', name: lname });
  }

  // The composition chain of a derived source: walk the from-links back to the raw
  // mount, in human titles — "Equipment registry → Unique asset types → this".
  function chainOf(name) {
    const parts = [];
    let cur = name, guard = 0;
    while (cur && guard++ < 12) {
      const s = sources.get(cur);
      if (!s) { parts.unshift(cur); break; }
      parts.unshift(s.title || cur);
      cur = s.from;
    }
    return parts;
  }

  // Give any lens or source a better human title/description — an event like all else.
  function doDescribe(rawName, title, description) {
    const name = String(rawName || '');
    const key = sources.has(name) ? name : (sources.has('lens:' + name) ? 'lens:' + name : null);
    if (!key) { logActivity(`describe: no lens or source “${name}”`); return; }
    G.submit(describeSrc, ZSet.fromRows([rec({ name: key, title: String(title || ''), description: String(description || '') })]), prov('agent'));
    logActivity(`described “${(sources.get(key) || {}).title || key}”`);
  }

  // The agent inspects a source: quick stats + data-quality signals (empties,
  // duplicate keys) over the materialized rows.  The result lands in the activity
  // log, which the agent perceives next turn — its analysis loop.
  function inspectSource(src) {
    const meta = sources.get(src);
    if (meta && meta.kind === 'documents') {
      const types = {};
      meta.docs.forEach((d) => { const e = String(d.name).split('.').pop().toLowerCase(); types[e] = (types[e] || 0) + 1; });
      logActivity(`inspected “${src}”: ${meta.docs.length} files · ${Object.entries(types).map(([k, v]) => `${v} ${k}`).join(', ')}`);
      return;
    }
    const id = sourceIds.get(src);
    if (id === undefined) { logActivity(`no source “${src}” to inspect`); return; }
    const rows = G.stateOf(id).support().map((r) => r.asObject());
    const cols = Object.keys((sources.get(src) || {}).schema || {});
    const stats = [];
    cols.forEach((c) => {
      const nums = rows.map((r) => Number(r[c])).filter((n) => Number.isFinite(n));
      if (nums.length > rows.length * 0.5 && nums.length) {
        stats.push(`${c} ${Math.min(...nums).toFixed(1)}–${Math.max(...nums).toFixed(1)}`);
      }
    });
    const issues = [];
    cols.forEach((c) => {
      const n = rows.filter((r) => r[c] == null || r[c] === '').length;
      if (n) issues.push(`${c}: ${n} empty`);
    });
    if (cols.length) {
      const c0 = cols[0]; const seen = new Set(); let dup = 0;
      rows.forEach((r) => { const v = String(r[c0]); if (seen.has(v)) dup += 1; else seen.add(v); });
      if (dup) issues.push(`${c0}: ${dup} duplicates`);
    }
    logActivity(`inspected “${src}”: ${rows.length} rows, ${cols.length} cols`
      + (stats.length ? ` · ranges ${stats.slice(0, 4).join('; ')}` : '')
      + (issues.length ? ` · gaps ${issues.slice(0, 5).join('; ')}` : ' · no empties'));
  }

  // ---- inline rich views (§11.6, §11.16): the AGENT communicates through visuals ----
  // Every visual is a lens rendered as a card IN THE CONVERSATION — a real table,
  // hierarchy, or document viewer, not markdown text.  Each card is addressed by the
  // feed seq of its 'view' item; keys are namespaced by `v<seq>` so cards never collide
  // and the append-only log still replays deterministically (§11.5).
  function vbuild(root, pfx) {
    return {
      add: (tag, sub, parent, index) => {
        TWIN_MUT.push({ op: 'create', key: `${pfx}:${sub}`, tag, parent: parent == null ? root : `${pfx}:${parent}`, index });
        return `${pfx}:${sub}`;
      },
      set: (sub, name, value) => TWIN_MUT.push({ op: 'setAttr', key: `${pfx}:${sub}`, name, value: String(value) }),
      text: (sub, t) => TWIN_MUT.push({ op: 'setText', key: `${pfx}:${sub}`, text: String(t) }),
      listen: (sub, type) => TWIN_MUT.push({ op: 'listen', key: `${pfx}:${sub}`, type }),
    };
  }
  function renderTableInto(root, pfx, rows, cols) {
    const b = vbuild(root, pfx);
    b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note'); b.text('note', `${rows.length} rows · ${cols.length} columns`);
    b.add('div', 'scroll', null, 1); b.set('scroll', 'class', 'view-scroll');
    b.add('table', 'tbl', 'scroll', 0);
    b.add('thead', 'hd', 'tbl', 0); b.add('tr', 'htr', 'hd', 0);
    cols.forEach((c, i) => { b.add('th', `h${i}`, 'htr', i); b.text(`h${i}`, c); });
    b.add('tbody', 'tb', 'tbl', 1);
    rows.forEach((row, ri) => {
      b.add('tr', `r${ri}`, 'tb', ri);
      cols.forEach((c, ci) => { b.add('td', `c${ri}_${ci}`, `r${ri}`, ci); b.text(`c${ri}_${ci}`, row[c] == null ? '' : String(row[c])); });
    });
  }
  function renderTreeInto(root, pfx, name) {
    const rows = rowsOf(name);
    const b = vbuild(root, pfx);
    const byId = new Map(rows.map((r) => [String(r.id), r]));
    const kids = new Map(); const roots = [];
    rows.forEach((r) => { const p = r.parentId != null ? String(r.parentId) : null; if (p && byId.has(p)) { (kids.get(p) || kids.set(p, []).get(p)).push(r); } else { roots.push(r); } });
    const sByA = new Map(); rowsOf('timeseries').forEach((t) => { const a = String(t.assetId); sByA.set(a, (sByA.get(a) || 0) + 1); });
    const eByA = new Map(); rowsOf('events').forEach((e) => String(e.assetIds || '').split(';').forEach((a) => { if (a) eByA.set(a, (eByA.get(a) || 0) + 1); }));
    const sub = new Map();
    const roll = (r) => { let sc = sByA.get(String(r.id)) || 0, ec = eByA.get(String(r.id)) || 0; (kids.get(String(r.id)) || []).forEach((c) => { const cv = roll(c); sc += cv.s; ec += cv.e; }); const v = { s: sc, e: ec }; sub.set(String(r.id), v); return v; };
    roots.forEach(roll);
    b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note'); b.text('note', `asset hierarchy · ${rows.length} items · • = instrumented · counts roll up the subtree`);
    b.add('div', 'tree', null, 1); b.set('tree', 'class', 'tree view-scroll');
    let idx = 0;
    const emit = (r, depth) => {
      const k = `tn:${r.id}`, v = sub.get(String(r.id)) || { s: 0, e: 0 };
      // reuse the tn: action key so tree nodes in the feed open the asset dashboard too
      TWIN_MUT.push({ op: 'create', key: k, tag: 'div', parent: `${pfx}:tree`, index: idx++ });
      TWIN_MUT.push({ op: 'setAttr', key: k, name: 'class', value: 'tree-node' });
      TWIN_MUT.push({ op: 'setAttr', key: k, name: 'style', value: `padding-left:${depth * 20 + 4}px` });
      TWIN_MUT.push({ op: 'create', key: `${k}:dot`, tag: 'span', parent: k, index: 0 });
      TWIN_MUT.push({ op: 'setAttr', key: `${k}:dot`, name: 'class', value: 'tree-dot' + (v.s ? ' on' : '') });
      TWIN_MUT.push({ op: 'create', key: `${k}:nm`, tag: 'span', parent: k, index: 1 });
      TWIN_MUT.push({ op: 'setAttr', key: `${k}:nm`, name: 'class', value: 'tree-name' });
      TWIN_MUT.push({ op: 'setText', key: `${k}:nm`, text: r.name || String(r.id) });
      if (v.s || v.e) { TWIN_MUT.push({ op: 'create', key: `${k}:b`, tag: 'span', parent: k, index: 2 }); TWIN_MUT.push({ op: 'setAttr', key: `${k}:b`, name: 'class', value: 'tree-badge' }); TWIN_MUT.push({ op: 'setText', key: `${k}:b`, text: `  ${v.s} sensors · ${v.e} events` }); }
      (kids.get(String(r.id)) || []).forEach((c) => emit(c, depth + 1));
    };
    roots.forEach((r) => emit(r, 0));
  }
  // Inline chart card — the agent presenting a sensor IN the conversation.  Same
  // §11.3 keys as every card (namespaced by v<seq>), points arrive from the host
  // boundary (twin_show_chart) with their provenance noted on the card.
  function chartInline(id, label, points, provenance) {
    const sq = append('view', { text: label || id, view: 'chart' });
    cardOpen(sq, label || id, { type: 'fetch', adapter: 'cognite-datapoints', id: String(id), label: String(label || id) });
    const b = vbuild(`item:${sq}:body`, `v${sq}`);
    if (!points || !points.length) { b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note'); b.text('note', 'no datapoints'); return; }
    const W = 1000, H = 360, pad = 46;
    const ts = points.map((p) => p[0]), vs = points.map((p) => p[1]);
    const tmin = Math.min(...ts), tmax = Math.max(...ts), vmin = Math.min(...vs), vmax = Math.max(...vs);
    const sx = (t) => pad + (tmax > tmin ? (t - tmin) / (tmax - tmin) : 0) * (W - 2 * pad);
    const sy = (v) => (H - pad) - (vmax > vmin ? (v - vmin) / (vmax - vmin) : 0) * (H - 2 * pad);
    let d = '';
    points.forEach((p, i) => { d += (i ? 'L' : 'M') + sx(p[0]).toFixed(1) + ' ' + sy(p[1]).toFixed(1) + ' '; });
    b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note');
    b.text('note', `${points.length} points · min ${fmtNum(vmin)} · max ${fmtNum(vmax)}${provenance ? ' · ' + provenance : ''}`);
    b.add('svg', 'svg', null, 1); b.set('svg', 'viewBox', `0 0 ${W} ${H}`); b.set('svg', 'class', 'chart');
    b.add('line', 'ax', 'svg', 0); b.set('ax', 'x1', pad); b.set('ax', 'y1', H - pad); b.set('ax', 'x2', W - pad); b.set('ax', 'y2', H - pad); b.set('ax', 'class', 'chart-axis');
    b.add('line', 'ay', 'svg', 1); b.set('ay', 'x1', pad); b.set('ay', 'y1', pad); b.set('ay', 'x2', pad); b.set('ay', 'y2', H - pad); b.set('ay', 'class', 'chart-axis');
    b.add('path', 'path', 'svg', 2); b.set('path', 'd', d.trim()); b.set('path', 'class', 'chart-line');
    b.add('text', 'ymax', 'svg', 3); b.set('ymax', 'x', pad - 8); b.set('ymax', 'y', pad + 4); b.set('ymax', 'class', 'chart-lbl'); b.text('ymax', fmtNum(vmax));
    b.add('text', 'ymin', 'svg', 4); b.set('ymin', 'x', pad - 8); b.set('ymin', 'y', H - pad); b.set('ymin', 'class', 'chart-lbl'); b.text('ymin', fmtNum(vmin));
  }
  function chartInlineMessage(label, msg) {
    logActivity(`chart “${label}”: ${msg}`);
  }

  function renderDocInto(root, pfx, name) {
    const b = vbuild(root, pfx);
    const ext = String(name).split('.').pop().toLowerCase();
    const src = '/file/' + encodeURIComponent(name);
    if (ext === 'pdf') { b.add('iframe', 'doc', null, 0); b.set('doc', 'src', src); b.set('doc', 'class', 'doc-frame'); }
    else if (ext === 'mp4') { b.add('video', 'doc', null, 0); b.set('doc', 'src', src); b.set('doc', 'controls', 'true'); b.set('doc', 'class', 'doc-frame'); }
    else { b.add('img', 'doc', null, 0); b.set('doc', 'src', src); b.set('doc', 'class', 'doc-img'); }
  }

  // A view card in the chat is a doorway: its title carries the open-event that
  // shows the same thing in depth in the detail stack (the client wires the click).
  function cardOpen(sq, title, ev) {
    TWIN_MUT.push({ op: 'setAttr', key: `item:${sq}:h`, name: 'class', value: 'view-title openable' });
    TWIN_MUT.push({ op: 'setAttr', key: `item:${sq}:h`, name: 'data-title', value: String(title) });
    TWIN_MUT.push({ op: 'setAttr', key: `item:${sq}:h`, name: 'data-ev', value: JSON.stringify(ev) });
  }

  // The agent's `show` tool — render a real component inline in the conversation.
  function doShow(a) {
    const view = String(a.view || 'table').toLowerCase();
    if (view === 'document' || view === 'doc') {
      const name = a.name || a.document || a.source;
      if (!name) return;
      const title = a.title || String(name);
      const sq = append('view', { text: title, view: 'document' });
      renderDocInto(`item:${sq}:body`, `v${sq}`, name);
      cardOpen(sq, title, { type: 'open_document', name: String(name) });
      return;
    }
    const srcName = String(a.source || a.name || '');
    const s = sources.get(srcName);
    if (!s) { append('system', { text: `No source “${srcName}” to show — mount it first.` }); return; }
    if (view === 'tree' || view === 'hierarchy') {
      const title = a.title || `${srcTitle(srcName)} — hierarchy`;
      const sq = append('view', { text: title, view: 'tree' });
      renderTreeInto(`item:${sq}:body`, `v${sq}`, srcName);
      cardOpen(sq, title, { type: 'open_source', name: srcName, mode: 'tree' });
      return;
    }
    // table (default)
    let rows = rowsOf(srcName);
    if (a.filter) { const q = String(a.filter).toLowerCase(); rows = rows.filter((r) => Object.values(r).some((v) => v != null && String(v).toLowerCase().includes(q))); }
    const limit = Math.max(1, Math.min(Number(a.limit) || 10, MAX_EXPLORE_ROWS));
    rows = rows.slice(0, limit);
    const cols = Array.isArray(a.columns) && a.columns.length ? a.columns : Object.keys(s.schema || {});
    const title = a.title || `${srcTitle(srcName)} — ${rows.length} of ${s.rowcount}`;
    const sq = append('view', { text: title, view: 'table' });
    renderTableInto(`item:${sq}:body`, `v${sq}`, rows, cols.length ? cols : Object.keys(rows[0] || {}));
    cardOpen(sq, srcTitle(srcName), { type: 'open_source', name: srcName });
  }

  // ---- agent tool surface (§12.1) -------------------------------------------
  function agentTool(json) {
    let c; try { c = JSON.parse(json); } catch (_) { return; }
    const a = c.args || {};
    switch (c.tool) {
      case 'think': append('thought', { text: String(a.text || '') }); break;
      case 'say': append('agent', { text: String(a.text || '') }); break;
      case 'ask': append('question', { text: String(a.question || a.text || ''), options: a.options || [] }); break;
      case 'record_profile': recordProfile(a.field, a.value); break;
      case 'inspect': inspectSource(a.source); break;
      case 'show': doShow(a); break;
      case 'plan': {
        const items = Array.isArray(a.items) ? a.items : [a.text || a.item];
        items.filter(Boolean).forEach((t) => addAgenda(t));
        break;
      }
      case 'work':
        if (a.task != null && a.task !== '') setAgenda(a.task, 'active');
        logActivity(String(a.text || a.task || 'working'));
        break;
      case 'done': {
        const ref = a.task != null ? a.task : a.text;
        if (ref != null && setAgenda(ref, 'done')) logActivity(`done: ${ref}`);
        break;
      }
      case 'finding': addFinding(a); break;
      case 'make_lens': makeLens(a); break;
      case 'describe': doDescribe(a.source || a.name || a.lens, a.title, a.description); break;
      case 'idle': break; // "nothing worth doing" — pacing lives in the Rust harness
      // read_source is effectful: handled at the Rust boundary, which calls mountSource.
      default: if (a.text) append('agent', { text: String(a.text) }); break;
    }
  }

  // ---- backward UI events (§11.4): captured RAW, then derived ------------------
  // Everything the user does is recorded in its RAWEST form first: the event object
  // lands verbatim in the `input` stream (timestamped at the Rust boundary — the wall
  // clock is a boundary fact, never computed in-graph).  Only after that commit do we
  // derive: feed items, view renders, and the agent's read of the user's behavior all
  // come FROM the raw event, each carrying `input#<seq>` lineage.  (Derivations run
  // after the commit returns, not nested inside its propagation.)
  let inputSeq = 0;
  function event(json) {
    let e; try { e = JSON.parse(json); } catch (_) { return; }
    if (!e || typeof e.type !== 'string') return;
    inputSeq += 1;
    const raw = Object.assign({ seq: inputSeq, ts: 0 }, e);
    G.submit(inputSrc, ZSet.fromRows([rec(raw)]), { author: 'user', origin: 'ui', note: 'raw' });
    deriveFromInput(raw);
  }

  function deriveFromInput(e) {
    const note = `input#${e.seq}`;
    // the agent perceives the user's raw behavior — it derives goals from what they DO
    userActions.push(describeAction(e));
    if (userActions.length > 12) userActions.shift();
    if (e.type === 'user_message' && e.text) {
      append('user', { text: String(e.text) }, note);
    } else if (e.type === 'choose' && e.target) {
      const m = /^option:(\d+):(\d+)$/.exec(String(e.target));
      if (!m) return;
      const s = Number(m[1]), idx = Number(m[2]);
      const q = feed.items.find((it) => it.seq === s);
      const opt = q && q.options ? q.options[idx] : null;
      append('user', { text: opt != null ? String(opt) : `option ${idx + 1}` }, note);
    } else if (e.type === 'open_source' && e.name) {
      openSource(String(e.name), e.mode, e.panel);
    } else if (e.type === 'open_document' && e.name) {
      openDocument(String(e.name), e.panel);
    } else if (e.type === 'open_asset' && e.id) {
      openAsset(String(e.id), e.panel);
    } else if (e.type === 'search') {
      search(String(e.query || ''), e.panel);
    } else if (e.type === 'watch') {
      watch(e.panel);
    } else if (e.type === 'star') {
      toggleStar(e);
    }
    // 'fetch' renders via the host boundary (twin_fetch); 'open_board' is hosted
    // client-side (the live board node) — both are still captured raw above.
    if (['open_source', 'open_asset', 'open_document', 'fetch', 'open_board', 'watch'].includes(e.type)) {
      pushRecent(e);
    }
  }

  // ---- recents + stars: LENSES over the raw input stream --------------------
  // Recently visited = a fold of the user's open-events; starred = a fold of their
  // star-events.  Both render as chips in the header — no client-side state.
  const recents = [];
  const recentsPanel = new ChipRow('recents-root', 'rc', false);
  const starsPanel = new ChipRow('stars-root', 'st', true);
  function minimalEv(e) {
    const ev = { type: e.type };
    for (const k of ['name', 'id', 'mode', 'adapter', 'label', 'query']) {
      if (e[k] != null) ev[k] = e[k];
    }
    return ev;
  }
  function pushRecent(e) {
    const ev = minimalEv(e);
    const jid = JSON.stringify(ev);
    const title = String(e.title || e.label || e.name || (e.id ? 'asset ' + e.id : '') || e.type);
    const i = recents.findIndex((r) => r.jid === jid);
    if (i >= 0) recents.splice(i, 1);
    recents.unshift({ jid, title, ev });
    if (recents.length > 8) recents.pop();
    recentsPanel.render(recents);
  }
  const stars = new Map(); // jid -> { title, ev }
  function toggleStar(e) {
    const ev = e.target && typeof e.target === 'object' ? e.target : minimalEv(e);
    const jid = JSON.stringify(ev);
    if (stars.has(jid)) stars.delete(jid);
    else stars.set(jid, { title: String(e.title || 'page'), ev });
    starsPanel.render([...stars.values()]);
  }

  function describeAction(e) {
    switch (e.type) {
      case 'user_message': return 'sent a message';
      case 'choose': return 'answered a question card';
      case 'open_source': return `browsed source “${e.name}”${e.mode ? ' as ' + e.mode : ''}`;
      case 'open_document': return `viewed document “${e.name}”`;
      case 'open_asset': return `opened the dashboard of asset ${e.id}`;
      case 'search': return e.query ? `searched for “${e.query}”` : 'cleared the search';
      case 'watch': return 'opened the Watch board';
      case 'fetch': return `charted series ${e.label || e.id}`;
      case 'open_board': return 'opened the twin board';
      case 'star': return `starred “${e.title || 'a page'}”`;
      default: return String(e.type);
    }
  }

  // ---- what the agent perceives (§12.3) --------------------------------------
  // Everything here is a fold over the event streams above — including userActions,
  // the user's RAW behavior, from which the agent derives goals and intent.
  function perceive() {
    const recent = feed.items.slice(-40).map((it) => ({ kind: it.kind, text: it.text, options: it.options }));
    const srcs = [...sources].map(([name, s]) => ({
      name, title: s.title, description: s.description,
      residence: s.residence, rowcount: s.rowcount, schema: s.schema, sample: s.sample,
    }));
    const sks = [...skills].map(([name, s]) => ({ name, description: s.description }));
    const ag = [...agenda].map(([id, a]) => ({ id, text: a.text, status: a.status }));
    return JSON.stringify({
      profile: profile.asObject(),
      sources: srcs,
      skills: sks,
      agenda: ag.filter((a) => a.status !== 'done').slice(0, 12),
      agendaDone: ag.filter((a) => a.status === 'done').length,
      findings: [...findings.values()].slice(-8).map((f) => ({ severity: f.severity, text: f.text })),
      lenses: [...lenses].map(([name, l]) => ({ name: 'lens:' + name, title: l.title, description: l.description, source: l.source, rowcount: l.rowcount })),
      activity: activityTail.slice(-8),
      userActions: userActions.slice(-10),
      feed: recent,
    });
  }

  return {
    agentTool, event, perceive, mountSource, sourceError, installSkill,
    chartSeries, chartMessage, chartInline, chartInlineMessage,
    // the shared mutation log (§11.3): replaying [0..total) rebuilds the DOM exactly.
    total() { return TWIN_MUT.length; },
    from(n) { return JSON.stringify(TWIN_MUT.slice(n)); },
  };
})();
globalThis.T = T;
