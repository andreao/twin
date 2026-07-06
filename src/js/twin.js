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

  const feed = new Feed('feed-root');
  const profile = new ProfilePanel('profile-root');
  const sourcesPanel = new SourcesPanel('sources-root');
  const skillsPanel = new SkillsPanel('skills-root');
  const agendaPanel = new AgendaPanel('agenda-root');
  const activityPanel = new ActivityPanel('activity-root');
  const findingsPanel = new FindingsPanel('findings-root');
  const lensPanel = new LensPanel('lenses-root');

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
  G.observe(lensesSrc, (delta) => {
    for (const [row] of delta.entries()) {
      const r = row.asObject();
      lenses.set(r.name, { source: r.source, code: r.code, rowcount: r.rowcount, ver: r.ver });
      lensPanel.set(r.name, r.source, r.code, r.rowcount);
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
      const info = { kind: 'documents', residence: meta.residence, rowcount: meta.rowcount, docs };
      sources.set('documents', info);
      sourcesPanel.set('documents', info);
      append('system', { text: `Mounted “documents” — ${docs.length} files (${meta.residence}).` });
      return;
    }
    let id = sourceIds.get(name);
    if (id === undefined) { id = G.source('src:' + name, true); sourceIds.set(name, id); }
    // materialize the rebuildable in-heap view (§15.1); truth stays in the file.
    G.submit(id, ZSet.fromRows(rows.map((r) => rec(r))), prov('upstream'));
    const schema = inferSchema(rows);
    const info = {
      kind: 'table', residence: meta.residence, locator: meta.locator,
      rowcount: meta.rowcount, materialized: meta.materialized || rows.length,
      schema, sample: rows.slice(0, 3),
    };
    sources.set(name, info);
    sourcesPanel.set(name, info);
    const cols = Object.keys(schema).length;
    const partial = info.materialized < info.rowcount ? `, ${info.materialized} synced local` : '';
    append('system', { text: `Mounted “${name}” — ${info.rowcount} rows, ${cols} columns (${info.residence}${partial}).` });
  }
  function sourceError(name, msg) {
    append('system', { text: `Couldn't read “${name}”: ${msg}` });
  }

  // ---- viewer lenses (§11.15): every visualization is a lens rendering into the
  // shared 'explorer-root'.  table / chart / document, dispatched by source kind. ----
  const MAX_EXPLORE_ROWS = 200;
  let viewerKeys = [];
  const clearViewer = () => { for (const k of viewerKeys) TWIN_MUT.push({ op: 'remove', key: k }); viewerKeys = []; };
  const vadd = (tag, key, parent, index) => { TWIN_MUT.push({ op: 'create', key, tag, parent, index }); viewerKeys.push(key); };
  const vset = (key, name, value) => TWIN_MUT.push({ op: 'setAttr', key, name, value: String(value) });
  const vtext = (key, text) => TWIN_MUT.push({ op: 'setText', key, text });
  const fmtNum = (n) => (Math.abs(n) >= 1000 || Number.isInteger(n)) ? n.toFixed(0) : Number(n.toPrecision(4)).toString();

  function openSource(name, mode) {
    const s = sources.get(name);
    if (!s) return;
    if (s.kind === 'documents') return renderDocuments(s);
    if (mode === 'tree' && name === 'assets') return renderTree(name, s);
    if (mode === 'timeline' && name === 'events') return renderTimeline(name, s);
    renderTable(name, s);
  }

  // per-source view modes (§11.15: several lenses over the same data)
  const MODES = { assets: [['table', 'Table'], ['tree', 'Hierarchy']], events: [['table', 'Table'], ['timeline', 'Timeline']] };
  function renderModes(name, current) {
    const modes = MODES[name];
    if (!modes) return;
    vadd('div', 'exp:modes', 'explorer-root', 0); vset('exp:modes', 'class', 'view-modes');
    modes.forEach(([m, lbl], i) => { const k = `mode:${name}:${m}`; vadd('button', k, 'exp:modes', i); vset(k, 'class', 'mode-btn' + (m === current ? ' active' : '')); vtext(k, lbl); });
  }

  function renderTable(name, s) {
    const id = sourceIds.get(name);
    if (id === undefined) return;
    const rows = G.stateOf(id).support().slice(0, MAX_EXPLORE_ROWS).map((r) => r.asObject());
    const cols = Object.keys(s.schema);
    const chartable = name === 'timeseries'; // a sensor row → chart its datapoints
    clearViewer();
    renderModes(name, 'table');
    vadd('div', 'exp:note', 'explorer-root', 1); vset('exp:note', 'class', 'explorer-note');
    vtext('exp:note', `${residenceLabel(s)} · ${cols.length} columns · showing ${rows.length} of ${s.rowcount} rows${chartable ? ' · click a sensor to chart it' : ''}`);
    vadd('table', 'exp:tbl', 'explorer-root', 2);
    vadd('thead', 'exp:thead', 'exp:tbl', 0);
    vadd('tr', 'exp:htr', 'exp:thead', 0);
    cols.forEach((c, i) => { vadd('th', `exp:th:${i}`, 'exp:htr', i); vtext(`exp:th:${i}`, c); });
    vadd('tbody', 'exp:tb', 'exp:tbl', 1);
    rows.forEach((row, ri) => {
      const rk = `exp:r:${ri}`;
      vadd('tr', rk, 'exp:tb', ri);
      if (chartable) { vset(rk, 'class', 'chartable'); vset(rk, 'data-series', row.id); vset(rk, 'data-label', row.externalId || row.name || row.id); }
      cols.forEach((c, ci) => { const ck = `exp:c:${ri}:${ci}`; vadd('td', ck, rk, ci); vtext(ck, row[c] == null ? '' : String(row[c])); });
    });
  }

  // asset hierarchy tree (§11.15) — the equipment structure, enriched at a glance with
  // per-subtree sensor + maintenance-event counts (rolled up over the links).
  function renderTree(name, s) {
    const id = sourceIds.get(name);
    if (id === undefined) return;
    const rows = G.stateOf(id).support().map((r) => r.asObject());
    clearViewer();
    renderModes(name, 'tree');
    vadd('div', 'exp:note', 'explorer-root', 1); vset('exp:note', 'class', 'explorer-note');
    vtext('exp:note', `asset hierarchy · ${rows.length} equipment items · • = instrumented · counts roll up the subtree`);
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
    vadd('div', 'exp:tree', 'explorer-root', 2); vset('exp:tree', 'class', 'tree');
    let idx = 0;
    const emit = (r, depth) => {
      const k = `tn:${r.id}`, v = sub.get(String(r.id)) || { s: 0, e: 0 };
      vadd('div', k, 'exp:tree', idx++); vset(k, 'class', 'tree-node'); vset(k, 'style', `padding-left:${depth * 20 + 4}px`);
      vadd('span', `${k}:dot`, k, 0); vset(`${k}:dot`, 'class', 'tree-dot' + (v.s ? ' on' : ''));
      vadd('span', `${k}:nm`, k, 1); vset(`${k}:nm`, 'class', 'tree-name'); vtext(`${k}:nm`, r.name || String(r.id));
      if (v.s || v.e) { vadd('span', `${k}:b`, k, 2); vset(`${k}:b`, 'class', 'tree-badge'); vtext(`${k}:b`, `  ${v.s} sensors · ${v.e} events`); }
      (kids.get(String(r.id)) || []).forEach((c) => emit(c, depth + 1));
    };
    roots.forEach((r) => emit(r, 0));
  }

  // events timeline (§11.15) — a bar chart of maintenance activity over time
  function renderTimeline(name, s) {
    const id = sourceIds.get(name);
    if (id === undefined) return;
    const rows = G.stateOf(id).support().map((r) => r.asObject());
    clearViewer();
    renderModes(name, 'timeline');
    const buckets = new Map(); let lo = null, hi = null;
    rows.forEach((r) => {
      const t = Number(r.startTime); if (!t) return;
      const d = new Date(t), m = d.getUTCFullYear() * 12 + d.getUTCMonth();
      buckets.set(m, (buckets.get(m) || 0) + 1);
      if (lo === null || m < lo) lo = m; if (hi === null || m > hi) hi = m;
    });
    vadd('div', 'exp:note', 'explorer-root', 1); vset('exp:note', 'class', 'explorer-note');
    if (lo === null) { vtext('exp:note', `${rows.length} events · none are dated`); return; }
    const months = []; for (let m = lo; m <= hi; m++) months.push(m);
    const counts = months.map((m) => buckets.get(m) || 0);
    const maxC = Math.max(...counts, 1);
    const mstr = (m) => `${Math.floor(m / 12)}-${String((m % 12) + 1).padStart(2, '0')}`;
    vtext('exp:note', `${rows.length} events (sample) · ${months.length} months · ${mstr(lo)} → ${mstr(hi)} · peak ${maxC}/mo`);
    const W = 1000, H = 340, pad = 44, bw = (W - 2 * pad) / months.length;
    vadd('svg', 'exp:svg', 'explorer-root', 2); vset('exp:svg', 'viewBox', `0 0 ${W} ${H}`); vset('exp:svg', 'class', 'chart');
    vadd('line', 'exp:ax', 'exp:svg', 0); vset('exp:ax', 'x1', pad); vset('exp:ax', 'y1', H - pad); vset('exp:ax', 'x2', W - pad); vset('exp:ax', 'y2', H - pad); vset('exp:ax', 'class', 'chart-axis');
    months.forEach((m, i) => {
      const c = counts[i], h = (c / maxC) * (H - 2 * pad), x = pad + i * bw, y = (H - pad) - h, k = `bar:${i}`;
      vadd('rect', k, 'exp:svg', i + 1); vset(k, 'x', x + 1); vset(k, 'y', y); vset(k, 'width', Math.max(bw - 2, 1)); vset(k, 'height', h); vset(k, 'class', 'chart-bar');
    });
    vadd('text', 'exp:lmin', 'exp:svg', months.length + 1); vset('exp:lmin', 'x', pad); vset('exp:lmin', 'y', H - pad + 16); vset('exp:lmin', 'class', 'chart-xlbl'); vtext('exp:lmin', mstr(lo));
    vadd('text', 'exp:lmax', 'exp:svg', months.length + 2); vset('exp:lmax', 'x', W - pad); vset('exp:lmax', 'y', H - pad + 16); vset('exp:lmax', 'class', 'chart-xlbl endlbl'); vtext('exp:lmax', mstr(hi));
  }

  function chartMessage(label, msg) {
    clearViewer();
    vadd('div', 'exp:note', 'explorer-root', 0); vset('exp:note', 'class', 'explorer-note');
    vtext('exp:note', `${label} — ${msg}.`);
  }

  // chart lens: raw datapoints (from Rust, downsampled) → an SVG line chart.
  function chartSeries(id, label, points, provenance) {
    clearViewer();
    if (!points || !points.length) { return chartMessage(label, 'no datapoints'); }
    const W = 1000, H = 360, pad = 46;
    const ts = points.map((p) => p[0]), vs = points.map((p) => p[1]);
    const tmin = Math.min(...ts), tmax = Math.max(...ts), vmin = Math.min(...vs), vmax = Math.max(...vs);
    const sx = (t) => pad + (tmax > tmin ? (t - tmin) / (tmax - tmin) : 0) * (W - 2 * pad);
    const sy = (v) => (H - pad) - (vmax > vmin ? (v - vmin) / (vmax - vmin) : 0) * (H - 2 * pad);
    let d = '';
    points.forEach((p, i) => { d += (i ? 'L' : 'M') + sx(p[0]).toFixed(1) + ' ' + sy(p[1]).toFixed(1) + ' '; });
    vadd('div', 'exp:note', 'explorer-root', 0); vset('exp:note', 'class', 'explorer-note');
    vtext('exp:note', `${label} · ${points.length} points · min ${fmtNum(vmin)} · max ${fmtNum(vmax)}${provenance ? ' · ' + provenance : ''}`);
    vadd('svg', 'exp:svg', 'explorer-root', 1); vset('exp:svg', 'viewBox', `0 0 ${W} ${H}`); vset('exp:svg', 'class', 'chart');
    vadd('line', 'exp:ax', 'exp:svg', 0); vset('exp:ax', 'x1', pad); vset('exp:ax', 'y1', H - pad); vset('exp:ax', 'x2', W - pad); vset('exp:ax', 'y2', H - pad); vset('exp:ax', 'class', 'chart-axis');
    vadd('line', 'exp:ay', 'exp:svg', 1); vset('exp:ay', 'x1', pad); vset('exp:ay', 'y1', pad); vset('exp:ay', 'x2', pad); vset('exp:ay', 'y2', H - pad); vset('exp:ay', 'class', 'chart-axis');
    vadd('path', 'exp:path', 'exp:svg', 2); vset('exp:path', 'd', d.trim()); vset('exp:path', 'class', 'chart-line');
    vadd('text', 'exp:ymax', 'exp:svg', 3); vset('exp:ymax', 'x', pad - 8); vset('exp:ymax', 'y', pad + 4); vset('exp:ymax', 'class', 'chart-lbl'); vtext('exp:ymax', fmtNum(vmax));
    vadd('text', 'exp:ymin', 'exp:svg', 4); vset('exp:ymin', 'x', pad - 8); vset('exp:ymin', 'y', H - pad); vset('exp:ymin', 'class', 'chart-lbl'); vtext('exp:ymin', fmtNum(vmin));
  }

  // Asset dashboard (§11.16 an application is a lens) — the core twin use-case:
  // "everything about this equipment." Composes assets × timeseries (assetId) ×
  // events (assetIds) so a click on a compressor shows its sensors + maintenance.
  const fmtDate = (ms) => { const t = Number(ms); if (!t) return ''; const d = new Date(t); return `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, '0')}-${String(d.getUTCDate()).padStart(2, '0')}`; };
  function rowsOf(name) { const id = sourceIds.get(name); return id === undefined ? [] : G.stateOf(id).support().map((r) => r.asObject()); }
  function openAsset(assetId) {
    const aid = String(assetId);
    const asset = rowsOf('assets').find((r) => String(r.id) === aid);
    if (!asset) return;
    const sensors = rowsOf('timeseries').filter((r) => String(r.assetId) === aid);
    const events = rowsOf('events')
      .filter((r) => String(r.assetIds || '').split(';').includes(aid))
      .sort((a, b) => Number(b.startTime) - Number(a.startTime));
    clearViewer();
    vadd('div', 'ad:back', 'explorer-root', 0); vset('ad:back', 'class', 'doc-back'); vtext('ad:back', '← back to hierarchy');
    vadd('div', 'ad:hn', 'explorer-root', 1); vset('ad:hn', 'class', 'ad-name'); vtext('ad:hn', asset.name || aid);
    vadd('div', 'ad:hd', 'explorer-root', 2); vset('ad:hd', 'class', 'ad-desc'); vtext('ad:hd', `${asset.description || ''}  ·  id ${aid}`);

    vadd('div', 'ad:st', 'explorer-root', 3); vset('ad:st', 'class', 'ad-section'); vtext('ad:st', `Sensors — ${sensors.length}`);
    vadd('div', 'ad:sl', 'explorer-root', 4); vset('ad:sl', 'class', 'sens-list');
    if (!sensors.length) { vadd('div', 'ad:sn', 'ad:sl', 0); vset('ad:sn', 'class', 'ad-empty'); vtext('ad:sn', 'no sensors linked to this asset'); }
    sensors.forEach((s, i) => {
      const k = `sens:${s.id}`;
      vadd('div', k, 'ad:sl', i); vset(k, 'class', 'sens-row'); vset(k, 'data-series', s.id); vset(k, 'data-label', s.externalId || s.name || s.id);
      vadd('span', `${k}:n`, k, 0); vset(`${k}:n`, 'class', 'sens-n'); vtext(`${k}:n`, s.externalId || s.name || String(s.id));
      if (s.unit) { vadd('span', `${k}:u`, k, 1); vset(`${k}:u`, 'class', 'sens-u'); vtext(`${k}:u`, ` [${s.unit}]`); }
    });

    vadd('div', 'ad:et', 'explorer-root', 5); vset('ad:et', 'class', 'ad-section'); vtext('ad:et', `Maintenance events — ${events.length}`);
    vadd('div', 'ad:el', 'explorer-root', 6);
    if (!events.length) { vadd('div', 'ad:en', 'ad:el', 0); vset('ad:en', 'class', 'ad-empty'); vtext('ad:en', 'no events linked (in the sampled events)'); }
    else {
      vadd('table', 'ad:etbl', 'ad:el', 0); vadd('tbody', 'ad:etb', 'ad:etbl', 0);
      events.slice(0, 60).forEach((e, i) => {
        const rk = `ade:${i}`;
        vadd('tr', rk, 'ad:etb', i);
        [fmtDate(e.startTime), e.type || '', e.subtype || '', e.description || ''].forEach((c, ci) => {
          const ck = `${rk}:${ci}`; vadd('td', ck, rk, ci); vtext(ck, String(c));
        });
      });
    }

    // Documents — the P&IDs/drawings that reference this asset (they carry assetIds,
    // so each drawing lands on the dashboard of the equipment it depicts).
    const docs = ((sources.get('documents') || {}).docs || [])
      .filter((d) => (d.assetIds || []).map(String).includes(aid));
    vadd('div', 'ad:dt', 'explorer-root', 7); vset('ad:dt', 'class', 'ad-section'); vtext('ad:dt', `Documents — ${docs.length}`);
    vadd('div', 'ad:dl', 'explorer-root', 8); vset('ad:dl', 'class', 'sens-list');
    if (!docs.length) { vadd('div', 'ad:dn', 'ad:dl', 0); vset('ad:dn', 'class', 'ad-empty'); vtext('ad:dn', 'no drawings reference this asset'); }
    docs.forEach((d, i) => {
      const k = `doc:${d.name}`;
      vadd('div', k, 'ad:dl', i); vset(k, 'class', 'sens-row');
      vadd('span', `${k}:n`, k, 0); vset(`${k}:n`, 'class', 'sens-n'); vtext(`${k}:n`, d.name);
    });
  }

  // Search — a PARAMETRIZED lens (§9.11): parametrized by the query, it derives
  // matches across the twin's entities and renders them, reusing the existing
  // action keys (tn:/sens:/doc:) so results are click-through to their views.
  function search(query) {
    const q = String(query || '').trim().toLowerCase();
    clearViewer();
    vadd('div', 'exp:note', 'explorer-root', 0); vset('exp:note', 'class', 'explorer-note');
    if (!q) { vtext('exp:note', 'Search assets, sensors, events, and documents…'); return; }
    const m = (v) => v != null && String(v).toLowerCase().includes(q);
    const assets = rowsOf('assets').filter((r) => m(r.name) || m(r.description)).slice(0, 40);
    const sensors = rowsOf('timeseries').filter((r) => m(r.externalId) || m(r.name) || m(r.description)).slice(0, 40);
    const events = rowsOf('events').filter((r) => m(r.description) || m(r.type)).slice(0, 30);
    const docs = ((sources.get('documents') || {}).docs || []).filter((d) => m(d.name)).slice(0, 40);
    const total = assets.length + sensors.length + events.length + docs.length;
    vtext('exp:note', `${total} match${total === 1 ? '' : 'es'} for “${query}”`);
    let idx = 1;
    const sec = (title, n) => {
      vadd('div', `se:h${idx}`, 'explorer-root', idx); vset(`se:h${idx}`, 'class', 'ad-section'); vtext(`se:h${idx}`, `${title} — ${n}`); idx++;
      const lk = `se:l${idx}`; vadd('div', lk, 'explorer-root', idx); vset(lk, 'class', 'sens-list'); idx++; return lk;
    };
    const card = (key, parent, i, name, sub) => {
      vadd('div', key, parent, i); vset(key, 'class', 'sens-row');
      vadd('span', `${key}:n`, key, 0); vset(`${key}:n`, 'class', 'sens-n'); vtext(`${key}:n`, name);
      if (sub) { vadd('span', `${key}:s`, key, 1); vset(`${key}:s`, 'class', 'sens-u'); vtext(`${key}:s`, ' ' + sub); }
    };
    if (assets.length) { const lk = sec('Assets', assets.length); assets.forEach((a, i) => card(`tn:${a.id}`, lk, i, a.name || String(a.id), a.description)); }
    if (sensors.length) { const lk = sec('Sensors', sensors.length); sensors.forEach((s, i) => { const k = `sens:${s.id}`; card(k, lk, i, s.externalId || s.name || String(s.id), s.unit ? `[${s.unit}]` : ''); vset(k, 'data-series', s.id); vset(k, 'data-label', s.externalId || s.name || s.id); }); }
    if (docs.length) { const lk = sec('Documents', docs.length); docs.forEach((d, i) => card(`doc:${d.name}`, lk, i, d.name, '')); }
    if (events.length) { const lk = sec('Events', events.length); events.forEach((e, i) => card(`se:ev${i}`, lk, i, `${fmtDate(e.startTime)} · ${e.type || ''}`.trim(), e.description || '')); }
  }

  // Watch (§12.4) — the twin watching itself: derived issues from the asset ↔ sensor
  // ↔ event links, surfaced as a queue you browse and click through to the asset.
  function watch() {
    clearViewer();
    const assets = rowsOf('assets'); const byId = new Map(assets.map((a) => [String(a.id), a]));
    const eByA = new Map(); rowsOf('events').forEach((e) => String(e.assetIds || '').split(';').forEach((a) => { if (a) eByA.set(a, (eByA.get(a) || 0) + 1); }));
    const sByA = new Map(); rowsOf('timeseries').forEach((t) => { const a = String(t.assetId); sByA.set(a, (sByA.get(a) || 0) + 1); });
    vadd('div', 'exp:note', 'explorer-root', 0); vset('exp:note', 'class', 'explorer-note');
    vtext('exp:note', 'Derived issues — the twin watching itself over the asset ↔ sensor ↔ event links.');
    let idx = 1;
    const section = (title, hint) => {
      vadd('div', `w:h${idx}`, 'explorer-root', idx); vset(`w:h${idx}`, 'class', 'ad-section'); vtext(`w:h${idx}`, title); idx++;
      if (hint) { vadd('div', `w:hh${idx}`, 'explorer-root', idx); vset(`w:hh${idx}`, 'class', 'ad-empty'); vtext(`w:hh${idx}`, hint); idx++; }
      const lk = `w:l${idx}`; vadd('div', lk, 'explorer-root', idx); vset(lk, 'class', 'sens-list'); idx++; return lk;
    };
    const card = (lk, sec, i, id, name, detail, sev) => {
      const k = `iss:${id}:${sec}:${i}`;
      vadd('div', k, lk, i); vset(k, 'class', `issue-card ${sev}`);
      vadd('span', `${k}:n`, k, 0); vset(`${k}:n`, 'class', 'sens-n'); vtext(`${k}:n`, name);
      vadd('span', `${k}:d`, k, 1); vset(`${k}:d`, 'class', 'sens-u'); vtext(`${k}:d`, ' ' + detail);
    };

    // 1) blind spots — maintenance activity but no monitoring
    const blind = [...eByA.entries()].filter(([a, c]) => c >= 2 && !sByA.get(a) && byId.has(a)).sort((x, y) => y[1] - x[1]).slice(0, 15);
    const lk1 = section(`Blind spots — ${blind.length}`, 'equipment with maintenance events but no sensors monitoring it');
    if (!blind.length) { vadd('div', 'w:e1', lk1, 0); vset('w:e1', 'class', 'ad-empty'); vtext('w:e1', 'none found'); }
    blind.forEach(([a, c], i) => card(lk1, 1, i, a, byId.get(a).name || a, `${c} events · 0 sensors`, 'sev-amber'));

    // 2) maintenance hotspots — where the work orders concentrate
    const hot = [...eByA.entries()].filter(([a]) => byId.has(a)).sort((x, y) => y[1] - x[1]).slice(0, 10);
    const lk2 = section(`Maintenance hotspots — top ${hot.length}`, 'assets with the most work orders');
    hot.forEach(([a, c], i) => card(lk2, 2, i, a, byId.get(a).name || a, `${c} events · ${sByA.get(a) || 0} sensors`, 'sev-blue'));
  }

  // document viewer: a gallery, then an embed of the chosen file (served at /file/<name>).
  function renderDocuments(s) {
    clearViewer();
    vadd('div', 'exp:note', 'explorer-root', 0); vset('exp:note', 'class', 'explorer-note');
    vtext('exp:note', `${s.docs.length} documents (P&IDs, drawings, training) — click to view`);
    vadd('div', 'exp:gal', 'explorer-root', 1); vset('exp:gal', 'class', 'doc-gallery');
    s.docs.forEach((doc, i) => {
      const k = `doc:${doc.name}`;
      vadd('div', k, 'exp:gal', i); vset(k, 'class', 'doc-item');
      vadd('div', `${k}:n`, k, 0); vset(`${k}:n`, 'class', 'doc-name'); vtext(`${k}:n`, doc.name);
      vadd('div', `${k}:t`, k, 1); vset(`${k}:t`, 'class', 'doc-type'); vtext(`${k}:t`, `${(doc.bytes / 1024).toFixed(0)} KB`);
    });
  }
  function openDocument(name) {
    clearViewer();
    vadd('div', 'exp:back', 'explorer-root', 0); vset('exp:back', 'class', 'doc-back'); vtext('exp:back', '← back to documents');
    const ext = String(name).split('.').pop().toLowerCase();
    const src = '/file/' + encodeURIComponent(name);
    if (ext === 'pdf') { vadd('iframe', 'exp:doc', 'explorer-root', 1); vset('exp:doc', 'src', src); vset('exp:doc', 'class', 'doc-frame'); }
    else if (ext === 'mp4') { vadd('video', 'exp:doc', 'explorer-root', 1); vset('exp:doc', 'src', src); vset('exp:doc', 'controls', 'true'); vset('exp:doc', 'class', 'doc-frame'); }
    else { vadd('img', 'exp:doc', 'explorer-root', 1); vset('exp:doc', 'src', src); vset('exp:doc', 'class', 'doc-img'); }
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
    G.submit(findingsSrc, ZSet.fromRows([rec({ id: findingSeq, severity: sev, text, source: String(a.source || '') })]), prov('agent'));
    logActivity(`finding (${sev}): ${text}`);
  }

  // The agent AUTHORS a lens (§4.1: lenses are data): pure JavaScript over a source's
  // rows, evaluated in-graph.  Purity is enforced by capability-absence — this isolate
  // has no IO of any kind, so lens code can compute but never effect.  The result
  // becomes a derived source `lens:<name>` (browsable/showable like any source) whose
  // full lineage — source, code, author — is recorded and shown on its card.
  function makeLens(a) {
    const srcName = String(a.source || '');
    const name = String(a.name || '').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '') || 'unnamed';
    const id = sourceIds.get(srcName);
    if (id === undefined) { logActivity(`lens “${name}”: no source “${srcName}” — mount it first`); return; }
    const code = String(a.code || '');
    let out;
    try { out = new Function('rows', code)(G.stateOf(id).support().map((r) => r.asObject())); }
    catch (err) { logActivity(`lens “${name}” failed: ${err.message}`); return; }
    if (!Array.isArray(out)) { logActivity(`lens “${name}”: code must return an array of rows`); return; }
    out = out.slice(0, 5000).map((r) => (r && typeof r === 'object' ? r : { value: r }));
    const lname = 'lens:' + name;
    // re-authoring the same name = a NEW version; old versions stay in the event log
    const ver = ((lenses.get(name) || {}).ver || 0) + 1;
    const lid = G.source(`src:${lname}#${ver}`, true);
    sourceIds.set(lname, lid);
    G.submit(lid, ZSet.fromRows(out.map((r) => rec(r))), { author: 'agent', origin: 'lens', note: `from ${srcName} v${ver}` });
    const schema = inferSchema(out);
    sources.set(lname, {
      kind: 'table', residence: 'derived', locator: `lens(${srcName})`,
      rowcount: out.length, materialized: out.length, schema, sample: out.slice(0, 3),
    });
    G.submit(lensesSrc, ZSet.fromRows([rec({ name, source: srcName, code, rowcount: out.length, ver })]), prov('agent'));
    logActivity(`authored lens “${name}” — ${out.length} rows from ${srcName}`);
    const sq = append('view', { text: a.title || `New lens “${name}” — ${out.length} rows derived from ${srcName}`, view: 'table' });
    renderTableInto(`item:${sq}:body`, `v${sq}`, out.slice(0, 8), Object.keys(schema));
  }

  // The agent inspects a source: quick stats + data-quality signals (empties,
  // duplicate keys) over the materialized rows.  The result lands in the activity
  // log, which the agent perceives next turn — its analysis loop.
  function inspectSource(src) {
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
  function renderDocInto(root, pfx, name) {
    const b = vbuild(root, pfx);
    const ext = String(name).split('.').pop().toLowerCase();
    const src = '/file/' + encodeURIComponent(name);
    if (ext === 'pdf') { b.add('iframe', 'doc', null, 0); b.set('doc', 'src', src); b.set('doc', 'class', 'doc-frame'); }
    else if (ext === 'mp4') { b.add('video', 'doc', null, 0); b.set('doc', 'src', src); b.set('doc', 'controls', 'true'); b.set('doc', 'class', 'doc-frame'); }
    else { b.add('img', 'doc', null, 0); b.set('doc', 'src', src); b.set('doc', 'class', 'doc-img'); }
  }

  // The agent's `show` tool — render a real component inline in the conversation.
  function doShow(a) {
    const view = String(a.view || 'table').toLowerCase();
    if (view === 'document' || view === 'doc') {
      const name = a.name || a.document || a.source;
      if (!name) return;
      const sq = append('view', { text: a.title || String(name), view: 'document' });
      renderDocInto(`item:${sq}:body`, `v${sq}`, name);
      return;
    }
    const srcName = String(a.source || a.name || '');
    const s = sources.get(srcName);
    if (!s) { append('system', { text: `No source “${srcName}” to show — mount it first.` }); return; }
    if (view === 'tree' || view === 'hierarchy') {
      const sq = append('view', { text: a.title || `${srcName} — hierarchy`, view: 'tree' });
      renderTreeInto(`item:${sq}:body`, `v${sq}`, srcName);
      return;
    }
    // table (default)
    let rows = rowsOf(srcName);
    if (a.filter) { const q = String(a.filter).toLowerCase(); rows = rows.filter((r) => Object.values(r).some((v) => v != null && String(v).toLowerCase().includes(q))); }
    const limit = Math.max(1, Math.min(Number(a.limit) || 10, MAX_EXPLORE_ROWS));
    rows = rows.slice(0, limit);
    const cols = Array.isArray(a.columns) && a.columns.length ? a.columns : Object.keys(s.schema || {});
    const sq = append('view', { text: a.title || `${srcName} — showing ${rows.length} of ${s.rowcount}`, view: 'table' });
    renderTableInto(`item:${sq}:body`, `v${sq}`, rows, cols.length ? cols : Object.keys(rows[0] || {}));
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
      openSource(String(e.name), e.mode);
    } else if (e.type === 'open_document' && e.name) {
      openDocument(String(e.name));
    } else if (e.type === 'open_asset' && e.id) {
      openAsset(String(e.id));
    } else if (e.type === 'search') {
      search(String(e.query || ''));
    } else if (e.type === 'watch') {
      watch();
    }
    // 'fetch' renders via the host boundary (twin_fetch); its raw capture above is all.
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
      default: return String(e.type);
    }
  }

  // ---- what the agent perceives (§12.3) --------------------------------------
  // Everything here is a fold over the event streams above — including userActions,
  // the user's RAW behavior, from which the agent derives goals and intent.
  function perceive() {
    const recent = feed.items.slice(-40).map((it) => ({ kind: it.kind, text: it.text, options: it.options }));
    const srcs = [...sources].map(([name, s]) => ({
      name, residence: s.residence, rowcount: s.rowcount, schema: s.schema, sample: s.sample,
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
      lenses: [...lenses].map(([name, l]) => ({ name: 'lens:' + name, source: l.source, rowcount: l.rowcount })),
      activity: activityTail.slice(-8),
      userActions: userActions.slice(-10),
      feed: recent,
    });
  }

  return {
    agentTool, event, perceive, mountSource, sourceError, installSkill,
    chartSeries, chartMessage,
    // the shared mutation log (§11.3): replaying [0..total) rebuilds the DOM exactly.
    total() { return TWIN_MUT.length; },
    from(n) { return JSON.stringify(TWIN_MUT.slice(n)); },
  };
})();
globalThis.T = T;
