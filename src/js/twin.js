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

  // Recorded streams (all in the event log):
  //   feed    — the conversation/activity
  //   profile — durable facts about the user
  //   src:*   — mounted/materialized data sources (created on demand)
  const feedSrc = G.source('feed', false);
  const profileSrc = G.source('profile', false);
  const skillsSrc = G.source('skills', false);

  const feed = new Feed('feed-root');
  const profile = new ProfilePanel('profile-root');
  const sourcesPanel = new SourcesPanel('sources-root');
  const skillsPanel = new SkillsPanel('skills-root');

  G.observe(feedSrc, (delta) => {
    for (const [row] of delta.entries()) feed.append(row.asObject());
  });
  G.observe(profileSrc, (delta) => {
    for (const [row] of delta.entries()) profile.set(row.get('field'), row.get('value'));
  });
  G.observe(skillsSrc, (delta) => {
    for (const [row] of delta.entries()) skillsPanel.set(row.get('name'), row.get('title'), row.get('description'));
  });

  const sourceIds = new Map(); // name -> graph source node id
  const sources = new Map();   // name -> { residence, locator, rowcount, schema, sample }
  const skills = new Map();    // name -> { description, files } — capabilities the agent has

  let seq = 0;
  const prov = (author) => ({ author, origin: 'agent', note: '' });

  function append(kind, payload) {
    seq += 1;
    G.submit(feedSrc, ZSet.fromRows([rec(Object.assign({ seq, kind }, payload))]),
      prov(kind === 'user' ? 'user' : 'agent'));
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
    let id = sourceIds.get(name);
    if (id === undefined) { id = G.source('src:' + name, true); sourceIds.set(name, id); }
    // materialize the rebuildable in-heap view (§15.1); truth stays in the file.
    G.submit(id, ZSet.fromRows(rows.map((r) => rec(r))), prov('upstream'));
    const schema = inferSchema(rows);
    const info = {
      residence: meta.residence, locator: meta.locator,
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

  // Render a mounted source's rows as a browsable table in the data explorer (§11.7-ish,
  // a plain snapshot for now).  Proves a lens that actually pulled something is
  // explorable: the twin renders the materialized rows into 'explorer-root'.
  let explorerKeys = [];
  const MAX_EXPLORE_ROWS = 200;
  function openSource(name) {
    const s = sources.get(name);
    const id = sourceIds.get(name);
    if (!s || id === undefined) return;
    const rows = G.stateOf(id).support().slice(0, MAX_EXPLORE_ROWS).map((r) => r.asObject());
    const cols = Object.keys(s.schema);
    const push = (m) => TWIN_MUT.push(m);
    for (const k of explorerKeys) push({ op: 'remove', key: k });
    explorerKeys = [];
    const add = (m) => { push(m); explorerKeys.push(m.key); };

    add({ op: 'create', key: 'exp:note', tag: 'div', parent: 'explorer-root', index: 0 });
    push({ op: 'setAttr', key: 'exp:note', name: 'class', value: 'explorer-note' });
    push({ op: 'setText', key: 'exp:note', text: `${s.residence} · ${cols.length} columns · showing ${rows.length} of ${s.rowcount} rows` });
    add({ op: 'create', key: 'exp:tbl', tag: 'table', parent: 'explorer-root', index: 1 });
    add({ op: 'create', key: 'exp:thead', tag: 'thead', parent: 'exp:tbl', index: 0 });
    add({ op: 'create', key: 'exp:htr', tag: 'tr', parent: 'exp:thead', index: 0 });
    cols.forEach((c, i) => { add({ op: 'create', key: `exp:th:${i}`, tag: 'th', parent: 'exp:htr', index: i }); push({ op: 'setText', key: `exp:th:${i}`, text: c }); });
    add({ op: 'create', key: 'exp:tb', tag: 'tbody', parent: 'exp:tbl', index: 1 });
    rows.forEach((row, ri) => {
      add({ op: 'create', key: `exp:r:${ri}`, tag: 'tr', parent: 'exp:tb', index: ri });
      cols.forEach((c, ci) => {
        const ck = `exp:c:${ri}:${ci}`;
        add({ op: 'create', key: ck, tag: 'td', parent: `exp:r:${ri}`, index: ci });
        push({ op: 'setText', key: ck, text: row[c] == null ? '' : String(row[c]) });
      });
    });
  }

  // Install a skill (§4.1) — called by the core skills-loader from the static dir.
  function installSkill(name, meta) {
    if (skills.has(name)) return;
    const title = meta.title || name;
    skills.set(name, { title, description: meta.description || '', files: meta.files || [] });
    G.submit(skillsSrc, ZSet.fromRows([rec({ name, title, description: meta.description || '' })]), prov('core'));
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
      // read_source is effectful: handled at the Rust boundary, which calls mountSource.
      default: if (a.text) append('agent', { text: String(a.text) }); break;
    }
  }

  // ---- backward UI events (§11.4): the user steering -------------------------
  function event(json) {
    let e; try { e = JSON.parse(json); } catch (_) { return; }
    if (e.type === 'user_message' && e.text) {
      append('user', { text: String(e.text) });
    } else if (e.type === 'choose' && e.target) {
      const m = /^option:(\d+):(\d+)$/.exec(e.target);
      if (!m) return;
      const s = Number(m[1]), idx = Number(m[2]);
      const q = feed.items.find((it) => it.seq === s);
      const opt = q && q.options ? q.options[idx] : null;
      append('user', { text: opt != null ? String(opt) : `option ${idx + 1}` });
    } else if (e.type === 'open_source' && e.name) {
      openSource(String(e.name));
    }
  }

  // ---- what the agent perceives (§12.3) --------------------------------------
  function perceive() {
    const recent = feed.items.slice(-40).map((it) => ({ kind: it.kind, text: it.text, options: it.options }));
    const srcs = [...sources].map(([name, s]) => ({
      name, residence: s.residence, rowcount: s.rowcount, schema: s.schema, sample: s.sample,
    }));
    const sks = [...skills].map(([name, s]) => ({ name, description: s.description }));
    return JSON.stringify({ profile: profile.asObject(), sources: srcs, skills: sks, feed: recent });
  }

  return {
    agentTool, event, perceive, mountSource, sourceError, installSkill,
    // the shared mutation log (§11.3): replaying [0..total) rebuilds the DOM exactly.
    total() { return TWIN_MUT.length; },
    from(n) { return JSON.stringify(TWIN_MUT.slice(n)); },
  };
})();
globalThis.T = T;
