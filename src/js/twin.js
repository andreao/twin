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
  const schemaSrc = G.source('schema', false);     // inferred + annotated schema claims, per field

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
  const agenda = new Map();    // id -> { text, desc, status }
  const findings = new Map();  // id -> { severity, text, source }
  const lenses = new Map();    // name -> { source, code, rowcount, ver }
  const activityTail = [];     // recent work steps (perception window)
  const taskSteps = new Map(); // task id -> its full step history (the task's own page)
  const stepIndex = new Map(); // step seq -> the step (every step has its own page)
  const userActions = [];      // recent raw user actions, described (perception window)

  let activeTaskId = null; // which agenda item the agent is working under, if any
  G.observe(agendaSrc, (delta) => {
    for (const [row] of delta.entries()) {
      const r = row.asObject();
      const prev = agenda.get(r.id) || {};
      const text = r.text || prev.text || '';
      const desc = (r.description != null && r.description !== '') ? String(r.description) : (prev.desc || '');
      agenda.set(r.id, { text, desc, status: r.status });
      if (r.status === 'active') activeTaskId = r.id;
      else if (r.id === activeTaskId) activeTaskId = null;
      agendaPanel.set(r.id, text, r.status);
      // any open page of this task follows along: chip and now-line stay current
      for (const [p, t] of taskPanels) {
        if (t !== r.id) continue;
        const word = r.status === 'active' ? 'in progress' : r.status === 'done' ? 'done' : 'planned';
        TWIN_MUT.push({ op: 'setText', key: `${panelPfx(p)}ta:st`, text: word });
        TWIN_MUT.push({ op: 'setAttr', key: `${panelPfx(p)}ta:st`, name: 'class', value: `fd-sev task-${r.status}` });
        TWIN_MUT.push({ op: 'setAttr', key: `${panelPfx(p)}ta:now`, name: 'hidden', value: r.status === 'active' ? null : 'true' });
      }
    }
    updateAgentNow();
  });
  G.observe(activitySrc, (delta) => {
    for (const [row] of delta.entries()) {
      const r = row.asObject();
      const e = { seq: r.seq || 0, kind: r.kind || 'note', text: r.text || '', subject: r.subject || '',
        detail: r.detail || '', tone: r.tone || '', ev: r.ev || '', task: r.task || 0 };
      activityTail.push(e);
      if (activityTail.length > 60) activityTail.shift();
      // every step is addressable — it has its own page (open_step)
      stepIndex.set(e.seq, e);
      if (stepIndex.size > 400) stepIndex.delete(stepIndex.keys().next().value);
      if (e.task) {
        let arr = taskSteps.get(e.task);
        if (!arr) { arr = []; taskSteps.set(e.task, arr); }
        arr.push(e);
        if (arr.length > 250) arr.shift();
        // an open page of this task shows what's happening live
        if (e.kind === 'note') {
          for (const [p, t] of taskPanels) {
            if (t === e.task) TWIN_MUT.push({ op: 'setText', key: `${panelPfx(p)}ta:now:t`, text: e.text });
          }
        }
      }
      activityPanel.add(r.seq, stepLine(e));
    }
    updateAgentNow();
  });
  G.observe(findingsSrc, (delta) => {
    for (const [row] of delta.entries()) {
      const r = row.asObject();
      const status = r.status || 'open';
      findings.set(r.id, { severity: r.severity, text: r.text, source: r.source, status, kind: r.kind || '' });
      findingsPanel.set(r.id, r.severity, r.text, r.source, status, r.kind || '');
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
      if (r.origin) s.origin = r.origin;
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
          TWIN_MUT.push({ op: 'setText', key: `item:${s.cardSeq}:d`, text: s.description || '' });
          TWIN_MUT.push({ op: 'setText', key: `item:${s.cardSeq}:m`, text: mountMeta(s) });
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
  // ---- schema inference (§8: schema is event-sourced data, not metadata) -----
  // The twin understands its data in two passes.  This is the FIRST: a statistical
  // profiler — pure computation, no model — that runs the moment a source lands.
  // Every conclusion is a CLAIM appended to the `schema` stream (field, facts,
  // author, lineage), folded into `fieldMeta` below.  The second pass is semantic:
  // the agent's `annotate` tool appends claims about what fields MEAN, which
  // override the profiler's on fold.  Nothing is ever edited in place.
  const fieldMeta = new Map(); // `${source}|${field}` -> folded claims (last non-empty wins)
  const profiles = new Map();  // source -> per-column value sets, for cross-source link detection
  const linked = new Set();    // reference claims already made (claim once, ever)
  G.observe(schemaSrc, (delta) => {
    for (const [row] of delta.entries()) {
      const r = row.asObject();
      const k = `${r.source}|${r.field}`;
      const m = fieldMeta.get(k) || {};
      for (const key of ['type', 'role', 'ref', 'refField', 'values', 'pattern', 'coverage', 'overlap', 'empty', 'title', 'description', 'author']) {
        if (r[key] != null && r[key] !== '') m[key] = r[key];
      }
      fieldMeta.set(k, m);
    }
  });
  function claim(source, field, facts, author) {
    G.submit(schemaSrc, ZSet.fromRows([rec(Object.assign({ source, field }, facts))]), prov(author || 'profiler'));
  }

  // Mine the dominant SHAPE of a string column — tag conventions, composite ids.
  // Tiered generalization: digit runs → '#'; if that doesn't converge, uppercase
  // codes (2+ letters) → 'A' runs; if that doesn't either, the common prefix.
  // The mined template is exact enough to parse against — rows that don't match
  // the dominant shape are the interesting ones.
  function minePattern(vals) {
    if (vals.length < 5) return null;
    const sample = vals.slice(0, 400).map(String);
    const digitMask = (s) => s.replace(/[0-9]+/g, (d) => '#'.repeat(Math.min(d.length, 6)));
    const top = (sigs) => {
      const m = new Map();
      for (const t of sigs) m.set(t, (m.get(t) || 0) + 1);
      let b = null;
      for (const e of m) if (!b || e[1] > b[1]) b = e;
      return b;
    };
    const pct = (n) => Math.round((n / sample.length) * 100);
    const sigs1 = sample.map(digitMask);
    let [t, n] = top(sigs1);
    if (t.includes('#') && n / sample.length >= 0.6) return { template: t, coverage: pct(n) };
    const sigs2 = sigs1.map((s) => s.replace(/[A-Z]{2,}/g, (a) => 'A'.repeat(Math.min(a.length, 6))));
    [t, n] = top(sigs2);
    if (t.includes('#') && n / sample.length >= 0.6) return { template: t, coverage: pct(n) };
    // No full template converged — look for a DOMINANT prefix instead: cluster
    // by the leading characters, take the biggest cluster, and report its common
    // prefix with honest coverage.  A column that is 82% "VAL_##-…" and 18%
    // junk is a convention WITH violations — the violations are the insight.
    const [seed, hits] = top(sigs1.map((s) => s.slice(0, 6)));
    if (seed.length >= 4 && hits / sample.length >= 0.5) {
      const cluster = sigs1.filter((s) => s.startsWith(seed));
      let pre = cluster[0];
      for (const s of cluster) {
        let i = 0;
        while (i < pre.length && i < s.length && pre[i] === s[i]) i += 1;
        pre = pre.slice(0, i);
      }
      if (pre.length >= 4) return { template: `starts ${pre}…`, coverage: pct(cluster.length) };
    }
    return null;
  }

  // Profile one source: types, gaps, keys, constants, enums, shapes, duplicate
  // columns — each fact a schema claim.  Deterministic and model-free, so it
  // re-derives identically on every boot (mounts are not journaled).
  const PROFILE_ROWS = 2000;
  const DISTINCT_CAP = PROFILE_ROWS; // within the sample, distinct sets are never truncated
  function profileSource(name) {
    const id = sourceIds.get(name);
    const meta = sources.get(name);
    if (id === undefined || !meta || meta.kind !== 'table') return;
    const rows = G.stateOf(id).support().slice(0, PROFILE_ROWS).map((r) => r.asObject());
    if (rows.length < 2) return;
    const cols = Object.keys(meta.schema || {});
    const prof = {};
    for (const c of cols) {
      const vals = rows.map((r) => r[c]).filter((v) => v != null && v !== '');
      const distinct = new Set();
      for (const v of vals) {
        distinct.add(String(v));
        if (distinct.size > DISTINCT_CAP) break;
      }
      const capped = distinct.size > DISTINCT_CAP;
      const types = new Set(vals.map((v) => typeof v));
      const type = types.size === 1 ? [...types][0] : (types.size ? 'mixed' : 'empty');
      const unique = !capped && vals.length === rows.length && distinct.size === rows.length;
      const facts = { type };
      const emptyPct = Math.round(((rows.length - vals.length) / rows.length) * 100);
      if (rows.length > vals.length) facts.empty = emptyPct || 1;
      if (unique) facts.role = 'key';
      else if (!capped && distinct.size === 1) {
        facts.role = 'constant';
        facts.values = [...distinct][0];
      } else if (!capped && type === 'string' && distinct.size >= 2 && distinct.size <= 12 && vals.length >= distinct.size * 3) {
        facts.role = 'enum';
        facts.values = [...distinct].slice(0, 8).join(', ') + (distinct.size > 8 ? ` +${distinct.size - 8} more` : '');
      }
      if (type === 'string' && !facts.values) {
        const p = minePattern(vals);
        if (p) {
          facts.pattern = p.template;
          facts.coverage = p.coverage;
        }
      }
      prof[c] = { distinct, capped, unique, type };
      claim(name, c, facts, 'profiler');
    }
    // duplicate columns: one is a copy of the other (e.g. name = externalId).
    // Verbatim copies claim plainly; mostly-copies claim with the overlap — the
    // rows where the copy DIVERGES are the interesting ones.
    for (let i = 0; i < cols.length; i += 1) {
      for (let j = i + 1; j < cols.length; j += 1) {
        const a = cols[i];
        const b = cols[j];
        const same = rows.filter((r) => String(r[a] == null ? '' : r[a]) === String(r[b] == null ? '' : r[b])).length;
        const frac = same / rows.length;
        if (frac >= 0.6 && rows.some((r) => r[a] != null && r[a] !== '')) {
          const facts = { role: 'redundant', refField: a };
          if (frac < 0.98) facts.overlap = Math.round(frac * 100);
          claim(name, b, facts, 'profiler');
        }
      }
    }
    profiles.set(name, prof);
    linkScan();
  }

  // Cross-source (and self-) reference detection: a non-key column whose values
  // sit inside another column's unique key set is a reference — assets.parentId
  // → assets.id, timeseries.assetId → assets.id.  ';'-joined values are split
  // first and claimed as a multi-reference (events.assetIds).
  function linkScan() {
    const keyCols = [];
    for (const [sname, prof] of profiles) {
      for (const c in prof) {
        if (prof[c].unique && !prof[c].capped) keyCols.push({ source: sname, col: c, set: prof[c].distinct });
      }
    }
    for (const [sname, prof] of profiles) {
      for (const c in prof) {
        const pc = prof[c];
        if (pc.capped) continue;
        // ';'-joined values are split into their parts first (the distinct set
        // holds strings, whatever the column's nominal type)
        let parts = pc.distinct;
        let multi = false;
        if ([...pc.distinct].some((v) => v.includes(';'))) {
          multi = true;
          parts = new Set();
          for (const v of pc.distinct) for (const p of v.split(';')) if (p) parts.add(p);
        }
        // a unique plain column is a key, not a reference — but a unique
        // ';'-joined column can still multi-reference another table
        if (pc.unique && !multi) continue;
        if (parts.size < 3) continue;
        for (const k of keyCols) {
          if (k.source === sname && k.col === c) continue;
          const key = `${sname}.${c}->${k.source}.${k.col}`;
          if (linked.has(key)) continue;
          let hit = 0;
          for (const v of parts) if (k.set.has(v)) hit += 1;
          if (hit / parts.size >= 0.95) {
            linked.add(key);
            claim(sname, c, { role: multi ? 'refs' : 'ref', ref: k.source, refField: k.col }, 'profiler');
          }
        }
      }
    }
  }

  // Semantics FLOW with the data (§8 lineage): a lens's column means what it
  // meant upstream.  Statistics stay local — every derivation is re-profiled —
  // but a field's human title and description inherit along the from-chain
  // until someone annotates the derived field itself.
  function semanticsOf(name, field) {
    let cur = name, guard = 0;
    while (cur && guard++ < 12) {
      const m = fieldMeta.get(`${cur}|${field}`);
      if (m && (m.title || m.description)) return m;
      cur = (sources.get(cur) || {}).from;
    }
    return null;
  }
  // The profile of one source as compact human lines — what the agent perceives
  // instead of guessing from sample rows, and what the field guide renders.
  function fieldLines(name) {
    const cols = Object.keys((sources.get(name) || {}).schema || {});
    const out = [];
    for (const c of cols) {
      const m = fieldMeta.get(`${name}|${c}`) || {};
      const sem = semanticsOf(name, c) || {};
      const bits = [];
      if (sem.title) bits.push(`“${sem.title}”`);
      if (m.type && m.type !== 'empty') bits.push(m.type);
      if (m.role === 'key') bits.push('unique key');
      else if (m.role === 'ref') bits.push(`references ${srcTitle(m.ref)}.${m.refField}`);
      else if (m.role === 'refs') bits.push(`multi-references ${srcTitle(m.ref)}.${m.refField} (';'-joined)`);
      else if (m.role === 'enum') bits.push(`one of: ${m.values}`);
      else if (m.role === 'constant') bits.push(`always “${m.values}”`);
      else if (m.role === 'redundant') bits.push(m.overlap ? `duplicates ${m.refField} on ${m.overlap}% of rows` : `duplicates ${m.refField}`);
      if (m.pattern) bits.push(`pattern ${m.pattern} (${m.coverage}% of rows)`);
      if (m.empty) bits.push(m.empty >= 100 ? 'always empty' : `${m.empty}% empty`);
      if (sem.description) bits.push(sem.description);
      // a field the profiler had nothing to say about still EXISTS — list it with
      // its schema type, or the agent can neither see nor annotate it and the
      // documented-count never reaches the schema's own
      if (!bits.length) bits.push(String((sources.get(name).schema || {})[c] || 'unprofiled'));
      out.push(`${c}: ${bits.join(' · ')}`);
    }
    return out;
  }
  const fieldTitle = (src, c) => {
    const m = semanticsOf(src, c);
    return (m && m.title) || c;
  };

  // Deterministic linking helpers (§8.1: explicit joins, no similarity scores) —
  // in scope for lens code, and used by the depth view to match picks to a well:
  // normalizeWellName reconciles naming across systems ("NO 15/9-F-14 B" ⇄
  // "15/9-F-14 B"); inInterval is the depth/time interval-join primitive.
  const normalizeWellName = (s) => String(s == null ? '' : s).toUpperCase()
    .replace(/^NO\s+/, '').replace(/\s*\/\s*/g, '/').replace(/\s+/g, ' ').trim();
  const inInterval = (v, lo, hi) => {
    const n = Number(v);
    return Number.isFinite(n) && n >= Number(lo) && n <= Number(hi);
  };

  // The twin watching its own understanding: a mounted source whose fields lack
  // documented meaning IS a data issue — filed as a finding the moment the source
  // lands (deterministic, model-free), updated as annotations arrive, and resolved
  // by itself when the last gap closes.  Ids are stable hashes of the source name,
  // never findingSeq, so replayed agent findings keep their ids across boots.
  const schemaFindings = new Map(); // source name -> finding id
  function schemaGapCheck(name) {
    const meta = sources.get(name);
    if (!meta || meta.kind !== 'table' || String(name).startsWith('lens:')) return;
    const cols = Object.keys(meta.schema || {});
    if (!cols.length) return;
    const missing = cols.filter((c) => !((fieldMeta.get(`${name}|${c}`) || {}).title));
    let id = schemaFindings.get(name);
    if (id == null) {
      let h = 0;
      for (const ch of String(name)) h = (h * 31 + ch.charCodeAt(0)) % 100000;
      id = 900000 + h;
      schemaFindings.set(name, id);
    }
    const prev = findings.get(id);
    if (missing.length) {
      const list = missing.slice(0, 6).join(', ') + (missing.length > 6 ? ` +${missing.length - 6} more` : '');
      const text = `${srcTitle(name)}: ${missing.length} of ${cols.length} fields have no documented meaning (${list})`;
      if (prev && prev.text === text && prev.status !== 'resolved') return;
      G.submit(findingsSrc, ZSet.fromRows([rec({ id, severity: 'info', text, source: name, kind: 'schema', status: 'open' })]), prov('twin'));
    } else if (prev && prev.status !== 'resolved') {
      G.submit(findingsSrc, ZSet.fromRows([rec({ id, severity: 'info', text: prev.text, source: name, kind: 'schema', status: 'resolved' })]), prov('twin'));
      logStep('finished', `documented every field of ${srcTitle(name)}`, { subject: `schema:${name}`, ev: { type: 'open_source', name } });
    }
  }

  // The agent's `annotate` tool — the SEMANTIC pass: what a field means, in words
  // the profiler cannot know.  An agent claim on the same stream, so it overrides
  // the statistical one on fold and replays from the journal like any tool call.
  function doAnnotate(a) {
    const src = resolveSourceName(a.source);
    const asked = String(a.field || '');
    const cols = Object.keys((sources.get(src) || {}).schema || {});
    if (!cols.length || !asked) {
      logStep('failed', `${String(a.source || '')}.${asked}`, { subject: `schema:${src}.${asked}`, tone: 'error', detail: 'no such source to annotate; mount or build it first' });
      return;
    }
    // meet the model halfway on casing/punctuation ("md" → "MD")
    const canon = (x) => String(x).toLowerCase().replace(/[^a-z0-9]/g, '');
    const field = cols.includes(asked) ? asked : cols.find((k) => canon(k) === canon(asked));
    if (!field) {
      logStep('failed', `${srcTitle(src)} · ${asked}`, { subject: `schema:${src}.${asked}`, tone: 'error', detail: `no field “${asked}” — fields are: ${cols.join(', ')}` });
      return;
    }
    const facts = { title: String(a.title || ''), description: String(a.description || '') };
    if (a.ref) {
      facts.role = 'ref';
      facts.ref = resolveSourceName(a.ref);
    }
    claim(src, field, facts, 'agent');
    logStep('described', `${srcTitle(src)} · ${field}`, { subject: `schema:${src}.${field}`, ev: { type: 'open_source', name: src } });
    schemaGapCheck(src); // one field closer — the schema-gap finding follows along
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
      info.cardSeq = workCard(`Mounted ${info.title}`, info.description, mountMeta(info), { type: 'open_source', name: 'documents' });
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
    profileSource(name); // first-pass schema inference, the moment the data lands
    schemaGapCheck(name); // an undocumented schema is an ISSUE, on the board from birth
    ensureSchemaTile(); // the board's way into the schema map, from the first mount
    info.cardSeq = workCard(`Mounted ${info.title}`, info.description, mountMeta(info), { type: 'open_source', name });
  }

  // One board tile into the schema map, created with the first mount — the twin's
  // structure is browsable the moment it has any.
  let schemaTileMade = false;
  function ensureSchemaTile() {
    if (schemaTileMade) return;
    schemaTileMade = true;
    TWIN_MUT.push({ op: 'create', key: 'tile:schemamap', tag: 'div', parent: 'board-root', index: 0 });
    TWIN_MUT.push({ op: 'setAttr', key: 'tile:schemamap', name: 'class', value: 'src-row' });
    TWIN_MUT.push({ op: 'setAttr', key: 'tile:schemamap', name: 'data-title', value: 'Schema map' });
    TWIN_MUT.push({ op: 'create', key: 'tile:schemamap:n', tag: 'div', parent: 'tile:schemamap', index: 0 });
    TWIN_MUT.push({ op: 'setAttr', key: 'tile:schemamap:n', name: 'class', value: 'src-n' });
    TWIN_MUT.push({ op: 'setText', key: 'tile:schemamap:n', text: 'Schema map' });
    TWIN_MUT.push({ op: 'create', key: 'tile:schemamap:d', tag: 'div', parent: 'tile:schemamap', index: 1 });
    TWIN_MUT.push({ op: 'setAttr', key: 'tile:schemamap:d', name: 'class', value: 'src-d' });
    TWIN_MUT.push({ op: 'setText', key: 'tile:schemamap:d', text: 'every source and lens, their fields, and how they reference each other' });
  }

  // The quiet facts line of a mount card — separate from the description, so prose
  // never runs into numbers.
  function mountMeta(s) {
    const origin = s.origin ? ` · from ${s.origin}` : '';
    return s.kind === 'documents'
      ? `${(s.docs || []).length} files · ${residenceLabel(s)}${origin}`
      : `${s.rowcount} rows · ${Object.keys(s.schema || {}).length} columns · ${residenceLabel(s)}${origin}`;
  }

  // A compact ACTION CARD in the chat: work that was done (by the agent or the
  // system on its behalf), titled like a human would say it, openable when it
  // points at something.  This is how background work reads as history from 0.
  function workCard(title, sub, meta, openEv, tone) {
    const sq = append('card', { text: title, sub: sub || '', meta: meta || '', tone: tone || '' });
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
  const openPanels = new Set(); // which columns are open — twin state, in the log
  const taskPanels = new Map(); // panel index -> task id it shows (for live updates)
  const agentPanels = new Set(); // panels showing the What's-happening page (live now-line)
  const fmtNum = (n) => (Math.abs(n) >= 1000 || Number.isInteger(n)) ? n.toFixed(0) : Number(n.toPrecision(4)).toString();
  // thousands separators by hand — the embedded V8 has no ICU, so no toLocaleString
  const fmtInt = (n) => String(Math.trunc(Number(n) || 0)).replace(/\B(?=(\d{3})+$)/g, ',');

  // The agent refers to sources and lenses the way a person does — by title, by
  // slug, with or without the "lens:" prefix.  Meet it halfway instead of failing:
  // "lens:Critical Machinery" resolves to lens:critical-machinery.
  function resolveSourceName(n) {
    const name = String(n || '');
    if (sources.has(name) || sourceIds.has(name)) return name;
    if (dashboards.has(name) || dashboards.has(dashSlug(name))) return dashboards.has(name) ? name : dashSlug(name);
    for (const [slug, d] of dashboards) {
      if (d.title.toLowerCase() === name.toLowerCase()) return slug;
    }
    const bare = name.replace(/^lens:\s*/i, '').trim();
    const slug = 'lens:' + bare.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
    if (sources.has(slug) || sourceIds.has(slug)) return slug;
    for (const [nm, l] of lenses) {
      if ((l.title || '').toLowerCase() === bare.toLowerCase()) return 'lens:' + nm;
    }
    return name;
  }

  // Panel addressing: a column can split vertically — the bottom half is the
  // same column index plus 0.5, its keys prefixed p<i>s: and its root
  // panel:<i>:sub.  Everything that compares panel indexes keeps working
  // because the halves order naturally between the columns.
  const panelPfx = (p) => (Number.isInteger(p) ? `p${p}:` : `p${Math.floor(p)}s:`);
  const panelRoot = (p) => (Number.isInteger(p) ? `panel:${p}:body` : `panel:${Math.floor(p)}:sub`);

  // Emit a close marker for every open column ≥ `from` (one marker at the lowest
  // index — the client tears down that column and everything right of it).
  function markClosed(from) {
    const above = [...openPanels].filter((pi) => pi >= from);
    if (!above.length) return;
    above.forEach((pi) => openPanels.delete(pi));
    TWIN_MUT.push({ op: 'setAttr', key: panelRoot(Math.min(...above)), name: 'data-closed', value: 'true' });
  }

  // The user closed a column: clear its content (and everything to its right) and
  // mark it closed in the log, so a replaying client rebuilds the same stack.
  function closePanel(from) {
    const f = Number(from) || 0;
    for (const [pi, ks] of [...panelKeys]) {
      if (pi >= f) {
        for (const k of ks) TWIN_MUT.push({ op: 'remove', key: k });
        panelKeys.delete(pi);
      }
    }
    for (const pi of [...taskPanels.keys()]) if (pi >= f) taskPanels.delete(pi);
    for (const pi of [...agentPanels]) if (pi >= f) agentPanels.delete(pi);
    purgeLivePanels(f);
    markClosed(f);
  }

  function viewer(panel, title, ev) {
    const p = Number(panel) || 0;
    // opening at p invalidates p and everything to its right (a column's bottom
    // half sits at p+0.5, so opening the top half clears its split too)
    for (const [pi, ks] of [...panelKeys]) {
      if (pi >= p) {
        for (const k of ks) TWIN_MUT.push({ op: 'remove', key: k });
        panelKeys.delete(pi);
      }
    }
    for (const pi of [...taskPanels.keys()]) if (pi >= p) taskPanels.delete(pi);
    for (const pi of [...agentPanels]) if (pi >= p) agentPanels.delete(pi);
    purgeLivePanels(p);
    markClosed(Math.floor(p) + 1);
    openPanels.add(p);
    const fresh = [];
    panelKeys.set(p, fresh);
    const root = panelRoot(p);
    TWIN_MUT.push({ op: 'setAttr', key: root, name: 'data-closed', value: null });
    // stamp the column with what it shows: a replaying client rebuilds the panel
    // chrome (title, star target) from these marks — open columns survive reloads
    // because the UI state IS in the log, not in the browser.
    TWIN_MUT.push({ op: 'setAttr', key: root, name: 'data-title', value: String(title || '') });
    if (ev) TWIN_MUT.push({ op: 'setAttr', key: root, name: 'data-ev', value: JSON.stringify(ev) });
    const K = (k) => `${panelPfx(p)}${k}`;
    return {
      panel: p,
      key: K,
      add(tag, k, parent, index) {
        const kk = K(k);
        TWIN_MUT.push({ op: 'create', key: kk, tag, parent: parent == null ? root : K(parent), index });
        fresh.push(kk);
      },
      set(k, name, value) { TWIN_MUT.push({ op: 'setAttr', key: K(k), name, value: value == null ? null : String(value) }); },
      text(k, t) { TWIN_MUT.push({ op: 'setText', key: K(k), text: String(t) }); },
    };
  }

  function openSource(name, mode, panel, title) {
    const d = dashboards.get(name);
    if (d) return openDashboard(name, d, panel, title);
    const s = sources.get(name);
    if (!s) return;
    const V = viewer(panel, title || srcTitle(name), { type: 'open_source', name, mode: mode || '' });
    if (s.kind === 'documents') return renderDocuments(V, s);
    if (mode === 'schema') return renderSchema(V, name, s);
    if (mode === 'tree' && name === 'assets') return renderTree(V, name, s);
    if (mode === 'timeline' && name === 'events') return renderTimeline(V, name, s);
    if (mode === 'depth') return renderDepthPage(V, name);
    if (AGG_VIEWS[mode]) return renderAggPanel(V, name, s, mode);
    renderTable(V, name, s);
  }

  function renderDepthPage(V, name) {
    renderModes(V, name, 'depth', 0);
    V.add('div', 'exp:depth', null, 1);
    renderDepthInto(V.key('exp:depth'), V.key('exp:depth') + ':d', name, {});
  }

  // per-source view modes (§11.15: several lenses over the same data) — every
  // table source carries a Schema view; the rest are EARNED from the data's own
  // shape: a date column earns a calendar, a small enum a board, numbers a
  // histogram (two of them a scatter), lat/lon a map, long text a reading view,
  // a depth column the depth log.  The switcher shows what the data can do.
  function modesFor(name) {
    const rows = sampleRows(name);
    const modes = [['table', 'Table']];
    if (name === 'assets') modes.push(['tree', 'Hierarchy']);
    if (name === 'events') modes.push(['timeline', 'Timeline']);
    if (dateFieldOf(rows)) modes.push(['calendar', 'Calendar']);
    if (laneFieldOf(rows)) modes.push(['kanban', 'Board']);
    const nums = numericFieldsOf(rows);
    if (nums.length >= 1) modes.push(['histogram', 'Histogram']);
    if (nums.length >= 2) modes.push(['scatter', 'Scatter']);
    if (geoFieldsOf(rows)) modes.push(['map', 'Map']);
    if (proseFieldOf(rows)) modes.push(['reading', 'Reading']);
    if (depthFieldOf(rows)) modes.push(['depth', 'Depth log']);
    modes.push(['schema', 'Schema']);
    return modes;
  }
  function renderModes(V, name, current, index) {
    const modes = modesFor(name);
    V.add('div', 'exp:modes', null, index || 0); V.set('exp:modes', 'class', 'view-modes');
    modes.forEach(([m, lbl], i) => { const k = `mode:${name}:${m}`; V.add('button', k, 'exp:modes', i); V.set(k, 'class', 'mode-btn' + (m === current ? ' active' : '')); V.text(k, lbl); });
  }

  // Every view of a source opens the same way: what it IS (the description),
  // where it CAME FROM (the derivation chain — each ancestor a chip you can
  // open, the code behind a toggle), then how to look at it (the switcher).
  // Field structure lives on the Schema page, not in the table header.
  function renderSourceHeader(V, name, s, current) {
    let i = 0;
    if (s.description) {
      V.add('div', 'exp:desc', null, i++); V.set('exp:desc', 'class', 'src-desc');
      V.text('exp:desc', s.description);
    }
    if (s.code) {
      V.add('div', 'exp:lin', null, i++); V.set('exp:lin', 'class', 'lens-chain');
      const parts = chainOf(name);
      parts.forEach((p, pi) => {
        if (pi) { V.add('span', `exp:lsep${pi}`, 'exp:lin', pi * 2 - 1); V.set(`exp:lsep${pi}`, 'class', 'chain-sep'); V.text(`exp:lsep${pi}`, '→'); }
        const k = `exp:lp${pi}`;
        const here = pi === parts.length - 1;
        V.add(here ? 'span' : 'button', k, 'exp:lin', pi * 2);
        V.set(k, 'class', 'chain-part' + (here ? ' here' : ''));
        if (!here) { V.set(k, 'data-name', p.name); V.set(k, 'data-title', p.title); }
        V.text(k, p.title);
      });
      V.add('button', 'exp:codebtn', 'exp:lin', parts.length * 2); V.set('exp:codebtn', 'class', 'code-toggle'); V.text('exp:codebtn', 'code');
      V.add('div', 'exp:code', null, i++); V.set('exp:code', 'class', 'lens-code'); V.set('exp:code', 'hidden', 'true');
      V.text('exp:code', s.code);
    }
    renderModes(V, name, current, i++);
    return i;
  }

  // ---- what a source's data can DO: cheap shape detection over a sample -------
  // Each detector reads a bounded sample; the switcher (modesFor) and the agent's
  // show tool both build on these, so a view is offered exactly when it can render.
  const SAMPLE_N = 500;
  function sampleRows(name) {
    const id = sourceIds.get(name);
    if (id === undefined) return [];
    const out = [];
    for (const r of G.stateOf(id).support()) {
      out.push(r.asObject());
      if (out.length >= SAMPLE_N) break;
    }
    return out;
  }
  const fieldsOf = (rows) => (rows.length ? Object.keys(rows[0]) : []);
  // a time is an epoch-ms number (2000..2100) or an ISO-dated string
  const asWhen = (v) => {
    if (typeof v === 'number' && v > 946684800000 && v < 4102444800000) return v;
    if (typeof v === 'string' && /^\d{4}-\d{2}-\d{2}/.test(v)) { const t = Date.parse(v); return Number.isNaN(t) ? null : t; }
    return null;
  };
  function dateFieldOf(rows) {
    for (const f of fieldsOf(rows)) {
      const vals = rows.map((r) => r[f]).filter((v) => v != null && v !== '');
      if (vals.length >= 3 && vals.filter((v) => asWhen(v) != null).length >= vals.length * 0.6) return f;
    }
    return null;
  }
  // a lane field: a small enum (2..12 values), status-flavored names first
  function laneFieldOf(rows) {
    if (rows.length < 4) return null;
    const candidates = [];
    for (const f of fieldsOf(rows)) {
      const vals = rows.map((r) => r[f]).filter((v) => v != null && v !== '' && typeof v !== 'object');
      if (vals.length < rows.length * 0.5) continue;
      const distinct = new Set(vals.map(String));
      if (distinct.size >= 2 && distinct.size <= 12 && vals.length >= distinct.size * 3) {
        candidates.push([f, /status|state|phase|stage|type|kind|severity|priority|category/i.test(f) ? 0 : 1, distinct.size]);
      }
    }
    candidates.sort((a, b) => a[1] - b[1] || a[2] - b[2]);
    return candidates.length ? candidates[0][0] : null;
  }
  function numericFieldsOf(rows) {
    const out = [];
    for (const f of fieldsOf(rows)) {
      const vals = rows.map((r) => r[f]).filter((v) => v != null && v !== '');
      if (!vals.length || vals.some((v) => typeof v !== 'number')) continue;
      if (asWhen(vals[0]) != null) continue; // times chart as calendars, not histograms
      if (new Set(vals).size < 2) continue;  // constants say nothing
      out.push(f);
    }
    return out;
  }
  function geoFieldsOf(rows) {
    const fs = fieldsOf(rows);
    const lat = fs.find((f) => /^lat/i.test(f) && rows.every((r) => r[f] == null || (typeof r[f] === 'number' && Math.abs(r[f]) <= 90)));
    const lon = fs.find((f) => /^(lon|lng)/i.test(f) && rows.every((r) => r[f] == null || (typeof r[f] === 'number' && Math.abs(r[f]) <= 180)));
    return lat && lon ? { lat, lon } : null;
  }
  // prose: a string field long enough to READ, not scan (reports, remarks, notes)
  function proseFieldOf(rows) {
    let best = null, bestAvg = 0;
    for (const f of fieldsOf(rows)) {
      const vals = rows.map((r) => r[f]).filter((v) => typeof v === 'string' && v.length);
      if (vals.length < Math.max(2, rows.length * 0.3)) continue;
      const avg = vals.reduce((a, v) => a + v.length, 0) / vals.length;
      if (avg > 120 && avg > bestAvg) { best = f; bestAvg = avg; }
    }
    return best;
  }
  // the field a card or section is NAMED by: title-flavored names first (in THIS
  // order — an id names a row last, not first), else the first short string
  function titleFieldOf(rows, skip) {
    const fs = fieldsOf(rows).filter((f) => f !== skip);
    for (const want of ['title', 'name', 'label', 'summary', 'subject', 'externalid', 'well', 'id']) {
      const f = fs.find((x) => x.toLowerCase() === want && rows.some((r) => r[x] != null && r[x] !== ''));
      if (f) return f;
    }
    return fs.find((f) => rows.some((r) => typeof r[f] === 'string' && r[f].length && r[f].length < 80)) || fs[0];
  }

  // ---- virtualized live tables (§11.7 as a viewer): the DOM holds a fixed POOL
  // of rows riding between two spacer rows, so the browser cost is the window, not
  // the dataset.  The client reports where it scrolled ({type:"viewport"} — pure
  // view state: never journaled, never in the raw input stream); the server moves
  // the window.  Data deltas patch the window in place through a graph observer —
  // a growing source is just a table whose spacers lengthen.  Scrolling costs
  // setText on ~a screenful; a delta costs the rows it actually touches.
  const ROW_H = 30;           // pinned by the .vtable css — the scroll↔row mapping
  const POOL_MAX = 120;
  const liveTables = new Map();     // panel -> table state
  const tableObserved = new Set();  // graph node ids already wired to liveTables

  function purgeLivePanels(from) {
    for (const p of [...liveTables.keys()]) if (p >= from) liveTables.delete(p);
    for (const p of [...liveViews.keys()]) if (p >= from) liveViews.delete(p);
  }

  function renderTable(V, name, s) {
    const id = sourceIds.get(name);
    if (id === undefined) return;
    const cols = Object.keys(s.schema);
    const chartable = name === 'timeseries'; // a sensor row → chart its datapoints
    let i = renderSourceHeader(V, name, s, 'table');
    V.add('div', 'exp:note', null, i++); V.set('exp:note', 'class', 'explorer-note');
    // the table scrolls by itself — the header above stays put; data-rowh marks
    // the scroller as virtualized for the client's viewport reporter
    V.add('div', 'exp:scroll', null, i);
    V.set('exp:scroll', 'class', 'table-scroll vtable');
    V.set('exp:scroll', 'data-rowh', ROW_H);
    V.add('table', 'exp:tbl', 'exp:scroll', 0);
    V.add('thead', 'exp:thead', 'exp:tbl', 0);
    V.add('tr', 'exp:htr', 'exp:thead', 0);
    cols.forEach((c, ci) => {
      V.add('th', `exp:th:${ci}`, 'exp:htr', ci);
      V.text(`exp:th:${ci}`, fieldTitle(name, c));
      const sem = semanticsOf(name, c);
      if (sem) V.set(`exp:th:${ci}`, 'title', sem.title ? `${c}${sem.description ? ' · ' + sem.description : ''}` : sem.description);
    });
    V.add('tbody', 'exp:tb', 'exp:tbl', 1);
    V.add('tr', 'exp:sp:t', 'exp:tb', 0); V.set('exp:sp:t', 'class', 'vspace');
    V.add('td', 'exp:sp:t:c', 'exp:sp:t', 0); V.set('exp:sp:t:c', 'colspan', cols.length);
    V.add('tr', 'exp:sp:b', 'exp:tb', 1); V.set('exp:sp:b', 'class', 'vspace');
    V.add('td', 'exp:sp:b:c', 'exp:sp:b', 0); V.set('exp:sp:b:c', 'colspan', cols.length);
    const t = {
      name, id, cols, chartable, V,
      first: 0, count: 40, pool: 0,
      order: G.stateOf(id).support(),   // arrival order; deltas maintain it below
      cellText: new Map(), rowMeta: new Map(), hidden: new Set(),
      spTop: -1, spBot: -1, note: '',
    };
    liveTables.set(V.panel, t);
    observeTableNode(id);
    layoutTable(t);
  }

  // render the current window: grow the pool as needed, fill cells (no-op
  // suppressed), size the spacers, refresh the note line
  function layoutTable(t) {
    const s = sources.get(t.name) || {};
    const total = t.order.length;
    t.first = Math.max(0, Math.min(t.first, Math.max(0, total - t.count)));
    const shown = Math.max(0, Math.min(t.count, total - t.first));
    const V = t.V;
    // the pool only grows (to the largest window seen), rows beyond it hide
    for (let pi = t.pool; pi < shown; pi++) {
      const rk = `exp:r:${pi}`;
      V.add('tr', rk, 'exp:tb', 1 + pi); // after the top spacer
      for (let ci = 0; ci < t.cols.length; ci++) V.add('td', `exp:c:${pi}:${ci}`, rk, ci);
    }
    t.pool = Math.max(t.pool, shown);
    for (let pi = 0; pi < t.pool; pi++) {
      const rk = `exp:r:${pi}`;
      if (pi >= shown) {
        if (!t.hidden.has(pi)) { V.set(rk, 'hidden', 'true'); t.hidden.add(pi); }
        continue;
      }
      if (t.hidden.has(pi)) { V.set(rk, 'hidden', null); t.hidden.delete(pi); }
      const row = t.order[t.first + pi];
      const o = row.asObject();
      if (t.chartable) {
        const series = o.id == null ? '' : String(o.id);
        if (t.rowMeta.get(pi) !== series) {
          t.rowMeta.set(pi, series);
          V.set(rk, 'class', 'chartable');
          V.set(rk, 'data-series', series);
          V.set(rk, 'data-label', o.externalId || o.name || series);
        }
      }
      for (let ci = 0; ci < t.cols.length; ci++) {
        const v = o[t.cols[ci]];
        const text = v == null ? '' : String(v);
        const ck = `${pi}:${ci}`;
        if (t.cellText.get(ck) !== text) { t.cellText.set(ck, text); V.text(`exp:c:${pi}:${ci}`, text); }
      }
    }
    const top = t.first * ROW_H, bot = Math.max(0, total - t.first - shown) * ROW_H;
    if (t.spTop !== top) { t.spTop = top; V.set('exp:sp:t:c', 'style', `height:${top}px`); }
    if (t.spBot !== bot) { t.spBot = bot; V.set('exp:sp:b:c', 'style', `height:${bot}px`); }
    const res = residenceLabel(s);
    const bits = [];
    if (res !== 'derived') bits.push(res); // every lens is derived — saying so is noise
    bits.push(`${t.cols.length} columns`);
    bits.push(total
      ? `rows ${fmtInt(t.first + 1)}–${fmtInt(t.first + shown)} of ${fmtInt(total)}`
      : 'no rows yet');
    if ((s.rowcount || 0) > total) bits.push(`${fmtInt(s.rowcount)} in the file`);
    if (t.chartable) bits.push('click a sensor to chart it');
    const note = bits.join(' · ');
    if (t.note !== note) { t.note = note; V.text('exp:note', note); }
  }

  // one observer per graph node feeds every live table showing it — a delta
  // maintains the cached order and re-lays only the windows it can touch
  function observeTableNode(id) {
    if (tableObserved.has(id)) return;
    tableObserved.add(id);
    G.observe(id, (delta) => {
      for (const t of liveTables.values()) {
        if (t.id !== id) continue;
        for (const [row, w] of delta.entries()) {
          if (w > 0) t.order.push(row);
          else {
            const k = zkey(row);
            const idx = t.order.findIndex((r) => zkey(r) === k);
            if (idx >= 0) t.order.splice(idx, 1);
          }
        }
        layoutTable(t);
      }
      // open aggregate views (calendar, board, histogram…) redraw whole — they
      // are bounded DOM by construction, so a redraw is a screenful, not a scan;
      // a dashboard subscribes to EVERY source its embedded views read
      for (const [, v] of [...liveViews]) {
        if (v.id === id || (v.ids && v.ids.has(id))) v.rerender();
      }
    });
  }

  // the client told us where a table scrolled — move that window (view state
  // only: the caller routes this around the journal and the raw input stream)
  function tableViewport(e) {
    let p = Number(e.panel) || 0;
    if (e.sub) p += 0.5;
    const t = liveTables.get(p);
    if (!t) return;
    const count = Math.max(10, Math.min(POOL_MAX, Math.floor(Number(e.count) || t.count)));
    const first = Math.max(0, Math.floor(Number(e.first) || 0));
    if (first === t.first && count === t.count) return;
    t.first = first;
    t.count = count;
    layoutTable(t);
  }

  // rows appended at the boundary (a watched file grew, a peer streamed) — the
  // graph takes them as an ordinary +delta; open tables patch via the observer.
  // With no rows it is a counts-only move (a partial mount's file grew).
  function appendRows(name, meta, rows) {
    const s = sources.get(name);
    if (!s) return;
    if (meta && meta.rowcount != null) s.rowcount = meta.rowcount;
    if (rows && rows.length) {
      const id = sourceIds.get(name);
      if (id !== undefined) {
        G.submit(id, ZSet.fromRows(rows.map((r) => rec(r))), prov('upstream'));
        s.materialized = (s.materialized || 0) + rows.length;
        if (s.rowcount != null && s.materialized > s.rowcount) s.rowcount = s.materialized;
      }
    }
    sourcesPanel.set(name, s); // the board tile's counts stay live
    for (const t of liveTables.values()) if (t.name === name) layoutTable(t);
  }

  // ---- the rest of the visualization set: every view is a lens over the SAME
  // rows (§11.15), aggregated in the graph and rendered as bounded DOM — a
  // calendar is ~40 cells whatever the year held, a board caps its cards, a
  // histogram is its bins.  Each has an Into form so the agent can present it
  // inline in the conversation, and a panel mode wired into the switcher.
  // Open aggregate views are LIVE: a delta on their node re-renders the panel.
  const liveViews = new Map(); // panel -> { id, rerender }

  // the model names fields loosely ("Status", "start time") — resolve against
  // the real keys and their human titles before giving up
  function resolveField(name, given, rows) {
    if (!given) return null;
    const real = fieldsOf(rows);
    const canon = (x) => String(x).toLowerCase().replace(/[^a-z0-9]/g, '');
    return real.find((k) => k === given)
      || real.find((k) => canon(k) === canon(given))
      || real.find((k) => canon(fieldTitle(name, k)) === canon(given))
      || null;
  }

  // ---- markdown, rendered (the agent's rich-text surface): a safe subset —
  // headings, lists, fenced code, bold/italic/code, http links — as mutations.
  function mdInline(b, parent, text, kb) {
    let i = 0;
    const parts = String(text).split(/(\*\*[^*]+\*\*|\*[^*\n]+\*|`[^`\n]+`|\[[^\]\n]+\]\(https?:\/\/[^\s)]+\))/g);
    for (const part of parts) {
      if (!part) continue;
      const k = `${kb}i${i++}`;
      let m;
      if ((m = /^\*\*([^*]+)\*\*$/.exec(part))) { b.add('strong', k, parent, i); b.text(k, m[1]); }
      else if ((m = /^\*([^*\n]+)\*$/.exec(part))) { b.add('em', k, parent, i); b.text(k, m[1]); }
      else if ((m = /^`([^`\n]+)`$/.exec(part))) { b.add('code', k, parent, i); b.text(k, m[1]); }
      else if ((m = /^\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)$/.exec(part))) {
        b.add('a', k, parent, i); b.text(k, m[1]); b.set(k, 'href', m[2]); b.set(k, 'target', '_blank'); b.set(k, 'rel', 'noopener');
      } else { b.add('span', k, parent, i); b.text(k, part); }
    }
  }
  // `seen` (optional) collects the graph node ids of every EMBEDDED view's
  // source, so a hosting page (a dashboard) can subscribe and stay live.
  function renderMarkdownInto(root, pfx, text, seen) {
    const b = vbuild(root, pfx);
    b.add('div', 'md', null, 0); b.set('md', 'class', 'md-body');
    const lines = String(text || '').split('\n');
    let i = 0, blk = 0, list = null, para = [];
    const flushPara = () => {
      if (!para.length) return;
      const k = `p${blk++}`;
      b.add('p', k, 'md', blk); mdInline(b, k, para.join(' '), k);
      para = [];
    };
    const flushList = () => { list = null; };
    while (i < lines.length) {
      const ln = lines[i];
      if (/^```/.test(ln)) {
        flushPara(); flushList();
        const info = ln.slice(3).trim().toLowerCase();
        const code = [];
        i++;
        while (i < lines.length && !/^```/.test(lines[i])) code.push(lines[i++]);
        i++;
        // COMPOSITION: a fenced block whose language is `view` embeds a real,
        // rendered view — the same JSON args as the show tool — inside the text
        if (info === 'view') { renderEmbed(b, blk++, code.join('\n'), seen); continue; }
        const k = `c${blk++}`;
        b.add('pre', k, 'md', blk); b.text(k, code.join('\n'));
        continue;
      }
      let m;
      if ((m = /^(#{1,4})\s+(.*)$/.exec(ln))) {
        flushPara(); flushList();
        const k = `h${blk++}`;
        b.add('div', k, 'md', blk); b.set(k, 'class', `md-h${m[1].length}`); mdInline(b, k, m[2], k);
      } else if ((m = /^\s*(?:[-*]|\d+\.)\s+(.*)$/.exec(ln))) {
        flushPara();
        const ordered = /^\s*\d+\./.test(ln);
        if (!list || list.ordered !== ordered) {
          const lk = `l${blk++}`;
          b.add(ordered ? 'ol' : 'ul', lk, 'md', blk);
          list = { key: lk, n: 0, ordered };
        }
        const k = `${list.key}x${list.n}`;
        b.add('li', k, list.key, list.n++); mdInline(b, k, m[1], k);
      } else if (/^\s*$/.test(ln)) {
        flushPara(); flushList();
      } else {
        flushList(); para.push(ln.trim());
      }
      i++;
    }
    flushPara();
  }

  // what a view block (or a show call) may name, and what it means here
  const EMBED_ALIAS = {
    table: 'table',
    calendar: 'calendar',
    kanban: 'kanban', board: 'kanban',
    histogram: 'histogram', hist: 'histogram', distribution: 'histogram',
    scatter: 'scatter',
    map: 'map', geo: 'map',
    reading: 'reading',
  };

  // one embedded view inside rendered markdown: parse the block's JSON, resolve
  // the source, render the real component into a framed slot — a bad block
  // degrades to a readable note plus the code itself, never a blank hole
  function renderEmbed(b, blk, jsonText, seen) {
    const k = `e${blk}`;
    const slot = b.add('div', k, 'md', blk);
    b.set(k, 'class', 'md-embed');
    let a = null;
    try { a = JSON.parse(jsonText); } catch (_) { /* handled below */ }
    const mode = a && EMBED_ALIAS[String(a.view || '').toLowerCase()];
    if (!mode) {
      b.add('div', `${k}n`, k, 0); b.set(`${k}n`, 'class', 'explorer-note');
      b.text(`${k}n`, 'this view block did not parse (expected JSON with a "view")');
      b.add('pre', `${k}p`, k, 1); b.text(`${k}p`, jsonText);
      return;
    }
    const srcName = resolveSourceName(a.source || a.name);
    const s = sources.get(srcName);
    if (!s) {
      b.add('div', `${k}n`, k, 0); b.set(`${k}n`, 'class', 'explorer-note');
      b.text(`${k}n`, `no source “${srcName}” to embed — mount it first`);
      return;
    }
    if (a.title) { b.add('div', `${k}t`, k, 0); b.set(`${k}t`, 'class', 'md-embed-t'); b.text(`${k}t`, String(a.title)); }
    if (mode === 'table') {
      const limit = Math.max(1, Math.min(Number(a.limit) || 8, 40));
      const rows = rowsOf(srcName).slice(0, limit);
      const cols = Array.isArray(a.columns) && a.columns.length
        ? a.columns.map((c) => resolveField(srcName, c, rows)).filter(Boolean)
        : Object.keys(s.schema || rows[0] || {});
      renderTableInto(slot, `${slot}:v`, rows, cols, `${rows.length} of ${fmtInt(s.rowcount)} rows`, srcName);
    } else {
      AGG_VIEWS[mode](slot, `${slot}:v`, srcName, a);
    }
    if (seen) {
      const id = sourceIds.get(srcName);
      if (id !== undefined) seen.push(id);
    }
  }

  // ---- dashboards: a DOCUMENT lens — markdown whose view blocks are live views.
  // Durable (the make_dashboard tool journals and replays), a tile on the board,
  // and while open it re-renders when any embedded source changes.
  const dashboards = new Map(); // slug -> { title, description, body }
  const dashTiles = new Set();
  const dashSlug = (t) => 'dash-' + String(t).toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
  function doMakeDashboard(a) {
    const title = String(a.name || a.title || '').trim();
    const body = String(a.body || a.markdown || a.text || '');
    if (!title || !body) return;
    const slug = dashSlug(title);
    const description = String(a.description || '').trim();
    dashboards.set(slug, { title, description, body });
    const views = (body.match(/^```\s*view\s*$/gm) || []).length;
    const facts = `a dashboard · ${views} live view${views === 1 ? '' : 's'}`;
    const rk = `sr:${slug}`;
    if (dashTiles.has(slug)) {
      TWIN_MUT.push({ op: 'setAttr', key: rk, name: 'data-title', value: title });
      TWIN_MUT.push({ op: 'setText', key: `${rk}:n`, text: title });
      TWIN_MUT.push({ op: 'setText', key: `${rk}:desc`, text: description });
      TWIN_MUT.push({ op: 'setText', key: `${rk}:d`, text: facts });
    } else {
      dashTiles.add(slug);
      TWIN_MUT.push({ op: 'create', key: rk, tag: 'div', parent: 'board-root', index: 0 });
      TWIN_MUT.push({ op: 'setAttr', key: rk, name: 'class', value: 'src-row' });
      TWIN_MUT.push({ op: 'setAttr', key: rk, name: 'data-title', value: title });
      TWIN_MUT.push({ op: 'create', key: `${rk}:n`, tag: 'div', parent: rk, index: 0 });
      TWIN_MUT.push({ op: 'setAttr', key: `${rk}:n`, name: 'class', value: 'src-n' });
      TWIN_MUT.push({ op: 'setText', key: `${rk}:n`, text: title });
      TWIN_MUT.push({ op: 'create', key: `${rk}:desc`, tag: 'div', parent: rk, index: 1 });
      TWIN_MUT.push({ op: 'setAttr', key: `${rk}:desc`, name: 'class', value: 'src-desc' });
      TWIN_MUT.push({ op: 'setText', key: `${rk}:desc`, text: description });
      TWIN_MUT.push({ op: 'create', key: `${rk}:d`, tag: 'div', parent: rk, index: 2 });
      TWIN_MUT.push({ op: 'setAttr', key: `${rk}:d`, name: 'class', value: 'src-d' });
      TWIN_MUT.push({ op: 'setText', key: `${rk}:d`, text: facts });
    }
    // an open copy redraws with the new body
    for (const v of [...liveViews.values()]) if (v.dash === slug) v.rerender();
  }
  function openDashboard(slug, d, panel, title) {
    const V = viewer(panel, title || d.title, { type: 'open_source', name: slug });
    let i = 0;
    if (d.description) { V.add('div', 'exp:desc', null, i++); V.set('exp:desc', 'class', 'src-desc'); V.text('exp:desc', d.description); }
    V.add('div', 'exp:dash', null, i);
    const seen = [];
    renderMarkdownInto(V.key('exp:dash'), V.key('exp:dash') + ':v', d.body, seen);
    liveViews.set(V.panel, { ids: new Set(seen), dash: slug, rerender: () => openSource(slug, '', V.panel) });
    seen.forEach(observeTableNode);
  }

  // ---- calendar: rows bucketed per day on their date field, the last months
  // that actually hold data, each a 7-wide grid with count-shaded cells.
  function renderCalendarInto(root, pfx, name, opts) {
    const rows = rowsOf(name);
    const sample = rows.slice(0, SAMPLE_N);
    const field = resolveField(name, opts && opts.field, sample) || dateFieldOf(sample);
    const b = vbuild(root, pfx);
    b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note');
    if (!field) { b.text('note', 'no date field to lay on a calendar'); return; }
    const perDay = new Map(); // 'YYYY-MM-DD' -> count
    let dated = 0;
    for (const r of rows) {
      const t = asWhen(r[field]);
      if (t == null) continue;
      dated++;
      const d = new Date(t);
      const key = `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, '0')}-${String(d.getUTCDate()).padStart(2, '0')}`;
      perDay.set(key, (perDay.get(key) || 0) + 1);
    }
    if (!perDay.size) { b.text('note', `no readable dates in ${fieldTitle(name, field)}`); return; }
    const months = [...new Set([...perDay.keys()].map((k) => k.slice(0, 7)))].sort();
    const shown = months.slice(-(Number(opts && opts.months) || 6));
    const counts = [...perDay.values()].sort((a, c) => a - c);
    const q = (p) => counts[Math.min(counts.length - 1, Math.floor(p * counts.length))];
    const hi = q(0.85), mid = q(0.5);
    b.text('note', `${fmtInt(dated)} rows across ${fmtInt(perDay.size)} days · by ${fieldTitle(name, field)}`
      + (months.length > shown.length ? ` · latest ${shown.length} of ${months.length} months` : ''));
    const MN = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
    shown.forEach((mo, mi) => {
      const [yy, mm] = mo.split('-').map(Number);
      const mk = `m${mi}`;
      b.add('div', mk, null, 1 + mi); b.set(mk, 'class', 'cal-month');
      b.add('div', `${mk}t`, mk, 0); b.set(`${mk}t`, 'class', 'cal-title'); b.text(`${mk}t`, `${MN[mm - 1]} ${yy}`);
      b.add('div', `${mk}g`, mk, 1); b.set(`${mk}g`, 'class', 'cal-grid');
      ['M', 'T', 'W', 'T', 'F', 'S', 'S'].forEach((w, wi) => {
        b.add('div', `${mk}w${wi}`, `${mk}g`, wi); b.set(`${mk}w${wi}`, 'class', 'cal-wd'); b.text(`${mk}w${wi}`, w);
      });
      const firstDow = (new Date(Date.UTC(yy, mm - 1, 1)).getUTCDay() + 6) % 7; // Monday-start
      const days = new Date(Date.UTC(yy, mm, 0)).getUTCDate();
      for (let p = 0; p < firstDow; p++) { b.add('div', `${mk}e${p}`, `${mk}g`, 7 + p); }
      for (let d = 1; d <= days; d++) {
        const key = `${mo}-${String(d).padStart(2, '0')}`;
        const n = perDay.get(key) || 0;
        const ck = `${mk}d${d}`;
        b.add('div', ck, `${mk}g`, 7 + firstDow + d - 1);
        b.set(ck, 'class', 'cal-day' + (n ? (n >= hi ? ' c3' : n >= mid ? ' c2' : ' c1') : ''));
        b.add('span', `${ck}n`, ck, 0); b.set(`${ck}n`, 'class', 'cal-n'); b.text(`${ck}n`, d);
        if (n) { b.add('span', `${ck}c`, ck, 1); b.set(`${ck}c`, 'class', 'cal-c'); b.text(`${ck}c`, n); b.set(ck, 'title', `${n} on ${key}`); }
      }
    });
  }

  // ---- board (kanban): rows grouped into lanes by a small enum, a capped stack
  // of cards per lane — the overview a status column is FOR.
  function renderKanbanInto(root, pfx, name, opts) {
    const rows = rowsOf(name);
    const sample = rows.slice(0, SAMPLE_N);
    const lane = resolveField(name, opts && (opts.lane || opts.field), sample) || laneFieldOf(sample);
    const b = vbuild(root, pfx);
    b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note');
    if (!lane) { b.text('note', 'no lane field: a board needs a column with a handful of values'); return; }
    const titleF = titleFieldOf(sample, lane);
    const dateF = dateFieldOf(sample);
    const groups = new Map();
    for (const r of rows) {
      const v = r[lane] == null || r[lane] === '' ? '(empty)' : String(r[lane]);
      if (!groups.has(v)) groups.set(v, []);
      groups.get(v).push(r);
    }
    const lanes = [...groups.entries()].sort((a, c) => c[1].length - a[1].length);
    const LANES = 8, CARDS = 25;
    const shown = lanes.slice(0, LANES);
    b.text('note', `${fmtInt(rows.length)} rows by ${fieldTitle(name, lane)}`
      + (lanes.length > LANES ? ` · ${lanes.length - LANES} smaller lanes folded` : ''));
    b.add('div', 'kb', null, 1); b.set('kb', 'class', 'kanban');
    shown.forEach(([v, list], li) => {
      const lk = `L${li}`;
      b.add('div', lk, 'kb', li); b.set(lk, 'class', 'klane');
      b.add('div', `${lk}h`, lk, 0); b.set(`${lk}h`, 'class', 'klane-h');
      b.add('span', `${lk}hn`, `${lk}h`, 0); b.text(`${lk}hn`, v);
      b.add('span', `${lk}hc`, `${lk}h`, 1); b.set(`${lk}hc`, 'class', 'klane-c'); b.text(`${lk}hc`, fmtInt(list.length));
      list.slice(0, CARDS).forEach((r, ci) => {
        const ck = `${lk}c${ci}`;
        b.add('div', ck, lk, 1 + ci); b.set(ck, 'class', 'kcard');
        b.add('div', `${ck}t`, ck, 0); b.set(`${ck}t`, 'class', 'kcard-t');
        b.text(`${ck}t`, r[titleF] == null ? '(untitled)' : String(r[titleF]));
        const when = dateF ? asWhen(r[dateF]) : null;
        if (when != null) { b.add('div', `${ck}d`, ck, 1); b.set(`${ck}d`, 'class', 'kcard-d'); b.text(`${ck}d`, fmtDate(when)); }
      });
      if (list.length > CARDS) {
        const mk = `${lk}m`;
        b.add('div', mk, lk, 1 + CARDS); b.set(mk, 'class', 'kcard-more'); b.text(mk, `and ${fmtInt(list.length - CARDS)} more`);
      }
    });
  }

  // ---- histogram: a numeric field in ~28 bins (a non-numeric field falls back
  // to a bar per value) — the distribution at a glance, any row count.
  function renderHistogramInto(root, pfx, name, opts) {
    const rows = rowsOf(name);
    const sample = rows.slice(0, SAMPLE_N);
    const field = resolveField(name, opts && opts.field, sample) || numericFieldsOf(sample)[0] || laneFieldOf(sample);
    const b = vbuild(root, pfx);
    b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note');
    if (!field) { b.text('note', 'no numeric field to bin'); return; }
    const numeric = sample.some((r) => typeof r[field] === 'number');
    let bars; // [label, count][]
    if (numeric) {
      const vals = rows.map((r) => r[field]).filter((v) => typeof v === 'number');
      if (!vals.length) { b.text('note', `${fieldTitle(name, field)} holds no numbers`); return; }
      const min = Math.min(...vals), max = Math.max(...vals);
      const BINS = Math.min(28, Math.max(6, new Set(vals).size));
      const w = (max - min) / BINS || 1;
      const counts = new Array(BINS).fill(0);
      for (const v of vals) counts[Math.min(BINS - 1, Math.floor((v - min) / w))]++;
      bars = counts.map((c, bi) => [fmtNum(min + bi * w), c]);
      b.text('note', `${fmtInt(vals.length)} values of ${fieldTitle(name, field)} · ${fmtNum(min)} to ${fmtNum(max)} · ${BINS} bins`);
    } else {
      const counts = new Map();
      for (const r of rows) { const v = r[field] == null || r[field] === '' ? '(empty)' : String(r[field]); counts.set(v, (counts.get(v) || 0) + 1); }
      bars = [...counts.entries()].sort((a, c) => c[1] - a[1]).slice(0, 20);
      b.text('note', `${fmtInt(rows.length)} rows by ${fieldTitle(name, field)}`
        + (counts.size > 20 ? ` · top 20 of ${counts.size} values` : ''));
    }
    const W = 1000, H = 320, pad = 46;
    const peak = Math.max(...bars.map((x) => x[1]), 1);
    b.add('svg', 'svg', null, 1); b.set('svg', 'viewBox', `0 0 ${W} ${H}`); b.set('svg', 'class', 'chart');
    b.add('line', 'ax', 'svg', 0); b.set('ax', 'x1', pad); b.set('ax', 'y1', H - pad); b.set('ax', 'x2', W - pad); b.set('ax', 'y2', H - pad); b.set('ax', 'class', 'chart-axis');
    const bw = (W - 2 * pad) / bars.length;
    bars.forEach(([label, c], bi) => {
      const h = (c / peak) * (H - 2 * pad);
      const k = `b${bi}`;
      b.add('rect', k, 'svg', 1 + bi);
      b.set(k, 'x', (pad + bi * bw + 1).toFixed(1)); b.set(k, 'y', (H - pad - h).toFixed(1));
      b.set(k, 'width', Math.max(1, bw - 2).toFixed(1)); b.set(k, 'height', Math.max(0.5, h).toFixed(1));
      b.set(k, 'class', 'chart-bar'); b.set(k, 'title', `${label}: ${c}`);
    });
    b.add('text', 'pk', 'svg', 1 + bars.length); b.set('pk', 'x', pad - 8); b.set('pk', 'y', pad + 4); b.set('pk', 'class', 'chart-lbl'); b.text('pk', fmtInt(peak));
    b.add('text', 'x0', 'svg', 2 + bars.length); b.set('x0', 'x', pad); b.set('x0', 'y', H - pad + 16); b.set('x0', 'class', 'chart-xlbl'); b.text('x0', bars[0][0]);
    b.add('text', 'x1', 'svg', 3 + bars.length); b.set('x1', 'x', W - pad); b.set('x1', 'y', H - pad + 16); b.set('x1', 'class', 'chart-xlbl endlbl'); b.text('x1', bars[bars.length - 1][0]);
  }

  // ---- scatter and map: two numbers against each other; a map is the same
  // picture with lat/lon axes and the aspect the latitude implies.
  function renderScatterInto(root, pfx, name, opts) {
    const rows = rowsOf(name);
    const sample = rows.slice(0, SAMPLE_N);
    const geo = opts && opts.geo ? geoFieldsOf(sample) : null;
    const nums = numericFieldsOf(sample);
    const fx = geo ? geo.lon : (resolveField(name, opts && opts.x, sample) || nums[0]);
    const fy = geo ? geo.lat : (resolveField(name, opts && opts.y, sample) || nums.find((f) => f !== fx));
    const b = vbuild(root, pfx);
    b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note');
    if (!fx || !fy) { b.text('note', geo ? 'no lat/lon pair found' : 'a scatter needs two numeric fields'); return; }
    let pts = [];
    for (const r of rows) if (typeof r[fx] === 'number' && typeof r[fy] === 'number') pts.push([r[fx], r[fy]]);
    if (!pts.length) { b.text('note', 'no rows carry both fields'); return; }
    const CAP = 1500;
    const totalPts = pts.length;
    if (totalPts > CAP) { const step = Math.ceil(totalPts / CAP); pts = pts.filter((_, i) => i % step === 0); }
    const xs = pts.map((p) => p[0]), ys = pts.map((p) => p[1]);
    const xmin = Math.min(...xs), xmax = Math.max(...xs), ymin = Math.min(...ys), ymax = Math.max(...ys);
    const W = 1000, pad = 50;
    // a map keeps ground proportions: squash x by cos(mid-latitude)
    const aspect = geo ? Math.max(0.2, Math.cos(((ymin + ymax) / 2) * Math.PI / 180)) : null;
    const H = geo
      ? Math.max(240, Math.min(720, Math.round((W - 2 * pad) * ((ymax - ymin) || 1) / (((xmax - xmin) || 1) * aspect)) + 2 * pad))
      : 460;
    const sx = (v) => pad + (xmax > xmin ? (v - xmin) / (xmax - xmin) : 0.5) * (W - 2 * pad);
    const sy = (v) => (H - pad) - (ymax > ymin ? (v - ymin) / (ymax - ymin) : 0.5) * (H - 2 * pad);
    b.text('note', `${fmtInt(pts.length)}${pts.length < totalPts ? ` of ${fmtInt(totalPts)}` : ''} points · ${fieldTitle(name, fx)} × ${fieldTitle(name, fy)}`);
    b.add('svg', 'svg', null, 1); b.set('svg', 'viewBox', `0 0 ${W} ${H}`); b.set('svg', 'class', 'chart');
    b.add('line', 'ax', 'svg', 0); b.set('ax', 'x1', pad); b.set('ax', 'y1', H - pad); b.set('ax', 'x2', W - pad); b.set('ax', 'y2', H - pad); b.set('ax', 'class', 'chart-axis');
    b.add('line', 'ay', 'svg', 1); b.set('ay', 'x1', pad); b.set('ay', 'y1', pad); b.set('ay', 'x2', pad); b.set('ay', 'y2', H - pad); b.set('ay', 'class', 'chart-axis');
    pts.forEach((p, i) => {
      const k = `d${i}`;
      b.add('circle', k, 'svg', 2 + i);
      b.set(k, 'cx', sx(p[0]).toFixed(1)); b.set(k, 'cy', sy(p[1]).toFixed(1)); b.set(k, 'r', '2.2'); b.set(k, 'class', 'chart-dot');
    });
    const n = 2 + pts.length;
    b.add('text', 'ymax', 'svg', n); b.set('ymax', 'x', pad - 8); b.set('ymax', 'y', pad + 4); b.set('ymax', 'class', 'chart-lbl'); b.text('ymax', fmtNum(ymax));
    b.add('text', 'ymin', 'svg', n + 1); b.set('ymin', 'x', pad - 8); b.set('ymin', 'y', H - pad); b.set('ymin', 'class', 'chart-lbl'); b.text('ymin', fmtNum(ymin));
    b.add('text', 'x0', 'svg', n + 2); b.set('x0', 'x', pad); b.set('x0', 'y', H - pad + 16); b.set('x0', 'class', 'chart-xlbl'); b.text('x0', fmtNum(xmin));
    b.add('text', 'x1', 'svg', n + 3); b.set('x1', 'x', W - pad); b.set('x1', 'y', H - pad + 16); b.set('x1', 'class', 'chart-xlbl endlbl'); b.text('x1', fmtNum(xmax));
  }

  // ---- reading view: long-text rows as sections you READ — heading from the
  // row's name, date alongside, body rendered as markdown (plain prose passes
  // through unharmed).  Reports become a document instead of truncated cells.
  function renderReadingInto(root, pfx, name, opts) {
    const rows = rowsOf(name);
    const sample = rows.slice(0, SAMPLE_N);
    const prose = resolveField(name, opts && opts.field, sample) || proseFieldOf(sample);
    const b = vbuild(root, pfx);
    b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note');
    if (!prose) { b.text('note', 'no long-text field to read'); return; }
    const titleF = titleFieldOf(sample, prose);
    const dateF = dateFieldOf(sample);
    let secs = rows.filter((r) => typeof r[prose] === 'string' && r[prose].length);
    if (dateF) secs = secs.slice().sort((a, c) => (asWhen(a[dateF]) || 0) - (asWhen(c[dateF]) || 0));
    const CAP = 30;
    b.text('note', `${fmtInt(secs.length)} entries · reading ${fieldTitle(name, prose)}`
      + (secs.length > CAP ? ` · first ${CAP} shown` : ''));
    secs.slice(0, CAP).forEach((r, si) => {
      const sk = `s${si}`;
      b.add('div', sk, null, 1 + si); b.set(sk, 'class', 'rd-sec');
      b.add('div', `${sk}h`, sk, 0); b.set(`${sk}h`, 'class', 'rd-h');
      b.add('span', `${sk}ht`, `${sk}h`, 0); b.set(`${sk}ht`, 'class', 'rd-t');
      b.text(`${sk}ht`, r[titleF] == null ? `entry ${si + 1}` : String(r[titleF]));
      const when = dateF ? asWhen(r[dateF]) : null;
      if (when != null) { b.add('span', `${sk}hd`, `${sk}h`, 1); b.set(`${sk}hd`, 'class', 'rd-d'); b.text(`${sk}hd`, fmtDate(when)); }
      b.add('div', `${sk}b`, sk, 1);
      renderMarkdownInto(`${pfx}:${sk}b`, `${pfx}:${sk}md`, r[prose]);
    });
  }

  // the panel form of every aggregate view: header + switcher, the Into body,
  // and a LIVE registration — a delta on the node redraws the open panel
  const AGG_VIEWS = {
    calendar: renderCalendarInto,
    kanban: renderKanbanInto,
    histogram: renderHistogramInto,
    scatter: (root, pfx, name, opts) => renderScatterInto(root, pfx, name, opts || {}),
    map: (root, pfx, name, opts) => renderScatterInto(root, pfx, name, Object.assign({ geo: true }, opts || {})),
    reading: renderReadingInto,
  };
  function renderAggPanel(V, name, s, mode) {
    const i = renderSourceHeader(V, name, s, mode);
    V.add('div', 'exp:agg', null, i);
    AGG_VIEWS[mode](V.key('exp:agg'), V.key('exp:agg') + ':v', name, {});
    const id = sourceIds.get(name);
    if (id !== undefined) {
      liveViews.set(V.panel, { id, rerender: () => openSource(name, mode, V.panel) });
      observeTableNode(id);
    }
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

  // The schema AS A PAGE (§8: schema is data, so it gets a view like any data):
  // one row per field — its name, its human title, its type, its structure, and
  // what it means.  References are doorways: clicking one opens the schema it
  // points at, so you walk the graph entity by entity.
  function renderSchema(V, name, s) {
    const cols = Object.keys(s.schema || {});
    let i = renderSourceHeader(V, name, s, 'schema');
    V.add('div', 'exp:note', null, i++); V.set('exp:note', 'class', 'explorer-note');
    V.text('exp:note', `${cols.length} fields · what the twin understands about ${srcTitle(name)} · references open the schema they point at`);
    // an undocumented schema offers its own way out, right where you see the gap
    if (cols.some((c) => !((fieldMeta.get(`${name}|${c}`) || {}).title))) {
      V.add('button', 'exp:annbtn', null, i++); V.set('exp:annbtn', 'class', 'steps-toggle');
      V.set('exp:annbtn', 'data-title', srcTitle(name)); V.text('exp:annbtn', 'document the fields');
    }
    V.add('div', 'exp:scroll', null, i); V.set('exp:scroll', 'class', 'table-scroll');
    V.add('table', 'sch:tbl', 'exp:scroll', 0);
    V.add('thead', 'sch:hd', 'sch:tbl', 0); V.add('tr', 'sch:htr', 'sch:hd', 0);
    ['Field', 'Title', 'Type', 'Structure', 'Description'].forEach((h, hi) => { V.add('th', `sch:h${hi}`, 'sch:htr', hi); V.text(`sch:h${hi}`, h); });
    V.add('tbody', 'sch:tb', 'sch:tbl', 1);
    cols.forEach((c, ri) => {
      const m = fieldMeta.get(`${name}|${c}`) || {};
      const sem = semanticsOf(name, c) || {};
      const rk = `sch:r${ri}`;
      V.add('tr', rk, 'sch:tb', ri);
      const td = (k, ci, txt, cls) => {
        V.add('td', k, rk, ci);
        if (cls) V.set(k, 'class', cls);
        if (txt) V.text(k, txt);
        return k;
      };
      td(`sch:f${ri}`, 0, c, 'sch-field');
      td(`sch:t${ri}`, 1, sem.title || '');
      td(`sch:y${ri}`, 2, m.type && m.type !== 'empty' ? m.type : '');
      // structure: the plain facts as text, the reference as a clickable doorway
      const bits = [];
      if (m.role === 'key') bits.push('unique key');
      else if (m.role === 'enum') bits.push(`one of: ${m.values}`);
      else if (m.role === 'constant') bits.push(`always “${m.values}”`);
      else if (m.role === 'redundant') bits.push(m.overlap ? `duplicates ${m.refField} on ${m.overlap}% of rows` : `duplicates ${m.refField}`);
      if (m.pattern) bits.push(`pattern ${m.pattern} (${m.coverage}%)`);
      if (m.empty) bits.push(m.empty >= 100 ? 'always empty' : `${m.empty}% empty`);
      const sk = td(`sch:s${ri}`, 3, null);
      const isRef = m.role === 'ref' || m.role === 'refs';
      if (bits.length || isRef) {
        V.add('span', `${sk}:x`, sk, 0); V.text(`${sk}:x`, bits.join(' · ') + (bits.length && isRef ? ' · ' : ''));
        if (isRef) {
          V.add('span', `sch:ref:${ri}`, sk, 1); V.set(`sch:ref:${ri}`, 'class', 'sch-ref');
          V.set(`sch:ref:${ri}`, 'data-name', m.ref); V.set(`sch:ref:${ri}`, 'data-title', srcTitle(m.ref));
          V.text(`sch:ref:${ri}`, `${m.role === 'refs' ? 'multi-references' : 'references'} ${srcTitle(m.ref)}.${m.refField}`);
        }
      }
      td(`sch:d${ri}`, 4, sem.description || '', 'sch-desc');
    });
  }

  // The whole schema AS A MAP: every table source is a node (mounts left, each
  // lens right of what it derives from), solid edges are references between
  // entities, dashed edges are derivations.  Every node opens its schema page.
  function openSchemaMap(panel) {
    const V = viewer(panel, 'Schema map', { type: 'open_schema' });
    const nodes = [...sources].filter(([, s]) => s.kind === 'table');
    V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
    if (!nodes.length) { V.text('exp:note', 'Nothing mounted yet — the map draws itself as sources land.'); return; }
    V.text('exp:note', `${nodes.length} sources and lenses · solid lines are references, dashed are derivations · click a node for its schema`);
    const depth = (n) => chainOf(n).length - 1;
    const colsBy = new Map();
    for (const [n] of nodes) { const d = depth(n); (colsBy.get(d) || colsBy.set(d, []).get(d)).push(n); }
    const NW = 190, NH = 46, GX = 250, GY = 62, PX = 16, PY = 14;
    const pos = new Map();
    let maxRows = 0;
    for (const [d, list] of colsBy) {
      list.forEach((n, r) => pos.set(n, { x: PX + d * GX, y: PY + r * GY }));
      maxRows = Math.max(maxRows, list.length);
    }
    const W = PX * 2 + (Math.max(...colsBy.keys()) * GX) + NW;
    const H = PY * 2 + (maxRows - 1) * GY + NH;
    V.add('svg', 'sm:svg', null, 1); V.set('sm:svg', 'viewBox', `0 0 ${W} ${H}`); V.set('sm:svg', 'class', 'schema-map');
    let idx = 0;
    // edges first (under the nodes), deduped per source→target pair
    const drawn = new Map(); // "a->b" -> label so far
    const edge = (a, b, label, cls) => {
      const p = pos.get(a), q = pos.get(b);
      if (!p || !q) return;
      const ek = `${a}->${b}`;
      if (drawn.has(ek)) return;
      drawn.set(ek, label);
      const [l, r] = p.x <= q.x ? [p, q] : [q, p];
      const k = `sm:e${drawn.size}`;
      V.add('line', k, 'sm:svg', idx++); V.set(k, 'class', cls);
      V.set(k, 'x1', l.x + NW); V.set(k, 'y1', l.y + NH / 2);
      V.set(k, 'x2', r.x); V.set(k, 'y2', r.y + NH / 2);
      if (label) {
        V.add('text', `${k}:l`, 'sm:svg', idx++); V.set(`${k}:l`, 'class', 'sm-lbl');
        V.set(`${k}:l`, 'x', (l.x + NW + r.x) / 2); V.set(`${k}:l`, 'y', (l.y + r.y + NH) / 2 - 5);
        V.text(`${k}:l`, label);
      }
    };
    for (const [n, s] of nodes) if (s.from) edge(s.from, n, '', 'sm-edge derive');
    for (const [key, m] of fieldMeta) {
      if (m.role !== 'ref' && m.role !== 'refs') continue;
      const [src, field] = key.split('|');
      edge(src, m.ref, field, 'sm-edge');
    }
    nodes.forEach(([n, s]) => {
      const k = `smn:${n}`;
      const p = pos.get(n);
      V.add('g', k, 'sm:svg', idx++); V.set(k, 'class', 'sm-node');
      V.set(k, 'data-name', n); V.set(k, 'data-title', srcTitle(n));
      V.add('rect', `${k}:r`, k, 0); V.set(`${k}:r`, 'x', p.x); V.set(`${k}:r`, 'y', p.y);
      V.set(`${k}:r`, 'width', NW); V.set(`${k}:r`, 'height', NH); V.set(`${k}:r`, 'rx', 10);
      V.add('text', `${k}:t`, k, 1); V.set(`${k}:t`, 'class', 'sm-t');
      V.set(`${k}:t`, 'x', p.x + 12); V.set(`${k}:t`, 'y', p.y + 19);
      V.text(`${k}:t`, String(srcTitle(n)).slice(0, 26));
      V.add('text', `${k}:m`, k, 2); V.set(`${k}:m`, 'class', 'sm-m');
      V.set(`${k}:m`, 'x', p.x + 12); V.set(`${k}:m`, 'y', p.y + 35);
      V.text(`${k}:m`, `${fmtInt(s.rowcount)} rows${s.from ? ` · from ${String(srcTitle(s.from)).slice(0, 18)}` : ''}`);
    });
  }

  function chartMessage(label, msg, panel) {
    const V = viewer(panel, String(label || ''), null);
    V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
    V.text('exp:note', `${label}: ${msg}.`);
  }

  // chart lens: raw datapoints (from Rust, downsampled) → an SVG line chart.
  function chartSeries(id, label, points, provenance, panel) {
    const V = viewer(panel, String(label || id), { type: 'fetch', adapter: 'cognite-datapoints', id: String(id), label: String(label || id) });
    if (!points || !points.length) {
      V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
      V.text('exp:note', `${label}: no datapoints.`);
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
  function openAsset(assetId, panel, title) {
    const aid = String(assetId);
    const asset = rowsOf('assets').find((r) => String(r.id) === aid);
    if (!asset) return;
    const V = viewer(panel, title || asset.name || `asset ${aid}`, { type: 'open_asset', id: aid });
    const sensors = rowsOf('timeseries').filter((r) => String(r.assetId) === aid);
    const events = rowsOf('events')
      .filter((r) => String(r.assetIds || '').split(';').includes(aid))
      .sort((a, b) => Number(b.startTime) - Number(a.startTime));
    V.add('div', 'ad:hn', null, 0); V.set('ad:hn', 'class', 'ad-name'); V.text('ad:hn', asset.name || aid);
    V.add('div', 'ad:hd', null, 1); V.set('ad:hd', 'class', 'ad-desc'); V.text('ad:hd', `${asset.description || ''}  ·  id ${aid}`);

    V.add('div', 'ad:st', null, 2); V.set('ad:st', 'class', 'ad-section'); V.text('ad:st', `Sensors · ${sensors.length}`);
    V.add('div', 'ad:sl', null, 3); V.set('ad:sl', 'class', 'sens-list');
    if (!sensors.length) { V.add('div', 'ad:sn', 'ad:sl', 0); V.set('ad:sn', 'class', 'ad-empty'); V.text('ad:sn', 'no sensors linked to this asset'); }
    sensors.forEach((s, i) => {
      const k = `sens:${s.id}`;
      V.add('div', k, 'ad:sl', i); V.set(k, 'class', 'sens-row'); V.set(k, 'data-series', s.id); V.set(k, 'data-label', s.externalId || s.name || s.id);
      V.add('span', `${k}:n`, k, 0); V.set(`${k}:n`, 'class', 'sens-n'); V.text(`${k}:n`, s.externalId || s.name || String(s.id));
      if (s.unit) { V.add('span', `${k}:u`, k, 1); V.set(`${k}:u`, 'class', 'sens-u'); V.text(`${k}:u`, ` [${s.unit}]`); }
    });

    V.add('div', 'ad:et', null, 4); V.set('ad:et', 'class', 'ad-section'); V.text('ad:et', `Maintenance events · ${events.length}`);
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
    V.add('div', 'ad:dt', null, 6); V.set('ad:dt', 'class', 'ad-section'); V.text('ad:dt', `Documents · ${docs.length}`);
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
    const V = viewer(panel, 'Search', { type: 'search', query: String(query || '') });
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
      V.add('div', `se:h${idx}`, null, idx); V.set(`se:h${idx}`, 'class', 'ad-section'); V.text(`se:h${idx}`, `${title} · ${n}`); idx++;
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
    const V = viewer(panel, 'Watch', { type: 'watch' });
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
    const lk1 = section(`Blind spots · ${blind.length}`, 'equipment with maintenance events but no sensors monitoring it');
    if (!blind.length) { V.add('div', 'w:e1', lk1, 0); V.set('w:e1', 'class', 'ad-empty'); V.text('w:e1', 'none found'); }
    blind.forEach(([a, c], i) => card(lk1, 1, i, a, byId.get(a).name || a, `${c} events · 0 sensors`, 'sev-amber'));

    // 2) maintenance hotspots — where the work orders concentrate
    const hot = [...eByA.entries()].filter(([a]) => byId.has(a)).sort((x, y) => y[1] - x[1]).slice(0, 10);
    const lk2 = section(`Maintenance hotspots · top ${hot.length}`, 'assets with the most work orders');
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
    const V = viewer(panel, String(name), { type: 'open_document', name: String(name) });
    const ext = String(name).split('.').pop().toLowerCase();
    const src = '/file/' + encodeURIComponent(name);
    if (ext === 'pdf') { V.add('iframe', 'exp:doc', null, 0); V.set('exp:doc', 'src', src); V.set('exp:doc', 'class', 'doc-frame'); }
    else if (ext === 'mp4') { V.add('video', 'exp:doc', null, 0); V.set('exp:doc', 'src', src); V.set('exp:doc', 'controls', 'true'); V.set('exp:doc', 'class', 'doc-frame'); }
    else { V.add('img', 'exp:doc', null, 0); V.set('exp:doc', 'src', src); V.set('exp:doc', 'class', 'doc-img'); }
  }

  function residenceLabel(s) {
    if (s.residence === 'mounted') return 'federated · live';
    if (s.residence === 'mounted-partial') return `federated · ${s.materialized} of ${s.rowcount} local`;
    return s.residence;
  }

  // Install a skill (§4.1) — called by the core skills-loader from the static dir.
  // Built-in capabilities are FACTS about the twin, not events that happened: they
  // register quietly (the agent perceives them and speaks for itself in its
  // greeting), never as cards in the chat.
  function installSkill(name, meta) {
    if (skills.has(name)) return;
    const title = meta.title || name;
    const files = meta.files || [];
    skills.set(name, { title, description: meta.description || '', files });
    G.submit(skillsSrc, ZSet.fromRows([rec({ name, title, description: meta.description || '' })]), prov('core'));
  }

  // ---- the agent's working state (agenda / activity / findings) ---------------
  // All pure event sourcing: each call appends an event; the observers above fold it
  // into the panels and the perception state.  Nothing is mutated in place.
  let agendaSeq = 0;
  function addAgenda(text, description) {
    const t = String(text || '').trim();
    if (!t) return;
    agendaSeq += 1;
    G.submit(agendaSrc, ZSet.fromRows([rec({ id: agendaSeq, text: t, description: String(description || ''), status: 'open' })]), prov('agent'));
  }
  // Resolve a task by id or text fragment and append a status-change event for it.
  // Returns the task id it hit, or null.
  function setAgenda(ref, status) {
    const q = String(ref).trim().toLowerCase();
    if (!q) return null;
    let hit = null;
    for (const [id, a] of agenda) {
      if (String(id) === q) { hit = id; break; }
      if (hit === null && a.status !== 'done' && a.text.toLowerCase().includes(q)) hit = id;
    }
    if (hit === null) return null;
    G.submit(agendaSrc, ZSet.fromRows([rec({ id: hit, text: agenda.get(hit).text, status })]), prov('agent'));
    return hit;
  }
  let actSeq = 0;
  // Every work step is a STRUCTURED event, stamped with the agenda item it happened
  // under — so a task's page can tell "the work so far" as a story, not a log dump.
  //   kind    — what happened (inspected/built/failed/flagged/…) → the row's label
  //   text    — the human line: WHAT, in words a person would say
  //   detail  — the quiet facts line: numbers and errors live here, never in text
  //   subject — the noun the step is about (stable across retries, for folding)
  //   tone    — ''/'warn'/'error' → how loud the row reads
  //   ev      — optional open-event: the row becomes a doorway to the thing itself
  function logStep(kind, text, o) {
    o = o || {};
    const last = activityTail[activityTail.length - 1];
    const detail = String(o.detail || '');
    if (last && last.kind === kind && last.text === String(text) && last.detail === detail) return; // no stutter
    actSeq += 1;
    G.submit(activitySrc, ZSet.fromRows([rec({
      seq: actSeq, kind: String(kind), text: String(text), detail,
      subject: String(o.subject || ''), tone: String(o.tone || ''),
      ev: o.ev ? JSON.stringify(o.ev) : '',
      task: o.task != null ? Number(o.task) : (activeTaskId || 0),
    })]), prov('agent'));
  }
  // Plain progress notes (the agent's `work` tool) — already sentences.
  function logActivity(text, taskId) { logStep('note', text, { task: taskId }); }
  let findingSeq = 0;
  function addFinding(a) {
    const text = String(a.text || a.description || '').trim();
    if (!text) return;
    for (const f of findings.values()) if (f.text === text) return; // already on the board
    findingSeq += 1;
    const sev = ['info', 'warn', 'critical'].includes(a.severity) ? a.severity : 'info';
    const src = String(a.source || '');
    G.submit(findingsSrc, ZSet.fromRows([rec({ id: findingSeq, severity: sev, text, source: src })]), prov('agent'));
    // Findings land on the Findings board and in the work log — NEVER in the chat.
    // The chat is the user's space: if a finding matters, the agent must TELL them
    // (say), a deliberate act, not a side effect of filing.
    logStep('flagged', text, {
      subject: `finding:${findingSeq}`,
      detail: src ? `in ${srcTitle(src)}` : '',
      tone: sev === 'critical' ? 'error' : sev === 'warn' ? 'warn' : '',
      ev: { type: 'open_finding', id: findingSeq },
    });
  }

  // A finding, opened: what's wrong, where (one click to the evidence source), and
  // what YOU can do about it — investigate / propose a fix (both brief the agent
  // through the chat) or mark it resolved (an event, folded everywhere).
  function openFinding(id, panel) {
    const f = findings.get(Number(id));
    const V = viewer(panel, 'Finding', { type: 'open_finding', id: Number(id) });
    if (!f) {
      V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
      V.text('exp:note', `No finding #${id}.`);
      return;
    }
    const sevWord = f.severity === 'critical' ? 'Critical issue' : f.severity === 'warn' ? 'Data issue' : 'Observation';
    V.add('div', 'fd:sev', null, 0); V.set('fd:sev', 'class', `fd-sev sev-${f.severity}${f.status === 'resolved' ? ' resolved' : ''}`);
    V.text('fd:sev', f.status === 'resolved' ? `${sevWord} · resolved` : sevWord);
    V.add('div', 'fd:t', null, 1); V.set('fd:t', 'class', 'fd-text'); V.text('fd:t', f.text);
    let i = 2;
    if (f.source && sources.has(f.source)) {
      V.add('div', 'fd:src', null, i++); V.set('fd:src', 'class', 'fd-src');
      V.set('fd:src', 'data-name', f.source); V.set('fd:src', 'data-title', srcTitle(f.source));
      V.text('fd:src', `in ${srcTitle(f.source)}`);
    }
    V.add('div', 'fd:cta', null, i++); V.set('fd:cta', 'class', 'fd-cta');
    if (f.kind === 'schema') {
      // the fix for a schema gap is documentation — one button briefs the agent
      V.add('button', 'fd:document', 'fd:cta', 0); V.set('fd:document', 'class', 'fd-btn primary');
      V.set('fd:document', 'data-title', srcTitle(f.source)); V.text('fd:document', 'Document the fields');
    } else {
      V.add('button', 'fd:investigate', 'fd:cta', 0); V.set('fd:investigate', 'class', 'fd-btn primary');
      V.set('fd:investigate', 'data-text', f.text); V.text('fd:investigate', 'Investigate');
      V.add('button', 'fd:fix', 'fd:cta', 1); V.set('fd:fix', 'class', 'fd-btn');
      V.set('fd:fix', 'data-text', f.text); V.text('fd:fix', 'Propose a fix');
    }
    V.add('button', 'fd:resolve', 'fd:cta', 2); V.set('fd:resolve', 'class', 'fd-btn');
    V.set('fd:resolve', 'data-id', id);
    V.text('fd:resolve', f.status === 'resolved' ? 'Resolved ✓' : 'Mark resolved');
  }

  // One row shape for every work line — a small label column, the text, and an
  // optional quiet sub-line (a task's description, a step's facts).
  function workRow(V, parent, key, idx, label, text, cls, clickable, sub) {
    V.add('div', key, parent, idx); V.set(key, 'class', `act-row${cls ? ' ' + cls : ''}`);
    V.add('span', `${key}:v`, key, 0); V.set(`${key}:v`, 'class', 'act-verb'); V.text(`${key}:v`, label);
    V.add('div', `${key}:c`, key, 1); V.set(`${key}:c`, 'class', 'act-content');
    V.add('div', `${key}:t`, `${key}:c`, 0); V.set(`${key}:t`, 'class', 'act-text'); V.text(`${key}:t`, text);
    if (sub) { V.add('div', `${key}:d`, `${key}:c`, 1); V.set(`${key}:d`, 'class', 'act-detail'); V.text(`${key}:d`, sub); }
    if (clickable) V.set(key, 'data-title', text);
  }

  // ---- the work log as a STORY (task pages + the agent page) -----------------
  // Labels are PLAIN WORDS a person scans, never system vocabulary: a row is
  // "created · Unique asset types · 12 rows from Equipment registry", not
  // "BUILT lens:unique-asset-types".  'described' steps are housekeeping and are
  // kept out of user-facing lists (they stay in the log and in perception).
  const KIND_VERB = { note: 'working', inspected: 'checked', built: 'created', flagged: 'issue',
    failed: 'error', described: 'updated', finished: 'done', chart: 'chart' };
  // A step as one line — the now-strip and the agent's perception both read this.
  function stepLine(e) {
    const verb = KIND_VERB[e.kind] || e.kind;
    const head = e.kind === 'note' ? e.text : `${verb} ${e.text}`;
    return e.detail ? `${head} · ${e.detail}` : head;
  }
  // Fold raw steps into a readable story:
  //  · retries of the same subject collapse into ONE failed row (attempt count, last error)
  //  · a success absorbs its own earlier failures ("worked after N failed attempts")
  //  · re-inspections/re-descriptions of the same subject keep only the latest
  //  · exact repeats disappear
  function foldSteps(entries) {
    const out = [];
    const failAt = new Map();   // subject -> index of its merged failed row
    const latestAt = new Map(); // "kind|subject" -> index, for latest-wins kinds
    const seen = new Set();
    const reindex = (m, i) => { for (const [k, v] of m) if (v > i) m.set(k, v - 1); };
    for (const e of entries) {
      const subj = e.subject || e.text;
      if (e.kind === 'failed') {
        const i = failAt.get(subj);
        if (i != null) { const r = out[i]; r.times += 1; r.seq = e.seq; r.text = e.text; r.detail = e.detail; continue; }
        failAt.set(subj, out.length);
        out.push(Object.assign({}, e, { times: 1 }));
        continue;
      }
      if (e.kind === 'built' && failAt.has(subj)) {
        const i = failAt.get(subj);
        const tries = out[i].times;
        out.splice(i, 1);
        failAt.delete(subj);
        reindex(failAt, i); reindex(latestAt, i);
        out.push(Object.assign({}, e, { tries })); // the step's page tells the retry story
        continue;
      }
      if (e.kind === 'inspected' || e.kind === 'described') {
        const lk = `${e.kind}|${subj}`;
        const i = latestAt.get(lk);
        if (i != null) { out[i] = Object.assign({}, e); continue; }
        latestAt.set(lk, out.length);
        out.push(Object.assign({}, e));
        continue;
      }
      const key = `${e.kind}|${e.text}|${e.detail}`;
      if (seen.has(key)) continue;
      seen.add(key);
      out.push(Object.assign({}, e));
    }
    return out;
  }
  // One step in a list: ONE line — a plain-word label and what it acted on.
  // Steps are uniform on purpose: the list answers "what did it do, in order";
  // everything else (facts, errors, links) lives on the step's own page, one
  // click away — every row opens it (the chevron says so).
  function stepRow(V, parent, key, idx, e) {
    const tone = e.tone === 'error' ? ' t-error' : e.tone === 'warn' ? ' t-warn' : '';
    V.add('div', key, parent, idx); V.set(key, 'class', `act-row openable${tone}`);
    V.add('span', `${key}:v`, key, 0); V.set(`${key}:v`, 'class', 'act-verb');
    V.text(`${key}:v`, KIND_VERB[e.kind] || e.kind);
    V.add('span', `${key}:t`, key, 1); V.set(`${key}:t`, 'class', 'act-text');
    V.text(`${key}:t`, e.text + (e.kind === 'failed' && e.times > 1 ? ` (${e.times} attempts)` : ''));
    V.add('span', `${key}:o`, key, 2); V.set(`${key}:o`, 'class', 'act-open'); V.text(`${key}:o`, '›');
    V.set(key, 'data-ev', JSON.stringify({ type: 'open_step', seq: e.seq, n: e.times || e.tries || 0 }));
    V.set(key, 'data-title', 'Step');
  }
  function stepRows(V, parent, keyPfx, entries) {
    entries.forEach((e, i) => stepRow(V, parent, `${keyPfx}${i}`, i, e));
    return entries.length;
  }
  // A note that merely restates the task's title is noise, not narration.  Match
  // loosely — word prefixes — so "Identifying X" still echoes the title "Identify X".
  function restatesTitle(text, title) {
    const words = (s) => String(s).toLowerCase().split(/[^a-z0-9]+/).filter((w) => w.length > 2);
    const a = words(text), b = words(title);
    if (!a.length || !b.length || a.length > b.length + 3) return false;
    const match = (w, v) => w === v || (w.length > 3 && v.length > 3 && (w.startsWith(v) || v.startsWith(w)));
    return b.filter((v) => a.some((w) => match(w, v))).length / b.length >= 0.8;
  }
  // What a task's page shows: its own steps, minus notes that just restate the
  // task's title.  ('described' steps stay — schema work is real work, and the
  // machine log is where it shows.)
  function storyOf(tid, taskText) {
    return foldSteps((taskSteps.get(tid) || []).filter((e) =>
      !(e.kind === 'note' && restatesTitle(e.text, taskText))));
  }

  // A STEP, opened: the one place with everything about it — what kind of step,
  // what it acted on, the facts or the error, the task it served, and a way to
  // open the thing it produced or touched.
  function openStep(seq, n, panel) {
    const e = stepIndex.get(Number(seq));
    const V = viewer(panel, 'Step', { type: 'open_step', seq: Number(seq), n: Number(n) || 0 });
    if (!e) {
      V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
      V.text('exp:note', 'This step has scrolled out of the window.');
      return;
    }
    const nn = Number(n) || 0;
    const sev = e.tone === 'error' ? 'sev-critical' : e.tone === 'warn' ? 'sev-warn' : 'sev-info';
    let i = 0;
    V.add('div', 'sp:k', null, i++); V.set('sp:k', 'class', `fd-sev ${sev}`);
    V.text('sp:k', KIND_VERB[e.kind] || e.kind);
    V.add('div', 'sp:t', null, i++); V.set('sp:t', 'class', 'task-title'); V.text('sp:t', e.text);
    if (e.kind === 'built' && nn) {
      V.add('div', 'sp:tries', null, i++); V.set('sp:tries', 'class', 'explorer-note');
      V.text('sp:tries', `succeeded after ${nn} failed attempt${nn === 1 ? '' : 's'}`);
    } else if (e.kind === 'failed' && nn > 1) {
      V.add('div', 'sp:tries', null, i++); V.set('sp:tries', 'class', 'explorer-note');
      V.text('sp:tries', `tried ${nn} times; the latest error:`);
    }
    if (e.detail) { V.add('div', 'sp:d', null, i++); V.set('sp:d', 'class', 'fd-text'); V.text('sp:d', e.detail); }
    const task = e.task ? agenda.get(e.task) : null;
    if (task) {
      V.add('div', 'sp:task', null, i++); V.set('sp:task', 'class', 'fd-src');
      V.set('sp:task', 'data-id', e.task); V.text('sp:task', `part of: ${task.text}`);
    }
    if (e.ev) {
      const word = e.kind === 'built' ? 'Open the view' : e.kind === 'flagged' ? 'Open the finding'
        : e.kind === 'inspected' ? 'Open the source' : 'Open it';
      V.add('div', 'sp:cta', null, i++); V.set('sp:cta', 'class', 'fd-cta');
      V.add('button', 'sp:open', 'sp:cta', 0); V.set('sp:open', 'class', 'fd-btn primary');
      // the human title, never the subject slug — panel tabs must read like words
      V.set('sp:open', 'data-ev', e.ev); V.set('sp:open', 'data-title', e.text || e.subject);
      V.text('sp:open', word);
    }
  }

  // The live line lives INSIDE the active plan row (no separate now-strip): the
  // latest note under the active task, kept current by the activity fold.
  function updateAgentNow() {
    if (!agentPanels.size || !activeTaskId) return;
    const n = [...activityTail].reverse().find((x) => x.kind === 'note' && x.task === activeTaskId);
    if (!n) return;
    for (const p of agentPanels) TWIN_MUT.push({ op: 'setText', key: `${panelPfx(p)}ag:live`, text: n.text });
  }

  // What's happening — the plan (the active item pulses and carries its live
  // note) and the recent steps.  Every task row opens the task's page; every
  // step row opens the step's page.
  function openAgentPage(panel) {
    const V = viewer(panel, 'What’s happening', { type: 'open_agent' });
    agentPanels.add(V.panel);
    // one list: the live moment on top — the same act-row grammar as the plan
    // items, verb chip "now", pulsing while the agent computes (the client
    // mirrors the harness's status detail into the agent-live text) — then the
    // plan items under it.
    V.add('div', 'ag:list', null, 0); V.set('ag:list', 'class', 'agent-block');
    V.add('div', 'ag:nowrow', 'ag:list', 0); V.set('ag:nowrow', 'class', 'act-row now-row');
    V.add('span', 'ag:nowrow:v', 'ag:nowrow', 0); V.set('ag:nowrow:v', 'class', 'act-verb now-verb'); V.text('ag:nowrow:v', 'now');
    V.add('div', 'ag:nowrow:c', 'ag:nowrow', 1); V.set('ag:nowrow:c', 'class', 'act-content');
    V.add('div', 'ag:now', 'ag:nowrow:c', 0); V.set('ag:now', 'class', 'act-text agent-live');
    const lastStep = activityTail[activityTail.length - 1];
    V.text('ag:now', lastStep ? `last: ${stepLine(lastStep)}` : 'quiet — waiting for something to do');
    const items = [...agenda].sort((x, y) => {
      const rank = (st) => (st === 'active' ? 0 : st === 'open' ? 1 : 2);
      return rank(x[1].status) - rank(y[1].status);
    });
    if (!items.length) {
      // no agenda yet — but the twin still KNOWS its backlog deterministically:
      // the schema gaps its background turns are being steered through.  Show
      // that instead of an empty shrug, so "working…" always has a visible why.
      const backlog = [];
      for (const [sname, meta] of sources) {
        if (meta.kind !== 'table' || sname.startsWith('lens:')) continue;
        const missing = Object.keys(meta.schema || {})
          .filter((c) => !((semanticsOf(sname, c) || {}).title)).length;
        if (missing > 0) backlog.push({ sname, title: srcTitle(sname), missing });
      }
      backlog.sort((x, y) => y.missing - x.missing);
      if (!backlog.length) {
        V.add('div', 'ag:none', 'ag:list', 1); V.set('ag:none', 'class', 'ad-empty');
        V.text('ag:none', 'no plan yet: the agent lays out its own work here');
      }
      backlog.slice(0, 5).forEach((w, bi) => {
        workRow(V, 'ag:list', `ag:bk${bi}`, bi + 1, bi ? 'then' : 'next',
          `Document the fields of ${w.title}`, '', true, `${w.missing} fields still need a meaning`);
        // clicking a backlog row briefs the agent to take that item NOW
        V.set(`ag:bk${bi}`, 'data-doc', w.title);
      });
    }
    let i = 1;
    let queued = 0; // only ONE thing is next — everything open behind it is "later"
    for (const [id, a] of items) {
      if (a.status === 'done' && i > 8) continue;
      const label = a.status === 'active' ? 'now' : a.status === 'done' ? 'done' : (queued++ ? 'later' : 'next');
      workRow(V, 'ag:list', `task:${id}`, i++, label,
        a.text, a.status === 'active' ? 'is-now' : a.status === 'done' ? 'is-done' : '', true, a.desc);
      if (a.status === 'active') {
        // what it's doing right now, as the active row's own quiet line
        V.add('div', 'ag:live', `task:${id}:c`, 2); V.set('ag:live', 'class', 'act-detail agent-live');
        const n = [...activityTail].reverse().find((x) => x.kind === 'note' && x.task === id);
        V.text('ag:live', n ? n.text : 'working on it…');
      }
    }
    V.add('div', 'ag:l2', null, 1); V.set('ag:l2', 'class', 'ad-section'); V.text('ag:l2', 'Recent steps');
    V.add('div', 'ag:acts', null, 2); V.set('ag:acts', 'class', 'agent-block');
    // fold first (chronological), then newest on top — this is a recency feed.
    // Issues are not steps: they live on the Findings board and in the telling.
    const recent = foldSteps(activityTail.filter((e) => e.kind !== 'flagged')).slice(-30).reverse();
    if (!recent.length) { V.add('div', 'ag:qn', 'ag:acts', 0); V.set('ag:qn', 'class', 'ad-empty'); V.text('ag:qn', 'quiet so far'); }
    stepRows(V, 'ag:acts', 'ag:act', recent);
  }

  // One beat of the TELLING — only what the agent chose to voice (its notes),
  // chose to flag (issues), or actually produced (created lenses) — rendered
  // with the CHAT'S OWN components: the same feed-item text and cards, the same
  // classes, so the two surfaces cannot drift apart.
  function storyRow(V, parent, key, idx, e) {
    if (e.kind === 'note') {
      V.add('div', key, parent, idx); V.set(key, 'class', 'feed-item thought');
      V.add('div', `${key}:t`, key, 0); V.set(`${key}:t`, 'class', 'text'); V.text(`${key}:t`, e.text);
      return;
    }
    const issue = e.kind === 'flagged';
    V.add('div', key, parent, idx); V.set(key, 'class', 'feed-item card');
    let j = 0;
    if (issue) {
      V.add('span', `${key}:g`, key, j++); V.set(`${key}:g`, 'class', `story-tag ${e.tone === 'error' ? 'tag-crit' : 'tag-warn'}`);
      V.text(`${key}:g`, e.tone === 'error' ? 'critical issue' : 'issue');
    }
    V.add('div', `${key}:h`, key, j++); V.set(`${key}:h`, 'class', `card-title openable${issue ? ' issue-t' : ''}`);
    V.text(`${key}:h`, issue ? e.text : `Created ${e.text}`);
    V.set(`${key}:h`, 'data-ev', e.ev); V.set(`${key}:h`, 'data-title', e.text);
    // a created lens explains itself: its one-sentence description, like the
    // chat's mount cards — looked up live, so a later re-describe shows through
    const made = e.kind === 'built' && e.subject ? sources.get(e.subject) : null;
    if (made && made.description) {
      V.add('div', `${key}:s`, key, j++); V.set(`${key}:s`, 'class', 'card-sub'); V.text(`${key}:s`, made.description);
    }
    const facts = [e.detail, e.tries ? `worked after ${e.tries} failed attempt${e.tries === 1 ? '' : 's'}` : '']
      .filter(Boolean).join(' · ');
    if (facts) { V.add('div', `${key}:d`, key, j++); V.set(`${key}:d`, 'class', 'card-meta'); V.text(`${key}:d`, facts); }
  }

  // A TASK, opened — a subagent's transcript, rendered EXACTLY like the main chat:
  // only what the agent chose to tell (notes as speech, issues and created lenses
  // as cards), a live edge while it runs, and the full machine log (inspections,
  // failed attempts, retries) behind ONE quiet "every step" toggle at the end.
  //   title+desc  "what is it?"             (full text, once — the panel tab just says Task)
  //   status row  "is this running, and what can I do?" (chip + matching actions, one line)
  //   the telling the agent's own account — the chat's components verbatim
  //   every step  the honest mechanical sequence, folded, one click away
  function openTask(id, panel) {
    const tid = Number(id);
    const a = agenda.get(tid);
    const V = viewer(panel, 'Task', { type: 'open_task', id: tid });
    if (!a) {
      V.add('div', 'exp:note', null, 0); V.set('exp:note', 'class', 'explorer-note');
      V.text('exp:note', `No task #${id}.`);
      return;
    }
    taskPanels.set(V.panel, tid);
    const word = a.status === 'active' ? 'in progress' : a.status === 'done' ? 'done' : 'planned';
    let i = 0;
    V.add('div', 'ta:t', null, i++); V.set('ta:t', 'class', 'task-title'); V.text('ta:t', a.text);
    if (a.desc) { V.add('div', 'ta:desc', null, i++); V.set('ta:desc', 'class', 'ad-desc'); V.text('ta:desc', a.desc); }
    // ONE header row: the status chip and the actions that fit it, side by side.
    // A running task doesn't offer "start it"; a done task offers its way back.
    V.add('div', 'ta:cta', null, i++); V.set('ta:cta', 'class', 'fd-cta task-head');
    V.add('span', 'ta:st', 'ta:cta', 0); V.set('ta:st', 'class', `fd-sev task-${a.status}`); V.text('ta:st', word);
    if (a.status === 'open') {
      V.add('button', 'ta:work', 'ta:cta', 1); V.set('ta:work', 'class', 'fd-btn primary');
      V.set('ta:work', 'data-text', a.text); V.text('ta:work', 'Start now');
    }
    V.add('button', 'ta:done', 'ta:cta', 2); V.set('ta:done', 'class', 'fd-btn');
    V.set('ta:done', 'data-id', tid);
    V.set('ta:done', 'data-status', a.status === 'done' ? 'open' : 'done');
    V.text('ta:done', a.status === 'done' ? 'Reopen' : 'Mark done');

    // no section header — the transcript IS the page; a hairline sets it off
    const story = storyOf(tid, a.text);
    const told = story.filter((e) => e.kind === 'note' || ((e.kind === 'built' || e.kind === 'flagged') && e.ev));
    V.add('div', 'ta:acts', null, i++); V.set('ta:acts', 'class', 'story');
    if (!told.length) {
      V.add('div', 'ta:none', 'ta:acts', 0); V.set('ta:none', 'class', 'ad-empty');
      V.text('ta:none', story.length ? 'working quietly so far' : 'nothing yet');
    }
    told.forEach((e, si) => storyRow(V, 'ta:acts', `ta:act${si}`, si, e));
    // the live edge: what the agent is doing right now, at the story's end —
    // kept current by the activity fold while the page is open
    V.add('div', 'ta:now', null, i++); V.set('ta:now', 'class', 'task-now');
    if (a.status !== 'active') V.set('ta:now', 'hidden', 'true');
    V.add('span', 'ta:now:dot', 'ta:now', 0); V.set('ta:now:dot', 'class', 'now-dot live');
    V.add('span', 'ta:now:t', 'ta:now', 1); V.text('ta:now:t', 'working on it…');
    // the machine log: everything that actually happened (inspections, failures,
    // retries — folded), behind one quiet toggle.  Honesty one click away, never
    // mixed into the telling.
    const log = story.filter((e) => e.kind !== 'flagged');
    if (log.length) {
      V.add('button', 'ta:allbtn', null, i++); V.set('ta:allbtn', 'class', 'steps-toggle');
      V.text('ta:allbtn', `every step · ${log.length}`);
      V.add('div', 'ta:all', null, i++); V.set('ta:all', 'class', 'agent-block'); V.set('ta:all', 'hidden', 'true');
      stepRows(V, 'ta:all', 'ta:log', log);
    }
  }

  // The user re-prioritized or closed a task directly — an event, folded everywhere.
  function setTaskStatus(id, status) {
    const tid = Number(id);
    const a = agenda.get(tid);
    const s = String(status);
    if (!a || a.status === s || !['open', 'active', 'done'].includes(s)) return;
    G.submit(agendaSrc, ZSet.fromRows([rec({ id: tid, text: a.text, status: s })]),
      { author: 'user', origin: 'derived', note: 'set_task' });
    if (s === 'done') logStep('finished', a.text, { task: tid });
  }

  // Resolving a finding is an event like everything else; the folds dim it on the
  // board, in the count, and on re-opened detail views.
  function resolveFinding(id) {
    const f = findings.get(Number(id));
    if (!f || f.status === 'resolved') return;
    G.submit(findingsSrc, ZSet.fromRows([rec({ id: Number(id), severity: f.severity, text: f.text, source: f.source, status: 'resolved' })]),
      { author: 'user', origin: 'derived', note: 'resolved' });
    logStep('finished', `resolved: ${f.text}`, { subject: `finding:${id}`, ev: { type: 'open_finding', id: Number(id) } });
  }

  // The agent AUTHORS a lens (§4.1: lenses are data): pure JavaScript over a source's
  // rows, evaluated in-graph.  Purity is enforced by capability-absence — this isolate
  // has no IO of any kind, so lens code can compute but never effect.  The result
  // becomes a derived source `lens:<name>` (browsable/showable like any source) whose
  // full lineage — source, code, author — is recorded and shown on its card.
  function makeLens(a) {
    const srcName = resolveSourceName(a.source);
    // Names must read like a human named them.  Strip any "lens" the model prefixed
    // (the tile IS a lens — saying so is noise), slug for the stable id, and keep the
    // agent's own wording (punctuation and all) as the display title.
    const cleaned = String(a.name || '').replace(/^(the\s+)?lens[\s:_-]*/i, '').trim();
    const name = cleaned.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '') || 'unnamed';
    const title = String(a.title || '').trim() || cleaned
      || (name.charAt(0).toUpperCase() + name.slice(1).replace(/-/g, ' '));
    const description = String(a.description || '').trim();
    // failures fold by the lens's stable slug, so retries collapse into one row and
    // the eventual success absorbs them ("worked after N failed attempts").
    const step = (kind, o) => logStep(kind, title, Object.assign({ subject: 'lens:' + name }, o));
    const id = sourceIds.get(srcName);
    if (id === undefined) {
      step('failed', { tone: 'error', detail: `no source named ${String(a.source || '')}; mount or build it first` });
      return;
    }
    const code = String(a.code || '');
    // Lens code gets `rows` plus `table(name)` — read-only access to any other
    // source's rows, so cross-references and joins are one call instead of an
    // impossible reach for an undefined global.  Still pure: no IO exists here.
    const table = (n) => {
      const nm = resolveSourceName(n);
      const sid = sourceIds.get(nm);
      if (sid === undefined) throw new Error(`table("${n}"): no such source; mount or build it first`);
      return G.stateOf(sid).support().map((r) => r.asObject());
    };
    let out;
    try {
      out = new Function('rows', 'table', 'normalizeWell', 'inInterval', code)(
        G.stateOf(id).support().map((r) => r.asObject()), table, normalizeWellName, inInterval);
    }
    catch (err) { step('failed', { tone: 'error', detail: `${err.message}; fix the code and re-author` }); return; }
    if (!Array.isArray(out)) { step('failed', { tone: 'error', detail: 'the code must return an array of rows' }); return; }
    if (!out.length) {
      // an empty lens is clutter, not insight — bounce it back to the agent instead
      // of landing a "0 rows" card in the chat and a dead tile on the board.
      step('failed', { tone: 'warn', detail: 'came back EMPTY (0 rows); check field names and types, or drop the idea' });
      return;
    }
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
    profileSource(lname); // derived rows get the same schema inference as mounts
    logStep('built', title, {
      subject: 'lens:' + name,
      detail: `${fmtInt(out.length)} rows from ${srcTitle(srcName)}${ver > 1 ? ` · v${ver}` : ''}`,
      ev: { type: 'open_source', name: lname },
    });
    // The lens lands on the BOARD, quietly.  Nothing enters the chat by itself —
    // if the agent wants the user to see it, it `show`s it: a deliberate act.
  }

  // The composition chain of a derived source: walk the from-links back to the raw
  // mount, in human titles — "Equipment registry → Unique asset types → this".
  function chainOf(name) {
    const parts = [];
    let cur = name, guard = 0;
    while (cur && guard++ < 12) {
      const s = sources.get(cur);
      if (!s) { parts.unshift({ name: cur, title: cur }); break; }
      parts.unshift({ name: cur, title: s.title || cur });
      cur = s.from;
    }
    return parts;
  }

  // Give any lens or source a better human title/description — an event like all else.
  function doDescribe(rawName, title, description, origin) {
    const name = String(rawName || '');
    const resolved = resolveSourceName(name);
    const key = sources.has(resolved) ? resolved : (sources.has('lens:' + name) ? 'lens:' + name : null);
    if (!key) {
      logStep('failed', name, { subject: name, tone: 'error', detail: 'nothing by that name to describe' });
      return;
    }
    G.submit(describeSrc, ZSet.fromRows([rec({
      name: key, title: String(title || ''), description: String(description || ''), origin: String(origin || ''),
    })]), prov('agent'));
    logStep('described', (sources.get(key) || {}).title || key,
      { subject: key, ev: { type: 'open_source', name: key } });
  }

  // The agent inspects a source: quick stats + data-quality signals (empties,
  // duplicate keys) over the materialized rows.  The result lands in the activity
  // log, which the agent perceives next turn — its analysis loop.  The step reads
  // like a person's summary: identifiers are never ranged, timestamps read as dates,
  // constants are skipped, empties are percentages, and counts carry separators.
  function inspectSource(src) {
    const key = resolveSourceName(src);
    const meta = sources.get(key);
    const title = srcTitle(key);
    if (meta && meta.kind === 'documents') {
      const types = {};
      meta.docs.forEach((d) => { const e = String(d.name).split('.').pop().toLowerCase(); types[e] = (types[e] || 0) + 1; });
      logStep('inspected', title, {
        subject: key,
        detail: `${meta.docs.length} files · ${Object.entries(types).map(([k, v]) => `${v} ${k}`).join(', ')}`,
        ev: { type: 'open_source', name: key },
      });
      return;
    }
    const id = sourceIds.get(key);
    if (id === undefined) {
      logStep('failed', String(src), { subject: String(src), tone: 'error', detail: 'nothing by that name to check; mount or build it first' });
      return;
    }
    const rows = G.stateOf(id).support().map((r) => r.asObject());
    const cols = Object.keys((meta || {}).schema || {});
    const stats = [];
    cols.forEach((c) => {
      if (/ids?$/i.test(c)) return; // identifiers (id, assetIds, parentId…) are labels, not measurements
      // only REAL numbers count — booleans and empty strings coerce to 0/1 and
      // produce nonsense ranges like "isString 0–1" or "unit 0–0"
      const nums = rows.map((r) => r[c]).filter((v) => typeof v === 'number' && Number.isFinite(v));
      if (nums.length <= rows.length * 0.5 || !nums.length) return;
      // timestamps read as dates, not 13-digit numbers; zeros are missing, not data
      if (/(time|date)$/i.test(c) || nums.every((v) => v === 0 || v > 3e10)) {
        const live = nums.filter((v) => v > 0);
        if (!live.length) return;
        const lo = fmtDate(Math.min(...live)), hi = fmtDate(Math.max(...live));
        stats.push(lo === hi ? `${c} on ${lo}` : `${c} ${lo} → ${hi}`);
        return;
      }
      const lo = Math.min(...nums), hi = Math.max(...nums);
      if (lo === hi) return; // a constant says nothing worth a range
      stats.push(`${c} ${fmtNum(lo)}–${fmtNum(hi)}`);
    });
    const gaps = [];
    cols.forEach((c) => {
      const n = rows.filter((r) => r[c] == null || r[c] === '').length;
      if (!n) return;
      const pct = Math.round((n / rows.length) * 100);
      gaps.push(`${c} ${n === rows.length ? 'always' : `${pct || '<1'}%`} empty`);
    });
    let dupFact = '';
    if (cols.length && rows.length) {
      const c0 = cols[0]; const seen = new Set(); let dup = 0;
      rows.forEach((r) => { const v = String(r[c0]); if (seen.has(v)) dup += 1; else seen.add(v); });
      if (dup) dupFact = `${fmtInt(dup)} rows repeat the same ${c0}`;
    }
    const more = (arr, n) => arr.slice(0, n).join(', ') + (arr.length > n ? ` +${arr.length - n} more` : '');
    const size = meta && meta.rowcount > rows.length
      ? `sampled ${fmtInt(rows.length)} of ${fmtInt(meta.rowcount)} rows`
      : `${fmtInt(rows.length)} row${rows.length === 1 ? '' : 's'}`;
    const parts = [size, `${cols.length} columns`];
    if (stats.length) parts.push(more(stats, 2));
    parts.push(gaps.length ? `gaps: ${more(gaps, 3)}` : 'no gaps');
    if (dupFact) parts.push(dupFact);
    logStep('inspected', title, { subject: key, detail: parts.join(' · '), ev: { type: 'open_source', name: key } });
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
  function renderTableInto(root, pfx, rows, cols, note, srcName) {
    const b = vbuild(root, pfx);
    b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note'); b.text('note', note || `${rows.length} rows · ${cols.length} columns`);
    b.add('div', 'scroll', null, 1); b.set('scroll', 'class', 'view-scroll');
    b.add('table', 'tbl', 'scroll', 0);
    b.add('thead', 'hd', 'tbl', 0); b.add('tr', 'htr', 'hd', 0);
    cols.forEach((c, i) => { b.add('th', `h${i}`, 'htr', i); b.text(`h${i}`, srcName ? fieldTitle(srcName, c) : c); });
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
    logStep('failed', String(label), { subject: `chart:${label}`, tone: 'warn', detail: `no chart: ${msg}` });
  }

  // Well-log depth view (§9.14: depth-indexed series are time series keyed
  // differently) — curves as side-by-side tracks along measured depth, with
  // formation picks drawn as horizontal markers across all tracks: the render a
  // drilling engineer actually reads.  Works on any source with a depth column
  // (a LAS mount, a trajectory, a kind-tagged WITSML tree).
  function depthFieldOf(rows) {
    for (const c of ['DEPT', 'dept', 'md', 'MD', 'depth', 'Depth', 'tvd', 'TVD']) {
      if (rows.some((r) => typeof r[c] === 'number')) return c;
    }
    return null;
  }
  function renderDepthInto(root, pfx, srcName, a) {
    const b = vbuild(root, pfx);
    const all = rowsOf(srcName);
    const df = depthFieldOf(all);
    if (!df) {
      b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note');
      b.text('note', 'no depth column found — a depth view needs DEPT / md / depth');
      return;
    }
    const rows = all.filter((r) => typeof r[df] === 'number').sort((x, y) => x[df] - y[df]);
    let curves = Array.isArray(a.curves) && a.curves.length
      ? a.curves.filter((c) => rows.some((r) => typeof r[c] === 'number') && c !== df)
      : Object.keys(rows[0] || {}).filter((c) =>
          c !== df && rows.filter((r) => typeof r[c] === 'number').length > rows.length * 0.5);
    curves = curves.slice(0, 4);
    if (!curves.length) {
      b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note');
      b.text('note', 'no numeric curves to draw against depth');
      return;
    }
    const dmin = rows[0][df], dmax = rows[rows.length - 1][df];
    // picks: an optional second source of formation tops, matched to this well
    let picks = [];
    const pname = a.picks || (sources.has('wellpicks') ? 'wellpicks' : null);
    if (pname && sources.has(resolveSourceName(pname)) && dmax > dmin) {
      const prows = rowsOf(resolveSourceName(pname));
      const pd = depthFieldOf(prows);
      const plabel = Object.keys(prows[0] || {}).find((c) => typeof prows[0][c] === 'string' && !/well/i.test(c));
      const pwell = Object.keys(prows[0] || {}).find((c) => /well/i.test(c));
      const thisWell = rows.find((r) => r.well != null);
      picks = prows.filter((p) => typeof p[pd] === 'number' && inInterval(p[pd], dmin, dmax)
        && (!pwell || !thisWell || normalizeWellName(p[pwell]) === normalizeWellName(thisWell.well)))
        .map((p) => ({ d: p[pd], label: plabel ? String(p[plabel]) : '' }));
    }
    const W = 1000, H = 640, padL = 70, padT = 36, padB = 14, padR = 10;
    const trackW = (W - padL - padR) / curves.length;
    const y = (d) => padT + (dmax > dmin ? (d - dmin) / (dmax - dmin) : 0) * (H - padT - padB);
    b.add('div', 'note', null, 0); b.set('note', 'class', 'explorer-note');
    b.text('note', `${fmtInt(rows.length)} stations · ${fmtNum(dmin)}–${fmtNum(dmax)} ${df}` +
      (picks.length ? ` · ${picks.length} formation tops` : ''));
    b.add('svg', 'svg', null, 1); b.set('svg', 'viewBox', `0 0 ${W} ${H}`); b.set('svg', 'class', 'chart depth');
    // depth axis: 5 labeled ticks down the left edge
    for (let i = 0; i <= 4; i++) {
      const d = dmin + ((dmax - dmin) * i) / 4;
      b.add('text', `dl${i}`, 'svg', i); b.set(`dl${i}`, 'x', padL - 8); b.set(`dl${i}`, 'y', y(d) + 4);
      b.set(`dl${i}`, 'class', 'chart-lbl'); b.set(`dl${i}`, 'text-anchor', 'end'); b.text(`dl${i}`, fmtNum(d));
    }
    curves.forEach((c, ci) => {
      const x0 = padL + ci * trackW;
      const vals = rows.map((r) => r[c]).filter((v) => typeof v === 'number');
      const vmin = Math.min(...vals), vmax = Math.max(...vals);
      const x = (v) => x0 + 6 + (vmax > vmin ? (v - vmin) / (vmax - vmin) : 0.5) * (trackW - 12);
      b.add('rect', `fr${ci}`, 'svg', 10 + ci * 3); b.set(`fr${ci}`, 'x', x0); b.set(`fr${ci}`, 'y', padT);
      b.set(`fr${ci}`, 'width', trackW); b.set(`fr${ci}`, 'height', H - padT - padB); b.set(`fr${ci}`, 'class', 'depth-track');
      b.add('text', `th${ci}`, 'svg', 11 + ci * 3); b.set(`th${ci}`, 'x', x0 + trackW / 2); b.set(`th${ci}`, 'y', padT - 10);
      b.set(`th${ci}`, 'class', 'chart-lbl'); b.set(`th${ci}`, 'text-anchor', 'middle');
      b.text(`th${ci}`, fieldTitle(srcName, c));
      let d = '';
      let pen = false;
      rows.forEach((r) => {
        if (typeof r[c] !== 'number') { pen = false; return; } // nulls break the line
        d += (pen ? 'L' : 'M') + x(r[c]).toFixed(1) + ' ' + y(r[df]).toFixed(1) + ' ';
        pen = true;
      });
      b.add('path', `cv${ci}`, 'svg', 12 + ci * 3); b.set(`cv${ci}`, 'd', d.trim()); b.set(`cv${ci}`, 'class', 'chart-line');
    });
    picks.forEach((p, i) => {
      b.add('line', `pk${i}`, 'svg', 40 + i * 2); b.set(`pk${i}`, 'x1', padL); b.set(`pk${i}`, 'y1', y(p.d));
      b.set(`pk${i}`, 'x2', W - padR); b.set(`pk${i}`, 'y2', y(p.d)); b.set(`pk${i}`, 'class', 'depth-pick');
      b.add('text', `pl${i}`, 'svg', 41 + i * 2); b.set(`pl${i}`, 'x', W - padR - 4); b.set(`pl${i}`, 'y', y(p.d) - 4);
      b.set(`pl${i}`, 'class', 'chart-lbl pick-lbl'); b.set(`pl${i}`, 'text-anchor', 'end'); b.text(`pl${i}`, p.label);
    });
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
    // the agent's rich-text surface: a markdown note, composed and shown inline
    if (view === 'note' || view === 'markdown' || view === 'text') {
      const title = a.title || 'Note';
      const sq = append('view', { text: title, view: 'note' });
      renderMarkdownInto(`item:${sq}:body`, `v${sq}`, String(a.text || a.markdown || a.body || ''));
      return;
    }
    // the aggregate views render inline exactly as they do in a panel
    if (EMBED_ALIAS[view] && EMBED_ALIAS[view] !== 'table') {
      const mode = EMBED_ALIAS[view];
      const srcName2 = resolveSourceName(a.source || a.name);
      const s2 = sources.get(srcName2);
      if (!s2) { append('system', { text: `No source “${srcName2}” to show — mount it first.` }); return; }
      const title = a.title || `${srcTitle(srcName2)} · ${mode}`;
      const sq = append('view', { text: title, view: mode });
      AGG_VIEWS[mode](`item:${sq}:body`, `v${sq}`, srcName2, a);
      cardOpen(sq, title, { type: 'open_source', name: srcName2, mode });
      return;
    }
    if (view === 'document' || view === 'doc') {
      const name = a.name || a.document || a.source;
      if (!name) return;
      const title = a.title || String(name);
      const sq = append('view', { text: title, view: 'document' });
      renderDocInto(`item:${sq}:body`, `v${sq}`, name);
      cardOpen(sq, title, { type: 'open_document', name: String(name) });
      return;
    }
    const srcName = resolveSourceName(a.source || a.name);
    const s = sources.get(srcName);
    if (!s) { append('system', { text: `No source “${srcName}” to show — mount it first.` }); return; }
    if (view === 'tree' || view === 'hierarchy') {
      const title = a.title || `${srcTitle(srcName)} — hierarchy`;
      const sq = append('view', { text: title, view: 'tree' });
      renderTreeInto(`item:${sq}:body`, `v${sq}`, srcName);
      cardOpen(sq, title, { type: 'open_source', name: srcName, mode: 'tree' });
      return;
    }
    if (view === 'depth' || view === 'log') {
      const title = a.title || `${srcTitle(srcName)} — depth view`;
      const sq = append('view', { text: title, view: 'depth' });
      renderDepthInto(`item:${sq}:body`, `v${sq}`, srcName, a);
      cardOpen(sq, title, { type: 'open_source', name: srcName, mode: 'depth' });
      return;
    }
    // table (default)
    let rows = rowsOf(srcName);
    if (a.filter) { const q = String(a.filter).toLowerCase(); rows = rows.filter((r) => Object.values(r).some((v) => v != null && String(v).toLowerCase().includes(q))); }
    const limit = Math.max(1, Math.min(Number(a.limit) || 10, MAX_EXPLORE_ROWS));
    rows = rows.slice(0, limit);
    // The model names columns loosely — "wellbore", "MD ", a field's human title —
    // so resolve each against the REAL keys (else the header renders but every
    // cell misses).  Unresolvable columns drop; none resolved = show everything.
    const real = Object.keys(s.schema || rows[0] || {});
    const canon = (x) => String(x).toLowerCase().replace(/[^a-z0-9]/g, '');
    const resolveCol = (c) => real.find((k) => k === c)
      || real.find((k) => canon(k) === canon(c))
      || real.find((k) => canon(fieldTitle(srcName, k)) === canon(c));
    const asked = Array.isArray(a.columns) ? a.columns.map(resolveCol).filter(Boolean) : [];
    const cols = asked.length ? [...new Set(asked)] : real;
    const title = a.title || srcTitle(srcName);
    const sq = append('view', { text: title, view: 'table' });
    renderTableInto(`item:${sq}:body`, `v${sq}`, rows, cols.length ? cols : Object.keys(rows[0] || {}),
      `showing ${rows.length} of ${s.rowcount} rows${a.filter ? ` matching “${a.filter}”` : ''}`, srcName);
    cardOpen(sq, srcTitle(srcName), { type: 'open_source', name: srcName });
  }

  // ---- agent tool surface (§12.1) -------------------------------------------
  function agentTool(json) {
    let c; try { c = JSON.parse(json); } catch (_) { return; }
    const a = c.args || {};
    switch (c.tool) {
      case 'think': {
        const t = String(a.text || '');
        append('thought', { text: t });
        // a thought voiced while a task is active IS that task's narration —
        // without this, transcripts are all cards and no speech (the model
        // thinks 20× for every `work` note it writes).
        if (activeTaskId) logStep('note', t);
        break;
      }
      case 'say': {
        // an answer can carry its EVIDENCE: exact phrases quoted from documents
        // (openable back to their source), and the quiet list of steps the turn
        // took — provenance in the chat, not just in the work log.
        const quotes = Array.isArray(a.quotes)
          ? a.quotes.filter((q) => q && q.text).slice(0, 4)
              .map((q) => ({ source: String(q.source || ''), text: String(q.text) }))
          : [];
        append('agent', {
          text: String(a.text || ''),
          quotes: quotes.length ? JSON.stringify(quotes) : '',
          steps: String(a.steps || ''),
        });
        break;
      }
      case 'ask': append('question', { text: String(a.question || a.text || ''), options: a.options || [] }); break;
      case 'record_profile': recordProfile(a.field, a.value); break;
      case 'inspect': inspectSource(a.source); break;
      case 'show': doShow(a); break;
      case 'make_dashboard': doMakeDashboard(a); break;
      case 'plan': {
        const items = Array.isArray(a.items) ? a.items : [a.text || a.item];
        items.filter(Boolean).forEach((t) => {
          if (typeof t === 'object') addAgenda(t.title || t.text, t.description);
          else addAgenda(t);
        });
        break;
      }
      case 'work':
        if (a.task != null && a.task !== '') setAgenda(a.task, 'active');
        logActivity(String(a.text || a.task || 'working'));
        break;
      case 'done': {
        const ref = a.task != null ? a.task : a.text;
        if (ref != null) {
          const hit = setAgenda(ref, 'done');
          if (hit != null) logStep('finished', agenda.get(hit).text, { task: hit });
        }
        break;
      }
      case 'finding': addFinding(a); break;
      case 'make_lens': makeLens(a); break;
      case 'describe': doDescribe(a.source || a.name || a.lens, a.title, a.description, a.origin); break;
      case 'annotate': doAnnotate(a); break;
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
    // a viewport report is VIEW state, not twin state: it moves a table's window
    // and touches neither the raw input stream nor the journal (like mouse
    // position, it re-derives from the next scroll — replaying it means nothing)
    if (e.type === 'viewport') { tableViewport(e); return; }
    inputSeq += 1;
    const raw = Object.assign({ seq: inputSeq, ts: 0 }, e);
    G.submit(inputSrc, ZSet.fromRows([rec(raw)]), { author: 'user', origin: 'ui', note: 'raw' });
    deriveFromInput(raw);
  }

  function deriveFromInput(e) {
    const note = `input#${e.seq}`;
    // "sub": the event targets a column's bottom half — address it as p + 0.5
    if (e.sub && e.panel != null) e = Object.assign({}, e, { panel: (Number(e.panel) || 0) + 0.5 });
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
      openSource(String(e.name), e.mode, e.panel, e.title);
    } else if (e.type === 'open_document' && e.name) {
      openDocument(String(e.name), e.panel);
    } else if (e.type === 'open_asset' && e.id) {
      openAsset(String(e.id), e.panel, e.title);
    } else if (e.type === 'open_board') {
      // the board is hosted client-side; the marker alone lets a replaying client
      // rebuild the column (and clears anything to its right, like every open)
      viewer(e.panel, e.title || 'The twin', { type: 'open_board' });
    } else if (e.type === 'search') {
      search(String(e.query || ''), e.panel);
    } else if (e.type === 'open_schema') {
      openSchemaMap(e.panel);
    } else if (e.type === 'watch') {
      watch(e.panel);
    } else if (e.type === 'open_finding') {
      openFinding(e.id, e.panel);
    } else if (e.type === 'resolve_finding') {
      resolveFinding(e.id);
    } else if (e.type === 'open_agent') {
      openAgentPage(e.panel);
    } else if (e.type === 'open_task') {
      openTask(e.id, e.panel);
    } else if (e.type === 'open_step') {
      openStep(e.seq, e.n, e.panel);
    } else if (e.type === 'set_task') {
      setTaskStatus(e.id, e.status);
    } else if (e.type === 'pause' || e.type === 'resume') {
      // captured raw like everything else; the Rust boundary flips the agent's
      // gate.  A quiet system line in the chat confirms the user's own action.
      append('system', { text: e.type === 'pause' ? 'Twin paused: no background work until you resume.' : 'Twin resumed.' }, `input#${e.seq}`);
    } else if (e.type === 'close_panel') {
      closePanel(e.panel);
    } else if (e.type === 'star') {
      toggleStar(e);
    }
    // 'fetch' renders via the host boundary (twin_fetch); 'open_board' is hosted
    // client-side (the live board node) — both are still captured raw above.
    if (['open_source', 'open_asset', 'open_document', 'fetch', 'open_board', 'watch', 'open_schema'].includes(e.type)) {
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
      case 'open_schema': return 'opened the schema map';
      case 'fetch': return `charted series ${e.label || e.id}`;
      case 'open_board': return 'opened the twin board';
      case 'open_agent': return 'checked what the agent is doing';
      case 'open_task': return `opened task #${e.id}`;
      case 'open_step': return 'looked at one of the agent’s steps';
      case 'set_task': return `set task #${e.id} to ${e.status}`;
      case 'pause': return 'paused the twin';
      case 'resume': return 'resumed the twin';
      case 'close_panel': return 'closed a column';
      case 'open_finding': return `opened finding #${e.id}`;
      case 'resolve_finding': return `resolved finding #${e.id}`;
      case 'star': return `starred “${e.title || 'a page'}”`;
      default: return String(e.type);
    }
  }

  // ---- what the agent perceives (§12.3) --------------------------------------
  // Everything here is a fold over the event streams above — including userActions,
  // the user's RAW behavior, from which the agent derives goals and intent.
  function perceive() {
    // The CHAT is not the context window: what the agent perceives as "feed" is
    // the conversation proper — what the user said and what the agent chose to
    // tell them.  Cards and rendered views are UI furniture, not conversation;
    // the agent's real context is the structured state around this (sources,
    // agenda, findings, lenses, activity), independent of what's on screen.
    const CONVO = new Set(['user', 'agent', 'question', 'thought', 'system']);
    const recent = feed.items.filter((it) => CONVO.has(it.kind)).slice(-40)
      .map((it) => ({ kind: it.kind, text: it.text, options: it.options }));
    // Sources carry their inferred schema as `fields` — types, keys, references,
    // enums, patterns, plus the agent's own annotations.  Far denser than sample
    // rows, so the samples shrink to two: the profile is the real context.
    const srcs = [...sources].map(([name, s]) => {
      const fl = fieldLines(name);
      return {
        name, title: s.title, description: s.description,
        residence: s.residence, rowcount: s.rowcount, locator: s.locator || '',
        fields: fl.length ? fl : Object.keys(s.schema || {}),
        sample: (s.sample || []).slice(0, 2),
      };
    });
    const sks = [...skills].map(([name, s]) => ({ name, title: s.title, description: s.description }));
    const ag = [...agenda].map(([id, a]) => ({ id, text: a.text, description: a.desc, status: a.status }));
    return JSON.stringify({
      profile: profile.asObject(),
      sources: srcs,
      skills: sks,
      agenda: ag.filter((a) => a.status !== 'done').slice(0, 12),
      agendaDone: ag.filter((a) => a.status === 'done').length,
      findings: [...findings.values()].slice(-8).map((f) => ({ severity: f.severity, text: f.text })),
      lenses: [...lenses].map(([name, l]) => ({ name: 'lens:' + name, title: l.title, description: l.description, source: l.source, rowcount: l.rowcount })),
      dashboards: [...dashboards].map(([slug, d]) => ({ name: slug, title: d.title, description: d.description })),
      activity: activityTail.slice(-8).map(stepLine),
      userActions: userActions.slice(-10),
      feed: recent,
    });
  }

  return {
    agentTool, event, perceive, mountSource, sourceError, installSkill, appendRows,
    chartSeries, chartMessage, chartInline, chartInlineMessage,
    // host-side effects (OCR, search, fetches) report into the same activity log
    log(kind, text, detail, subject, tone) { logStep(kind, text, { detail, subject, tone }); },
    // the shared mutation log (§11.3): replaying [0..total) rebuilds the DOM exactly.
    total() { return TWIN_MUT.length; },
    from(n) { return JSON.stringify(TWIN_MUT.slice(n)); },
  };
})();
globalThis.T = T;
